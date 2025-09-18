use alloy::primitives::{Address, FixedBytes};
use alloy::sol_types::SolValue;
use anyhow::Result;
use std::{
    env,
    sync::{Arc, Mutex},
};
use tracing::info;

use crate::creator::core::Creator;
use crate::usecases::gas_killer::storage_validator::StorageValidator;
use crate::usecases::gas_killer::task_data::GasKillerTaskData;

/// Configuration for the gas analyzer
#[derive(Debug, Clone)]
pub struct GasAnalyzerConfig {
    /// RPC URL to fork from for analysis
    #[allow(dead_code)]
    pub fork_rpc_url: String,
    /// Block number or tag to fork from (e.g., "latest", "12345678")
    #[allow(dead_code)]
    pub fork_block: String,
    /// Target contract address for analysis
    pub target_address: Address,
    /// Function selector for analysis
    pub target_function: FixedBytes<4>,
    /// Call data for the transaction
    #[allow(dead_code)]
    pub call_data: Vec<u8>,
}

impl Default for GasAnalyzerConfig {
    fn default() -> Self {
        Self {
            fork_rpc_url: env::var("RPC_URL")
                .unwrap_or_else(|_| "https://ethereum-holesky.publicnode.com".to_string()),
            fork_block: "latest".to_string(),
            target_address: Address::ZERO,
            target_function: FixedBytes::ZERO,
            call_data: vec![],
        }
    }
}

/// Creator implementation for the gas killer use case
pub struct GasKillerCreator {
    #[allow(dead_code)]
    config: GasAnalyzerConfig,
    current_round: Arc<Mutex<u64>>,
    storage_validator: StorageValidator,
}

impl GasKillerCreator {
    /// Creates a new GasKillerCreator with the given configuration
    #[allow(dead_code)]
    pub fn new(config: GasAnalyzerConfig) -> Result<Self> {
        let storage_validator = StorageValidator::from_env()?;
        Ok(Self {
            config,
            current_round: Arc::new(Mutex::new(0)),
            storage_validator,
        })
    }

    /// Creates a new GasKillerCreator with a specific starting round
    #[allow(dead_code)]
    pub fn with_round(config: GasAnalyzerConfig, starting_round: u64) -> Result<Self> {
        let storage_validator = StorageValidator::from_env()?;
        Ok(Self {
            config,
            current_round: Arc::new(Mutex::new(starting_round)),
            storage_validator,
        })
    }

    /// Creates payload bytes from the task data
    ///
    /// The payload is deterministically encoded from the task data to ensure
    /// that the same analysis produces the same payload hash for consensus.
    fn create_payload_from_task_data(&self, task_data: &GasKillerTaskData) -> Result<Vec<u8>> {
        // Create a deterministic payload by ABI-encoding key fields
        // This must match the logic in GasKillerValidator::reconstruct_payload_hash
        let payload_data = (
            task_data.transition_index,
            task_data.target_address,
            task_data.target_function,
            task_data.call_data.clone(),
        );

        let payload = payload_data.abi_encode();
        Ok(payload)
    }
}

#[async_trait::async_trait]
impl Creator for GasKillerCreator {
    type TaskData = GasKillerTaskData;
    async fn get_payload_and_round(&self) -> Result<(Vec<u8>, u64)> {
        info!("Starting gas analysis for creator");

        // Get the next round number
        let current_round = {
            let mut round = self.current_round.lock().unwrap();
            *round += 1;
            *round
        };

        // Use the stored storage validator for real gas analysis
        // This performs the same gas analysis as the validator
        let analysis_result = self
            .storage_validator
            .perform_gas_analysis(self.config.target_address, &self.config.call_data)
            .await?;

        // Create task data with real analysis results
        let task_data = GasKillerTaskData {
            storage_updates: analysis_result.storage_updates,
            transition_index: current_round,
            target_address: self.config.target_address,
            target_function: self.config.target_function,
            call_data: self.config.call_data.clone(),
        };

        let payload = self.create_payload_from_task_data(&task_data)?;

        info!("Gas analysis completed for round {}", current_round);
        Ok((payload, current_round))
    }

    fn get_task_metadata(&self) -> GasKillerTaskData {
        // Return default task data for metadata
        // In a real implementation, this might return cached analysis results
        GasKillerTaskData::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, FixedBytes};

    fn create_test_config() -> GasAnalyzerConfig {
        GasAnalyzerConfig {
            fork_rpc_url: "https://ethereum-holesky.publicnode.com".to_string(),
            fork_block: "latest".to_string(),
            target_address: Address::ZERO,
            target_function: FixedBytes::ZERO,
            call_data: vec![],
        }
    }

    /// Helper function to set up RPC_URL environment variable for tests
    fn setup_test_env() -> Option<String> {
        let original_rpc_url = std::env::var("RPC_URL").ok();
        if original_rpc_url.is_none() {
            unsafe {
                std::env::set_var("RPC_URL", "https://ethereum-holesky.publicnode.com");
            }
        }
        original_rpc_url
    }

    /// Helper function to restore RPC_URL environment variable after tests
    fn restore_test_env(original_rpc_url: Option<String>) {
        if original_rpc_url.is_none() {
            unsafe {
                std::env::remove_var("RPC_URL");
            }
        }
    }

    #[tokio::test]
    async fn test_creator_creation() {
        let original_rpc_url = setup_test_env();

        let config = create_test_config();
        let creator = GasKillerCreator::new(config).unwrap();
        assert_eq!(*creator.current_round.lock().unwrap(), 0);

        restore_test_env(original_rpc_url);
    }

    #[tokio::test]
    async fn test_creator_with_round() {
        let original_rpc_url = setup_test_env();

        let config = create_test_config();
        let creator = GasKillerCreator::with_round(config, 5).unwrap();
        assert_eq!(*creator.current_round.lock().unwrap(), 5);

        restore_test_env(original_rpc_url);
    }

    #[tokio::test]
    async fn test_create_payload_from_task_data() {
        let original_rpc_url = setup_test_env();

        let config = create_test_config();
        let creator = GasKillerCreator::new(config).unwrap();

        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            target_function: FixedBytes::from([0x12, 0x34, 0x56, 0x78]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
        };

        let result = creator.create_payload_from_task_data(&task_data);
        assert!(result.is_ok());

        let payload = result.unwrap();
        assert!(!payload.is_empty());

        restore_test_env(original_rpc_url);
    }

    #[tokio::test]
    async fn test_get_task_metadata_default() {
        let original_rpc_url = setup_test_env();

        let config = create_test_config();
        let creator = GasKillerCreator::new(config).unwrap();

        let metadata = creator.get_task_metadata();
        assert_eq!(metadata.target_address, Address::ZERO);
        assert_eq!(metadata.target_function, FixedBytes::ZERO);

        restore_test_env(original_rpc_url);
    }

    #[tokio::test]
    async fn test_creator_validator_hash_consistency() {
        use crate::usecases::gas_killer::validator::GasKillerValidator;

        let original_rpc_url = setup_test_env();

        let config = create_test_config();
        let creator = GasKillerCreator::new(config).unwrap();
        let validator = GasKillerValidator::new();

        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            target_function: FixedBytes::from([0x12, 0x34, 0x56, 0x78]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
        };

        // Create payload using creator
        let creator_payload = creator.create_payload_from_task_data(&task_data).unwrap();

        // Create a test message to validate with validator
        use crate::validator::interface::ValidatorTrait;
        use crate::wire;
        use commonware_codec::{EncodeSize, Write};

        let aggregation = wire::Aggregation::<GasKillerTaskData>::new(
            1, // round
            task_data.clone(),
            None, // payload
        );

        let mut msg_bytes = Vec::with_capacity(aggregation.encode_size());
        aggregation.write(&mut msg_bytes);

        // Get payload hash using validator
        let validator_payload_hash = validator
            .get_payload_from_message(&msg_bytes)
            .await
            .unwrap();

        // Hash the creator payload to compare
        use commonware_cryptography::{Hasher, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&creator_payload);
        let creator_payload_hash = hasher.finalize();

        // Both should produce the same hash
        assert_eq!(creator_payload_hash, validator_payload_hash);

        restore_test_env(original_rpc_url);
    }
}

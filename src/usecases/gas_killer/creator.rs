use alloy::primitives::{Address, FixedBytes};
use alloy::sol_types::SolValue;
use anyhow::Result;
use std::sync::{Arc, Mutex};
use tracing::info;

use crate::creator::core::Creator;
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
            fork_rpc_url: "https://ethereum-holesky.publicnode.com".to_string(),
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
}

impl GasKillerCreator {
    /// Creates a new GasKillerCreator with the given configuration
    #[allow(dead_code)]
    pub fn new(config: GasAnalyzerConfig) -> Self {
        Self {
            config,
            current_round: Arc::new(Mutex::new(0)),
        }
    }

    /// Creates a new GasKillerCreator with a specific starting round
    #[allow(dead_code)]
    pub fn with_round(config: GasAnalyzerConfig, starting_round: u64) -> Self {
        Self {
            config,
            current_round: Arc::new(Mutex::new(starting_round)),
        }
    }

    /// Runs gas analysis using the gas-analyzer-rs library
    #[allow(dead_code)]
    async fn run_gas_analysis(&self) -> Result<GasKillerTaskData> {
        info!("Starting gas analysis with config: {:?}", self.config);

        // TODO: Implement actual gas analysis using gas-analyzer-rs
        // For now, return mock data
        let transition_index = {
            let current_round = self.current_round.lock().unwrap();
            *current_round
        };

        // Create task data with mock analysis results
        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04], // Mock storage updates
            transition_index,
            target_address: self.config.target_address,
            target_function: self.config.target_function,
            gas_savings: 1000, // Mock gas savings
            call_data: self.config.call_data.clone(), // Use call data from config
        };

        info!(
            "Mock gas analysis completed. Estimated gas: {}",
            task_data.gas_savings
        );
        Ok(task_data)
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
            task_data.gas_savings, // Use actual gas savings from task data
        );

        let payload = payload_data.abi_encode();
        Ok(payload)
    }
}

#[async_trait::async_trait]
impl Creator for GasKillerCreator {
    type TaskData = GasKillerTaskData;
    async fn get_payload_and_round(&self) -> Result<(Vec<u8>, u64)> {
        // TODO: Implement actual gas analysis
        // For now, return default task data
        let task_data = GasKillerTaskData::default();

        let payload = self.create_payload_from_task_data(&task_data)?;

        // Increment round
        let current_round = {
            let mut round = self.current_round.lock().unwrap();
            *round += 1;
            *round
        };

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

    #[tokio::test]
    async fn test_creator_creation() {
        let config = create_test_config();
        let creator = GasKillerCreator::new(config);
        assert_eq!(*creator.current_round.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_creator_with_round() {
        let config = create_test_config();
        let creator = GasKillerCreator::with_round(config, 5);
        assert_eq!(*creator.current_round.lock().unwrap(), 5);
    }

    #[tokio::test]
    async fn test_create_payload_from_task_data() {
        let config = create_test_config();
        let creator = GasKillerCreator::new(config);

        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            target_function: FixedBytes::from([0x12, 0x34, 0x56, 0x78]),
            gas_savings: 1000,
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
        };

        let result = creator.create_payload_from_task_data(&task_data);
        assert!(result.is_ok());

        let payload = result.unwrap();
        assert!(!payload.is_empty());
    }

    #[tokio::test]
    async fn test_get_task_metadata_default() {
        let config = create_test_config();
        let creator = GasKillerCreator::new(config);

        let metadata = creator.get_task_metadata();
        assert_eq!(metadata.target_address, Address::ZERO);
        assert_eq!(metadata.target_function, FixedBytes::ZERO);
    }

    #[tokio::test]
    async fn test_creator_validator_hash_consistency() {
        use crate::usecases::gas_killer::validator::GasKillerValidator;

        let config = create_test_config();
        let creator = GasKillerCreator::new(config);
        let validator = GasKillerValidator::new();

        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            target_function: FixedBytes::from([0x12, 0x34, 0x56, 0x78]),
            gas_savings: 1000,
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
    }
}

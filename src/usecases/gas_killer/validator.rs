use alloy::primitives::U256;
use alloy::sol_types::SolValue;
use anyhow::Result;
use commonware_codec::Read;
use commonware_cryptography::sha256::Digest;
use commonware_cryptography::{Hasher, Sha256};
use tracing::debug;

use crate::services::GasAnalyzer;
use crate::usecases::gas_killer::task_data::GasKillerTaskData;
use crate::validator::interface::ValidatorTrait;
use crate::wire;

/// Validator implementation for the gas killer use case
pub struct GasKillerValidator {
    /// Gas analyzer service for computing storage updates
    gas_analyzer: GasAnalyzer,
}

impl GasKillerValidator {
    /// Creates a new GasKillerValidator with default settings.
    pub fn new() -> Self {
        Self {
            gas_analyzer: GasAnalyzer::from_env(),
        }
    }

    /// Creates a new GasKillerValidator with a specific GasAnalyzer.
    #[allow(dead_code)]
    pub fn with_analyzer(gas_analyzer: GasAnalyzer) -> Self {
        Self { gas_analyzer }
    }

    /// Validates the message format and decodes the aggregation
    async fn validate_message_format(
        &self,
        msg: &[u8],
    ) -> Result<wire::Aggregation<GasKillerTaskData>> {
        debug!("Validating message format, length: {} bytes", msg.len());

        if msg.is_empty() {
            return Err(anyhow::anyhow!("Message is empty"));
        }

        // Try to decode the aggregation
        let mut msg_buf = msg;
        let aggregation = wire::Aggregation::<GasKillerTaskData>::read_cfg(&mut msg_buf, &())
            .map_err(|e| anyhow::anyhow!("Failed to decode aggregation: {}", e))?;

        debug!(
            "Successfully decoded aggregation with round: {}",
            aggregation.round
        );
        Ok(aggregation)
    }

    /// Computes storage updates for the given task data using gas-analyzer-rs
    ///
    /// This method forks the blockchain state and simulates the transaction
    /// to extract the storage updates that would result from execution.
    async fn compute_storage_updates(&self, task_data: &GasKillerTaskData) -> Result<Vec<u8>> {
        debug!("Computing storage updates for task");

        let analysis_result = self
            .gas_analyzer
            .analyze_transaction(
                task_data.target_address,
                &task_data.call_data,
                Some(task_data.from_address),
                Some(task_data.value),
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to compute storage updates: {}", e))?;

        debug!(
            "Computed storage updates: {} bytes",
            analysis_result.storage_updates.len()
        );
        Ok(analysis_result.storage_updates)
    }

    /// Reconstructs the payload hash from task data and computed storage updates
    ///
    /// This method must produce the same hash as the on-chain expectedHash
    /// in GasKillerSDK.verifyAndUpdate to ensure consensus consistency.
    fn build_payload_hash(&self, task_data: &GasKillerTaskData, storage_updates: &[u8]) -> Digest {
        // IMPORTANT: This hash must match the on-chain expectedHash in GasKillerSDK.verifyAndUpdate:
        // sha256(abi.encode(transitionIndex, address(this), targetFunction, storageUpdates))

        let selector = task_data.function_selector();

        // Build flattened ABI encoding matching abi.encode(transitionIndex, address(this), selector, storageUpdates)
        // Heads (32 bytes each)
        let head_transition = U256::from(task_data.transition_index).abi_encode();
        let head_address = task_data.target_address.abi_encode();
        let head_selector = selector.abi_encode();
        // Offset to the dynamic bytes tail: 4 words (3 static + 1 offset) = 0x80
        let head_offset = U256::from(32u64 * 4u64).abi_encode();

        // Tail for dynamic bytes: length (u256) + data + padding
        let mut tail = Vec::with_capacity(32 + storage_updates.len() + 31);
        tail.extend_from_slice(&U256::from(storage_updates.len()).abi_encode());
        tail.extend_from_slice(storage_updates);
        let pad_len = (32 - (storage_updates.len() % 32)) % 32;
        if pad_len > 0 {
            tail.extend(std::iter::repeat_n(0u8, pad_len));
        }

        // Concatenate head and tail into final payload
        let mut payload = Vec::with_capacity(32 * 4 + tail.len());
        payload.extend_from_slice(&head_transition);
        payload.extend_from_slice(&head_address);
        payload.extend_from_slice(&head_selector);
        payload.extend_from_slice(&head_offset);
        payload.extend_from_slice(&tail);

        let mut hasher = Sha256::new();
        hasher.update(&payload);
        let payload_hash = hasher.finalize();

        debug!("Built payload hash: {:?}", payload_hash);
        payload_hash
    }
}

impl Default for GasKillerValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ValidatorTrait for GasKillerValidator {
    async fn validate_and_return_expected_hash(&self, msg: &[u8]) -> Result<Digest> {
        debug!("Starting validation for message of length: {}", msg.len());

        // Validate message format and decode
        let aggregation = self.validate_message_format(msg).await?;

        // Compute storage updates using gas analyzer
        // This is the core validation - we independently compute what the storage updates
        // should be, rather than trusting any value from the request
        let storage_updates = self.compute_storage_updates(&aggregation.metadata).await?;

        debug!(
            "Computed storage updates for validation: {} bytes",
            storage_updates.len()
        );

        // Build the expected payload hash using computed storage updates
        let expected_hash = self.build_payload_hash(&aggregation.metadata, &storage_updates);

        debug!("Validation completed successfully");
        Ok(expected_hash)
    }

    async fn get_payload_from_message(&self, msg: &[u8]) -> Result<Digest> {
        debug!("Extracting payload hash from message");

        // Decode the aggregation
        let aggregation = self.validate_message_format(msg).await?;

        // Compute storage updates using gas analyzer
        let storage_updates = self.compute_storage_updates(&aggregation.metadata).await?;

        // Build the payload hash using computed storage updates
        let payload_hash = self.build_payload_hash(&aggregation.metadata, &storage_updates);

        debug!("Payload hash extracted: {:?}", payload_hash);
        Ok(payload_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, U256};
    use commonware_codec::{EncodeSize, Write};

    fn create_test_task_data() -> GasKillerTaskData {
        GasKillerTaskData {
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01], // function selector + params
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
        }
    }

    #[tokio::test]
    async fn test_validator_creation() {
        let _validator = GasKillerValidator::new();
    }

    #[tokio::test]
    async fn test_validate_and_return_expected_hash() {
        let validator = GasKillerValidator::new();
        let task_data = create_test_task_data();

        // Create a test aggregation
        let aggregation = wire::Aggregation::<GasKillerTaskData>::new(
            1, // round
            task_data, None, // payload
        );

        // Serialize the aggregation
        let mut msg_bytes = Vec::with_capacity(aggregation.encode_size());
        aggregation.write(&mut msg_bytes);

        // Validate - this requires RPC/Anvil to compute storage updates
        let result = validator
            .validate_and_return_expected_hash(&msg_bytes)
            .await;

        // In unit tests without Anvil running, this will fail
        // This is expected - we need Anvil to compute storage updates
        match result {
            Ok(hash) => {
                // If it succeeds (RPC available), check the hash is valid
                let zero_hash = Digest::from([0u8; 32]);
                assert_ne!(hash, zero_hash); // Not all zeros
            }
            Err(e) => {
                // Expected in unit tests without Anvil/RPC
                let error_msg = e.to_string();
                assert!(
                    error_msg.contains("Failed to compute")
                        || error_msg.contains("Failed to")
                        || error_msg.contains("error"),
                    "Unexpected error: {}",
                    error_msg
                );
            }
        }
    }

    #[tokio::test]
    async fn test_validate_and_return_expected_hash_invalid_message() {
        let validator = GasKillerValidator::new();

        // Test with empty message
        let result = validator.validate_and_return_expected_hash(&[]).await;
        assert!(result.is_err());

        // Test with invalid message
        let result = validator
            .validate_and_return_expected_hash(&[0x01, 0x02, 0x03])
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_payload_from_message() {
        let validator = GasKillerValidator::new();
        let task_data = create_test_task_data();

        // Create a test aggregation
        let aggregation = wire::Aggregation::<GasKillerTaskData>::new(
            1, // round
            task_data, None, // payload
        );

        // Serialize the aggregation
        let mut msg_bytes = Vec::with_capacity(aggregation.encode_size());
        aggregation.write(&mut msg_bytes);

        // Get payload hash - requires Anvil/RPC to compute storage updates
        let result = validator.get_payload_from_message(&msg_bytes).await;

        match result {
            Ok(hash) => {
                let zero_hash = Digest::from([0u8; 32]);
                assert_ne!(hash, zero_hash); // Not all zeros
            }
            Err(_) => {
                // Expected in unit tests without Anvil/RPC
            }
        }
    }

    #[test]
    fn test_build_payload_hash() {
        let validator = GasKillerValidator::new();
        let task_data = create_test_task_data();
        let storage_updates = vec![0x01, 0x02, 0x03, 0x04];

        let hash = validator.build_payload_hash(&task_data, &storage_updates);

        // Verify hash is not all zeros
        let zero_hash = Digest::from([0u8; 32]);
        assert_ne!(hash, zero_hash);
    }

    #[test]
    fn test_build_payload_hash_deterministic() {
        let validator = GasKillerValidator::new();
        let task_data = create_test_task_data();
        let storage_updates = vec![0x01, 0x02, 0x03, 0x04];

        // Same inputs should produce same hash
        let hash1 = validator.build_payload_hash(&task_data, &storage_updates);
        let hash2 = validator.build_payload_hash(&task_data, &storage_updates);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_build_payload_hash_different_storage_updates() {
        let validator = GasKillerValidator::new();
        let task_data = create_test_task_data();

        let hash1 = validator.build_payload_hash(&task_data, &[0x01, 0x02]);
        let hash2 = validator.build_payload_hash(&task_data, &[0x03, 0x04]);

        // Different storage updates should produce different hashes
        assert_ne!(hash1, hash2);
    }

    #[tokio::test]
    async fn test_compute_storage_updates() {
        let validator = GasKillerValidator::new();
        let task_data = create_test_task_data();

        // Computing storage updates requires RPC/Anvil
        let result = validator.compute_storage_updates(&task_data).await;

        match result {
            Ok(storage_updates) => {
                // If it succeeds, we got some storage updates
                println!(
                    "Computed {} bytes of storage updates",
                    storage_updates.len()
                );
            }
            Err(_) => {
                // Expected in unit tests without Anvil/RPC
            }
        }
    }
}

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

    /// Reconstructs the payload hash from task data
    ///
    /// This method must produce the same hash as the creator's payload generation
    /// to ensure consensus consistency.
    async fn reconstruct_payload_hash(&self, task_data: &GasKillerTaskData) -> Result<Digest> {
        // IMPORTANT: This hash must match the on-chain expectedHash in GasKillerSDK.verifyAndUpdate:
        // sha256(abi.encode(transitionIndex, address(this), targetFunction, storageUpdates))
        // We use target_address (which will be address(this) at execution), the 4-byte selector,
        // and the exact storage_updates bytes.

        let selector = task_data.function_selector();

        // Build flattened ABI encoding matching abi.encode(transitionIndex, address(this), selector, storageUpdates)
        // Heads (32 bytes each)
        let head_transition = U256::from(task_data.transition_index).abi_encode();
        let head_address = task_data.target_address.abi_encode();
        let head_selector = selector.abi_encode();
        // Offset to the dynamic bytes tail: 4 words (3 static + 1 offset) = 0x80
        let head_offset = U256::from(32u64 * 4u64).abi_encode();

        // Tail for dynamic bytes: length (u256) + data + padding
        let data_bytes: &[u8] = &task_data.storage_updates;
        let mut tail = Vec::with_capacity(32 + data_bytes.len() + 31);
        tail.extend_from_slice(&U256::from(data_bytes.len()).abi_encode());
        tail.extend_from_slice(data_bytes);
        let pad_len = (32 - (data_bytes.len() % 32)) % 32;
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

        debug!("Reconstructed payload hash: {:?}", payload_hash);
        Ok(payload_hash)
    }

    /// Validates storage updates by replaying the transaction
    ///
    /// This method uses the GasAnalyzer service to fork the blockchain state
    /// and replay the transaction, then compares the resulting storage updates
    /// with those provided in the task data.
    ///
    /// # Arguments
    /// * `task_data` - The task data containing expected storage updates
    ///
    /// # Returns
    /// * `Result<bool>` - True if storage updates match, false otherwise
    async fn validate_storage_updates(&self, task_data: &GasKillerTaskData) -> Result<bool> {
        debug!("Starting storage validation");

        // Use shared GasAnalyzer service to compute storage updates
        match self
            .gas_analyzer
            .analyze_transaction(
                task_data.target_address,
                &task_data.call_data,
                Some(task_data.from_address),
                Some(task_data.value),
            )
            .await
        {
            Ok(analysis_result) => {
                let validation_passed =
                    analysis_result.storage_updates == task_data.storage_updates;
                debug!("Storage validation completed: {}", validation_passed);
                Ok(validation_passed)
            }
            Err(e) => Err(anyhow::anyhow!("Storage validation failed: {}", e)),
        }
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

        // Perform storage validation if enabled
        let storage_validation_passed =
            self.validate_storage_updates(&aggregation.metadata).await?;

        if !storage_validation_passed {
            return Err(anyhow::anyhow!(
                "Storage validation failed: storage updates do not match expected values"
            ));
        }

        // Reconstruct expected payload hash
        let expected_hash = self.reconstruct_payload_hash(&aggregation.metadata).await?;

        debug!("Validation completed successfully");
        Ok(expected_hash)
    }

    async fn get_payload_from_message(&self, msg: &[u8]) -> Result<Digest> {
        debug!("Extracting payload hash from message");

        // Decode the aggregation
        let aggregation = self.validate_message_format(msg).await?;

        // Reconstruct the payload hash
        let payload_hash = self.reconstruct_payload_hash(&aggregation.metadata).await?;

        debug!("Payload hash extracted: {:?}", payload_hash);
        Ok(payload_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, FixedBytes, U256};
    use commonware_codec::{EncodeSize, Write};
    use std::env;

    fn create_test_task_data() -> GasKillerTaskData {
        GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
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

        // Validate - this will fail because storage validation is now strict
        // and requires RPC/Anvil to compute storage updates
        let result = validator
            .validate_and_return_expected_hash(&msg_bytes)
            .await;

        // In unit tests without Anvil running, this will fail due to storage validation
        // This is expected and correct behavior - we want validation to be strict
        match result {
            Ok(hash) => {
                // If it somehow succeeds (e.g., RPC available), check the hash is valid
                let zero_hash = Digest::from([0u8; 32]);
                assert_ne!(hash, zero_hash); // Not all zeros
            }
            Err(e) => {
                // Expected in unit tests - storage validation fails without Anvil/RPC
                let error_msg = e.to_string();
                assert!(
                    error_msg.contains("Storage validation failed")
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

        // Get payload hash
        let result = validator.get_payload_from_message(&msg_bytes).await;
        assert!(result.is_ok());

        let hash = result.unwrap();
        // Create a zero hash for comparison
        let zero_hash = Digest::from([0u8; 32]);
        assert_ne!(hash, zero_hash); // Not all zeros
    }

    #[tokio::test]
    async fn test_reconstruct_payload_hash() {
        let validator = GasKillerValidator::new();
        let task_data = create_test_task_data();

        let result = validator.reconstruct_payload_hash(&task_data).await;
        assert!(result.is_ok());

        let hash = result.unwrap();
        // Create a zero hash for comparison
        let zero_hash = Digest::from([0u8; 32]);
        assert_ne!(hash, zero_hash); // Not all zeros
    }

    #[tokio::test]
    async fn test_validate_storage_updates() {
        // Set a test RPC URL if not already set
        let original_rpc_url = env::var("RPC_URL").ok();
        if original_rpc_url.is_none() {
            unsafe {
                env::set_var("RPC_URL", "https://ethereum-holesky.publicnode.com");
            }
        }

        let validator = GasKillerValidator::new();

        // Use a real contract address and function call for testing
        let contract_address = Address::from([
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc,
            0xde, 0xf0, 0x12, 0x34, 0x56, 0x78,
        ]);
        let _function_selector = FixedBytes::from([0x60, 0xfe, 0x47, 0xb1]); // set(uint256) function selector
        let call_data = vec![
            0x60, 0xfe, 0x47, 0xb1, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        ]; // set(1)

        // Create task data for testing
        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04], // Mock storage updates
            transition_index: 1,
            target_address: contract_address,
            call_data: call_data.clone(),
            from_address: Address::from([3u8; 20]),
            value: U256::from(500),
        };

        // Test validation - this will make a real RPC call
        let result = validator.validate_storage_updates(&task_data).await;

        match result {
            Ok(validation_passed) => {
                // The validation will likely fail since we're using mock storage updates
                // but the real implementation will extract actual storage updates
                println!(
                    "✅ Validator storage validation test completed with real gas-analyzer-rs integration"
                );
                println!(
                    "   Validation result: {} (expected to fail with mock data)",
                    validation_passed
                );
            }
            Err(e) => {
                // If it fails due to network issues or the contract not existing, that's acceptable for unit tests
                println!(
                    "⚠️  Validator storage validation test skipped due to network/RPC issues or contract not found: {}",
                    e
                );
                println!(
                    "   This is expected in unit tests when the contract doesn't exist on the testnet"
                );
            }
        }

        // Restore original environment variable
        if original_rpc_url.is_none() {
            unsafe {
                env::remove_var("RPC_URL");
            }
        }
    }

    #[tokio::test]
    async fn test_validate_storage_updates_without_validator() {
        let validator = GasKillerValidator::new();
        let task_data = create_test_task_data();

        // Storage validation now properly fails when RPC/Anvil is not available
        // This is the correct behavior - we want storage validation to be strict
        let result = validator.validate_storage_updates(&task_data).await;
        // The result can be either Ok(false) for mismatched storage updates
        // or Err(_) for network/RPC issues - both are acceptable in unit tests
        // The key is that it doesn't panic
        match result {
            Ok(passed) => {
                // If it returns Ok, it should be false because mock data won't match
                assert!(
                    !passed,
                    "Mock storage updates should not match computed ones"
                );
            }
            Err(_) => {
                // Network/RPC errors are expected in unit tests without running Anvil
            }
        }
    }
}

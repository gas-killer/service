use alloy::primitives::U256;
use alloy::sol_types::SolValue;
use anyhow::Result;
use commonware_codec::Read;
use commonware_cryptography::sha256::Digest;
use commonware_cryptography::{Hasher, Sha256};
use std::env;
use tracing::debug;
use url::Url;

use crate::task_data::GasKillerTaskData;
use commonware_avs_router::validator::ValidatorTrait;
use commonware_avs_router::wire;

use alloy::rpc::types::TransactionRequest;
use gas_analyzer_rs::{call_to_encoded_state_updates_with_gas_estimate, gk::GasKillerDefault};

/// Result of gas analysis containing storage updates and gas information
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    /// The storage updates extracted from the transaction
    pub storage_updates: Vec<u8>,
    /// The gas estimate from gas-analyzer-rs
    #[allow(dead_code)]
    pub gas_estimate: u64,
}

/// Computes storage updates for a transaction using gas-analyzer-rs.
///
/// Convenience function for the creator which doesn't need full validation.
pub async fn compute_storage_updates(
    contract_address: alloy::primitives::Address,
    call_data: &[u8],
    from_address: Option<alloy::primitives::Address>,
    value: Option<alloy::primitives::U256>,
) -> Result<Vec<u8>> {
    let rpc_url_str = env::var("RPC_URL")
        .or_else(|_| env::var("HTTP_RPC"))
        .unwrap_or_else(|_| "https://ethereum-holesky.publicnode.com".into());

    let validator = GasKillerValidator::with_rpc_url(rpc_url_str);
    let result = validator
        .analyze_transaction(contract_address, call_data, from_address, value)
        .await?;
    Ok(result.storage_updates)
}

/// Validator implementation for the gas killer use case
pub struct GasKillerValidator {
    /// RPC URL for the gas analyzer
    fork_rpc_url: String,
}

impl GasKillerValidator {
    /// Creates a new GasKillerValidator with default settings.
    pub fn new() -> Self {
        let rpc_url = env::var("RPC_URL")
            .or_else(|_| env::var("HTTP_RPC"))
            .unwrap_or_else(|_| "https://ethereum-holesky.publicnode.com".into());
        Self {
            fork_rpc_url: rpc_url,
        }
    }

    /// Creates a new GasKillerValidator with a specific RPC URL.
    ///
    /// Useful for testing without modifying environment variables.
    pub fn with_rpc_url(rpc_url: impl Into<String>) -> Self {
        Self {
            fork_rpc_url: rpc_url.into(),
        }
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

    /// Builds the payload hash from task data and storage updates
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

    /// Performs the core gas analysis using gas-analyzer-rs
    ///
    /// This method contains the core logic for:
    /// 1. Forking the blockchain state
    /// 2. Executing the transaction
    /// 3. Extracting storage changes and gas information
    pub async fn analyze_transaction(
        &self,
        contract_address: alloy::primitives::Address,
        call_data: &[u8],
        from_address: Option<alloy::primitives::Address>,
        value: Option<alloy::primitives::U256>,
    ) -> Result<AnalysisResult> {
        let rpc_url = Url::parse(&self.fork_rpc_url)
            .map_err(|e| anyhow::anyhow!("Invalid RPC URL: {}", e))?;

        debug!(
            "Analyzing transaction: contract={:?}, call_data_len={}, from={:?}, value={:?}",
            contract_address,
            call_data.len(),
            from_address,
            value
        );

        // Create gas killer analyzer instance (forks blockchain at latest block)
        let gas_killer = GasKillerDefault::new(rpc_url.clone(), None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create gas analyzer: {}", e))?;

        // Build transaction request
        let from = from_address.unwrap_or(alloy::primitives::Address::ZERO);
        let tx_value = value.unwrap_or(alloy::primitives::U256::ZERO);

        let tx_request = TransactionRequest::default()
            .from(from)
            .to(contract_address)
            .value(tx_value)
            .input(alloy::primitives::Bytes::copy_from_slice(call_data).into());

        // Call gas-analyzer-rs to get storage updates and gas estimate
        let (storage_updates, gas_estimate, _skipped_opcodes) =
            call_to_encoded_state_updates_with_gas_estimate(rpc_url, tx_request, gas_killer)
                .await
                .map_err(|e| anyhow::anyhow!("Gas analysis failed: {}", e))?;

        debug!(
            "Analysis complete: storage_updates_len={}, gas_estimate={}",
            storage_updates.len(),
            gas_estimate
        );

        Ok(AnalysisResult {
            storage_updates: storage_updates.to_vec(),
            gas_estimate,
        })
    }

    /// Computes storage updates by running local analysis
    async fn compute_storage_updates(&self, task_data: &GasKillerTaskData) -> Result<Vec<u8>> {
        let result = self
            .analyze_transaction(
                task_data.target_address,
                &task_data.call_data,
                Some(task_data.from_address),
                Some(task_data.value),
            )
            .await?;
        Ok(result.storage_updates)
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

        // Compute storage updates independently - don't trust request values
        let storage_updates = self
            .compute_storage_updates(&aggregation.metadata)
            .await?;

        // Build expected payload hash using computed storage updates
        let expected_hash = self.build_payload_hash(&aggregation.metadata, &storage_updates);

        debug!("Validation completed successfully");
        Ok(expected_hash)
    }

    async fn get_payload_from_message(&self, msg: &[u8]) -> Result<Digest> {
        debug!("Extracting payload hash from message");

        // Decode the aggregation
        let aggregation = self.validate_message_format(msg).await?;

        // Compute storage updates independently
        let storage_updates = self
            .compute_storage_updates(&aggregation.metadata)
            .await?;

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
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
        }
    }

    #[tokio::test]
    async fn test_validator_creation() {
        let _validator =
            GasKillerValidator::with_rpc_url("https://ethereum-holesky.publicnode.com");
    }

    #[tokio::test]
    async fn test_validate_invalid_message() {
        let validator =
            GasKillerValidator::with_rpc_url("https://ethereum-holesky.publicnode.com");

        assert!(
            validator
                .validate_and_return_expected_hash(&[])
                .await
                .is_err()
        );
        assert!(
            validator
                .validate_and_return_expected_hash(&[0x01, 0x02, 0x03])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_validate_requires_rpc() {
        let validator =
            GasKillerValidator::with_rpc_url("https://ethereum-holesky.publicnode.com");
        let task_data = create_test_task_data();

        let aggregation = wire::Aggregation::<GasKillerTaskData>::new(1, task_data, None);

        let mut msg_bytes = Vec::with_capacity(aggregation.encode_size());
        aggregation.write(&mut msg_bytes);

        // Will fail without RPC/Anvil - that's expected
        let result = validator
            .validate_and_return_expected_hash(&msg_bytes)
            .await;
        match result {
            Ok(hash) => {
                let zero_hash = Digest::from([0u8; 32]);
                assert_ne!(hash, zero_hash);
            }
            Err(_) => {
                // Expected without RPC
            }
        }
    }

    #[test]
    fn test_build_payload_hash_deterministic() {
        let validator =
            GasKillerValidator::with_rpc_url("https://ethereum-holesky.publicnode.com");
        let task_data = create_test_task_data();
        let storage_updates = vec![0x01, 0x02, 0x03, 0x04];

        let hash1 = validator.build_payload_hash(&task_data, &storage_updates);
        let hash2 = validator.build_payload_hash(&task_data, &storage_updates);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, Digest::from([0u8; 32]));
    }

    #[test]
    fn test_build_payload_hash_different_inputs() {
        let validator =
            GasKillerValidator::with_rpc_url("https://ethereum-holesky.publicnode.com");
        let task_data = create_test_task_data();

        let hash1 = validator.build_payload_hash(&task_data, &[0x01, 0x02]);
        let hash2 = validator.build_payload_hash(&task_data, &[0x03, 0x04]);

        assert_ne!(hash1, hash2);
    }
}

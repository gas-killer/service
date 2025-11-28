//! Gas Killer Node Validator
//!
//! Validates aggregation messages and reconstructs the expected payload hash.

use alloy::primitives::U256;
use alloy::sol_types::SolValue;
use anyhow::Result;
use commonware_avs_core::validator::ValidatorTrait;
use commonware_avs_core::wire;
use commonware_codec::Read;
use commonware_cryptography::sha256::Digest;
use commonware_cryptography::{Hasher, Sha256};
use gas_killer_common::GasKillerTaskData;
use tracing::debug;

/// Validator implementation for the gas killer node
pub struct GasKillerNodeValidator;

impl GasKillerNodeValidator {
    pub fn new() -> Self {
        Self
    }

    /// Validates the message format and decodes the aggregation
    fn validate_message_format(&self, msg: &[u8]) -> Result<wire::Aggregation<GasKillerTaskData>> {
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
    fn reconstruct_payload_hash(&self, task_data: &GasKillerTaskData) -> Result<Digest> {
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
}

impl Default for GasKillerNodeValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ValidatorTrait for GasKillerNodeValidator {
    async fn validate_and_return_expected_hash(&self, msg: &[u8]) -> Result<Digest> {
        debug!("Starting validation for message of length: {}", msg.len());

        // Validate message format and decode
        let aggregation = self.validate_message_format(msg)?;

        // Reconstruct expected payload hash
        let expected_hash = self.reconstruct_payload_hash(&aggregation.metadata)?;

        debug!("Validation completed successfully");
        Ok(expected_hash)
    }

    async fn get_payload_from_message(&self, msg: &[u8]) -> Result<Digest> {
        debug!("Extracting payload hash from message");

        // Decode the aggregation
        let aggregation = self.validate_message_format(msg)?;

        // Reconstruct the payload hash
        let payload_hash = self.reconstruct_payload_hash(&aggregation.metadata)?;

        debug!("Payload hash extracted: {:?}", payload_hash);
        Ok(payload_hash)
    }
}

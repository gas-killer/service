use crate::usecases::gas_killer::task_data::GasKillerTaskData;
use alloy::sol_types::SolValue;
use anyhow::Result;

/// Minimal creator implementation for the gas killer use case
///
/// This is a placeholder implementation that only provides the payload creation
/// functionality needed by validator tests. The real creator is being implemented
/// by another developer.
#[derive(Debug)]
pub struct GasKillerCreator;

impl GasKillerCreator {
    /// Creates payload bytes from the task data
    ///
    /// The payload is deterministically encoded from the task data to ensure
    /// that the same analysis produces the same payload hash for consensus.
    /// This method is used by validator tests to verify hash consistency.
    #[allow(dead_code)]
    pub fn create_payload_from_task_data(task_data: &GasKillerTaskData) -> Result<Vec<u8>> {
        // Create a deterministic payload by ABI-encoding key fields
        // This must match the logic in GasKillerValidator::reconstruct_payload_hash
        let payload_data = (
            task_data.transition_index,
            task_data.target_address,
            task_data.from_address,
            task_data.value,
            task_data.call_data.clone(),
        );

        let payload = payload_data.abi_encode();
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, U256};

    #[tokio::test]
    async fn test_create_payload_from_task_data() {
        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
        };

        let result = GasKillerCreator::create_payload_from_task_data(&task_data);
        assert!(result.is_ok());

        let payload = result.unwrap();
        assert!(!payload.is_empty());
    }

    #[tokio::test]
    async fn test_creator_validator_hash_consistency() {
        use crate::usecases::gas_killer::validator::GasKillerValidator;

        let validator = GasKillerValidator::new();

        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
        };

        // Create payload using creator
        let creator_payload = GasKillerCreator::create_payload_from_task_data(&task_data).unwrap();

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

use super::task::GasKillerTaskData;
use crate::validator::interface::ValidatorTrait;
use crate::wire;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use commonware_codec::DecodeExt;
use commonware_cryptography::{Hasher, Sha256, sha256::Digest};
use tracing::{debug, info};

/// Gas Killer validator for validating gas optimization tasks
pub struct GasKillerValidator {}

impl GasKillerValidator {
    /// Creates a new GasKillerValidator
    pub fn new() -> Self {
        info!("Creating Gas Killer validator");
        Self {}
    }
}

impl Default for GasKillerValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ValidatorTrait for GasKillerValidator {
    /// Validates a message and returns the expected hash
    async fn validate_and_return_expected_hash(&self, msg: &[u8]) -> Result<Digest> {
        debug!("Validating gas killer message");

        // Decode the aggregation message
        let aggregation: wire::Aggregation<GasKillerTaskData> = wire::Aggregation::decode(msg)?;

        // Validate task data
        let task_data = &aggregation.metadata;

        if task_data.chain_id == 0 {
            return Err(anyhow!("Invalid chain ID: 0"));
        }

        if task_data.target_contract == alloy_primitives::Address::ZERO {
            return Err(anyhow!("Invalid target contract: zero address"));
        }

        if task_data.target_method.is_empty() {
            return Err(anyhow!("Target method cannot be empty"));
        }

        // Compute hash of the task data
        let mut hasher = Sha256::new();
        hasher.update(task_data.task_id.as_bytes());
        hasher.update(&task_data.chain_id.to_le_bytes());
        hasher.update(task_data.target_contract.as_slice());
        hasher.update(task_data.target_method.as_bytes());
        hasher.update(&task_data.params);

        Ok(hasher.finalize())
    }

    /// Extracts and hashes the payload from a message
    async fn get_payload_from_message(&self, msg: &[u8]) -> Result<Digest> {
        debug!("Extracting payload from gas killer message");

        // Decode the aggregation message
        let aggregation: wire::Aggregation<GasKillerTaskData> = wire::Aggregation::decode(msg)?;

        // Hash the task data
        let task_data = &aggregation.metadata;
        let mut hasher = Sha256::new();
        hasher.update(task_data.task_id.as_bytes());
        hasher.update(&task_data.chain_id.to_le_bytes());
        hasher.update(task_data.target_contract.as_slice());
        hasher.update(task_data.target_method.as_bytes());
        hasher.update(&task_data.params);

        Ok(hasher.finalize())
    }
}

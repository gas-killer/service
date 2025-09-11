use crate::bindings::blssigcheckoperatorstateretriever::BLSSigCheckOperatorStateRetriever::getNonSignerStakesAndSignatureReturn;
use alloy_primitives::{Bytes, FixedBytes};
use anyhow::Result;
use async_trait::async_trait;

use super::types::BlsVerificationData;

/// BLS-specific executor trait that handles the lower-level BLS signature verification
#[async_trait]
pub trait BlsExecutorTrait<T = ()>: Send + Sync
where
    T: Send + Sync,
{
    async fn execute_bls_verification(
        &mut self,
        payload_hash: &[u8],
        verification_data: BlsVerificationData,
        task_data: Option<&T>,
    ) -> Result<crate::executor::core::ExecutionResult>;
}

/// Contract-specific handler for BLS signature verification in EigenLayer context
///
/// This trait supports both basic verification (without task data) and enhanced verification
/// (with task data) through an optional parameter. Implementations can choose to:
/// 1. Return an error if task data is required but not provided (recommended for safety)
/// 2. Use default/placeholder values if task data is not provided (not recommended)
/// 3. Use the provided task data for enhanced functionality
///
/// Note: It's recommended to validate task data and return clear error messages
/// rather than using placeholder values that may cause issues later.
#[async_trait]
pub trait BlsSignatureVerificationHandler<T = ()>: Send + Sync
where
    T: Send + Sync,
{
    async fn handle_verification(
        &mut self,
        msg_hash: FixedBytes<32>,
        quorum_numbers: Bytes,
        current_block_number: u32,
        non_signer_data: getNonSignerStakesAndSignatureReturn,
        task_data: Option<&T>,
    ) -> Result<crate::executor::core::ExecutionResult>;
}

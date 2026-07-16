//! Schnorr submitter: turns aggregate-signature observations into on-chain
//! `verifyAndUpdate` calls (`SIGNATURE_SCHEME=schnorr` mode).
//!
//! Mirror of [`commonware_avs_router::submitter::Submitter`] over the schnorr
//! coordinator's [`SchnorrCertified`] observations instead of the reporter's BLS
//! certificates. Dispositions are identical:
//!
//! - `skip_digest(height)`: the coordinator gave up on the height — log and
//!   notify the sequencer ([`ResolutionKind::Skipped`]); nothing goes on-chain.
//! - No assignment / digest mismatch: [`ResolutionKind::Foreign`].
//! - Expected digest: submit the single aggregate signature `(s, Raddr)` plus the
//!   strictly ascending non-signer list through
//!   [`GasKillerHandler::handle_schnorr_verification`], with bounded retries.
//!
//! The proof arguments are constant-size regardless of signer count — that is the
//! entire point of the aggregate scheme.

use crate::executor::GasKillerHandler;
use crate::factories::SimpleWalletProvider;
use crate::schnorr_coordinator::{SchnorrCertified, SchnorrCertifiedReceiver};
use alloy_primitives::{Address, FixedBytes, U256};
use alloy_provider::Provider;
use anyhow::Result;
use commonware_avs_core::wire::skip_digest;
use commonware_avs_router::executor::ExecutionResult;
use commonware_avs_router::sequencer::{
    Resolution, ResolutionKind, ResolutionSender, SharedAssignments,
};
use commonware_cryptography::sha256::Digest;
use gas_killer_common::bindings::ReadOnlyProvider;
use gas_killer_common::schnorr::AggregateSignature;
use gas_killer_common::task_data::GasKillerTaskData;
use std::time::Duration;
use tracing::{error, info, warn};

/// Retries after the first failed submission attempt.
const MAX_RETRIES: u32 = 2;

/// Delay before the first retry; doubles per attempt.
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// Consumes aggregate-signature observations and drives the on-chain submission.
pub struct SchnorrSubmitter {
    /// L1 read-side provider: supplies the reference block for the registry's
    /// aggregate-key/weight snapshot check (operator state lives on L1 only).
    view_only_provider: ReadOnlyProvider,
    /// Executes the final `verifyAndUpdate` transaction (multi-chain write side).
    handler: GasKillerHandler<SimpleWalletProvider>,
    /// Height → expected digest + task, written by the sequencer.
    assignments: SharedAssignments<GasKillerTaskData>,
    /// Verified aggregate signatures from the coordinator.
    certified: SchnorrCertifiedReceiver,
    /// Final dispositions back to the sequencer.
    resolutions: ResolutionSender,
    /// Application namespace mixed into the skip digest; kept in lockstep with the
    /// coordinator's namespace so a coordinator skip is recognized here.
    namespace: Vec<u8>,
}

impl SchnorrSubmitter {
    pub fn new(
        view_only_provider: ReadOnlyProvider,
        handler: GasKillerHandler<SimpleWalletProvider>,
        assignments: SharedAssignments<GasKillerTaskData>,
        certified: SchnorrCertifiedReceiver,
        resolutions: ResolutionSender,
        namespace: Vec<u8>,
    ) -> Self {
        Self {
            view_only_provider,
            handler,
            assignments,
            certified,
            resolutions,
            namespace,
        }
    }

    /// Runs until the coordinator side of the certified channel closes.
    pub async fn run(mut self) {
        while let Some(certified) = self.certified.recv().await {
            self.handle_certified(certified).await;
        }
        info!("schnorr certified channel closed; submitter exiting");
    }

    /// Notifies the sequencer of a height's final disposition.
    fn notify(&self, height: u64, kind: ResolutionKind) {
        if self.resolutions.send(Resolution { height, kind }).is_err() {
            // The sequencer is gone; the process is shutting down.
            warn!(height, "resolution channel closed");
        }
    }

    async fn handle_certified(&mut self, certified: SchnorrCertified) {
        let SchnorrCertified {
            height,
            digest,
            signature,
            non_signers,
        } = certified;

        // Skip observation: the coordinator abandoned the height.
        if digest == skip_digest(&self.namespace, height) {
            info!(height, "skip observed; nothing to submit");
            self.notify(height, ResolutionKind::Skipped);
            return;
        }
        let Some(signature) = signature else {
            // A task digest without a signature is a coordinator bug — never
            // submit, release the height as failed.
            error!(height, "certified task digest without a signature (BUG)");
            self.notify(height, ResolutionKind::Executed { success: false });
            return;
        };

        // Look up the sequencer's assignment for this height.
        let assignment = match self.assignments.read() {
            Ok(assignments) => assignments.get(&height).cloned(),
            Err(_) => {
                error!(height, "assignments lock poisoned; treating as unassigned");
                None
            }
        };
        let Some(assignment) = assignment else {
            info!(
                height,
                digest = %digest,
                "signature for unassigned height; nothing to submit"
            );
            self.notify(height, ResolutionKind::Foreign);
            return;
        };
        if assignment.digest != digest {
            warn!(
                height,
                expected = %assignment.digest,
                certified = %digest,
                "signature digest does not match assignment; height consumed"
            );
            self.notify(height, ResolutionKind::Foreign);
            return;
        }

        // Expected digest: submit on-chain with bounded retries (same policy as
        // the BLS submitter — transient RPC errors recover, deterministic
        // rejections release the height as failed after the budget).
        let mut backoff = INITIAL_RETRY_BACKOFF;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self
                .submit(height, digest, &signature, &non_signers, &assignment.task)
                .await
            {
                Ok(result) => {
                    info!(
                        height,
                        tx = %result.transaction_hash,
                        block = ?result.block_number,
                        status = ?result.status,
                        "schnorr verifyAndUpdate submitted"
                    );
                    let success = result.status.unwrap_or(true);
                    self.notify(height, ResolutionKind::Executed { success });
                    return;
                }
                Err(error) if attempt <= MAX_RETRIES => {
                    warn!(
                        height,
                        attempt,
                        max_retries = MAX_RETRIES,
                        %error,
                        backoff_secs = backoff.as_secs_f64(),
                        "submission failed; retrying"
                    );
                    ::tokio::time::sleep(backoff).await;
                    backoff *= 2;
                }
                Err(error) => {
                    error!(
                        height,
                        attempts = attempt,
                        %error,
                        "submission failed after retries; releasing height"
                    );
                    self.notify(height, ResolutionKind::Executed { success: false });
                    return;
                }
            }
        }
    }

    /// One end-to-end submission attempt for a certified task digest.
    async fn submit(
        &mut self,
        height: u64,
        digest: Digest,
        signature: &AggregateSignature,
        non_signers: &[Address],
        task: &GasKillerTaskData,
    ) -> Result<ExecutionResult> {
        let msg_hash = FixedBytes::<32>::from_slice(digest.as_ref());

        // `(s, Raddr)` in the registry's calldata shape: the 52-byte wire encoding
        // is `s (32 BE) ‖ Raddr (20)`.
        let sig_bytes = signature.to_bytes();
        let s = U256::from_be_slice(&sig_bytes[..32]);
        let r_addr = Address::from_slice(&sig_bytes[32..]);

        // The reference block for the registry's aggregate/weight snapshot. The
        // executor passes `current - 1` on-chain so eth_estimateGas (which
        // simulates at the current block) sees a strictly past block — and the
        // registry fail-closes unless it is at or above its effectiveBlock
        // watermark (all registrations happen before the target deploys).
        let current_block_number = self
            .view_only_provider
            .get_block_number()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get block number: {}", e))?;

        info!(
            height,
            non_signers = non_signers.len(),
            reference_block = current_block_number.saturating_sub(1),
            "submitting aggregate schnorr signature"
        );

        self.handler
            .handle_schnorr_verification(
                height,
                msg_hash,
                current_block_number
                    .try_into()
                    .map_err(|e| anyhow::anyhow!("block number overflows u32: {}", e))?,
                s,
                r_addr,
                non_signers.to_vec(),
                Some(task),
            )
            .await
    }
}

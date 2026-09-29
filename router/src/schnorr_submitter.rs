//! Schnorr submitter: settles each finished signing session's task.
//!
//! A signed session renders the single aggregate signature `(s, Raddr)` plus the strictly
//! ascending non-signer list through [`GasKillerHandler::handle_schnorr_verification`], with
//! bounded retries; the handler checks the signed digest against the router's own trace before
//! rendering. A session that timed out or whose router trace failed settles its task `failed`.
//!
//! The proof arguments are constant-size regardless of signer count — that is the
//! entire point of the aggregate scheme.

use crate::executor::GasKillerHandler;
use crate::factories::SimpleWalletProvider;
use crate::metrics::HeightOutcome;
use crate::schnorr_coordinator::SessionOutcome;
use crate::sequencer::DispatchedTask;
use alloy_primitives::{Address, FixedBytes, U256};
use alloy_provider::Provider;
use anyhow::Result;
use commonware_avs_router::executor::ExecutionResult;
use gas_killer_common::bindings::ReadOnlyProvider;
use gas_killer_common::schnorr::AggregateSignature;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// Retries after the first failed submission attempt.
const MAX_RETRIES: u32 = 2;

/// Delay before the first retry; doubles per attempt.
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// Settles finished signing sessions. Each concurrent session settles through its own clone.
#[derive(Clone)]
pub struct SchnorrSubmitter {
    /// L1 read-side provider: supplies the reference block for the registry's
    /// aggregate-key/weight snapshot check (operator state lives on L1 only).
    view_only_provider: ReadOnlyProvider,
    /// Renders the final `verifyAndUpdate` payload (multi-chain write side).
    handler: GasKillerHandler<SimpleWalletProvider>,
}

impl SchnorrSubmitter {
    pub fn new(
        view_only_provider: ReadOnlyProvider,
        handler: GasKillerHandler<SimpleWalletProvider>,
    ) -> Self {
        Self {
            view_only_provider,
            handler,
        }
    }

    /// Settles `dispatched`'s task from how its session ended, which began at `started`.
    pub async fn settle(
        &mut self,
        height: u64,
        dispatched: &DispatchedTask,
        started: Instant,
        outcome: SessionOutcome,
    ) -> HeightOutcome {
        match outcome {
            SessionOutcome::Signed {
                digest,
                signature,
                non_signers,
            } => {
                self.render(
                    height,
                    dispatched,
                    started,
                    digest,
                    &signature,
                    &non_signers,
                )
                .await
            }
            SessionOutcome::TimedOut => {
                self.handler
                    .settle_failed(
                        dispatched,
                        "no aggregate signature before the round timeout",
                    )
                    .await;
                HeightOutcome::TimedOut
            }
            SessionOutcome::TraceFailed(reason) => {
                self.handler
                    .settle_failed(dispatched, &format!("task enrichment failed: {reason}"))
                    .await;
                HeightOutcome::TraceFailed
            }
        }
    }

    /// Renders a signed session with bounded retries — transient RPC errors recover,
    /// deterministic rejections fail the task after the budget. Only the last failure settles
    /// the task, so every retry can still render it.
    async fn render(
        &mut self,
        height: u64,
        dispatched: &DispatchedTask,
        started: Instant,
        digest: [u8; 32],
        signature: &AggregateSignature,
        non_signers: &[Address],
    ) -> HeightOutcome {
        let mut backoff = INITIAL_RETRY_BACKOFF;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self
                .submit(height, dispatched, started, digest, signature, non_signers)
                .await
            {
                Ok(_) => {
                    info!(
                        height,
                        task_id = dispatched.task_id,
                        "schnorr payload rendered"
                    );
                    return HeightOutcome::Ready;
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
                        "submission failed after retries; failing task"
                    );
                    self.handler
                        .settle_render_failed(dispatched, &format!("verification failed: {error}"))
                        .await;
                    return HeightOutcome::Failed;
                }
            }
        }
    }

    /// One end-to-end render attempt for a signed digest.
    async fn submit(
        &mut self,
        height: u64,
        dispatched: &DispatchedTask,
        started: Instant,
        digest: [u8; 32],
        signature: &AggregateSignature,
        non_signers: &[Address],
    ) -> Result<ExecutionResult> {
        let msg_hash = FixedBytes::<32>::from(digest);

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
                dispatched,
                started,
                msg_hash,
                current_block_number
                    .try_into()
                    .map_err(|e| anyhow::anyhow!("block number overflows u32: {}", e))?,
                s,
                r_addr,
                non_signers.to_vec(),
            )
            .await
    }
}

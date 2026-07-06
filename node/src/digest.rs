//! Shared digest resolution: what digest does this node vouch for at a height?
//!
//! Both signing paths — the aggregation engine's automaton (ECDSA mode) and the
//! Schnorr participant actor (schnorr mode) — must answer the same question the
//! same way: wait for the TaskBook's resolution of the height, then either the
//! task's validated digest (EVMSketch, retried with backoff within ~`ROUND_TIMEOUT`)
//! or the skip digest. Factored here so the two paths cannot drift.

use commonware_cryptography::sha256::Digest;
use gas_killer_common::{GasKillerTaskData, GasKillerValidator, skip_digest};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::task_book::{Resolution, TaskBookMailbox};

/// First retry delay after a validation error; doubles per attempt.
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// Ceiling for the exponential retry backoff.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// Resolves a height to the digest this node is willing to sign. Cheap to clone.
#[derive(Clone)]
pub(crate) struct DigestResolver {
    /// Source of per-height directives (fed by the router over channel 1).
    task_book: TaskBookMailbox,
    /// Recomputes storage updates via EVMSketch and hashes the expected payload.
    validator: Arc<GasKillerValidator>,
    /// Total time budget for retrying validation errors (~`ROUND_TIMEOUT`).
    retry_budget: Duration,
}

impl DigestResolver {
    /// `retry_budget` bounds how long an announced task is retried on validation
    /// errors before the height resolves to its skip digest; wire it to
    /// `round_timeout()` so the node gives up in lockstep with the router.
    pub fn new(
        task_book: TaskBookMailbox,
        validator: Arc<GasKillerValidator>,
        retry_budget: Duration,
    ) -> Self {
        Self {
            task_book,
            validator,
            retry_budget,
        }
    }

    /// Waits for the TaskBook's resolution of `height` and returns the digest this
    /// node vouches for. `None` means the TaskBook actor is gone (shutdown).
    pub async fn resolve(&self, height: u64) -> Option<Digest> {
        let resolution = match self.task_book.subscribe(height).await {
            Ok(resolution) => resolution,
            Err(_) => {
                warn!(height, "task book unavailable; abandoning digest request");
                return None;
            }
        };

        Some(match resolution {
            Resolution::Skip => {
                info!(height, "height skipped; resolving skip digest");
                skip_digest(height)
            }
            Resolution::Announce(task) => self.digest_for_announce(height, &task).await,
        })
    }

    /// Computes the expected digest for an announced task, retrying errors with
    /// backoff until `retry_budget` is spent, then falling back to the skip digest.
    ///
    /// `expected_digest_for_task` returns untyped (anyhow) errors, so transient
    /// RPC failures and deterministic validation failures are indistinguishable
    /// here; both are retried within the budget and both end in `skip_digest(h)`.
    /// That satisfies liveness rule 3 (deterministic failures resolve to skip —
    /// at worst after the budget) without ever wedging on a flaky RPC. The one
    /// cheaply detectable deterministic failure (missing block height) skips
    /// immediately.
    async fn digest_for_announce(&self, height: u64, task: &GasKillerTaskData) -> Digest {
        if task.block_height == 0 {
            // Deterministic: validation requires a fork height, and every honest
            // node rejects this identically. No point burning the retry budget.
            warn!(
                height,
                "announced task has no block height; resolving skip digest"
            );
            return skip_digest(height);
        }

        let deadline = Instant::now() + self.retry_budget;
        let mut backoff = INITIAL_RETRY_BACKOFF;
        loop {
            match self.validator.expected_digest_for_task(task).await {
                Ok(digest) => {
                    debug!(
                        height,
                        transition_index = task.transition_index,
                        ?digest,
                        "validated announced task"
                    );
                    return digest;
                }
                Err(error) if Instant::now() + backoff < deadline => {
                    debug!(
                        height,
                        %error,
                        backoff_ms = backoff.as_millis() as u64,
                        "task validation failed; retrying"
                    );
                    ::tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_RETRY_BACKOFF);
                }
                Err(error) => {
                    // Budget exhausted: treat as failed validation and resolve the
                    // skip digest so the height can still certify (the router is
                    // broadcasting Skip{h} on its own round timeout by now).
                    warn!(
                        height,
                        %error,
                        budget_secs = self.retry_budget.as_secs_f64(),
                        "task validation budget exhausted; resolving skip digest"
                    );
                    return skip_digest(height);
                }
            }
        }
    }
}

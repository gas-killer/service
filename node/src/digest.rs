//! Digest resolution for the Schnorr participant: what digest does this node vouch for?
//!
//! The coordinator's `CommitRequest` carries the task; the node recomputes its storage updates
//! with EVMSketch and hashes the expected payload, retrying errors with backoff within
//! ~`ROUND_TIMEOUT`. Every honest operator derives the same digest, because the digest of a task
//! is deterministic: `GasKillerValidator::expected_digest_for_task(task)`. The retry budget only
//! bounds how long a transient validation error is retried before the node declines to commit;
//! it never changes the digest value.

use commonware_cryptography::sha256::Digest;
use gas_killer_common::{GasKillerTaskData, GasKillerValidator};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// First retry delay after a validation error; doubles per attempt.
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// Ceiling for the exponential retry backoff.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// Resolves a task to the digest this node is willing to sign. Cheap to clone.
#[derive(Clone)]
pub(crate) struct DigestResolver {
    /// Recomputes storage updates via EVMSketch and hashes the expected payload.
    validator: Arc<GasKillerValidator>,
    /// Total time budget for retrying validation errors (~`ROUND_TIMEOUT`).
    retry_budget: Duration,
}

impl DigestResolver {
    /// `retry_budget` bounds how long a task is retried on validation errors before the node
    /// declines to commit; wire it to `round_timeout()` so the node gives up in lockstep with
    /// the router.
    pub fn new(validator: Arc<GasKillerValidator>, retry_budget: Duration) -> Self {
        Self {
            validator,
            retry_budget,
        }
    }

    /// The digest this node vouches for `task` at `height`, or `None` when it cannot be
    /// derived: the retry budget ran out, or the task is deterministically invalid.
    ///
    /// `expected_digest_for_task` returns untyped (anyhow) errors, so transient RPC failures
    /// and deterministic validation failures are indistinguishable here; both are retried
    /// within the budget. The one cheaply detectable deterministic failure (missing block
    /// height) gives up immediately.
    pub async fn resolve(&self, height: u64, task: &GasKillerTaskData) -> Option<Digest> {
        if task.block_height == 0 {
            // Deterministic: validation requires a fork height, and every honest node rejects
            // this identically. No point burning the retry budget.
            warn!(height, "task has no block height; declining to commit");
            return None;
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
                        "validated task"
                    );
                    return Some(digest);
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
                    warn!(
                        height,
                        %error,
                        budget_secs = self.retry_budget.as_secs_f64(),
                        "task validation budget exhausted; declining to commit"
                    );
                    return None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task every honest node rejects the same way gets no commit, so it can never reach a
    /// quorum; it must not wait out the retry budget first.
    #[tokio::test]
    async fn a_task_without_a_block_height_is_declined_at_once() {
        let resolver = DigestResolver::new(
            Arc::new(GasKillerValidator::with_rpc_url("http://localhost:1")),
            Duration::from_secs(600),
        );
        let task = GasKillerTaskData {
            block_height: 0,
            ..Default::default()
        };
        let resolved = tokio::time::timeout(Duration::from_secs(1), resolver.resolve(7, &task))
            .await
            .expect("declining must not wait out the budget");
        assert!(resolved.is_none());
    }
}

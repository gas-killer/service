//! Digest resolution for the Schnorr participant: what digest does this node vouch for?
//!
//! The coordinator's `CommitRequest` carries the task; the node recomputes its storage updates
//! with EVMSketch and hashes the expected payload, retrying errors with backoff. Every honest
//! operator derives the same digest, because the digest of a task is deterministic:
//! `GasKillerValidator::expected_digest_for_task(task)`. The budget (~`ROUND_TIMEOUT`) bounds the
//! whole resolution, queueing and slow traces included, before the node declines to commit; it
//! never changes the digest value.

use commonware_cryptography::sha256::Digest;
use gas_killer_common::{DigestClaim, GasKillerTaskData, GasKillerValidator};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

use crate::trace_pool::TracePool;

/// First retry delay after a validation error; doubles per attempt.
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// Ceiling for the exponential retry backoff.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// Resolves a task to the digest this node is willing to sign. Cheap to clone.
#[derive(Clone)]
pub(crate) struct DigestResolver {
    /// Recomputes storage updates via EVMSketch and hashes the expected payload.
    validator: Arc<GasKillerValidator>,
    /// Where traces run, and how many at once.
    traces: TracePool,
    /// Wall-clock budget for resolving one task, retries included (~`ROUND_TIMEOUT`).
    retry_budget: Duration,
}

impl DigestResolver {
    /// `retry_budget` bounds how long a task may take to resolve before the node declines to
    /// commit; wire it to `round_timeout()` so the node gives up in lockstep with the router.
    pub fn new(
        validator: Arc<GasKillerValidator>,
        traces: TracePool,
        retry_budget: Duration,
    ) -> Self {
        Self {
            validator,
            traces,
            retry_budget,
        }
    }

    /// The digest this node vouches for `task` at `height`, or `None` when it cannot be
    /// derived: the budget ran out, or the task is deterministically invalid. A trace still
    /// running when the budget runs out is aborted: the round it was for is over.
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

        let deadline = ::tokio::time::Instant::from_std(Instant::now() + self.retry_budget);
        let mut backoff = INITIAL_RETRY_BACKOFF;
        loop {
            // The task's flight is claimed before a trace slot, so a later attempt of a session
            // whose first trace is still queued or running waits for that trace, not behind
            // unrelated ones, and never holds a slot to do nothing.
            let outcome = ::tokio::time::timeout_at(deadline, async {
                let turn = match self.validator.claim_digest(task).await {
                    DigestClaim::Known(digest) => return Ok(digest),
                    DigestClaim::Trace(turn) => turn,
                };
                let validator = Arc::clone(&self.validator);
                let owned = task.clone();
                self.traces
                    .run(async move { validator.trace_digest(&owned, turn).await })
                    .await
            })
            .await;
            let Ok(outcome) = outcome else {
                warn!(
                    height,
                    budget_secs = self.retry_budget.as_secs_f64(),
                    "task validation ran past its budget; declining to commit"
                );
                return None;
            };
            match outcome {
                Ok(digest) => {
                    debug!(
                        height,
                        transition_index = task.transition_index,
                        ?digest,
                        "validated task"
                    );
                    return Some(digest);
                }
                Err(error) if ::tokio::time::Instant::now() + backoff < deadline => {
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
    use crate::trace_pool::TraceRuntime;
    use gas_killer_common::ValidatorMetrics;

    /// A task every honest node rejects the same way gets no commit, so it can never reach a
    /// quorum; it must not wait out the retry budget first.
    #[tokio::test]
    async fn a_task_without_a_block_height_is_declined_at_once() {
        let runtime = TraceRuntime::new(1).unwrap();
        let resolver = DigestResolver::new(
            Arc::new(GasKillerValidator::with_rpc_url("http://localhost:1")),
            runtime.pool(),
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
        drop(resolver);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }

    /// A later attempt of a session starts while the first attempt's trace is still running
    /// when the coordinator's commit stage runs out first. It must wait for that trace, not
    /// take a slot to do nothing or queue behind unrelated tasks.
    async fn a_second_resolve_of_a_task_in_flight_waits_outside_the_queue(concurrency: usize) {
        // Accepts connections into its backlog and never answers them, so traces hang.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", silent.local_addr().unwrap());
        let runtime = TraceRuntime::new(concurrency).unwrap();
        let metrics = Arc::new(ValidatorMetrics::new());
        let resolver = DigestResolver::new(
            Arc::new(GasKillerValidator::with_rpc_url(url)),
            runtime.pool().with_metrics(Arc::clone(&metrics)),
            Duration::from_secs(2),
        );
        let task = GasKillerTaskData {
            block_height: 1,
            ..Default::default()
        };
        let unrelated = GasKillerTaskData {
            block_height: 2,
            ..Default::default()
        };

        let resolve = |task: GasKillerTaskData| {
            let resolver = resolver.clone();
            tokio::spawn(async move { resolver.resolve(7, &task).await })
        };
        let leader = resolve(task.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;
        let follower = resolve(task);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let other = resolve(unrelated);
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Only the unrelated task can be short of a slot, and only when the leader holds the
        // last one.
        assert_eq!(
            metrics.validation_queue_depth.get(),
            i64::from(concurrency == 1)
        );

        for handle in [leader, follower, other] {
            assert!(handle.await.unwrap().is_none());
        }
        drop(resolver);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn with_one_slot_a_second_resolve_does_not_queue_behind_unrelated_tasks() {
        a_second_resolve_of_a_task_in_flight_waits_outside_the_queue(1).await;
    }

    #[tokio::test]
    async fn with_two_slots_a_second_resolve_leaves_the_other_slot_to_an_unrelated_task() {
        a_second_resolve_of_a_task_in_flight_waits_outside_the_queue(2).await;
    }

    /// A trace that neither answers nor errors must not outlive the round: the node gives up at
    /// the budget instead of holding a trace slot for a session that is already over.
    #[tokio::test]
    async fn a_trace_that_never_answers_is_abandoned_at_the_budget() {
        // Accepts connections into its backlog and never answers them.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", silent.local_addr().unwrap());
        let runtime = TraceRuntime::new(1).unwrap();
        let resolver = DigestResolver::new(
            Arc::new(GasKillerValidator::with_rpc_url(url)),
            runtime.pool(),
            Duration::from_millis(300),
        );
        let task = GasKillerTaskData {
            block_height: 1,
            ..Default::default()
        };
        let started = Instant::now();
        let resolved = tokio::time::timeout(Duration::from_secs(5), resolver.resolve(7, &task))
            .await
            .expect("the budget must bound a trace that never answers");
        assert!(resolved.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(resolver);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }
}

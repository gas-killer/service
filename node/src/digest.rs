//! Digest resolution for the Schnorr participant: what digest does this node vouch for?
//!
//! The coordinator's `CommitRequest` carries the task; the node recomputes its storage updates
//! with EVMSketch and hashes the expected payload, retrying errors with backoff. Every honest
//! operator derives the same digest, because the digest of a task is deterministic:
//! `GasKillerValidator::expected_digest_for_task(task)`. The caller's deadline bounds the whole
//! resolution, queueing and slow traces included, before the node declines to commit; it never
//! changes the digest value.

use commonware_cryptography::sha256::Digest;
use gas_killer_common::{
    DigestClaim, GasKillerTaskData, GasKillerValidator, NestedSpec, TracePool, ValidationErrorClass,
};
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
    /// Where traces run, and how many at once.
    traces: TracePool,
}

impl DigestResolver {
    pub fn new(validator: Arc<GasKillerValidator>, traces: TracePool) -> Self {
        Self { validator, traces }
    }

    /// Whether a nested request's expiry is one this node will sign: no further ahead of its own
    /// chain head than `NESTED_MAX_EXPIRY_BLOCKS`. A head that cannot be read declines too,
    /// since the bound cannot be checked.
    pub async fn expiry_within_bound(&self, task: &GasKillerTaskData, spec: &NestedSpec) -> bool {
        let bound = gas_killer_common::nested_max_expiry_blocks();
        let head = async {
            let chain = self
                .validator
                .detect_chain_for_address(task.target_address)
                .await?;
            self.validator.chain_head(chain).await
        };
        match head.await {
            Ok(head) if spec.expiry_block <= head.saturating_add(bound) => true,
            Ok(head) => {
                warn!(
                    expiry_block = spec.expiry_block,
                    head, bound, "nested expiry beyond this node's bound; declining to commit"
                );
                false
            }
            Err(error) => {
                warn!(%error, "cannot read the chain head to bound a nested expiry; declining to commit");
                false
            }
        }
    }

    /// The digest this node vouches for `task` at `height`, or `None` when it cannot be
    /// derived by `deadline` or the task is deterministically invalid. A trace still running at
    /// the deadline is aborted. Every attempt of a session must share one deadline, the end of
    /// the router's round: one set per attempt would let a later attempt trace the task afresh
    /// after the round it was for is over.
    ///
    /// Infra errors are retried with backoff until the deadline. A deterministic one (see
    /// [`ValidationErrorClass`]) declines once a second trace reproduces it. The router runs the
    /// same checks before it creates a task, so a node that fails one usually traced a different
    /// state than the router did — an endpoint on another fork at the head, which can clear
    /// within seconds — and one retry is cheap next to waiting out the round.
    pub async fn resolve(
        &self,
        height: u64,
        task: &GasKillerTaskData,
        nested: Option<NestedSpec>,
        deadline: Instant,
    ) -> Option<Digest> {
        if task.block_height == 0 {
            // Deterministic: validation requires a fork height, and every honest node rejects
            // this identically. No point burning the retry budget.
            warn!(height, "task has no block height; declining to commit");
            return None;
        }

        let deadline = ::tokio::time::Instant::from_std(deadline);
        let mut backoff = INITIAL_RETRY_BACKOFF;
        let mut deterministic_seen = false;
        loop {
            // The task's flight is claimed before a trace slot, so a later attempt of a session
            // whose first trace is still queued or running waits for that trace, not behind
            // unrelated ones, and never holds a slot to do nothing.
            let outcome = ::tokio::time::timeout_at(deadline, async {
                let turn = match self.validator.claim_digest_for(task, nested.as_ref()).await {
                    DigestClaim::Known(digest) => return Some(Ok(digest)),
                    DigestClaim::Trace(turn) => turn,
                };
                // `timeout_at` polls this before its timer, so a turn handed over in the tick
                // the round ends would otherwise start a trace before the timeout lands.
                if ::tokio::time::Instant::now() >= deadline {
                    return None;
                }
                let validator = Arc::clone(&self.validator);
                let owned = task.clone();
                Some(
                    self.traces
                        .run(async move { validator.trace_digest(&owned, turn).await })
                        .await,
                )
            })
            .await
            .ok()
            .flatten();
            let Some(outcome) = outcome else {
                self.validator
                    .observe_validation_error(ValidationErrorClass::Timeout);
                warn!(
                    height,
                    class = ValidationErrorClass::Timeout.as_str(),
                    "task validation ran past its round; declining to commit"
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
                Err(error) => {
                    if !self.should_retry(
                        height,
                        &error,
                        backoff,
                        deadline,
                        &mut deterministic_seen,
                    ) {
                        return None;
                    }
                    ::tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_RETRY_BACKOFF);
                }
            }
        }
    }

    /// Records a failed attempt and decides whether to retry it after `backoff`.
    /// `deterministic_seen` carries whether an earlier attempt of this resolve already failed
    /// deterministically.
    fn should_retry(
        &self,
        height: u64,
        error: &anyhow::Error,
        backoff: Duration,
        deadline: ::tokio::time::Instant,
        deterministic_seen: &mut bool,
    ) -> bool {
        let class = ValidationErrorClass::of(error);
        self.validator.observe_validation_error(class);
        if class == ValidationErrorClass::Deterministic {
            if *deterministic_seen {
                warn!(
                    height,
                    %error,
                    class = class.as_str(),
                    "task failed the same way on a second trace; declining to commit"
                );
                return false;
            }
            *deterministic_seen = true;
        }
        if ::tokio::time::Instant::now() + backoff >= deadline {
            warn!(
                height,
                %error,
                class = class.as_str(),
                "task validation failed with no time left in its round; declining to commit"
            );
            return false;
        }
        debug!(
            height,
            %error,
            class = class.as_str(),
            backoff_ms = backoff.as_millis() as u64,
            "task validation failed; retrying"
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gas_killer_common::{DeterministicFailure, TraceRuntime, ValidatorMetrics};

    /// A task every honest node rejects the same way gets no commit, so it can never reach a
    /// quorum; it must not wait out the retry budget first.
    #[tokio::test]
    async fn a_task_without_a_block_height_is_declined_at_once() {
        let runtime = TraceRuntime::new(1).unwrap();
        let resolver = DigestResolver::new(
            Arc::new(GasKillerValidator::with_rpc_url("http://localhost:1")),
            runtime.pool(),
        );
        let task = GasKillerTaskData {
            block_height: 0,
            ..Default::default()
        };
        let deadline = Instant::now() + Duration::from_secs(600);
        let resolved = tokio::time::timeout(
            Duration::from_secs(1),
            resolver.resolve(7, &task, None, deadline),
        )
        .await
        .expect("declining must not wait out the deadline");
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
        );
        let deadline = Instant::now() + Duration::from_secs(2);
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
            tokio::spawn(async move { resolver.resolve(7, &task, None, deadline).await })
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
    /// the deadline instead of holding a trace slot for a session that is already over.
    #[tokio::test]
    async fn a_trace_that_never_answers_is_abandoned_at_the_deadline() {
        // Accepts connections into its backlog and never answers them.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", silent.local_addr().unwrap());
        let (resolver, metrics, runtime) = resolver_with_metrics(&url);
        let task = GasKillerTaskData {
            block_height: 1,
            ..Default::default()
        };
        let started = Instant::now();
        let resolved = tokio::time::timeout(
            Duration::from_secs(5),
            resolver.resolve(7, &task, None, started + Duration::from_millis(300)),
        )
        .await
        .expect("the deadline must bound a trace that never answers");
        assert!(resolved.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
        // A hung provider raises no error, so only the timeout class can show it.
        assert_eq!(validation_errors(&metrics, "timeout"), 1);
        assert_eq!(validation_errors(&metrics, "infra"), 0);
        drop(resolver);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }

    /// A turn claimed once the round is already over must not start a trace. `timeout_at`
    /// polls its future before its timer, so without a check of its own a waiter that takes the
    /// turn in the tick the deadline fires would take a slot and spawn a trace before the
    /// timeout cancelled it.
    #[tokio::test]
    async fn a_turn_claimed_after_the_deadline_starts_no_trace() {
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", silent.local_addr().unwrap());
        let runtime = TraceRuntime::new(1).unwrap();
        let metrics = Arc::new(ValidatorMetrics::new());
        let resolver = DigestResolver::new(
            Arc::new(GasKillerValidator::with_rpc_url(url)),
            runtime.pool().with_metrics(Arc::clone(&metrics)),
        );
        let task = GasKillerTaskData {
            block_height: 1,
            ..Default::default()
        };

        let over = Instant::now() - Duration::from_millis(1);
        assert!(resolver.resolve(7, &task, None, over).await.is_none());
        assert_eq!(slots_requested(&metrics), None);
        drop(resolver);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }

    fn validation_errors(metrics: &ValidatorMetrics, class: &str) -> u64 {
        let prefix = format!("gas_killer_validation_errors_total{{class=\"{class}\"}} ");
        metrics
            .encode()
            .lines()
            .find_map(|line| line.strip_prefix(prefix.as_str()).map(str::to_owned))
            .map_or(0, |count| count.parse().unwrap())
    }

    fn resolver_with_metrics(url: &str) -> (DigestResolver, Arc<ValidatorMetrics>, TraceRuntime) {
        let runtime = TraceRuntime::new(1).unwrap();
        let metrics = Arc::new(ValidatorMetrics::new());
        let validator =
            GasKillerValidator::with_rpc_url(url).with_validator_metrics(Arc::clone(&metrics));
        let resolver = DigestResolver::new(Arc::new(validator), runtime.pool());
        (resolver, metrics, runtime)
    }

    /// A deterministic failure is retried once, in case this node's endpoint traced a state
    /// that has since moved, and declined when the retry reproduces it.
    #[tokio::test]
    async fn a_deterministic_failure_is_declined_once_a_retry_reproduces_it() {
        let (resolver, metrics, runtime) = resolver_with_metrics("http://localhost:1");
        let error = anyhow::Error::new(DeterministicFailure::new("no state updates"))
            .context("Gas analysis failed");
        let deadline = ::tokio::time::Instant::now() + Duration::from_secs(600);

        let mut seen = false;
        assert!(resolver.should_retry(7, &error, INITIAL_RETRY_BACKOFF, deadline, &mut seen));
        assert!(!resolver.should_retry(7, &error, INITIAL_RETRY_BACKOFF, deadline, &mut seen));
        assert_eq!(validation_errors(&metrics, "deterministic"), 2);
        assert_eq!(validation_errors(&metrics, "infra"), 0);
        drop(resolver);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }

    /// An endpoint that refuses every connection is an outage, not a verdict on the task: the
    /// node keeps retrying until the round ends, and counts every attempt as infra.
    #[tokio::test]
    async fn an_unreachable_endpoint_is_retried_as_infra_until_the_deadline() {
        let refused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", refused.local_addr().unwrap());
        drop(refused);
        let (resolver, metrics, runtime) = resolver_with_metrics(&url);
        let task = GasKillerTaskData {
            block_height: 1,
            ..Default::default()
        };
        let started = Instant::now();
        let resolved = resolver
            .resolve(7, &task, None, started + Duration::from_millis(1600))
            .await;

        assert!(resolved.is_none());
        assert!(
            started.elapsed() >= Duration::from_millis(1000),
            "gave up early"
        );
        assert!(validation_errors(&metrics, "infra") >= 2, "retried");
        assert_eq!(validation_errors(&metrics, "deterministic"), 0);
        drop(resolver);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }

    fn slots_requested(metrics: &ValidatorMetrics) -> Option<String> {
        metrics
            .encode()
            .lines()
            .find_map(|line| line.strip_prefix("gas_killer_validation_queue_wait_seconds_count "))
            .map(str::to_owned)
            .filter(|count| count != "0")
    }

    /// A later attempt of a session shares the first attempt's deadline, so once the first
    /// attempt's trace is aborted at the end of the round the later one gives up too, rather
    /// than taking the turn and tracing afresh for a session nobody will sign.
    #[tokio::test]
    async fn a_later_attempt_gives_up_with_the_first_instead_of_tracing_again() {
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", silent.local_addr().unwrap());
        let runtime = TraceRuntime::new(1).unwrap();
        let metrics = Arc::new(ValidatorMetrics::new());
        let resolver = DigestResolver::new(
            Arc::new(GasKillerValidator::with_rpc_url(url)),
            runtime.pool().with_metrics(Arc::clone(&metrics)),
        );
        let task = GasKillerTaskData {
            block_height: 1,
            ..Default::default()
        };
        let round_ends = Instant::now() + Duration::from_millis(300);

        let first = tokio::spawn({
            let (resolver, task) = (resolver.clone(), task.clone());
            async move { resolver.resolve(7, &task, None, round_ends).await }
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        let later = tokio::spawn({
            let (resolver, task) = (resolver.clone(), task.clone());
            async move { resolver.resolve(7, &task, None, round_ends).await }
        });

        assert!(first.await.unwrap().is_none());
        let started = Instant::now();
        assert!(later.await.unwrap().is_none());
        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(
            slots_requested(&metrics).as_deref(),
            Some("1"),
            "only the first attempt may ever have asked for a trace slot"
        );
        drop(resolver);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }
}

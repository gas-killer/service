//! Per-chain RPC circuit breaker.
//!
//! An RPC provider that is down takes the whole task lifecycle with it: the ingress cannot
//! validate a submission, the creator cannot analyse it, and the executor cannot submit the
//! resulting transaction. Left unwatched, that surfaces only as tasks failing one by one, with no
//! operator signal and no backpressure on new work.
//!
//! [`RpcHealth`] counts consecutive failures per chain. Once a chain reaches the configured
//! threshold it is *degraded*: `gas_killer_rpc_healthy{chain}` drops to zero and the ingress
//! refuses new submissions with `503 RPC_UNAVAILABLE` instead of accepting work that cannot
//! complete. Any single success clears it — the breaker tracks a run of failures, not a total, so
//! it never latches on a provider that has recovered.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use gas_killer_common::ChainRole;
use tracing::warn;

use crate::metrics::{MetricsCollector, chain_labels};

/// Failure run and current verdict for one chain.
struct ChainState {
    /// Failures since the last success. Reset, not decremented, so an intermittent provider that
    /// succeeds every few calls is not treated as down.
    consecutive_failures: AtomicU32,
    degraded: AtomicBool,
}

/// Tracks per-chain RPC availability and publishes it as `gas_killer_rpc_healthy{chain}`.
///
/// Cheap to share behind an `Arc`: state is a fixed map built at construction, so recording an
/// outcome takes no lock. Chains absent from the map are unconfigured and always report healthy —
/// a chain the router never calls cannot be the reason a submission is refused.
pub struct RpcHealth {
    threshold: u32,
    chains: HashMap<ChainRole, ChainState>,
    metrics: Option<Arc<MetricsCollector>>,
}

impl RpcHealth {
    /// Builds a breaker tracking `chains`, every one of them starting healthy.
    ///
    /// Seeding the gauge at construction matters: a chain that has never failed must read 1 rather
    /// than be absent, so an alert on `gas_killer_rpc_healthy == 0` fires on a real outage instead
    /// of on a router that has not yet made a call.
    pub fn new(
        threshold: u32,
        chains: impl IntoIterator<Item = ChainRole>,
        metrics: Option<Arc<MetricsCollector>>,
    ) -> Self {
        let chains: HashMap<_, _> = chains
            .into_iter()
            .map(|chain| {
                (
                    chain,
                    ChainState {
                        consecutive_failures: AtomicU32::new(0),
                        degraded: AtomicBool::new(false),
                    },
                )
            })
            .collect();
        let health = Self {
            threshold,
            chains,
            metrics,
        };
        for &chain in health.chains.keys() {
            health.publish(chain, true);
        }
        health
    }

    /// Records a successful RPC call, clearing any failure run on that chain.
    ///
    /// This is the only way out of the degraded state, so every path that talks to a chain should
    /// report its successes — otherwise a recovered provider stays marked down.
    pub fn record_success(&self, chain: ChainRole) {
        let Some(state) = self.chains.get(&chain) else {
            return;
        };
        let failures = state.consecutive_failures.swap(0, Ordering::Relaxed);
        // `swap` on the flag makes the transition exactly-once: concurrent successes race to flip
        // it and only the winner logs, so a recovery is one log line rather than one per caller.
        if state.degraded.swap(false, Ordering::Relaxed) {
            warn!(
                chain = %chain.name(),
                failure_count = failures,
                recovered_at = unix_now(),
                "RPC recovered: chain is serving requests again"
            );
            self.publish(chain, true);
        }
    }

    /// Records a failed RPC call, degrading the chain once the run reaches the threshold.
    pub fn record_failure(&self, chain: ChainRole) {
        let Some(state) = self.chains.get(&chain) else {
            return;
        };
        let failures = state.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if failures >= self.threshold && !state.degraded.swap(true, Ordering::Relaxed) {
            warn!(
                chain = %chain.name(),
                failure_count = failures,
                threshold = self.threshold,
                degraded_at = unix_now(),
                "RPC degraded: consecutive failures reached the threshold, refusing new submissions"
            );
            self.publish(chain, false);
        }
    }

    /// Whether `chain` is currently degraded. An unconfigured chain is never degraded.
    pub fn is_degraded(&self, chain: ChainRole) -> bool {
        self.chains
            .get(&chain)
            .is_some_and(|state| state.degraded.load(Ordering::Relaxed))
    }

    /// The chain whose outage is currently blocking new submissions, or `None` when work can be
    /// accepted.
    ///
    /// Only L1 blocks. Every round's certificate is anchored to an L1 reference block because
    /// operator state lives on L1, so an L1 outage means no submission can aggregate whichever
    /// chain its target executes on. A degraded L2 is recorded and alerted but does not shed
    /// traffic: an L1 target is unaffected by it, and an L2 target is refused precisely by chain
    /// detection rather than wholesale.
    pub fn blocking_chain(&self) -> Option<ChainRole> {
        self.is_degraded(ChainRole::L1).then_some(ChainRole::L1)
    }

    /// Mirrors a chain's verdict into `gas_killer_rpc_healthy{chain}`.
    fn publish(&self, chain: ChainRole, healthy: bool) {
        if let Some(m) = &self.metrics {
            m.rpc_healthy
                .get_or_create(&chain_labels(chain))
                .set(healthy as i64);
        }
    }
}

/// Current unix time in seconds, for the `degraded_at` / `recovered_at` field on a transition log
/// line. Falls back to 0 if the system clock predates the epoch.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A breaker over both chains, publishing into a fresh registry the test can read back.
    fn breaker(threshold: u32) -> (RpcHealth, Arc<MetricsCollector>) {
        let metrics = Arc::new(MetricsCollector::new());
        let health = RpcHealth::new(
            threshold,
            [ChainRole::L1, ChainRole::L2],
            Some(Arc::clone(&metrics)),
        );
        (health, metrics)
    }

    /// The published `gas_killer_rpc_healthy` value for a chain, or `None` if it has no series.
    fn gauge(metrics: &MetricsCollector, chain: ChainRole) -> Option<i64> {
        metrics
            .rpc_healthy
            .get(&chain_labels(chain))
            .map(|g| g.get())
    }

    #[test]
    fn every_watched_chain_starts_healthy() {
        let (health, metrics) = breaker(5);

        // Seeded rather than left absent: an alert on `== 0` should fire on an outage, not on a
        // router that has not made its first call.
        assert_eq!(gauge(&metrics, ChainRole::L1), Some(1));
        assert_eq!(gauge(&metrics, ChainRole::L2), Some(1));
        assert!(!health.is_degraded(ChainRole::L1));
        assert!(health.blocking_chain().is_none());
    }

    #[test]
    fn failures_below_the_threshold_do_not_degrade() {
        let (health, metrics) = breaker(5);

        for _ in 0..4 {
            health.record_failure(ChainRole::L1);
        }
        assert!(
            !health.is_degraded(ChainRole::L1),
            "a run shorter than the threshold is noise, not an outage"
        );
        assert_eq!(gauge(&metrics, ChainRole::L1), Some(1));
    }

    #[test]
    fn reaching_the_threshold_degrades_and_blocks_submissions() {
        let (health, metrics) = breaker(3);

        for _ in 0..3 {
            health.record_failure(ChainRole::L1);
        }
        assert!(health.is_degraded(ChainRole::L1));
        assert_eq!(gauge(&metrics, ChainRole::L1), Some(0));
        assert_eq!(health.blocking_chain(), Some(ChainRole::L1));

        // Further failures keep it degraded without re-announcing the transition.
        health.record_failure(ChainRole::L1);
        assert_eq!(gauge(&metrics, ChainRole::L1), Some(0));
    }

    #[test]
    fn a_success_resets_the_run_before_the_threshold() {
        let (health, _metrics) = breaker(3);

        health.record_failure(ChainRole::L1);
        health.record_failure(ChainRole::L1);
        health.record_success(ChainRole::L1);
        // The counter is reset, not decremented, so two more failures are still short of three.
        health.record_failure(ChainRole::L1);
        health.record_failure(ChainRole::L1);
        assert!(
            !health.is_degraded(ChainRole::L1),
            "an intermittent provider that keeps answering is not down"
        );
    }

    #[test]
    fn one_success_clears_a_degraded_chain() {
        let (health, metrics) = breaker(2);

        health.record_failure(ChainRole::L1);
        health.record_failure(ChainRole::L1);
        assert!(health.is_degraded(ChainRole::L1));

        health.record_success(ChainRole::L1);
        assert!(!health.is_degraded(ChainRole::L1));
        assert_eq!(gauge(&metrics, ChainRole::L1), Some(1));
        assert!(health.blocking_chain().is_none());
    }

    #[test]
    fn chains_degrade_independently_and_only_l1_blocks() {
        let (health, metrics) = breaker(2);

        health.record_failure(ChainRole::L2);
        health.record_failure(ChainRole::L2);
        assert!(health.is_degraded(ChainRole::L2));
        assert_eq!(gauge(&metrics, ChainRole::L2), Some(0));

        // L2's outage is alerted but does not shed traffic: every round anchors to an L1 reference
        // block, so L1 is what a submission cannot do without. An L1 target is unaffected, and an
        // L2 target is refused by chain detection instead.
        assert!(!health.is_degraded(ChainRole::L1));
        assert_eq!(gauge(&metrics, ChainRole::L1), Some(1));
        assert!(health.blocking_chain().is_none());
    }

    #[test]
    fn an_unwatched_chain_never_degrades() {
        let metrics = Arc::new(MetricsCollector::new());
        let health = RpcHealth::new(1, [ChainRole::L1], Some(Arc::clone(&metrics)));

        health.record_failure(ChainRole::L2);
        assert!(
            !health.is_degraded(ChainRole::L2),
            "a chain the router never calls cannot be the reason a submission is refused"
        );
        assert_eq!(gauge(&metrics, ChainRole::L2), None);
    }

    #[test]
    fn a_breaker_without_metrics_still_tracks_state() {
        let health = RpcHealth::new(1, [ChainRole::L1], None);

        health.record_failure(ChainRole::L1);
        assert_eq!(health.blocking_chain(), Some(ChainRole::L1));
        health.record_success(ChainRole::L1);
        assert!(health.blocking_chain().is_none());
    }
}

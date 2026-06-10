use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;
use std::sync::atomic::{AtomicI64, AtomicU64};

pub struct MetricsCollector {
    registry: Registry,
    /// Ingress requests that passed validation and were queued.
    pub ingress_accepted: Counter<u64, AtomicU64>,
    /// Ingress requests rejected by validation.
    pub ingress_rejected: Counter<u64, AtomicU64>,
    /// Tasks dequeued and handed to the creator for aggregation.
    pub tasks_created: Counter<u64, AtomicU64>,
    /// EVM storage-update computation duration (seconds).
    pub storage_computation_seconds: Histogram,
    /// Aggregation rounds that ended in a successful verifyAndUpdate transaction.
    pub aggregation_rounds_completed: Counter<u64, AtomicU64>,
    /// Aggregation rounds that failed (hash mismatch, tx error, etc.).
    pub aggregation_rounds_failed: Counter<u64, AtomicU64>,
    /// Full handle_verification duration including contract calls and tx submission (seconds).
    pub execution_duration_seconds: Histogram,
    /// Time from creator dispatching a task to the executor receiving threshold signatures (seconds).
    /// Captures P2P transit + node EVMSketch + BLS signing + aggregation.
    pub p2p_round_trip_seconds: Histogram,
    /// Current number of tasks sitting in the ingress queue waiting to be processed.
    pub task_queue_depth: Gauge<i64, AtomicI64>,
    /// Time to detect which chain a target contract is deployed on (seconds).
    pub executor_chain_detection_seconds: Histogram,
    /// Time for the payload-hash preflight computation (seconds).
    pub executor_hash_preflight_seconds: Histogram,
    /// Time for supportsInterface ERC-165 check (seconds).
    pub executor_supports_interface_seconds: Histogram,
    /// Time from calling verifyAndUpdate to receiving the pending tx handle (seconds).
    pub executor_tx_send_seconds: Histogram,
    /// Time waiting for the verifyAndUpdate receipt to be mined (seconds).
    pub executor_receipt_confirmation_seconds: Histogram,
}

impl MetricsCollector {
    pub fn new() -> Self {
        let mut registry = Registry::default();

        let ingress_accepted = Counter::default();
        registry.register(
            "gas_killer_ingress_requests_accepted",
            "Total ingress task requests accepted and queued",
            ingress_accepted.clone(),
        );

        let ingress_rejected = Counter::default();
        registry.register(
            "gas_killer_ingress_requests_rejected",
            "Total ingress task requests rejected by validation",
            ingress_rejected.clone(),
        );

        let tasks_created = Counter::default();
        registry.register(
            "gas_killer_tasks_created",
            "Total tasks dequeued and processed by the creator",
            tasks_created.clone(),
        );

        let storage_computation_seconds =
            Histogram::new([0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 60.0, 120.0, 300.0]);
        registry.register(
            "gas_killer_storage_computation_seconds",
            "EVM storage-update computation duration in seconds",
            storage_computation_seconds.clone(),
        );

        let aggregation_rounds_completed = Counter::default();
        registry.register(
            "gas_killer_aggregation_rounds_completed",
            "Total aggregation rounds completed with a successful verifyAndUpdate transaction",
            aggregation_rounds_completed.clone(),
        );

        let aggregation_rounds_failed = Counter::default();
        registry.register(
            "gas_killer_aggregation_rounds_failed",
            "Total aggregation rounds that failed (hash mismatch, tx error, interface check, etc.)",
            aggregation_rounds_failed.clone(),
        );

        // Fast reverts (~sub-second, fail at tx send) and confirmed runs (~block-time dominated); Buckets resolve both ends.
        let execution_duration_seconds = Histogram::new([
            0.5, 1.0, 2.0, 5.0, 8.0, 12.0, 16.0, 20.0, 24.0, 30.0, 45.0, 60.0, 120.0, 300.0,
        ]);
        registry.register(
            "gas_killer_execution_duration_seconds",
            "Duration of handle_verification including all contract calls and tx submission",
            execution_duration_seconds.clone(),
        );

        let p2p_round_trip_seconds =
            Histogram::new([0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0]);
        registry.register(
            "gas_killer_p2p_round_trip_seconds",
            "Time from creator dispatching a task to executor receiving threshold signatures (P2P transit + node EVMSketch + BLS signing + aggregation)",
            p2p_round_trip_seconds.clone(),
        );

        let task_queue_depth = Gauge::default();
        registry.register(
            "gas_killer_task_queue_depth",
            "Current number of tasks in the ingress queue awaiting processing",
            task_queue_depth.clone(),
        );

        // Single same-RPC round-trips (~5-150ms); fine low-end buckets so p50/p95 resolve.
        let rpc_buckets = [
            0.005, 0.01, 0.02, 0.03, 0.05, 0.075, 0.1, 0.15, 0.25, 0.5, 1.0, 2.5,
        ];
        let executor_chain_detection_seconds = Histogram::new(rpc_buckets);
        registry.register(
            "gas_killer_executor_chain_detection_seconds",
            "Time to detect which chain a target contract is deployed on",
            executor_chain_detection_seconds.clone(),
        );

        let executor_hash_preflight_seconds = Histogram::new(rpc_buckets);
        registry.register(
            "gas_killer_executor_hash_preflight_seconds",
            "Time for the payload-hash preflight computation",
            executor_hash_preflight_seconds.clone(),
        );

        let executor_supports_interface_seconds = Histogram::new(rpc_buckets);
        registry.register(
            "gas_killer_executor_supports_interface_seconds",
            "Time for the supportsInterface ERC-165 check",
            executor_supports_interface_seconds.clone(),
        );

        let executor_tx_send_seconds = Histogram::new([0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0]);
        registry.register(
            "gas_killer_executor_tx_send_seconds",
            "Time from calling verifyAndUpdate to receiving the pending tx handle",
            executor_tx_send_seconds.clone(),
        );

        // Block-time driven (~1-2 confirmations); dense through the 8-30s window.
        let executor_receipt_confirmation_seconds = Histogram::new([
            1.0, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 15.0, 18.0, 24.0, 30.0, 45.0, 60.0, 120.0,
        ]);
        registry.register(
            "gas_killer_executor_receipt_confirmation_seconds",
            "Time waiting for the verifyAndUpdate receipt to be mined",
            executor_receipt_confirmation_seconds.clone(),
        );

        Self {
            registry,
            ingress_accepted,
            ingress_rejected,
            tasks_created,
            storage_computation_seconds,
            aggregation_rounds_completed,
            aggregation_rounds_failed,
            execution_duration_seconds,
            p2p_round_trip_seconds,
            task_queue_depth,
            executor_chain_detection_seconds,
            executor_hash_preflight_seconds,
            executor_supports_interface_seconds,
            executor_tx_send_seconds,
            executor_receipt_confirmation_seconds,
        }
    }

    pub fn encode(&self) -> String {
        let mut output = String::new();
        encode(&mut output, &self.registry).expect("metrics encoding failed");
        output
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

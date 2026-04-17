use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;
use std::sync::atomic::AtomicU64;

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
            Histogram::new([0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 60.0, 120.0, 300.0].into_iter());
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

        let execution_duration_seconds =
            Histogram::new([1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0].into_iter());
        registry.register(
            "gas_killer_execution_duration_seconds",
            "Duration of handle_verification including all contract calls and tx submission",
            execution_duration_seconds.clone(),
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

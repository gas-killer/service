use crate::creator::{DispatchTime, TaskRounds};
use crate::factories::{
    create_creator, create_gas_killer_executor, create_listening_creator_with_server,
};
use crate::metrics::MetricsCollector;
use crate::store::SqliteStore;
use crate::{GasKillerCreatorType, GasKillerOrchestrator, GasKillerValidator};
use commonware_avs_router::executor::bls::BlsVerificationData;
use commonware_avs_router::orchestrator::builder::OrchestratorBuilder;

use commonware_runtime::{Clock, Metrics};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::info;

/// Builder extension for creating gas killer orchestrators.
///
/// This provides gas-killer-specific building functionality while
/// maintaining the separation between generic and concrete implementations.
pub struct GasKillerOrchestratorBuilder;

impl GasKillerOrchestratorBuilder {
    /// Builds a GasKillerOrchestrator using the configured builder.
    ///
    /// This method takes a configured OrchestratorBuilder and uses it
    /// to create a GasKillerOrchestrator with all the gas-killer-specific
    /// dependencies.
    ///
    /// # Arguments
    /// * `builder` - The configured orchestrator builder
    /// * `validator` - The shared validator, owned by the caller (which also runs its
    ///   speculative pre-build loop), used by both the creator and the orchestrator
    /// * `metrics` - The shared metrics collector
    /// * `context` - The runtime context the upstream executor registers its
    ///   EigenLayer state-retrieval metrics on
    ///
    /// # Returns
    /// * `Result<GasKillerOrchestrator<C>>` - The constructed gas killer orchestrator
    pub async fn build<C: Clock + Metrics>(
        builder: OrchestratorBuilder<C>,
        validator: Arc<GasKillerValidator>,
        metrics: Arc<MetricsCollector>,
        context: &impl Metrics,
    ) -> Result<GasKillerOrchestrator<C>, Box<dyn std::error::Error>> {
        // The validator is created and owned by the caller (which also spawns the speculative
        // pre-build loop on it); it is shared here by both the creator and the orchestrator.

        // Per-round dispatch timestamps for P2P round-trip measurement: the creator inserts a
        // timestamp keyed by round, the executor removes it when threshold sigs arrive.
        let dispatch_time: DispatchTime = Arc::new(Mutex::new(HashMap::new()));

        // Per-round task correlation for status transitions: the creator records which task each
        // round is aggregating, the executor settles that task ready/failed when the round resolves.
        let task_rounds: TaskRounds = Arc::new(Mutex::new(HashMap::new()));

        // Create gas-killer-specific dependencies. The ingress path opens the durable store and
        // hands it back so the executor can share the same handle for status transitions; the
        // no-ingress path has no store (no task lifecycle to drive).
        let use_ingress = std::env::var("INGRESS").unwrap_or_default().to_lowercase() == "true";
        let (task_creator, store): (GasKillerCreatorType, Option<SqliteStore>) = if use_ingress {
            let addr =
                std::env::var("INGRESS_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
            info!(address = %addr, "Using GasKiller creator with HTTP server");
            let (creator, store) = create_listening_creator_with_server(
                addr,
                Arc::clone(&validator),
                Arc::clone(&metrics),
                Arc::clone(&dispatch_time),
                Arc::clone(&task_rounds),
            )
            .await?;
            (creator, Some(store))
        } else {
            info!("Using GasKiller creator without ingress");
            (create_creator().await?, None)
        };

        let executor =
            create_gas_killer_executor(metrics, dispatch_time, context, store, task_rounds).await?;

        // Unwrap the Arc to get the validator for the orchestrator
        // This is safe because we control all references
        let validator_for_orchestrator = Arc::unwrap_or_clone(validator);

        builder.build_with::<_, _, _, BlsVerificationData>(
            task_creator,
            executor,
            validator_for_orchestrator,
        )
    }
}

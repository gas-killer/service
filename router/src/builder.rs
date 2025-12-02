use crate::factories::{
    create_creator, create_gas_killer_executor, create_listening_creator_with_server,
};
use crate::{GasKillerCreatorType, GasKillerOrchestrator, GasKillerValidator};
use commonware_avs_router::orchestrator::builder::OrchestratorBuilder;

use commonware_runtime::Clock;
use std::sync::Arc;
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
    ///
    /// # Returns
    /// * `Result<GasKillerOrchestrator<C>>` - The constructed gas killer orchestrator
    pub async fn build<C: Clock>(
        builder: OrchestratorBuilder<C>,
    ) -> Result<GasKillerOrchestrator<C>, Box<dyn std::error::Error>> {
        // Create shared validator first - used by both creator and orchestrator
        let validator = Arc::new(GasKillerValidator::new()?);

        // Create gas-killer-specific dependencies
        let use_ingress = std::env::var("INGRESS").unwrap_or_default().to_lowercase() == "true";
        let task_creator: GasKillerCreatorType = if use_ingress {
            let addr =
                std::env::var("INGRESS_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
            info!(address = %addr, "Using GasKiller creator with HTTP server");
            create_listening_creator_with_server(addr, Arc::clone(&validator)).await?
        } else {
            info!("Using GasKiller creator without ingress");
            create_creator().await?
        };

        let executor = create_gas_killer_executor().await?;

        // Unwrap the Arc to get the validator for the orchestrator
        // This is safe because we control all references
        let validator_for_orchestrator =
            Arc::try_unwrap(validator).unwrap_or_else(|arc| (*arc).clone());

        builder.build(task_creator, executor, validator_for_orchestrator)
    }
}

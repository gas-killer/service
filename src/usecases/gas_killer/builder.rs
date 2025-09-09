use crate::orchestrator::OrchestratorBuilder;
use crate::usecases::gas_killer::GasKillerOrchestrator;
use crate::usecases::gas_killer::factories::{
    create_gas_killer_creator, create_gas_killer_executor, create_gas_killer_validator,
};
use commonware_runtime::Clock;
use tracing::info;

/// Builder extension for creating Gas Killer orchestrators.
///
/// This provides gas-killer-specific building functionality while
/// maintaining separation between generic and concrete implementations.
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
        info!("Building Gas Killer orchestrator");

        // Create gas-killer-specific dependencies
        let task_creator = create_gas_killer_creator().await?;
        let executor = create_gas_killer_executor().await?;
        let validator = create_gas_killer_validator().await?;

        builder.build(task_creator, executor, validator)
    }
}

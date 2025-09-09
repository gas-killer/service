use super::creator::GasKillerCreator;
use super::executor::GasKillerExecutor;
use super::validator::GasKillerValidator;
use anyhow::Result;
use tracing::info;

/// Creates a new Gas Killer creator instance
pub async fn create_gas_killer_creator() -> Result<GasKillerCreator> {
    info!("Creating Gas Killer creator");
    
    // Create channels for queue and orchestrator communication
    let (queue_sender, queue_receiver) = tokio::sync::mpsc::unbounded_channel();
    let (orchestrator_sender, _orchestrator_receiver) = tokio::sync::mpsc::unbounded_channel();
    
    // For production, these would be connected to actual message queue and orchestrator
    // For now, we drop the queue_sender as it would be owned by the ingress component
    drop(queue_sender);
    
    Ok(GasKillerCreator::new(queue_receiver, orchestrator_sender))
}

/// Creates a new Gas Killer executor instance
pub async fn create_gas_killer_executor() -> Result<GasKillerExecutor> {
    Ok(GasKillerExecutor::new())
}

/// Creates a new Gas Killer validator instance
pub async fn create_gas_killer_validator() -> Result<GasKillerValidator> {
    Ok(GasKillerValidator::new())
}
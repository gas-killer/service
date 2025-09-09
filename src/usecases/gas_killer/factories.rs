use crate::usecases::gas_killer::creator::{GasKillerCreator, SimpleGasKillerQueue};
use crate::usecases::gas_killer::ingress::{GasKillerIngressState, start_gas_killer_http_server};
use crate::usecases::gas_killer::types::EnrichedGasKillerRequest;
use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Factory function to create Gas Killer creator with HTTP server
pub async fn create_gas_killer_creator_with_server(
    addr: String,
    max_queue_size: usize,
) -> Result<GasKillerCreator<SimpleGasKillerQueue>> {
    // Create the shared queue
    let queue_vec = Arc::new(Mutex::new(Vec::<EnrichedGasKillerRequest>::new()));

    // Create the queue wrapper
    let queue = SimpleGasKillerQueue::new(queue_vec.clone());

    // Create the creator with a poll interval
    let poll_interval = Duration::from_secs(1);
    let creator = GasKillerCreator::new(Arc::new(queue), poll_interval);

    // Create ingress state with the same queue
    let ingress_state = Arc::new(GasKillerIngressState {
        queue: queue_vec,
        max_queue_size,
    });

    // Start the HTTP server in a background task
    tokio::spawn(async move {
        if let Err(e) = start_gas_killer_http_server(ingress_state, &addr).await {
            tracing::error!("Gas Killer HTTP server error: {}", e);
        }
    });

    Ok(creator)
}

/// Factory function to create standalone Gas Killer HTTP server
pub async fn start_gas_killer_ingress(addr: String, max_queue_size: usize) -> Result<()> {
    let ingress_state = Arc::new(GasKillerIngressState::new(max_queue_size));

    start_gas_killer_http_server(ingress_state, &addr)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start Gas Killer HTTP server: {}", e))
}

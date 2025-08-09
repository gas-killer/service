use alloy::{
    hex,
    primitives::{Address, U256},
    sol_types::SolValue,
};
use alloy_provider::ProviderBuilder;
use alloy_signer_local::PrivateKeySigner;
use std::{env, str::FromStr, sync::Arc};
use tokio::sync::Mutex;
use tracing::info;

use crate::bindings::counter::Counter;
use crate::handlers::{CounterProvider, TaskCreator, TaskDetails};
use crate::ingress::{TaskRequest, start_http_server};
use commonware_eigenlayer::config::AvsDeployment;

pub struct ListeningCreator {
    counter: Counter::CounterInstance<(), CounterProvider>,
    queue: Arc<Mutex<Vec<TaskRequest>>>,
}

impl ListeningCreator {
    pub fn new(provider: CounterProvider, counter_address: Address) -> Self {
        let counter = Counter::new(counter_address, provider.clone());
        Self {
            counter,
            queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn get_current_number(&self) -> anyhow::Result<u64> {
        let current_number = self.counter.number().call().await?;
        Ok(current_number._0.to::<u64>())
    }

    pub async fn encode_number_call(&self, number: U256) -> Vec<u8> {
        number.abi_encode()
    }

    // Pulls the next task from the queue, or returns None if empty
    pub async fn get_next_task(&self) -> Option<TaskRequest> {
        let mut queue = self.queue.lock().await;
        if !queue.is_empty() {
            Some(queue.remove(0))
        } else {
            None
        }
    }

    // Single entry point that can be called by the orchestrator
    // This is where queue requests would be pulled from
    // Returns (payload, round, target_contract, target_function, function_params)
    pub async fn get_payload_and_round(&self) -> anyhow::Result<(Vec<u8>, u64, String, String, Vec<u8>)> {
        // Wait for a task to be available
        let task = loop {
            if let Some(task) = self.get_next_task().await {
                break task;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };
        let current_number = self.get_current_number().await?;
        
        // Decode the hex-encoded function parameters
        let params_hex = task.body.function_params.strip_prefix("0x")
            .unwrap_or(&task.body.function_params);
        let function_params = hex::decode(params_hex)
            .map_err(|e| anyhow::anyhow!("Failed to decode function parameters: {}", e))?;
        
        // Create a payload that combines round info and function params
        let mut payload = self.encode_number_call(U256::from(current_number)).await;
        payload.extend_from_slice(&function_params);

        Ok((
            payload,
            current_number,
            task.body.target_contract,
            task.body.target_function,
            function_params,
        ))
    }

    // Optional: Method to get payload for a specific round number
    #[allow(dead_code)]
    pub async fn get_payload_for_round(&self, round_number: u64) -> anyhow::Result<(Vec<u8>, u64)> {
        let encoded = self.encode_number_call(U256::from(round_number)).await;
        info!("Created payload for specific round: {}", round_number);
        Ok((encoded, round_number))
    }

    // Start the HTTP server in a background task
    pub async fn start_http_server(self: Arc<Self>, addr: String) {
        let queue = self.queue.clone();
        tokio::spawn(async move {
            start_http_server(queue, &addr).await;
        });
    }
}

impl TaskCreator for ListeningCreator {
    async fn get_task_details(&self) -> anyhow::Result<TaskDetails> {
        let (payload, round, target_contract, target_function, function_params) = 
            self.get_payload_and_round()
                .await
                .map_err(|e| anyhow::anyhow!("ListeningCreator error: {}", e))?;
        
        Ok(TaskDetails {
            payload,
            round,
            target_contract,
            target_function,
            function_params,
        })
    }
}

// Helper function to create a new ListeningCreator instance and start HTTP server
pub async fn create_listening_creator_with_server(
    addr: String,
) -> anyhow::Result<Arc<ListeningCreator>> {
    let http_rpc = env::var("HTTP_RPC").expect("HTTP_RPC must be set");
    let private_key = env::var("PRIVATE_KEY").expect("PRIVATE_KEY must be set");
    let signer = PrivateKeySigner::from_str(&private_key)?;
    let provider = ProviderBuilder::new()
        .wallet(signer)
        .connect(&http_rpc)
        .await?;
    let deployment =
        AvsDeployment::load().map_err(|e| anyhow::anyhow!("Failed to load deployment: {}", e))?;
    let counter_address = deployment
        .counter_address()
        .map_err(|e| anyhow::anyhow!("Failed to get counter address: {}", e))?;
    let creator = Arc::new(ListeningCreator::new(provider, counter_address));
    let server_creator = creator.clone();
    tokio::spawn(async move {
        server_creator.start_http_server(addr).await;
    });
    Ok(creator)
}

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
use crate::handlers::{CounterProvider, TaskCreator};
use crate::ingress::{TaskRequest, start_http_server};
use commonware_eigenlayer::config::AvsDeployment;

pub struct ListeningCreator {
    #[allow(dead_code)]
    counter: Counter::CounterInstance<(), CounterProvider>,
    queue: Arc<Mutex<Vec<TaskRequest>>>,
    task_counter: Arc<Mutex<u64>>,  // Internal task counter for round numbers
}

impl ListeningCreator {
    pub fn new(provider: CounterProvider, counter_address: Address) -> Self {
        let counter = Counter::new(counter_address, provider.clone());
        Self {
            counter,
            queue: Arc::new(Mutex::new(Vec::new())),
            task_counter: Arc::new(Mutex::new(0)),
        }
    }

    #[allow(dead_code)]
    pub async fn get_current_number(&self) -> anyhow::Result<u64> {
        let current_number = self.counter.number().call().await?;
        Ok(current_number._0.to::<u64>())
    }

    #[allow(dead_code)]
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
    pub async fn get_payload_and_round(&self) -> anyhow::Result<(Vec<u8>, u64)> {
        // Wait for a task to be available
        let task = loop {
            if let Some(task) = self.get_next_task().await {
                break task;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };
        
        // Increment and get the task counter for round number
        let round = {
            let mut counter = self.task_counter.lock().await;
            *counter += 1;
            *counter
        };
        
        // Decode the hex-encoded function parameters
        let params_hex = task.body.function_params.strip_prefix("0x")
            .unwrap_or(&task.body.function_params);
        let function_params = hex::decode(params_hex)
            .map_err(|e| anyhow::anyhow!("Failed to decode function parameters: {}", e))?;
        
        // Create a payload that encodes all the information:
        // [4 bytes: target_contract length][target_contract bytes]
        // [4 bytes: target_function length][target_function bytes]
        // [4 bytes: function_params length][function_params bytes]
        let mut payload = Vec::new();
        
        // Encode target contract
        let contract_bytes = task.body.target_contract.as_bytes();
        payload.extend_from_slice(&(contract_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(contract_bytes);
        
        // Encode target function
        let function_bytes = task.body.target_function.as_bytes();
        payload.extend_from_slice(&(function_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(function_bytes);
        
        // Encode function parameters
        payload.extend_from_slice(&(function_params.len() as u32).to_le_bytes());
        payload.extend_from_slice(&function_params);
        
        info!(
            "Created payload for round {} with target: {} function: {}",
            round, task.body.target_contract, task.body.target_function
        );

        Ok((payload, round))
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
    async fn get_payload_and_round(&self) -> anyhow::Result<(Vec<u8>, u64)> {
        self.get_payload_and_round()
            .await
            .map_err(|e| anyhow::anyhow!("ListeningCreator error: {}", e))
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

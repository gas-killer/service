//! Shared configuration types and utilities for Gas Killer AVS components

use commonware_avs_usecases::{EigenStakingClient, QuorumInfo};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;

/// Configuration for loading BLS private keys from JSON files
#[derive(Debug, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub struct KeyConfig {
    pub privateKey: String,
}

/// Configuration for connecting to the orchestrator
#[derive(Debug, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub struct OrchestratorConfig {
    pub g2_x1: String,
    pub g2_x2: String,
    pub g2_y1: String,
    pub g2_y2: String,
    pub port: String,
    #[serde(default)]
    pub address: Option<String>,
}

/// Loads a BLS private key from a JSON file
///
/// # Arguments
/// * `path` - Path to the JSON file containing the key
///
/// # Panics
/// Panics if the file cannot be read or parsed
pub fn load_key_from_file(path: &str) -> String {
    let contents = fs::read_to_string(path).expect("Could not read key file");
    let config: KeyConfig = serde_json::from_str(&contents).expect("Could not parse key file");
    config.privateKey
}

/// Loads orchestrator configuration from a JSON file
///
/// # Arguments
/// * `path` - Path to the JSON file containing the orchestrator config
///
/// # Panics
/// Panics if the file cannot be read or parsed
pub fn load_orchestrator_config(path: &str) -> OrchestratorConfig {
    let contents = fs::read_to_string(path).expect("Could not read orchestrator config file");
    serde_json::from_str(&contents).expect("Could not parse orchestrator config file")
}

/// Fetches operator states from the EigenLayer contracts
///
/// Reads RPC URLs and deployment path from environment variables:
/// - `HTTP_RPC`: HTTP RPC endpoint
/// - `WS_RPC`: WebSocket RPC endpoint
/// - `AVS_DEPLOYMENT_PATH`: Path to AVS deployment JSON
///
/// # Errors
/// Returns an error if environment variables are missing or RPC calls fail
pub async fn get_operator_states() -> Result<Vec<QuorumInfo>, Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    let http_rpc = env::var("HTTP_RPC").expect("HTTP_RPC must be set");
    let ws_rpc = env::var("WS_RPC").expect("WS_RPC must be set");
    let avs_deployment_path =
        env::var("AVS_DEPLOYMENT_PATH").expect("AVS_DEPLOYMENT_PATH must be set");

    let client = EigenStakingClient::new(http_rpc, ws_rpc, avs_deployment_path).await?;
    client.get_operator_states().await
}

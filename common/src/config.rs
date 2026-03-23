//! Shared configuration types and utilities for Gas Killer AVS components

use commonware_avs_eigenlayer::{EigenStakingClient, QuorumInfo};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;

/// Chain role identifiers — L1 (primary) and L2 (optional secondary).
///
/// These are role labels, not chain-specific names. The actual numeric chain ID
/// is discovered at runtime by querying `eth_chainId` on the configured RPC endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ChainId {
    /// The primary (L1) chain
    #[default]
    L1,
    /// The secondary (L2) chain
    L2,
}

impl ChainId {
    /// Returns the role name as a string
    pub fn name(&self) -> &'static str {
        match self {
            ChainId::L1 => "l1",
            ChainId::L2 => "l2",
        }
    }
}

impl std::fmt::Display for ChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// The ordered list of roles to check when detecting where a contract is deployed.
/// L1 is checked first as the primary chain.
pub const CHAIN_DETECTION_ORDER: [ChainId; 2] = [ChainId::L1, ChainId::L2];

/// Detects which chain role has code deployed at the given address.
///
/// Checks each chain in `CHAIN_DETECTION_ORDER` by calling the provided
/// async `get_code` closure. Returns the first chain where non-empty code is found.
///
/// # Arguments
/// * `address` - The contract address to look up
/// * `supported_chains` - Slice of chains the caller supports (filtered against detection order)
/// * `get_code` - Async closure `(ChainId, Address) -> Result<Bytes>` that fetches bytecode
pub async fn detect_chain_for_address<F, Fut>(
    address: alloy_primitives::Address,
    supported_chains: &[ChainId],
    get_code: F,
) -> anyhow::Result<ChainId>
where
    F: Fn(ChainId, alloy_primitives::Address) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<alloy_primitives::Bytes>>,
{
    for &chain_id in &CHAIN_DETECTION_ORDER {
        if !supported_chains.contains(&chain_id) {
            continue;
        }

        match get_code(chain_id, address).await {
            Ok(code) => {
                if !code.is_empty() {
                    tracing::debug!(
                        chain = %chain_id,
                        address = %address,
                        code_len = code.len(),
                        "Found contract code on chain"
                    );
                    return Ok(chain_id);
                }
            }
            Err(e) => {
                tracing::debug!(
                    chain = %chain_id,
                    error = %e,
                    "Failed to check code on chain"
                );
            }
        }
    }

    Err(anyhow::anyhow!(
        "No contract code found at address {} on any supported chain",
        address
    ))
}

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

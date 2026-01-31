//! Shared configuration types and utilities for Gas Killer AVS components

use commonware_avs_usecases::{EigenStakingClient, QuorumInfo};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;

/// Supported chain identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ChainId {
    /// Sepolia testnet (chain ID: 11155111)
    #[default]
    Sepolia = 11155111,
    /// Gnosis mainnet (chain ID: 100)
    Gnosis = 100,
}

impl ChainId {
    /// Creates a ChainId from a numeric chain ID
    pub fn from_u64(chain_id: u64) -> Option<Self> {
        match chain_id {
            11155111 => Some(ChainId::Sepolia),
            100 => Some(ChainId::Gnosis),
            _ => None,
        }
    }

    /// Returns the numeric chain ID
    pub fn as_u64(&self) -> u64 {
        match self {
            ChainId::Sepolia => 11155111,
            ChainId::Gnosis => 100,
        }
    }

    /// Returns the chain name
    pub fn name(&self) -> &'static str {
        match self {
            ChainId::Sepolia => "sepolia",
            ChainId::Gnosis => "gnosis",
        }
    }
}

impl std::fmt::Display for ChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Configuration for a specific chain
#[derive(Debug, Clone)]
pub struct ChainConfig {
    /// Chain identifier
    pub chain_id: ChainId,
    /// HTTP RPC endpoint
    pub http_rpc: String,
    /// WebSocket RPC endpoint
    pub ws_rpc: String,
    /// Path to AVS deployment configuration
    pub avs_deployment_path: String,
}

impl ChainConfig {
    /// Loads chain configuration from environment variables with chain-specific prefixes
    ///
    /// For Sepolia (default): HTTP_RPC, WS_RPC, AVS_DEPLOYMENT_PATH
    /// For Gnosis: GNOSIS_HTTP_RPC, GNOSIS_WS_RPC, GNOSIS_AVS_DEPLOYMENT_PATH
    pub fn from_env(chain_id: ChainId) -> Result<Self, String> {
        match chain_id {
            ChainId::Sepolia => {
                let http_rpc = env::var("HTTP_RPC")
                    .map_err(|_| "HTTP_RPC must be set for Sepolia")?;
                let ws_rpc = env::var("WS_RPC")
                    .map_err(|_| "WS_RPC must be set for Sepolia")?;
                let avs_deployment_path = env::var("AVS_DEPLOYMENT_PATH")
                    .map_err(|_| "AVS_DEPLOYMENT_PATH must be set for Sepolia")?;
                Ok(Self {
                    chain_id,
                    http_rpc,
                    ws_rpc,
                    avs_deployment_path,
                })
            }
            ChainId::Gnosis => {
                let http_rpc = env::var("GNOSIS_HTTP_RPC")
                    .map_err(|_| "GNOSIS_HTTP_RPC must be set for Gnosis")?;
                let ws_rpc = env::var("GNOSIS_WS_RPC")
                    .map_err(|_| "GNOSIS_WS_RPC must be set for Gnosis")?;
                let avs_deployment_path = env::var("GNOSIS_AVS_DEPLOYMENT_PATH")
                    .map_err(|_| "GNOSIS_AVS_DEPLOYMENT_PATH must be set for Gnosis")?;
                Ok(Self {
                    chain_id,
                    http_rpc,
                    ws_rpc,
                    avs_deployment_path,
                })
            }
        }
    }
}

/// Multi-chain configuration manager
#[derive(Debug, Clone)]
pub struct MultiChainConfig {
    /// Sepolia configuration (always required)
    pub sepolia: ChainConfig,
    /// Gnosis configuration (optional)
    pub gnosis: Option<ChainConfig>,
}

impl MultiChainConfig {
    /// Loads multi-chain configuration from environment variables
    pub fn from_env() -> Result<Self, String> {
        let sepolia = ChainConfig::from_env(ChainId::Sepolia)?;

        // Gnosis is optional - only load if env vars are present
        let gnosis = ChainConfig::from_env(ChainId::Gnosis).ok();

        Ok(Self { sepolia, gnosis })
    }

    /// Gets the configuration for a specific chain
    pub fn get_chain_config(&self, chain_id: ChainId) -> Option<&ChainConfig> {
        match chain_id {
            ChainId::Sepolia => Some(&self.sepolia),
            ChainId::Gnosis => self.gnosis.as_ref(),
        }
    }

    /// Returns the HTTP RPC URL for a given chain
    pub fn get_http_rpc(&self, chain_id: ChainId) -> Option<&str> {
        self.get_chain_config(chain_id).map(|c| c.http_rpc.as_str())
    }

    /// Returns the WS RPC URL for a given chain
    pub fn get_ws_rpc(&self, chain_id: ChainId) -> Option<&str> {
        self.get_chain_config(chain_id).map(|c| c.ws_rpc.as_str())
    }

    /// Returns all supported chain IDs
    pub fn supported_chains(&self) -> Vec<ChainId> {
        let mut chains = vec![ChainId::Sepolia];
        if self.gnosis.is_some() {
            chains.push(ChainId::Gnosis);
        }
        chains
    }
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

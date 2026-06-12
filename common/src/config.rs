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
pub enum ChainRole {
    /// The primary (L1) chain
    #[default]
    L1,
    /// The secondary (L2) chain
    L2,
}

impl ChainRole {
    /// Returns the role name as a string
    pub fn name(&self) -> &'static str {
        match self {
            ChainRole::L1 => "l1",
            ChainRole::L2 => "l2",
        }
    }

    /// Returns the configured HTTP RPC URL for this chain role.
    ///
    /// Reads `HTTP_RPC` for L1 and `L2_HTTP_RPC` for L2.
    pub fn rpc_url(&self) -> anyhow::Result<String> {
        match self {
            ChainRole::L1 => env::var("HTTP_RPC")
                .map_err(|_| anyhow::anyhow!("HTTP_RPC environment variable is not set")),
            ChainRole::L2 => env::var("L2_HTTP_RPC")
                .map_err(|_| anyhow::anyhow!("L2_HTTP_RPC environment variable is not set")),
        }
    }
}

impl std::fmt::Display for ChainRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// The ordered list of roles to check when detecting where a contract is deployed.
/// L1 is checked first as the primary chain.
pub const CHAIN_DETECTION_ORDER: [ChainRole; 2] = [ChainRole::L1, ChainRole::L2];

/// Detects which chain role has code deployed at the given address.
///
/// Checks each chain in `CHAIN_DETECTION_ORDER` by calling the provided
/// async `get_code` closure. Returns the first chain where non-empty code is found.
///
/// # Arguments
/// * `address` - The contract address to look up
/// * `supported_chains` - Slice of chains the caller supports (filtered against detection order)
/// * `get_code` - Async closure `(ChainRole, Address) -> Result<Bytes>` that fetches bytecode
pub async fn detect_chain_for_address<F, Fut>(
    address: alloy_primitives::Address,
    supported_chains: &[ChainRole],
    get_code: F,
) -> anyhow::Result<ChainRole>
where
    F: Fn(ChainRole, alloy_primitives::Address) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<alloy_primitives::Bytes>>,
{
    for &chain_role in &CHAIN_DETECTION_ORDER {
        if !supported_chains.contains(&chain_role) {
            continue;
        }

        match get_code(chain_role, address).await {
            Ok(code) => {
                if !code.is_empty() {
                    tracing::debug!(
                        chain = %chain_role,
                        address = %address,
                        code_len = code.len(),
                        "Found contract code on chain"
                    );
                    return Ok(chain_role);
                }
            }
            Err(e) => {
                tracing::debug!(
                    chain = %chain_role,
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

/// Default P2P channel message backlog depth.
///
/// The backlog bounds how many queued messages the channel will hold before the sender
/// blocks or drops new messages. Configurable at runtime via `P2P_MESSAGE_BACKLOG`.
pub const DEFAULT_P2P_MESSAGE_BACKLOG: usize = 256;

/// Default P2P channel rate limit in messages per second.
///
/// Configurable at runtime via `P2P_MESSAGES_PER_SECOND`. Accepts fractional values
/// (e.g. `0.5` for one message every two seconds).
pub const DEFAULT_P2P_MESSAGES_PER_SECOND: f64 = 1.0;

/// Reads the P2P channel backlog depth from `P2P_MESSAGE_BACKLOG`, defaulting to
/// [`DEFAULT_P2P_MESSAGE_BACKLOG`].
pub fn p2p_message_backlog() -> usize {
    env::var("P2P_MESSAGE_BACKLOG")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v: &usize| v > 0)
        .unwrap_or(DEFAULT_P2P_MESSAGE_BACKLOG)
}

/// Reads the P2P channel rate limit from `P2P_MESSAGES_PER_SECOND`, defaulting to
/// [`DEFAULT_P2P_MESSAGES_PER_SECOND`].
pub fn p2p_messages_per_second() -> f64 {
    env::var("P2P_MESSAGES_PER_SECOND")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v: &f64| v > 0.0 && v.is_finite())
        .unwrap_or(DEFAULT_P2P_MESSAGES_PER_SECOND)
}

/// Maximum age (in blocks) of a reference block, falling back to the contract default
/// when `BLOCK_STALE_MEASURE` is unset or unparseable.
///
/// Mirrors `DEFAULT_BLOCK_STALE_MEASURE` in `GasKillerSDK.sol`. The service reuses this
/// value as an off-chain policy bound: it rejects gas-analysis requests whose
/// `block_height` is older than this window (see ingress validation), and sizes the
/// speculative executor cache to cover it.
pub const DEFAULT_BLOCK_STALE_MEASURE: u64 = 300;

/// Reads the staleness window from `BLOCK_STALE_MEASURE`, defaulting to
/// [`DEFAULT_BLOCK_STALE_MEASURE`].
pub fn block_stale_measure() -> u64 {
    env::var("BLOCK_STALE_MEASURE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_BLOCK_STALE_MEASURE)
}

/// Runtime configuration for the speculative executor pre-build loop.
///
/// The loop watches each chain's head and pre-builds the EVMSketch executor for the
/// latest block so a task's first validation hits the executor cache instead of paying
/// the live `build()` cost (~80–120 ms) on the critical path.
#[derive(Clone, Copy, Debug)]
pub struct SpeculativePrebuildConfig {
    /// Whether the loop runs at all (`SPECULATIVE_PREBUILD`, default `true`).
    pub enabled: bool,
    /// How often to poll each chain's head (`SPECULATIVE_PREBUILD_POLL_MS`, default 2000).
    pub poll_interval: std::time::Duration,
    /// Blocks behind head to target (`SPECULATIVE_PREBUILD_CONFIRMATIONS`, default 0).
    ///
    /// The cached executor only feeds the (discarded) gas estimate — never the signed
    /// `storage_updates` — so building at the unconfirmed tip is consensus-safe. A
    /// non-zero depth trades a small hit-rate loss for fewer wasted builds on reorgs.
    pub confirmation_depth: u64,
}

impl SpeculativePrebuildConfig {
    /// Builds the config from environment variables, applying defaults for any unset or
    /// unparseable values.
    pub fn from_env() -> Self {
        let enabled = env::var("SPECULATIVE_PREBUILD")
            .map(|v| !matches!(v.trim().to_lowercase().as_str(), "false" | "0" | "no"))
            .unwrap_or(true);
        let poll_ms = env::var("SPECULATIVE_PREBUILD_POLL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&ms| ms > 0)
            .unwrap_or(2000);
        let confirmation_depth = env::var("SPECULATIVE_PREBUILD_CONFIRMATIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Self {
            enabled,
            poll_interval: std::time::Duration::from_millis(poll_ms),
            confirmation_depth,
        }
    }
}

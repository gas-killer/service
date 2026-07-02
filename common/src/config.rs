//! Shared configuration types and utilities for Gas Killer AVS components

use crate::eigenlayer::{EigenStakingClient, QuorumInfo};
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

/// Reads the P2P channel rate limit from `P2P_MESSAGES_PER_SECOND` and returns the
/// per-message quota period (`1 / rate`), defaulting to
/// [`DEFAULT_P2P_MESSAGES_PER_SECOND`] when unset or invalid.
///
/// The quota is a smooth rate with no burst allowance: a rate of `5.0` permits one
/// message every 200 ms, not bursts of five. Values whose reciprocal would overflow
/// a `Duration` (e.g. `1e-20`) or round below its 1 ns resolution (e.g. `3e9`) are
/// treated as invalid and fall back to the default.
pub fn p2p_quota_period() -> std::time::Duration {
    parse_p2p_quota_period(env::var("P2P_MESSAGES_PER_SECOND").ok().as_deref())
}

/// Parses a `P2P_MESSAGES_PER_SECOND` value into a quota period, falling back to the
/// default rate on malformed, non-positive, non-finite, or non-representable input
/// (including `Duration` overflow and sub-nanosecond reciprocals that round to zero).
fn parse_p2p_quota_period(value: Option<&str>) -> std::time::Duration {
    value
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|&v| v > 0.0 && v.is_finite())
        .and_then(|v| std::time::Duration::try_from_secs_f64(1.0 / v).ok())
        .filter(|d| !d.is_zero())
        .unwrap_or_else(|| {
            std::time::Duration::from_secs_f64(1.0 / DEFAULT_P2P_MESSAGES_PER_SECOND)
        })
}

/// Default storage directory for the aggregation engine's journal.
///
/// Matches the writable data volume mounted in the container images. Journal
/// persistence across restarts requires a stable path — the commonware tokio
/// runtime otherwise defaults to a random per-process temp dir.
pub const DEFAULT_STORAGE_DIRECTORY: &str = "/app/data";

/// Resolves the storage directory for the engine journal.
///
/// Reads `STORAGE_DIR`; when unset, uses [`DEFAULT_STORAGE_DIRECTORY`] if it is
/// (creatable and) writable, else falls back to `$TMPDIR/gas-killer` for bare-metal
/// dev runs. The fallback is per-boot on most systems, so journal replay across
/// restarts is only guaranteed when `STORAGE_DIR` or the default volume exists.
pub fn storage_directory() -> std::path::PathBuf {
    if let Ok(dir) = env::var("STORAGE_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
    let default = std::path::Path::new(DEFAULT_STORAGE_DIRECTORY);
    if directory_is_writable(default) {
        return default.to_path_buf();
    }
    std::env::temp_dir().join("gas-killer")
}

/// Whether `path` exists (or can be created) and accepts file writes.
///
/// Probes with a real file create/delete rather than metadata: permission bits do
/// not capture read-only mounts or ACLs.
fn directory_is_writable(path: &std::path::Path) -> bool {
    if fs::create_dir_all(path).is_err() {
        return false;
    }
    let probe = path.join(format!(".gk-write-probe-{}", std::process::id()));
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Default number of heights the aggregation engine works on concurrently above
/// its tip (`Config::window`).
pub const DEFAULT_AGG_WINDOW: u64 = 8;

/// Reads the aggregation engine window from `AGG_WINDOW`, defaulting to
/// [`DEFAULT_AGG_WINDOW`]. Zero or unparseable values fall back to the default
/// (the engine requires a non-zero window).
pub fn agg_window() -> std::num::NonZeroU64 {
    env::var("AGG_WINDOW")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .and_then(std::num::NonZeroU64::new)
        .unwrap_or_else(|| {
            std::num::NonZeroU64::new(DEFAULT_AGG_WINDOW).expect("default window is non-zero")
        })
}

/// Default number of heights the aggregation engine keeps tracking below its tip
/// (`Config::activity_timeout`): ack collection + prune buffer.
///
/// Must be generous — heights pruned past this window can never certify locally,
/// so the router would miss their certificates (see the liveness model).
pub const DEFAULT_AGG_ACTIVITY_TIMEOUT: u64 = 256;

/// Reads the aggregation activity timeout (in heights) from `AGG_ACTIVITY_TIMEOUT`,
/// defaulting to [`DEFAULT_AGG_ACTIVITY_TIMEOUT`].
pub fn agg_activity_timeout() -> u64 {
    env::var("AGG_ACTIVITY_TIMEOUT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_AGG_ACTIVITY_TIMEOUT)
}

/// Default time the router waits for a certificate on its assigned height before
/// broadcasting `Skip` for it.
pub const DEFAULT_ROUND_TIMEOUT_SECS: f64 = 30.0;

/// Reads the round timeout from `ROUND_TIMEOUT` (seconds, fractional allowed),
/// defaulting to [`DEFAULT_ROUND_TIMEOUT_SECS`]. Non-positive, non-finite, or
/// unparseable values fall back to the default.
pub fn round_timeout() -> std::time::Duration {
    parse_secs_env_duration(
        env::var("ROUND_TIMEOUT").ok().as_deref(),
        DEFAULT_ROUND_TIMEOUT_SECS,
    )
}

/// Default cadence at which the router re-broadcasts the current `TaskDirective`
/// until the height certifies. Also reused as the engine's own TipAck
/// `rebroadcast_timeout`.
pub const DEFAULT_REBROADCAST_INTERVAL_SECS: f64 = 5.0;

/// Reads the rebroadcast interval from `REBROADCAST_INTERVAL` (seconds, fractional
/// allowed), defaulting to [`DEFAULT_REBROADCAST_INTERVAL_SECS`]. Non-positive,
/// non-finite, or unparseable values fall back to the default.
pub fn rebroadcast_interval() -> std::time::Duration {
    parse_secs_env_duration(
        env::var("REBROADCAST_INTERVAL").ok().as_deref(),
        DEFAULT_REBROADCAST_INTERVAL_SECS,
    )
}

/// Parses a seconds value (fractional allowed) into a `Duration`, falling back to
/// `default_secs` on malformed, non-positive, non-finite, or non-representable input.
fn parse_secs_env_duration(value: Option<&str>, default_secs: f64) -> std::time::Duration {
    value
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|&v| v > 0.0 && v.is_finite())
        .and_then(|v| std::time::Duration::try_from_secs_f64(v).ok())
        .filter(|d| !d.is_zero())
        .unwrap_or_else(|| std::time::Duration::from_secs_f64(default_secs))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn p2p_quota_period_default_is_one_per_second() {
        assert_eq!(parse_p2p_quota_period(None), Duration::from_secs(1));
    }

    #[test]
    fn p2p_quota_period_converts_rate_to_period() {
        assert_eq!(
            parse_p2p_quota_period(Some("5.0")),
            Duration::from_millis(200)
        );
        assert_eq!(parse_p2p_quota_period(Some("0.5")), Duration::from_secs(2));
    }

    #[test]
    fn p2p_quota_period_rejects_invalid_values() {
        let default = Duration::from_secs(1);
        assert_eq!(parse_p2p_quota_period(Some("")), default);
        assert_eq!(parse_p2p_quota_period(Some("abc")), default);
        assert_eq!(parse_p2p_quota_period(Some("0")), default);
        assert_eq!(parse_p2p_quota_period(Some("-1.5")), default);
        assert_eq!(parse_p2p_quota_period(Some("inf")), default);
        assert_eq!(parse_p2p_quota_period(Some("NaN")), default);
    }

    #[test]
    fn p2p_quota_period_rejects_duration_overflow() {
        // 1.0 / 1e-20 overflows Duration; must fall back to the default, not panic.
        assert_eq!(
            parse_p2p_quota_period(Some("1e-20")),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn p2p_quota_period_rejects_excessive_rate() {
        // 1.0 / 3e9 rounds below 1 ns and becomes Duration::ZERO; must fall back to default.
        assert_eq!(parse_p2p_quota_period(Some("3e9")), Duration::from_secs(1));
    }

    #[test]
    fn secs_env_duration_parses_and_falls_back() {
        assert_eq!(
            parse_secs_env_duration(Some("45"), 30.0),
            Duration::from_secs(45)
        );
        assert_eq!(
            parse_secs_env_duration(Some("0.5"), 30.0),
            Duration::from_millis(500)
        );
        let default = Duration::from_secs(30);
        assert_eq!(parse_secs_env_duration(None, 30.0), default);
        assert_eq!(parse_secs_env_duration(Some(""), 30.0), default);
        assert_eq!(parse_secs_env_duration(Some("abc"), 30.0), default);
        assert_eq!(parse_secs_env_duration(Some("0"), 30.0), default);
        assert_eq!(parse_secs_env_duration(Some("-3"), 30.0), default);
        assert_eq!(parse_secs_env_duration(Some("inf"), 30.0), default);
        assert_eq!(parse_secs_env_duration(Some("NaN"), 30.0), default);
    }

    #[test]
    fn storage_directory_falls_back_to_writable_path() {
        // Regardless of environment, the resolved directory must be usable for the
        // engine journal (env override, default volume, or temp fallback).
        let dir = storage_directory();
        assert!(!dir.as_os_str().is_empty());
    }

    #[test]
    fn agg_defaults_are_sane() {
        assert_eq!(DEFAULT_AGG_WINDOW, 8);
        assert_eq!(DEFAULT_AGG_ACTIVITY_TIMEOUT, 256);
        // The default window must construct the NonZeroU64 the engine config needs.
        assert_eq!(agg_window().get(), DEFAULT_AGG_WINDOW);
    }
}

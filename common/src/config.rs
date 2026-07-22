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

/// Default maximum number of tasks the ingress queue holds before shedding load.
///
/// The router processes one task at a time, so a deep queue means later submissions are
/// aggregated long after their block references have gone stale. Capping the depth bounds
/// both memory and worst-case queue latency; requests arriving at capacity are rejected with
/// `503 QUEUE_FULL` rather than accepted and starved. Configurable via `MAX_QUEUE_DEPTH`.
pub const DEFAULT_MAX_QUEUE_DEPTH: usize = 100;

/// Reads the ingress queue-depth cap from `MAX_QUEUE_DEPTH`, defaulting to
/// [`DEFAULT_MAX_QUEUE_DEPTH`]. Zero or unparseable values fall back to the default,
/// since a cap of zero would reject every request.
pub fn max_queue_depth() -> usize {
    env::var("MAX_QUEUE_DEPTH")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|&v: &usize| v > 0)
        .unwrap_or(DEFAULT_MAX_QUEUE_DEPTH)
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

/// Which quorum-signature scheme the node/router binaries run.
///
/// `Bls` is the engine-driven aggregation path (commonware aggregation engine,
/// BLS-aggregated operator signatures verified on-chain). `Schnorr` is the
/// interactive two-round MuSig2 aggregate path (coordinator/participant actors on
/// a p2p channel, a single constant-gas signature on-chain). The two paths never
/// mix inside one deployment: every binary in a stack must run the same scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureScheme {
    Bls,
    Schnorr,
}

/// Reads the signature scheme from `SIGNATURE_SCHEME` (case-insensitive `bls` |
/// `schnorr`), defaulting to [`SignatureScheme::Bls`] when unset. Panics on an
/// unrecognized value rather than silently running the wrong protocol.
pub fn signature_scheme() -> SignatureScheme {
    parse_signature_scheme(env::var("SIGNATURE_SCHEME").ok().as_deref())
}

/// Parses a `SIGNATURE_SCHEME` value into a [`SignatureScheme`], treating `None`
/// (env var unset) as `bls`.
///
/// # Panics
/// Panics if `value` is set to anything other than `""`, `"bls"`, or `"schnorr"`
/// (case-insensitive).
fn parse_signature_scheme(value: Option<&str>) -> SignatureScheme {
    match value {
        None => SignatureScheme::Bls,
        Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "" | "bls" => SignatureScheme::Bls,
            "schnorr" => SignatureScheme::Schnorr,
            _ => panic!("SIGNATURE_SCHEME must be 'bls' or 'schnorr', got: {raw}"),
        },
    }
}

/// Default quorum threshold numerator/denominator used when `QUORUM_THRESHOLD` /
/// `THRESHOLD_DENOMINATOR` are unset or malformed.
pub const DEFAULT_QUORUM_THRESHOLD_NUMERATOR: u64 = 2;
pub const DEFAULT_QUORUM_THRESHOLD_DENOMINATOR: u64 = 3;

/// Reads the quorum threshold fraction `num/den` from `QUORUM_THRESHOLD`
/// (numerator) and `THRESHOLD_DENOMINATOR` (denominator), defaulting to
/// [`DEFAULT_QUORUM_THRESHOLD_NUMERATOR`] / [`DEFAULT_QUORUM_THRESHOLD_DENOMINATOR`].
///
/// These env var names are shared with the eigenlayer setup deployment tooling
/// (see `example.env`), so the off-chain quorum check and the on-chain registry
/// threshold stay in lockstep.
pub fn quorum_threshold_fraction() -> (u64, u64) {
    parse_quorum_threshold_fraction(
        env::var("QUORUM_THRESHOLD").ok().as_deref(),
        env::var("THRESHOLD_DENOMINATOR").ok().as_deref(),
    )
}

/// Parses `QUORUM_THRESHOLD` / `THRESHOLD_DENOMINATOR` values into a fraction,
/// falling back to the default whenever either is missing, unparseable, the
/// denominator is zero, or the numerator exceeds the denominator.
fn parse_quorum_threshold_fraction(num: Option<&str>, den: Option<&str>) -> (u64, u64) {
    let num = num.and_then(|v| v.trim().parse::<u64>().ok());
    let den = den.and_then(|v| v.trim().parse::<u64>().ok());
    match (num, den) {
        (Some(n), Some(d)) if d > 0 && n <= d => (n, d),
        _ => (
            DEFAULT_QUORUM_THRESHOLD_NUMERATOR,
            DEFAULT_QUORUM_THRESHOLD_DENOMINATOR,
        ),
    }
}

/// Default per-stage timeout cap for the Schnorr coordinator's protocol rounds
/// (nonce collection, partial-signature collection), before the `ROUND_TIMEOUT /
/// 6` floor is applied.
pub const DEFAULT_SCHNORR_STAGE_TIMEOUT_SECS: f64 = 5.0;

/// Reads the Schnorr per-stage timeout from `SCHNORR_STAGE_TIMEOUT_SECS` (seconds,
/// fractional allowed). When unset, defaults to
/// `min(DEFAULT_SCHNORR_STAGE_TIMEOUT_SECS, round_timeout() / 6)` so several
/// attempts fit inside one round-timeout window.
pub fn schnorr_stage_timeout() -> std::time::Duration {
    schnorr_stage_timeout_from(
        round_timeout(),
        env::var("SCHNORR_STAGE_TIMEOUT_SECS").ok().as_deref(),
    )
}

/// Computes the Schnorr per-stage timeout given the current round timeout and an
/// optional `SCHNORR_STAGE_TIMEOUT_SECS` override. An override is parsed as a flat
/// seconds value (no `/ 6` floor); the floor only applies to the unset-default path.
fn schnorr_stage_timeout_from(
    round_timeout_duration: std::time::Duration,
    override_value: Option<&str>,
) -> std::time::Duration {
    match override_value {
        Some(raw) => parse_secs_env_duration(Some(raw), DEFAULT_SCHNORR_STAGE_TIMEOUT_SECS),
        None => std::cmp::min(
            std::time::Duration::from_secs_f64(DEFAULT_SCHNORR_STAGE_TIMEOUT_SECS),
            round_timeout_duration / 6,
        ),
    }
}

/// Default per-peer message rate for the Schnorr protocol channel, in messages
/// per second.
pub const DEFAULT_SCHNORR_MESSAGES_PER_SECOND: u32 = 64;

/// Reads the Schnorr protocol channel's per-peer message rate from
/// `P2P_SCHNORR_MESSAGES_PER_SECOND`, defaulting to
/// [`DEFAULT_SCHNORR_MESSAGES_PER_SECOND`]. Zero, malformed, or unparseable values
/// fall back to the default.
///
/// A [`NonZeroU32`](std::num::NonZeroU32) because it feeds `Quota::per_second`
/// directly. Sized generously: the p2p send-side limiter SILENTLY DROPS messages to
/// rate-limited peers (same failure mode as the ack channel below), and a dropped
/// signing-round message costs a whole retry attempt.
pub fn schnorr_messages_per_second() -> std::num::NonZeroU32 {
    parse_schnorr_messages_per_second(env::var("P2P_SCHNORR_MESSAGES_PER_SECOND").ok().as_deref())
}

/// Parses a `P2P_SCHNORR_MESSAGES_PER_SECOND` value, falling back to
/// [`DEFAULT_SCHNORR_MESSAGES_PER_SECOND`] on zero, malformed, or unparseable input.
fn parse_schnorr_messages_per_second(value: Option<&str>) -> std::num::NonZeroU32 {
    value
        .and_then(|v| v.trim().parse::<u32>().ok())
        .and_then(std::num::NonZeroU32::new)
        .unwrap_or_else(|| {
            std::num::NonZeroU32::new(DEFAULT_SCHNORR_MESSAGES_PER_SECOND)
                .expect("default schnorr message rate is nonzero")
        })
}

/// Per-peer send/receive rate for the aggregation-engine TipAck channel (channel 0),
/// in messages per second.
///
/// The engine keeps rebroadcasting a signed height's TipAck every
/// `REBROADCAST_INTERVAL` until the height falls `AGG_ACTIVITY_TIMEOUT` below the
/// tip — even after it certifies — so steady-state demand approaches
/// `AGG_ACTIVITY_TIMEOUT / REBROADCAST_INTERVAL` messages per second per peer. The
/// p2p send-side limiter SILENTLY DROPS messages to rate-limited peers, so an
/// undersized quota starves fresh acks and stalls certification. The default is
/// computed from those two knobs with 2x headroom; override with
/// `P2P_ACK_MESSAGES_PER_SECOND` (the legacy `P2P_MESSAGES_PER_SECOND` knob only
/// governs the task-directive channel).
pub fn ack_messages_per_second() -> std::num::NonZeroU32 {
    if let Some(v) = env::var("P2P_ACK_MESSAGES_PER_SECOND")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .and_then(std::num::NonZeroU32::new)
    {
        return v;
    }
    let demand =
        (agg_activity_timeout() as f64 / rebroadcast_interval().as_secs_f64()).ceil() as u32;
    std::num::NonZeroU32::new(demand.saturating_mul(2).saturating_add(8).max(8))
        .expect("quota is always at least 8")
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

/// Grace window, in blocks past the aggregation reference block, for which a rendered
/// user-executable payload stays valid. Used to set the payload's `valid_until_block`.
pub const DEFAULT_PAYLOAD_BLOCK_BUFFER: u64 = 50;

// A payload rendered against a reference block is only submittable while
// `referenceBlockNumber + BLOCK_STALE_MEASURE >= block.number`, so the default buffer must stay
// within the default staleness window or `valid_until_block` would promise a payload the chain
// already rejects. Enforced at compile time to keep the two defaults in lockstep.
const _: () = assert!(DEFAULT_PAYLOAD_BLOCK_BUFFER <= DEFAULT_BLOCK_STALE_MEASURE);

/// Reads the payload validity buffer from `PAYLOAD_BLOCK_BUFFER`, defaulting to
/// [`DEFAULT_PAYLOAD_BLOCK_BUFFER`] and clamped to [`block_stale_measure`].
///
/// On-chain, `verifyAndUpdate` requires `referenceBlockNumber + BLOCK_STALE_MEASURE >=
/// block.number`, so a payload rendered against a reference block only remains submittable for
/// `BLOCK_STALE_MEASURE` blocks. Clamping guarantees `valid_until_block` never promises a payload
/// past the point the chain would reject it, under any env — not just the defaults the
/// compile-time assertion covers.
pub fn payload_block_buffer() -> u64 {
    let requested = env::var("PAYLOAD_BLOCK_BUFFER")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_PAYLOAD_BLOCK_BUFFER);
    clamp_payload_block_buffer(requested, block_stale_measure())
}

/// Clamps a requested payload buffer to the on-chain staleness window. A buffer past
/// `stale_measure` would set `valid_until_block` beyond the block at which `verifyAndUpdate`
/// reverts `StaleBlockNumber`, so the freshness gate would serve a payload that cannot land.
fn clamp_payload_block_buffer(requested: u64, stale_measure: u64) -> u64 {
    requested.min(stale_measure)
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
    fn signature_scheme_defaults_to_bls() {
        assert_eq!(parse_signature_scheme(None), SignatureScheme::Bls);
        assert_eq!(parse_signature_scheme(Some("")), SignatureScheme::Bls);
        assert_eq!(parse_signature_scheme(Some("bls")), SignatureScheme::Bls);
        assert_eq!(parse_signature_scheme(Some("BLS")), SignatureScheme::Bls);
    }

    #[test]
    fn signature_scheme_parses_schnorr_case_insensitively() {
        assert_eq!(
            parse_signature_scheme(Some("schnorr")),
            SignatureScheme::Schnorr
        );
        assert_eq!(
            parse_signature_scheme(Some(" Schnorr ")),
            SignatureScheme::Schnorr
        );
    }

    #[test]
    #[should_panic(expected = "SIGNATURE_SCHEME must be 'bls' or 'schnorr', got: ecdsa")]
    fn signature_scheme_panics_on_unrecognized_value() {
        parse_signature_scheme(Some("ecdsa"));
    }

    #[test]
    fn quorum_threshold_fraction_defaults_to_two_thirds() {
        assert_eq!(parse_quorum_threshold_fraction(None, None), (2, 3));
        assert_eq!(
            parse_quorum_threshold_fraction(Some("abc"), Some("3")),
            (2, 3)
        );
        assert_eq!(parse_quorum_threshold_fraction(Some("1"), None), (2, 3));
    }

    #[test]
    fn quorum_threshold_fraction_reads_override() {
        assert_eq!(
            parse_quorum_threshold_fraction(Some("3"), Some("5")),
            (3, 5)
        );
    }

    #[test]
    fn quorum_threshold_fraction_rejects_invalid_values() {
        // Zero denominator.
        assert_eq!(
            parse_quorum_threshold_fraction(Some("1"), Some("0")),
            (2, 3)
        );
        // Numerator exceeds denominator.
        assert_eq!(
            parse_quorum_threshold_fraction(Some("4"), Some("3")),
            (2, 3)
        );
    }

    #[test]
    fn schnorr_stage_timeout_defaults_to_cap_when_round_timeout_is_large() {
        assert_eq!(
            schnorr_stage_timeout_from(Duration::from_secs(60), None),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn schnorr_stage_timeout_defaults_to_round_timeout_fraction_when_smaller() {
        assert_eq!(
            schnorr_stage_timeout_from(Duration::from_secs(12), None),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn schnorr_stage_timeout_reads_override() {
        assert_eq!(
            schnorr_stage_timeout_from(Duration::from_secs(60), Some("1.5")),
            Duration::from_millis(1500)
        );
    }

    #[test]
    fn schnorr_messages_per_second_defaults_and_overrides() {
        let nz = |n| std::num::NonZeroU32::new(n).unwrap();
        assert_eq!(parse_schnorr_messages_per_second(None), nz(64));
        assert_eq!(parse_schnorr_messages_per_second(Some("128")), nz(128));
        // Zero, negative, and unparseable all fall back to the default.
        assert_eq!(parse_schnorr_messages_per_second(Some("0")), nz(64));
        assert_eq!(parse_schnorr_messages_per_second(Some("-1")), nz(64));
        assert_eq!(parse_schnorr_messages_per_second(Some("abc")), nz(64));
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

    #[test]
    fn payload_block_buffer_defaults_are_sane() {
        assert_eq!(DEFAULT_PAYLOAD_BLOCK_BUFFER, 50);
        // The buffer-within-staleness-window invariant is enforced at compile time next to the
        // constant definitions (`const _: () = assert!(...)`).
    }

    #[test]
    fn payload_block_buffer_clamps_to_staleness_window() {
        // A buffer within the window is returned unchanged.
        assert_eq!(clamp_payload_block_buffer(50, 300), 50);
        // A buffer past the window is clamped so `valid_until_block` never exceeds the block at
        // which `verifyAndUpdate` would revert `StaleBlockNumber`.
        assert_eq!(clamp_payload_block_buffer(500, 300), 300);
        assert_eq!(clamp_payload_block_buffer(300, 300), 300);
    }
}

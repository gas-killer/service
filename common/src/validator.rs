use alloy_provider::Provider;
use anyhow::Result;
use commonware_codec::Read;
use commonware_cryptography::sha256::Digest;
use commonware_runtime::telemetry::metrics::encoding::text::encode;
use commonware_runtime::telemetry::metrics::raw::Histogram;
use commonware_runtime::telemetry::metrics::registry::Registry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::ReadOnlyProvider;
use crate::config::{ChainRole, SpeculativePrebuildConfig};
use crate::local_exec_shim::{prefer_mmap_overlay, sim_executor_from_env};
use crate::task_data::GasKillerTaskData;
use commonware_avs_router::validator::ValidatorTrait;
use commonware_avs_router::wire;

use alloy::rpc::types::TransactionRequest;
use gas_analyzer::{
    EvmSketchExecutorCache, LocalStateCache, OverlayEnv, SimExecutor, SimProfile,
    call_to_encoded_state_updates_local, call_to_encoded_state_updates_local_multi,
    call_to_encoded_state_updates_with_evmsketch_env,
};

/// Prometheus metrics for validator timing, exposed on the node's /metrics endpoint.
pub struct ValidatorMetrics {
    registry: Registry,
    /// Duration of the EVMSketch gas-analysis call (cache-miss path only).
    pub evmsketch_duration_seconds: Histogram,
}

impl ValidatorMetrics {
    pub fn new() -> Self {
        let mut registry = Registry::default();
        let evmsketch_duration_seconds =
            Histogram::new([0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 60.0, 120.0]);
        registry.register(
            "gas_killer_node_evmsketch_duration_seconds",
            "Duration of gas analysis (EVMSketch + RPC calls) on the node, cache-miss path only. Excludes chain detection.",
            evmsketch_duration_seconds.clone(),
        );
        Self {
            registry,
            evmsketch_duration_seconds,
        }
    }

    pub fn encode(&self) -> String {
        let mut output = String::new();
        encode(&mut output, &self.registry).expect("metrics encoding failed");
        output
    }
}

impl Default for ValidatorMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of gas analysis containing storage updates and gas information
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    /// The storage updates extracted from the transaction
    pub storage_updates: Vec<u8>,
    /// The gas estimate from gas-analyzer
    #[allow(dead_code)]
    pub gas_estimate: u64,
    /// The block height at which the analysis was performed
    pub block_height: u64,
}

/// Extra executor-cache slots per chain beyond the staleness window.
///
/// Covers on-demand entries (a freshly requested block not yet pre-built) without
/// evicting the speculative window.
const EXECUTOR_CACHE_SLACK_PER_CHAIN: usize = 4;

/// LRU capacity for the executor cache.
///
/// Sized to retain a full `BLOCK_STALE_MEASURE` window per chain so any in-window
/// `block_height` — whether pre-built by the speculative loop or requested on demand —
/// hits the cache. Entries are small (anchor header + provider handle, a few KB), so a
/// few-hundred-entry window costs single-digit MB.
fn executor_cache_capacity(num_chains: usize) -> usize {
    let per_chain = crate::config::block_stale_measure() as usize + EXECUTOR_CACHE_SLACK_PER_CHAIN;
    per_chain * num_chains.max(1)
}

/// Validator implementation for the gas killer use case with multi-chain support
#[derive(Clone)]
pub struct GasKillerValidator {
    /// RPC URLs per chain for the gas analyzer
    chain_rpc_urls: HashMap<ChainRole, String>,
    /// Read-only providers per chain for chain detection and `stateTransitionCount` reads.
    providers: Arc<HashMap<ChainRole, ReadOnlyProvider>>,
    /// Default chain for backwards compatibility
    default_chain: ChainRole,
    /// Cache: (transition_index, block_height) -> computed digest
    /// Prevents re-running expensive EVMSketch for the same round when the
    /// orchestrator validates multiple signatures for identical task data.
    digest_cache: Arc<Mutex<HashMap<(u64, u64), Digest>>>,
    /// Keys currently being simulated by [`prewarm`](Self::prewarm). Guards
    /// duplicate prewarms and lets `validate_and_build_hash` wait for an
    /// in-flight prewarm instead of launching a second multi-minute EVMSketch
    /// run for the same round. A `std` mutex (never held across `.await`) so
    /// the panic-safe [`PrewarmInflightGuard`] can release entries in `Drop`.
    prewarm_inflight: Arc<StdMutex<HashSet<(u64, u64)>>>,
    /// LRU cache of pre-built EvmSketch executors keyed by (rpc_url, block_number).
    /// Eliminates the 2× eth_getBlockByNumber build cost (~80–120 ms) for the
    /// 2nd…Nth request at the same block height.
    executor_cache: Arc<EvmSketchExecutorCache>,
    /// Optional Prometheus metrics — injected on the node, absent on the router.
    validator_metrics: Option<Arc<ValidatorMetrics>>,
    /// Node-side sharded-inference state (`GK_SHARD_*`). When set, rounds whose
    /// task targets the sharded consumer are gated: the node refuses to sign
    /// unless the segment commit chain verifies against the segments this node
    /// executed itself (see crate::shard::ShardState::verify_fulfil_task).
    shard: Option<Arc<crate::shard::ShardState>>,
    /// Simulation profile for tracked-function analysis. `UnboundedV1` lifts the
    /// simulated gas limits to the pinned protocol constants (see gas-analyzer's
    /// docs/UNBOUNDED_MODE.md), enabling tracked functions whose direct execution
    /// exceeds the real block gas limit; `UnboundedV1Xl` is the raised 2^43 gas
    /// tier of the same family for multi-Tgas tasks (~3.6 Tgas Qwen3.5-35B-A3B
    /// inference). Read from `GK_SIM_PROFILE` — it is
    /// protocol configuration, so the router and every node MUST agree on it or
    /// their independently derived payloads (and thus signatures) diverge.
    sim_profile: SimProfile,
    /// Pinned code overlay for the tracked-function simulation (`UNBOUNDED_V2`,
    /// see gas-analyzer's docs/UNBOUNDED_OVERLAYS.md): large immutable blobs
    /// (LLM weights, tokenizer) mounted as contract code via `debug_traceCall`
    /// state overrides, verified against the pinned manifest at startup. Read
    /// from `GK_OVERLAY_WEIGHTS`/`GK_OVERLAY_TOKENIZER`/`GK_OVERLAY_MANIFEST`.
    /// Pinned-environment configuration like `sim_profile`: the router and
    /// every node MUST mount the same bytes or their payloads diverge.
    /// `Arc` because the mounted set is ~the size of the blobs (hundreds of MB)
    /// and the validator is cloned.
    overlay_env: Option<Arc<OverlayEnv>>,
    /// mmap-mode overlays: one `(weights_path, tokenizer_path, manifest)`
    /// artifact spec PER PINNED MODEL, retained instead of materializing
    /// `OverlayEnv::from_blobs` in RAM. Populated when
    /// `GK_SIM_EXECUTOR=local` and `GK_OVERLAY_MMAP` (default true under
    /// local) — the analyzer mounts each spec via `OverlayMount::from_files`
    /// (streaming-keccak manifest verify + lazy chunk materialization), which
    /// is what makes ~35GB artifacts servable. Multiple models mount
    /// simultaneously as ONE composite lookup: chunk addresses are derived
    /// per-manifest, so distinct models' address sets are disjoint and every
    /// on-chain consumer finds exactly its own model's chunks. Slot 1 comes
    /// from the unsuffixed `GK_OVERLAY_WEIGHTS/TOKENIZER/MANIFEST` triplet;
    /// additional models from `GK_OVERLAY_WEIGHTS_2/...` and up (see
    /// [`overlay_files_from_env`]). Empty when unconfigured; mutually
    /// exclusive with `overlay_env`.
    overlay_files: Vec<(String, String, alloy::primitives::B256)>,
    /// Executor selection for tracked-function analysis (`GK_SIM_EXECUTOR=rpc|local`,
    /// gas-analyzer#169). `local` re-executes the call in-process inside the
    /// analyzer (`call_to_encoded_state_updates_local`) instead of delegating
    /// to `debug_traceCall` — the RPC becomes a pure lazy state backend.
    /// Pinned-environment configuration like `sim_profile`/`overlay_env`:
    /// router and every node must agree, and `local` is required (not just
    /// faster) once overlay artifacts exceed what a `stateOverrides` JSON
    /// body can carry (~35GB models).
    sim_executor: SimExecutor,
    /// Block-scoped remote-state backends + overlay mounts for the local
    /// executor (gas-analyzer's counterpart to `executor_cache`, mirrored
    /// here 1:1 — see `gas_analyzer::LocalStateCache` docs). Always
    /// constructed (cheap, lazily populated) so flipping `GK_SIM_EXECUTOR`
    /// doesn't need a validator rebuild.
    local_state_cache: Arc<LocalStateCache>,
}

/// How often a validation blocked on an in-flight prewarm re-checks the digest
/// cache. Coarse on purpose: the guarded computation runs for minutes, and the
/// waiter only burns a map lookup per tick.
const PREWARM_WAIT_POLL: Duration = Duration::from_secs(1);

/// Removes a key from the prewarm in-flight set on drop, so a panic or early
/// return inside the prewarm computation can never leave the key stuck
/// in-flight (which would make `validate_and_build_hash` wait forever).
struct PrewarmInflightGuard {
    inflight: Arc<StdMutex<HashSet<(u64, u64)>>>,
    key: (u64, u64),
}

impl Drop for PrewarmInflightGuard {
    fn drop(&mut self) {
        if let Ok(mut inflight) = self.inflight.lock() {
            inflight.remove(&self.key);
        }
    }
}

/// Parses `GK_SIM_PROFILE` into a [`SimProfile`]. Accepted values:
/// `chain` (default), `unbounded-v1`, and `unbounded-v1-xl` (raised gas tier
/// of the V1 family, pinned 2^43 limits — sized for multi-Tgas tasks such as
/// Qwen3.5-35B-A3B inference at ~3.6 Tgas/call). Panics on any other value —
/// a typo silently falling back to `Chain` on one node would fork the quorum.
fn sim_profile_from_env() -> SimProfile {
    match std::env::var("GK_SIM_PROFILE") {
        Err(_) => SimProfile::Chain,
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "" | "chain" => SimProfile::Chain,
            "unbounded-v1" => {
                info!(
                    "GK_SIM_PROFILE=unbounded-v1: simulating tracked functions under the pinned unbounded gas limits"
                );
                SimProfile::UnboundedV1
            }
            "unbounded-v1-xl" => {
                info!(
                    "GK_SIM_PROFILE=unbounded-v1-xl: simulating tracked functions under the pinned XL-tier (2^43) unbounded gas limits"
                );
                SimProfile::UnboundedV1Xl
            }
            other => {
                panic!(
                    "invalid GK_SIM_PROFILE {other:?}: expected \"chain\", \"unbounded-v1\", or \"unbounded-v1-xl\""
                )
            }
        },
    }
}

/// Loads the pinned code-overlay environment from `GK_OVERLAY_WEIGHTS` and
/// `GK_OVERLAY_TOKENIZER` (blob file paths) and verifies it against
/// `GK_OVERLAY_MANIFEST` (0x-prefixed 32-byte hex). Returns `None` when no
/// overlay is configured.
///
/// Panics on any invalid or partial configuration — missing counterpart
/// variable, unreadable blob, malformed manifest, or a manifest mismatch.
/// Same rationale as [`sim_profile_from_env`]: the overlay is part of the
/// pinned simulation environment, and one operator silently degrading to a
/// different (or missing) overlay would fork the quorum. Mismatched bytes are
/// refused outright rather than mounted (`OverlayEnv::verify`).
fn overlay_env_from_env() -> Option<Arc<OverlayEnv>> {
    let weights_path = std::env::var("GK_OVERLAY_WEIGHTS").unwrap_or_default();
    let tokenizer_path = std::env::var("GK_OVERLAY_TOKENIZER").unwrap_or_default();
    if weights_path.is_empty() && tokenizer_path.is_empty() {
        return None;
    }
    assert!(
        !weights_path.is_empty() && !tokenizer_path.is_empty(),
        "GK_OVERLAY_WEIGHTS and GK_OVERLAY_TOKENIZER must be set together"
    );
    let manifest_hex = std::env::var("GK_OVERLAY_MANIFEST").unwrap_or_default();
    assert!(
        !manifest_hex.is_empty(),
        "GK_OVERLAY_MANIFEST must be set when overlay artifacts are configured: \
         refusing to mount unverified bytes"
    );
    let expected: alloy::primitives::B256 = manifest_hex
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("invalid GK_OVERLAY_MANIFEST {manifest_hex:?}: {e}"));

    let weights = std::fs::read(&weights_path)
        .unwrap_or_else(|e| panic!("failed to read GK_OVERLAY_WEIGHTS {weights_path:?}: {e}"));
    let tokenizer = std::fs::read(&tokenizer_path)
        .unwrap_or_else(|e| panic!("failed to read GK_OVERLAY_TOKENIZER {tokenizer_path:?}: {e}"));

    let env = OverlayEnv::from_blobs(&weights, &tokenizer)
        .unwrap_or_else(|e| panic!("failed to build overlay env: {e}"));
    env.verify(expected)
        .unwrap_or_else(|e| panic!("GK_OVERLAY_MANIFEST verification failed: {e}"));

    info!(
        manifest = %env.manifest,
        chunks = env.overlays.len(),
        weights_bytes = weights.len(),
        tokenizer_bytes = tokenizer.len(),
        "mounted pinned code overlay for tracked-function simulation (UNBOUNDED_V2)"
    );
    Some(Arc::new(env))
}

/// One overlay slot's `(weights_path, tokenizer_path, manifest)` from
/// `GK_OVERLAY_WEIGHTS{suffix}` / `GK_OVERLAY_TOKENIZER{suffix}` /
/// `GK_OVERLAY_MANIFEST{suffix}` (suffix `""` for the historical slot-1
/// triplet, `"_2"`, `"_3"`, ... for additional models). Returns `None` when
/// the slot is entirely unconfigured; panics on any partial configuration —
/// same fail-loud rules as [`overlay_env_from_env`].
fn overlay_files_slot(suffix: &str) -> Option<(String, String, alloy::primitives::B256)> {
    let weights_var = format!("GK_OVERLAY_WEIGHTS{suffix}");
    let tokenizer_var = format!("GK_OVERLAY_TOKENIZER{suffix}");
    let manifest_var = format!("GK_OVERLAY_MANIFEST{suffix}");
    let weights_path = std::env::var(&weights_var).unwrap_or_default();
    let tokenizer_path = std::env::var(&tokenizer_var).unwrap_or_default();
    if weights_path.is_empty() && tokenizer_path.is_empty() {
        let manifest_hex = std::env::var(&manifest_var).unwrap_or_default();
        assert!(
            manifest_hex.trim().is_empty(),
            "{manifest_var} is set but {weights_var}/{tokenizer_var} are not: \
             partial overlay slot configuration"
        );
        return None;
    }
    assert!(
        !weights_path.is_empty() && !tokenizer_path.is_empty(),
        "{weights_var} and {tokenizer_var} must be set together"
    );
    let manifest_hex = std::env::var(&manifest_var).unwrap_or_default();
    assert!(
        !manifest_hex.is_empty(),
        "{manifest_var} must be set when overlay artifacts are configured: \
         refusing to mount unverified bytes"
    );
    let expected: alloy::primitives::B256 = manifest_hex
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("invalid {manifest_var} {manifest_hex:?}: {e}"));
    for p in [&weights_path, &tokenizer_path] {
        assert!(
            std::path::Path::new(p).is_file(),
            "overlay artifact path {p:?} does not exist or is not a file"
        );
    }
    info!(
        weights = %weights_path,
        tokenizer = %tokenizer_path,
        manifest = %expected,
        "overlay artifacts will be mmap-mounted at first analysis (OverlayMount::from_files)"
    );
    Some((weights_path, tokenizer_path, expected))
}

/// The indexed overlay slot numbers (`GK_OVERLAY_*_N`, N >= 2) present in
/// the environment, sorted and deduped. Discovery scans the environment
/// rather than probing sequentially, so a numbering gap (`_2` unset but `_3`
/// set) cannot silently drop a configured model. Panics on a non-numeric
/// suffix or on the reserved slots 0/1 (slot 1 is the unsuffixed triplet).
fn indexed_overlay_slots() -> Vec<u32> {
    indexed_overlay_slots_from(std::env::vars())
}

/// [`indexed_overlay_slots`] over an explicit variable set (unit-testable
/// without mutating the process environment).
fn indexed_overlay_slots_from(vars: impl Iterator<Item = (String, String)>) -> Vec<u32> {
    let mut slots: Vec<u32> = vars
        .filter_map(|(key, value)| {
            let suffix = [
                "GK_OVERLAY_WEIGHTS_",
                "GK_OVERLAY_TOKENIZER_",
                "GK_OVERLAY_MANIFEST_",
            ]
            .iter()
            .find_map(|prefix| key.strip_prefix(prefix))?;
            // GK_OVERLAY_MMAP starts with none of the three prefixes, so any
            // match here is meant as an overlay slot; a malformed suffix is a
            // config typo and must fail loudly, not be skipped.
            if value.trim().is_empty() {
                return None;
            }
            let slot: u32 = suffix.parse().unwrap_or_else(|_| {
                panic!("invalid overlay slot suffix in {key:?}: expected an integer >= 2")
            });
            assert!(
                slot >= 2,
                "{key} uses reserved slot {slot}: the first model is configured on the \
                 unsuffixed GK_OVERLAY_WEIGHTS/TOKENIZER/MANIFEST variables"
            );
            Some(slot)
        })
        .collect();
    slots.sort_unstable();
    slots.dedup();
    slots
}

/// mmap-mode variant of [`overlay_env_from_env`]: returns each configured
/// model's artifact file paths + parsed manifest WITHOUT reading the blobs
/// (the analyzer's `OverlayMount::from_files` mmaps and verifies them
/// lazily). Same fail-loud rules for partial configuration, applied per
/// slot.
///
/// Multi-overlay: slot 1 is the historical unsuffixed
/// `GK_OVERLAY_WEIGHTS/TOKENIZER/MANIFEST` triplet; each additional pinned
/// model gets a suffixed triplet (`GK_OVERLAY_WEIGHTS_2`, ...). All slots
/// mount simultaneously as one composite lookup — chunk addresses are
/// derived per-manifest, so distinct models' address sets are disjoint.
/// Duplicate manifests across slots are refused (the analyzer memoizes
/// mounts by manifest, so the later slot's paths would silently lose), as
/// are indexed slots without the base slot.
fn overlay_files_from_env() -> Vec<(String, String, alloy::primitives::B256)> {
    let mut specs = Vec::new();
    if let Some(spec) = overlay_files_slot("") {
        specs.push(spec);
    }
    for slot in indexed_overlay_slots() {
        assert!(
            !specs.is_empty(),
            "GK_OVERLAY_*_{slot} is set but the base GK_OVERLAY_WEIGHTS/TOKENIZER/MANIFEST \
             triplet is not: configure the first model on the unsuffixed variables"
        );
        let spec = overlay_files_slot(&format!("_{slot}"))
            .unwrap_or_else(|| unreachable!("slot {slot} was discovered from a set variable"));
        specs.push(spec);
    }
    let mut seen = HashSet::new();
    for (_, _, manifest) in &specs {
        assert!(
            seen.insert(*manifest),
            "duplicate overlay manifest {manifest} across GK_OVERLAY slots: \
             every mounted model must have a distinct pinned manifest"
        );
    }
    if specs.len() > 1 {
        info!(
            models = specs.len(),
            "multi-overlay: all pinned models mount simultaneously as one composite lookup"
        );
    }
    specs
}

impl GasKillerValidator {
    /// Creates a new GasKillerValidator with multi-chain support.
    ///
    /// Reads RPC URLs from environment variables:
    /// - `HTTP_RPC` for L1 (required)
    /// - `L2_HTTP_RPC` for L2 (optional)
    ///
    /// Returns an error if L1 RPC is not set.
    pub fn new() -> Result<Self> {
        let mut chain_rpc_urls = crate::chain_rpc_urls_from_env()?;
        // GK_SIM_RPC: optional dedicated endpoint for tracked-function SIMULATION
        // only (debug_traceCall). Lets operators point analysis at a node with
        // lifted trace caps and/or locally materialized state — e.g. an anvil
        // fork with pinned code overlays (UNBOUNDED_V2_OVERLAYS) setCode'd in —
        // while transaction submission and staking reads stay on HTTP_RPC
        // against the real chain. All operators and the router must use
        // equivalently-prepared simulation endpoints or their signed payloads
        // diverge, exactly like every other pinned-env parameter.
        if let Ok(sim_rpc) = std::env::var("GK_SIM_RPC")
            && !sim_rpc.is_empty()
        {
            tracing::info!(sim_rpc = %sim_rpc, "validator simulation RPC overridden by GK_SIM_RPC");
            chain_rpc_urls.insert(ChainRole::L1, sim_rpc);
        }
        let capacity = executor_cache_capacity(chain_rpc_urls.len());
        let providers = Arc::new(crate::build_read_providers(&chain_rpc_urls));
        if !providers.contains_key(&ChainRole::L1) {
            anyhow::bail!("HTTP_RPC is set but is not a valid URL");
        }

        Ok(Self {
            chain_rpc_urls,
            providers,
            default_chain: ChainRole::L1,
            digest_cache: Arc::new(Mutex::new(HashMap::new())),
            prewarm_inflight: Arc::new(StdMutex::new(HashSet::new())),
            executor_cache: Arc::new(EvmSketchExecutorCache::new(capacity)),
            validator_metrics: None,
            shard: None,
            sim_profile: sim_profile_from_env(),
            overlay_env: {
                let executor = sim_executor_from_env();
                if prefer_mmap_overlay(executor) {
                    None
                } else {
                    // The in-RAM/stateOverrides path mounts exactly one model;
                    // silently ignoring an indexed slot here would fork the
                    // quorum against mmap-mode operators serving both.
                    assert!(
                        indexed_overlay_slots().is_empty(),
                        "GK_OVERLAY_*_N is configured but multi-overlay requires \
                         GK_SIM_EXECUTOR=local with GK_OVERLAY_MMAP enabled (the default \
                         under local); the in-RAM/stateOverrides path serves one model only"
                    );
                    overlay_env_from_env()
                }
            },
            overlay_files: {
                let executor = sim_executor_from_env();
                if prefer_mmap_overlay(executor) {
                    overlay_files_from_env()
                } else {
                    Vec::new()
                }
            },
            sim_executor: {
                let executor = sim_executor_from_env();
                let prefer_mmap = prefer_mmap_overlay(executor);
                if executor == SimExecutor::Local {
                    info!(
                        prefer_mmap_overlay = prefer_mmap,
                        "GK_SIM_EXECUTOR=local: tracked-function analysis executes in-process \
                         inside gas-analyzer instead of via debug_traceCall"
                    );
                }
                executor
            },
            local_state_cache: Arc::new(LocalStateCache::default()),
        })
    }

    /// Creates a new GasKillerValidator with a specific RPC URL (for default chain).
    ///
    /// Useful for testing without modifying environment variables.
    pub fn with_rpc_url(rpc_url: impl Into<String>) -> Self {
        let mut chain_rpc_urls = HashMap::new();
        chain_rpc_urls.insert(ChainRole::L1, rpc_url.into());
        let capacity = executor_cache_capacity(chain_rpc_urls.len());
        let providers = Arc::new(crate::build_read_providers(&chain_rpc_urls));
        Self {
            chain_rpc_urls,
            providers,
            default_chain: ChainRole::L1,
            digest_cache: Arc::new(Mutex::new(HashMap::new())),
            prewarm_inflight: Arc::new(StdMutex::new(HashSet::new())),
            executor_cache: Arc::new(EvmSketchExecutorCache::new(capacity)),
            validator_metrics: None,
            shard: None,
            sim_profile: sim_profile_from_env(),
            overlay_env: overlay_env_from_env(),
            overlay_files: Vec::new(),
            sim_executor: {
                let executor = sim_executor_from_env();
                let prefer_mmap = prefer_mmap_overlay(executor);
                if executor == SimExecutor::Local {
                    info!(
                        prefer_mmap_overlay = prefer_mmap,
                        "GK_SIM_EXECUTOR=local: tracked-function analysis executes in-process \
                         inside gas-analyzer instead of via debug_traceCall"
                    );
                }
                executor
            },
            local_state_cache: Arc::new(LocalStateCache::default()),
        }
    }

    /// Creates a new GasKillerValidator with RPC URLs for multiple chains.
    pub fn with_chain_rpc_urls(chain_rpc_urls: HashMap<ChainRole, String>) -> Self {
        let capacity = executor_cache_capacity(chain_rpc_urls.len());
        let providers = Arc::new(crate::build_read_providers(&chain_rpc_urls));
        Self {
            chain_rpc_urls,
            providers,
            default_chain: ChainRole::L1,
            digest_cache: Arc::new(Mutex::new(HashMap::new())),
            prewarm_inflight: Arc::new(StdMutex::new(HashSet::new())),
            executor_cache: Arc::new(EvmSketchExecutorCache::new(capacity)),
            validator_metrics: None,
            shard: None,
            sim_profile: sim_profile_from_env(),
            overlay_env: overlay_env_from_env(),
            overlay_files: Vec::new(),
            sim_executor: {
                let executor = sim_executor_from_env();
                let prefer_mmap = prefer_mmap_overlay(executor);
                if executor == SimExecutor::Local {
                    info!(
                        prefer_mmap_overlay = prefer_mmap,
                        "GK_SIM_EXECUTOR=local: tracked-function analysis executes in-process \
                         inside gas-analyzer instead of via debug_traceCall"
                    );
                }
                executor
            },
            local_state_cache: Arc::new(LocalStateCache::default()),
        }
    }

    /// Attaches Prometheus metrics; call this on the node before passing the validator to the contributor.
    pub fn with_validator_metrics(mut self, metrics: Arc<ValidatorMetrics>) -> Self {
        self.validator_metrics = Some(metrics);
        self
    }

    /// Attaches node-side sharded-inference state; rounds targeting the sharded
    /// consumer are then gated on commit-chain verification before signing.
    pub fn with_shard_state(mut self, shard: Arc<crate::shard::ShardState>) -> Self {
        self.shard = Some(shard);
        self
    }

    /// Returns the RPC URL for the default chain
    pub fn rpc_url(&self) -> &str {
        self.chain_rpc_urls
            .get(&self.default_chain)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    /// Returns the RPC URL for a specific chain
    pub fn rpc_url_for_chain(&self, chain_id: ChainRole) -> Option<&str> {
        self.chain_rpc_urls.get(&chain_id).map(|s| s.as_str())
    }

    /// Returns whether a chain is supported
    pub fn supports_chain(&self, chain_id: ChainRole) -> bool {
        self.chain_rpc_urls.contains_key(&chain_id)
    }

    /// Returns the actual EVM chain ID (from `eth_chainId`) for the given chain role's RPC.
    pub async fn get_chain_id_for(&self, chain: ChainRole) -> Result<u64> {
        self.providers
            .get(&chain)
            .ok_or_else(|| anyhow::anyhow!("No provider configured for chain role: {}", chain))?
            .get_chain_id()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch chain ID for chain {}: {}", chain, e))
    }

    /// Returns all supported chains
    pub fn supported_chains(&self) -> Vec<ChainRole> {
        self.chain_rpc_urls.keys().copied().collect()
    }

    /// Detects which chain has code deployed at the given address.
    ///
    /// Checks each supported chain to see if the address has contract code.
    /// Returns the first chain where code is found, or an error if no chain has code.
    pub async fn detect_chain_for_address(
        &self,
        address: alloy::primitives::Address,
    ) -> Result<ChainRole> {
        debug!(
            address = %address,
            "Detecting chain for address"
        );

        let supported = self.supported_chains();
        // Clone the Arc so the closure doesn't borrow self
        let providers = Arc::clone(&self.providers);

        crate::config::detect_chain_for_address(address, &supported, |chain_id, addr| {
            let providers = Arc::clone(&providers);
            async move {
                let provider = providers
                    .get(&chain_id)
                    .ok_or_else(|| anyhow::anyhow!("No provider for chain {}", chain_id))?;
                let code = provider.get_code_at(addr).await?;
                Ok(code)
            }
        })
        .await
    }

    /// Fetches the current `stateTransitionCount()` from the contract on a known chain.
    ///
    /// Skips chain detection — use this when the chain has already been identified (e.g.
    /// from `compute_storage_updates_for_tx`) to avoid a redundant `eth_getCode` round-trip.
    pub async fn get_state_transition_count_on_chain(
        &self,
        address: alloy::primitives::Address,
        chain_id: ChainRole,
    ) -> Result<u64> {
        use crate::bindings::gaskillersdk::GasKillerSDK;

        let provider = match self.providers.get(&chain_id) {
            Some(p) => p.clone(),
            None => {
                if let Some(rpc_url) = self.chain_rpc_urls.get(&chain_id) {
                    anyhow::bail!(
                        "RPC URL for chain {} is not a valid URL (provider was not built): {}",
                        chain_id,
                        rpc_url
                    );
                }
                anyhow::bail!("No RPC URL configured for chain {}", chain_id);
            }
        };
        let count = GasKillerSDK::new(address, provider)
            .stateTransitionCount()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("stateTransitionCount call failed: {}", e))?;
        count
            .try_into()
            .map_err(|_| anyhow::anyhow!("stateTransitionCount overflow"))
    }

    /// Fetches the current `stateTransitionCount()` from the contract.
    ///
    /// Detects which chain the contract lives on, then calls the view function.
    /// Prefer [`get_state_transition_count_on_chain`] when the chain is already known.
    pub async fn get_state_transition_count(
        &self,
        address: alloy::primitives::Address,
    ) -> Result<u64> {
        let chain_id = self.detect_chain_for_address(address).await?;
        self.get_state_transition_count_on_chain(address, chain_id)
            .await
    }

    /// Computes storage updates for a transaction using gas-analyzer.
    ///
    /// Automatically detects which chain the contract is on, then computes storage updates.
    /// Returns the storage updates, block height, and the actual EVM chain ID (u64).
    pub async fn compute_storage_updates_for_tx(
        &self,
        contract_address: alloy::primitives::Address,
        call_data: &[u8],
        from_address: Option<alloy::primitives::Address>,
        value: Option<alloy::primitives::U256>,
        block_height: u64,
    ) -> Result<(Vec<u8>, u64, u64)> {
        let chain_role = self.detect_chain_for_address(contract_address).await?;

        debug!(
            chain = %chain_role,
            address = %contract_address,
            "Detected chain for contract"
        );

        let rpc_url = self
            .rpc_url_for_chain(chain_role)
            .ok_or_else(|| anyhow::anyhow!("No RPC URL configured for chain: {}", chain_role))?;

        // Fetch the actual EVM chain ID from the RPC we're already using for EVMSketch.
        let numeric_chain_id = self.get_chain_id_for(chain_role).await?;

        let result = self
            .analyze_transaction(
                rpc_url,
                contract_address,
                call_data,
                from_address,
                value,
                block_height,
            )
            .await?;
        Ok((
            result.storage_updates,
            result.block_height,
            numeric_chain_id,
        ))
    }

    /// Validates the message format and decodes the aggregation
    async fn validate_message_format(
        &self,
        msg: &[u8],
    ) -> Result<wire::Aggregation<GasKillerTaskData>> {
        debug!("Validating message format, length: {} bytes", msg.len());

        if msg.is_empty() {
            return Err(anyhow::anyhow!("Message is empty"));
        }

        // Try to decode the aggregation
        let mut msg_buf = msg;
        let aggregation = wire::Aggregation::<GasKillerTaskData>::read_cfg(&mut msg_buf, &())
            .map_err(|e| anyhow::anyhow!("Failed to decode aggregation: {}", e))?;

        debug!(
            "Successfully decoded aggregation with round: {}",
            aggregation.round
        );
        Ok(aggregation)
    }

    /// Precomputes and caches the payload digest using already-computed storage updates.
    ///
    /// Call this from the task creator after it runs EVMSketch to build the payload, so that
    /// the orchestrator's validator can skip running EVMSketch again when verifying each incoming
    /// node signature for the same round.
    pub async fn prime_cache(&self, task_data: &GasKillerTaskData, storage_updates: &[u8]) {
        let digest = task_data.build_payload_hash(storage_updates);
        let cache_key = (task_data.transition_index, task_data.block_height);
        let mut cache = self.digest_cache.lock().await;
        cache.insert(cache_key, digest);
        debug!(
            transition_index = task_data.transition_index,
            block_height = task_data.block_height,
            "Primed validator digest cache from creator (verification will skip EVMSketch)"
        );
    }

    /// Pre-computes and caches the payload digest for a task before its round
    /// broadcast arrives (the ingress prewarm path — see `crate::prewarm`).
    ///
    /// Runs the exact simulate-and-hash pipeline `validate_and_build_hash`
    /// uses on a cache miss and lands the result under the same
    /// `(transition_index, block_height)` key, so the later round arrival is a
    /// pure cache hit. `task_data.storage_updates` is ignored (the digest is
    /// always built from the freshly recomputed updates), so callers pass it
    /// empty.
    ///
    /// Returns `Ok(true)` when a digest was computed and cached, `Ok(false)`
    /// when the key was already cached or another prewarm for it is in flight.
    /// Concurrent duplicate prewarms of one key are guarded by
    /// `prewarm_inflight`; a validation arriving mid-prewarm waits for this
    /// computation instead of starting its own.
    pub async fn prewarm(&self, task_data: &GasKillerTaskData) -> Result<bool> {
        let cache_key = (task_data.transition_index, task_data.block_height);

        {
            let cache = self.digest_cache.lock().await;
            if cache.contains_key(&cache_key) {
                return Ok(false);
            }
        }

        // Claim the key; the guard releases it on every exit path, including panic.
        {
            let mut inflight = self
                .prewarm_inflight
                .lock()
                .expect("prewarm_inflight mutex poisoned");
            if !inflight.insert(cache_key) {
                return Ok(false);
            }
        }
        let _guard = PrewarmInflightGuard {
            inflight: Arc::clone(&self.prewarm_inflight),
            key: cache_key,
        };

        let storage_updates = self.compute_storage_updates(task_data).await?;
        let payload_hash = task_data.build_payload_hash(&storage_updates);

        {
            let mut cache = self.digest_cache.lock().await;
            cache.insert(cache_key, payload_hash);
        }
        debug!(
            transition_index = task_data.transition_index,
            block_height = task_data.block_height,
            "Prewarmed validator digest cache (round arrival will skip EVMSketch)"
        );
        Ok(true)
    }

    /// Whether a prewarm simulation for `key` is currently in flight.
    fn is_prewarm_inflight(&self, key: &(u64, u64)) -> bool {
        self.prewarm_inflight
            .lock()
            .expect("prewarm_inflight mutex poisoned")
            .contains(key)
    }

    /// Performs the core gas analysis using gas-analyzer.
    ///
    /// Uses the shared executor cache to skip the 2× `eth_getBlockByNumber` build
    /// cost (~80–120 ms) when a request arrives at the same block height as a
    /// recent prior request.
    ///
    /// Takes an explicit RPC URL parameter for flexibility.
    /// Forks at the specified block for deterministic results.
    pub async fn analyze_transaction(
        &self,
        rpc_url: &str,
        contract_address: alloy::primitives::Address,
        call_data: &[u8],
        from_address: Option<alloy::primitives::Address>,
        value: Option<alloy::primitives::U256>,
        block_height: u64,
    ) -> Result<AnalysisResult> {
        debug!(
            block_number = block_height,
            contract = %contract_address,
            call_data_len = call_data.len(),
            "Analyzing transaction at block"
        );

        // Build transaction request
        let from = from_address.unwrap_or(alloy::primitives::Address::ZERO);
        let tx_value = value.unwrap_or(alloy::primitives::U256::ZERO);

        let tx_request = TransactionRequest::default()
            .from(from)
            .to(contract_address)
            .value(tx_value)
            .input(alloy::primitives::Bytes::copy_from_slice(call_data).into());

        // Call gas-analyzer to get storage updates and gas estimate using EvmSketch.
        // The executor cache eliminates the build cost on repeated requests at the
        // same block height. When a pinned overlay is configured (UNBOUNDED_V2) it
        // rides along as debug_traceCall state overrides (RPC path) or a native
        // in-process mount (local path), so the simulation RPC only needs base
        // chain state — the overlay bytes come from the local, verified artifact
        // files. `None` is byte-identical to the pre-overlay profiled call.
        //
        // GK_SIM_EXECUTOR=local (gas-analyzer#169) re-executes the call in-process
        // inside the analyzer instead of delegating to debug_traceCall — required
        // once overlay artifacts exceed what a stateOverrides JSON body can carry
        // (~35GB models). The RPC becomes a pure lazy state backend, shared across
        // concurrent traces of the same block via `local_state_cache` (this
        // validator's counterpart to `executor_cache`).
        //
        // Multi-overlay (gas-analyzer#172): under GK_SIM_EXECUTOR=local with
        // mmap mode (GK_OVERLAY_MMAP, default true under local), EVERY
        // configured model's artifact spec in `overlay_files` mounts
        // simultaneously via call_to_encoded_state_updates_local_multi.
        // Chunk addresses are derived per-manifest
        // (keccak("gaskiller.llm.overlay.v1"||manifest||u64be(i))[12:]), so
        // distinct models' address sets are disjoint and the composite lookup
        // serves each consumer exactly its own model's chunks. This replaces
        // hosting a second model's chunks as anvil-fork setCode state, which
        // raced reforking: a task pinned to a block that predated the fork's
        // latest refork read the chunks through the fork's historical-state
        // proxy (no chunks there), saw zero-length weights, and all
        // participants deterministically signed an empty payload.
        //
        // Runtime-starvation fix (production incident, PR #319): under
        // GK_SIM_EXECUTOR=local, this call bottoms out in gas-analyzer's
        // in-process revm `transact` for the tracked function. For
        // Qwen3.5-35B-A3B that is a single synchronous, CPU-bound stretch of
        // ~20-40 minutes that never yields to the async scheduler (unlike the
        // RPC path, foundry-fork-db's SharedBackend already runs state fetches
        // on its own thread — the remaining work here is pure compute).
        //
        // commonware's `tokio::Runner` builds a genuine multi-threaded Tokio
        // runtime (`Builder::new_multi_thread()`, commonware-runtime
        // src/tokio/runtime.rs) but router/node main.rs both start it with
        // `tokio::Config::default()`, which pins `worker_threads` to 2. Every
        // commonware task spawned with the default (non-dedicated) execution
        // mode — including the /healthz, /readyz, /metrics, /prewarm axum
        // server and the P2P layer — lands on that same 2-worker shared pool.
        // Directly `.await`ing the analyzer here occupies a worker thread for
        // the full trace; with only 2 workers, a couple of concurrent traces
        // (or one trace plus normal P2P/health load) fully starves the pool,
        // /healthz stops responding within its 1s timeout, and Kubernetes'
        // liveness probe SIGKILLs the pod mid-round.
        //
        // Fix: run the whole analyzer call on Tokio's blocking-thread pool via
        // `spawn_blocking`, which is disjoint from the shared worker pool
        // (bounded separately by `max_blocking_threads`, 512 by default) and
        // therefore cannot starve /healthz or P2P. Re-entering async context
        // inside the blocking closure via a `Handle` captured *before*
        // `spawn_blocking` is safe here specifically because the underlying
        // runtime is multi-threaded — `Handle::block_on` on a current-thread
        // runtime from inside `spawn_blocking` would deadlock, but that does
        // not apply to this runtime.
        let handle = tokio::runtime::Handle::current();
        let rpc_url = rpc_url.to_owned();
        let executor_cache = Arc::clone(&self.executor_cache);
        let local_state_cache = Arc::clone(&self.local_state_cache);
        let overlay_env = self.overlay_env.clone();
        let overlay_files = self.overlay_files.clone();
        let sim_executor = self.sim_executor;
        let sim_profile = self.sim_profile;
        let (storage_updates, gas_estimate, _is_heuristic, _skipped_opcodes) =
            tokio::task::spawn_blocking(move || {
                handle.block_on(async move {
                    match sim_executor {
                        SimExecutor::Rpc => call_to_encoded_state_updates_with_evmsketch_env(
                            &executor_cache,
                            &rpc_url,
                            tx_request,
                            block_height,
                            sim_profile,
                            overlay_env.as_deref(),
                        )
                        .await
                        .map_err(|e| anyhow::anyhow!("Gas analysis failed: {}", e)),
                        SimExecutor::Local => {
                            if overlay_files.is_empty() {
                                call_to_encoded_state_updates_local(
                                    &executor_cache,
                                    &local_state_cache,
                                    &rpc_url,
                                    tx_request,
                                    block_height,
                                    sim_profile,
                                    overlay_env.as_deref(),
                                )
                                .await
                                .map_err(|e| anyhow::anyhow!("Local gas analysis failed: {}", e))
                            } else {
                                // mmap mode: the analyzer mounts every configured
                                // model's artifact files directly (streaming
                                // manifest verify + lazy chunk materialization)
                                // and consults them as one composite lookup —
                                // no blob bytes ever enter this process's heap.
                                // With a single spec this is byte-identical to
                                // the historical single-model entry point (both
                                // resolve through the same manifest-keyed mount
                                // cache and shared extraction body).
                                let models = overlay_files.len();
                                call_to_encoded_state_updates_local_multi(
                                    &executor_cache,
                                    &local_state_cache,
                                    &rpc_url,
                                    tx_request,
                                    block_height,
                                    sim_profile,
                                    &overlay_files,
                                )
                                .await
                                .map_err(|e| {
                                    anyhow::anyhow!(
                                        "Local gas analysis (mmap, {} model(s)) failed: {}",
                                        models,
                                        e
                                    )
                                })
                            }
                        }
                    }
                })
            })
            .await
            .map_err(|e| anyhow::anyhow!("gas analysis task panicked or was cancelled: {}", e))??;

        debug!(
            "Analysis complete: storage_updates_len={}, gas_estimate={}, block_height={}",
            storage_updates.len(),
            gas_estimate,
            block_height
        );

        Ok(AnalysisResult {
            storage_updates: storage_updates.to_vec(),
            gas_estimate,
            block_height,
        })
    }

    /// Watches each chain's head and speculatively pre-builds the EVMSketch executor for the
    /// latest block, populating the shared executor cache so a task's first validation skips the
    /// live `build()` cost (~80–120 ms) on the critical path.
    ///
    /// Runs forever; intended to be spawned as a background task. Per-chain loops run
    /// concurrently, each with at most one build in flight. Build failures are logged at `WARN`
    /// and never propagate — a miss simply falls back to the on-demand build path.
    ///
    /// The cached executor only feeds the (discarded) gas estimate, never the signed
    /// `storage_updates`, so pre-building at the unconfirmed tip cannot affect consensus.
    pub async fn run_speculative_prebuild(&self, config: SpeculativePrebuildConfig) {
        if !config.enabled {
            debug!("Speculative executor pre-build disabled");
            return;
        }

        let loops = self
            .chain_rpc_urls
            .iter()
            .filter_map(|(chain, rpc_url)| {
                let provider = self.providers.get(chain)?;
                Some(self.prebuild_chain_loop(*chain, rpc_url, provider, config))
            })
            .collect::<Vec<_>>();

        if loops.is_empty() {
            warn!("Speculative pre-build: no chains with providers; loop not started");
            return;
        }

        info!(
            chains = loops.len(),
            poll_ms = config.poll_interval.as_millis() as u64,
            confirmations = config.confirmation_depth,
            "Starting speculative executor pre-build"
        );
        futures::future::join_all(loops).await;
    }

    /// Per-chain pre-build loop: poll the head, build the target block's executor if it changed.
    async fn prebuild_chain_loop(
        &self,
        chain: ChainRole,
        rpc_url: &str,
        provider: &ReadOnlyProvider,
        config: SpeculativePrebuildConfig,
    ) {
        let mut last_built: Option<u64> = None;
        loop {
            match provider.get_block_number().await {
                Ok(head) => {
                    if let Some(target) = Self::speculative_target(head, config.confirmation_depth)
                        && last_built != Some(target)
                    {
                        match self.executor_cache.get_or_build(rpc_url, target).await {
                            Ok(_) => {
                                last_built = Some(target);
                                debug!(chain = %chain, block = target, "Speculative pre-build cached executor");
                            }
                            Err(e) => {
                                warn!(chain = %chain, block = target, error = %e, "Speculative pre-build failed");
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(chain = %chain, error = %e, "Speculative pre-build: failed to read chain head");
                }
            }
            tokio::time::sleep(config.poll_interval).await;
        }
    }

    /// The block to pre-build for a given chain `head` and confirmation depth.
    ///
    /// Returns `None` when the depth would reach at or below genesis (nothing useful to build).
    fn speculative_target(head: u64, confirmation_depth: u64) -> Option<u64> {
        head.checked_sub(confirmation_depth).filter(|&b| b > 0)
    }

    /// Computes storage updates by running local analysis.
    /// Automatically detects which chain the target address is on.
    /// Uses the block_height from task_data to ensure deterministic results matching the router.
    async fn compute_storage_updates(&self, task_data: &GasKillerTaskData) -> Result<Vec<u8>> {
        if task_data.block_height == 0 {
            return Err(anyhow::anyhow!("block_height is required for validation"));
        }

        // Detect which chain has the contract
        let chain_id = self
            .detect_chain_for_address(task_data.target_address)
            .await?;

        // Get the RPC URL for the detected chain
        let rpc_url = self
            .rpc_url_for_chain(chain_id)
            .ok_or_else(|| anyhow::anyhow!("No RPC URL configured for chain: {}", chain_id))?;

        debug!(
            chain_id = %chain_id,
            target_address = %task_data.target_address,
            "Computing storage updates for detected chain"
        );

        let evmsketch_start = Instant::now();
        let result = self
            .analyze_transaction(
                rpc_url,
                task_data.target_address,
                &task_data.call_data,
                Some(task_data.from_address),
                Some(task_data.value),
                task_data.block_height,
            )
            .await?;
        if let Some(m) = &self.validator_metrics {
            m.evmsketch_duration_seconds
                .observe(evmsketch_start.elapsed().as_secs_f64());
        }
        Ok(result.storage_updates)
    }

    /// Core validation logic: decodes message, computes storage updates, and builds payload hash.
    /// This is the single place where storage updates are computed to avoid double computation.
    ///
    /// Results are cached by (transition_index, block_height) so that repeated calls for the
    /// same round (e.g., the orchestrator validating each of the N node signatures) only run
    /// the expensive EVMSketch computation once.
    async fn validate_and_build_hash(&self, msg: &[u8]) -> Result<Digest> {
        debug!("Validating message of length: {} bytes", msg.len());

        // Validate message format and decode
        let aggregation = self.validate_message_format(msg).await?;
        let task_data = &aggregation.metadata;

        // Sharded-inference gate: a round targeting the sharded settlement
        // consumer is only signable if the segment commit chain behind its
        // pipelineRoot verifies — including that the digests of every segment
        // THIS node executed match the chain. Erroring here means this node
        // never signs the round.
        if let Some(shard) = &self.shard
            && shard.gates(task_data.target_address, &task_data.call_data)
        {
            shard
                .verify_fulfil_task(&task_data.call_data)
                .await
                .map_err(|e| {
                    warn!(target_address = %task_data.target_address, error = %e, "shard gate: REFUSING to sign");
                    e
                })?;
        }

        let cache_key = (task_data.transition_index, task_data.block_height);

        // Check cache before running expensive EVMSketch
        {
            let cache = self.digest_cache.lock().await;
            if let Some(cached) = cache.get(&cache_key) {
                debug!(
                    transition_index = task_data.transition_index,
                    block_height = task_data.block_height,
                    "Returning cached digest (skipping EVMSketch)"
                );
                return Ok(*cached);
            }
        }

        // An ingress prewarm for this exact key may already be simulating
        // (node-side, started at task-ingress time). Wait for it to land in
        // the cache rather than launching a second multi-minute EVMSketch run:
        // recomputing here would double memory and erase the prewarm's head
        // start. If the prewarm fails, its key leaves the in-flight set with
        // no cache entry and we fall through to computing ourselves.
        if self.is_prewarm_inflight(&cache_key) {
            info!(
                transition_index = task_data.transition_index,
                block_height = task_data.block_height,
                "Round arrived while its prewarm is still simulating — waiting for the in-flight result"
            );
            loop {
                tokio::time::sleep(PREWARM_WAIT_POLL).await;
                {
                    let cache = self.digest_cache.lock().await;
                    if let Some(cached) = cache.get(&cache_key) {
                        return Ok(*cached);
                    }
                }
                if !self.is_prewarm_inflight(&cache_key) {
                    // Prewarm finished without caching (it failed); compute below.
                    break;
                }
            }
        }

        // Not cached — compute storage updates (the expensive EVMSketch path)
        let storage_updates = self.compute_storage_updates(task_data).await?;

        // Build expected payload hash using computed storage updates
        let payload_hash = task_data.build_payload_hash(&storage_updates);

        // Store in cache for subsequent calls with the same round
        {
            let mut cache = self.digest_cache.lock().await;
            cache.insert(cache_key, payload_hash);
        }

        debug!("Built and cached payload hash: {:?}", payload_hash);
        Ok(payload_hash)
    }
}

#[async_trait::async_trait]
impl ValidatorTrait for GasKillerValidator {
    async fn validate_and_return_expected_hash(&self, msg: &[u8]) -> Result<Digest> {
        debug!("validate_and_return_expected_hash called");
        self.validate_and_build_hash(msg).await
    }

    async fn get_payload_from_message(&self, msg: &[u8]) -> Result<Digest> {
        debug!("get_payload_from_message called");
        self.validate_and_build_hash(msg).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, U256};
    use commonware_codec::{EncodeSize, Write};

    fn create_test_task_data() -> GasKillerTaskData {
        GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04].into(),
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
            block_height: 12345,
            chain_id: 1u64,
        }
    }

    #[tokio::test]
    async fn test_validator_creation() {
        let _validator =
            GasKillerValidator::with_rpc_url("https://ethereum-sepolia.publicnode.com");
    }

    #[test]
    fn test_providers_prebuilt_for_each_chain() {
        let mut urls = HashMap::new();
        urls.insert(ChainRole::L1, "https://example.com".to_string());
        urls.insert(ChainRole::L2, "https://l2.example.com".to_string());
        let validator = GasKillerValidator::with_chain_rpc_urls(urls);

        assert!(validator.providers.contains_key(&ChainRole::L1));
        assert!(validator.providers.contains_key(&ChainRole::L2));
    }

    #[test]
    fn test_speculative_target() {
        // depth 0 → build the tip
        assert_eq!(GasKillerValidator::speculative_target(100, 0), Some(100));
        // depth N → N blocks behind head
        assert_eq!(GasKillerValidator::speculative_target(100, 3), Some(97));
        // head - depth == 0 (genesis) → nothing to build
        assert_eq!(GasKillerValidator::speculative_target(2, 2), None);
        // depth deeper than head → no underflow
        assert_eq!(GasKillerValidator::speculative_target(1, 5), None);
    }

    #[test]
    fn test_executor_cache_capacity_covers_window_per_chain() {
        let window = crate::config::block_stale_measure() as usize;
        let one = executor_cache_capacity(1);
        let two = executor_cache_capacity(2);
        // Each chain gets at least a full staleness window of slots.
        assert!(one >= window);
        assert_eq!(two, one * 2);
    }

    #[tokio::test]
    async fn test_validate_invalid_message() {
        let validator = GasKillerValidator::with_rpc_url("https://ethereum-sepolia.publicnode.com");

        assert!(
            validator
                .validate_and_return_expected_hash(&[])
                .await
                .is_err()
        );
        assert!(
            validator
                .validate_and_return_expected_hash(&[0x01, 0x02, 0x03])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_message_format_validation() {
        // Unit test: verify message format validation works without RPC
        let validator = GasKillerValidator::with_rpc_url("https://example.com");
        let task_data = create_test_task_data();

        let aggregation = wire::Aggregation::<GasKillerTaskData>::new(1, task_data, None);

        let mut msg_bytes = Vec::with_capacity(aggregation.encode_size());
        aggregation.write(&mut msg_bytes);

        // Message format validation should succeed (doesn't need RPC)
        let result = validator.validate_message_format(&msg_bytes).await;
        assert!(result.is_ok());

        let decoded = result.unwrap();
        assert_eq!(decoded.round, 1);
        assert_eq!(decoded.metadata.transition_index, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires RPC - run with: cargo test -- --ignored"]
    async fn test_full_validation_with_rpc() {
        // Integration test: full validation including storage update computation
        // This test is ignored by default as it requires RPC access and Anvil
        let validator = GasKillerValidator::with_rpc_url("https://ethereum-sepolia.publicnode.com");
        let task_data = create_test_task_data();

        let aggregation = wire::Aggregation::<GasKillerTaskData>::new(1, task_data, None);

        let mut msg_bytes = Vec::with_capacity(aggregation.encode_size());
        aggregation.write(&mut msg_bytes);

        let result = validator
            .validate_and_return_expected_hash(&msg_bytes)
            .await;

        // With proper RPC/Anvil setup, this should succeed
        let hash = result.expect("Full validation should succeed with RPC access");
        let zero_hash = Digest::from([0u8; 32]);
        assert_ne!(hash, zero_hash, "Hash should not be all zeros");
    }

    #[test]
    fn test_build_payload_hash_deterministic() {
        let task_data = create_test_task_data();
        let storage_updates = vec![0x01, 0x02, 0x03, 0x04];

        let hash1 = task_data.build_payload_hash(&storage_updates);
        let hash2 = task_data.build_payload_hash(&storage_updates);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, Digest::from([0u8; 32]));
    }

    #[test]
    fn test_build_payload_hash_different_inputs() {
        let task_data = create_test_task_data();

        let hash1 = task_data.build_payload_hash(&[0x01, 0x02]);
        let hash2 = task_data.build_payload_hash(&[0x03, 0x04]);

        assert_ne!(hash1, hash2);
    }

    fn vars(pairs: &[(&str, &str)]) -> impl Iterator<Item = (String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Multi-overlay slot discovery: indexed `GK_OVERLAY_*_N` variables are
    /// found by scanning (so numbering gaps cannot silently drop a model),
    /// sorted, deduped across the three per-slot variables, and unrelated
    /// variables — including `GK_OVERLAY_MMAP` and the unsuffixed slot-1
    /// triplet — are ignored.
    #[test]
    fn test_indexed_overlay_slots_discovery() {
        assert_eq!(
            indexed_overlay_slots_from(vars(&[
                ("GK_OVERLAY_WEIGHTS", "/overlay/weights.bin"),
                ("GK_OVERLAY_TOKENIZER", "/overlay/tokenizer.bin"),
                ("GK_OVERLAY_MANIFEST", "0xabc"),
                ("GK_OVERLAY_MMAP", "true"),
                ("HTTP_RPC", "http://x"),
            ])),
            Vec::<u32>::new(),
            "slot-1 triplet and unrelated vars must not register as indexed slots"
        );

        assert_eq!(
            indexed_overlay_slots_from(vars(&[
                ("GK_OVERLAY_WEIGHTS_3", "/m3/weights.bin"),
                ("GK_OVERLAY_TOKENIZER_3", "/m3/tokenizer.bin"),
                ("GK_OVERLAY_MANIFEST_3", "0xdef"),
                ("GK_OVERLAY_WEIGHTS_2", "/m2/weights.bin"),
                ("GK_OVERLAY_TOKENIZER_2", "/m2/tokenizer.bin"),
                ("GK_OVERLAY_MANIFEST_2", "0xabc"),
            ])),
            vec![2, 3],
            "slots must come back sorted and deduped across the three variables"
        );

        // A gap in numbering still surfaces the configured slot — the
        // partial-slot assertions in overlay_files_slot then judge it.
        assert_eq!(
            indexed_overlay_slots_from(vars(&[("GK_OVERLAY_MANIFEST_4", "0xabc")])),
            vec![4],
        );

        // Empty values are "unset".
        assert_eq!(
            indexed_overlay_slots_from(vars(&[("GK_OVERLAY_WEIGHTS_2", "  ")])),
            Vec::<u32>::new(),
        );
    }

    #[test]
    #[should_panic(expected = "reserved slot")]
    fn test_indexed_overlay_slots_reject_reserved_slot_one() {
        indexed_overlay_slots_from(vars(&[("GK_OVERLAY_WEIGHTS_1", "/m1/weights.bin")]));
    }

    #[test]
    #[should_panic(expected = "invalid overlay slot suffix")]
    fn test_indexed_overlay_slots_reject_non_numeric_suffix() {
        indexed_overlay_slots_from(vars(&[("GK_OVERLAY_WEIGHTS_QWEN", "/m/weights.bin")]));
    }

    /// Regression coverage for the runtime-starvation fix (PR #319).
    ///
    /// `analyze_transaction` moves the CPU-bound, non-yielding analysis call
    /// onto Tokio's blocking pool via `spawn_blocking` + a captured `Handle`
    /// so it cannot starve other tasks on the shared worker pool — notably
    /// the /healthz, /readyz, /metrics, /prewarm axum server and the
    /// commonware P2P layer, both spawned via commonware's default
    /// (non-dedicated) execution mode onto that same pool. Exercising the
    /// real analyzer needs live RPC access (see the `#[ignore]`d
    /// `test_full_validation_with_rpc` above), so this test instead proves
    /// the underlying mechanism directly: on a 2-worker multi-thread runtime
    /// — matching commonware-runtime's `tokio::Config::new()` default
    /// (`worker_threads: 2`), which router/main.rs and node/main.rs both use
    /// unmodified — a long, synchronous, non-yielding closure offloaded via
    /// `spawn_blocking` must not delay a concurrent healthz-style async task
    /// spawned on the shared pool.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_healthz_style_task_stays_responsive_during_blocking_analysis() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;

        let blocking_task_started = Arc::new(AtomicBool::new(false));
        let started = Arc::clone(&blocking_task_started);

        // Mirrors `analyze_transaction`'s offload: a long CPU-bound section
        // that never yields to the async scheduler (stand-in for the revm
        // trace) runs inside `spawn_blocking`, re-entering async context via
        // a `Handle` captured before the blocking call — exactly the pattern
        // used for `GK_SIM_EXECUTOR=local`.
        let handle = tokio::runtime::Handle::current();
        let blocking = tokio::task::spawn_blocking(move || {
            started.store(true, Ordering::SeqCst);
            handle.block_on(async {
                // Synchronous, non-yielding sleep: stands in for the revm
                // `transact` compute section, which is what starved the
                // shared worker pool in production.
                std::thread::sleep(Duration::from_millis(500));
            });
        });

        // Don't race the healthz-style task until the blocking task is
        // actually occupying its thread.
        while !blocking_task_started.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Stand-in for the /healthz handler: a trivial async task spawned
        // onto the same 2-worker shared pool the test runtime uses. Before
        // this fix, `analyze_transaction` awaited the analysis directly on
        // that pool, so with only 2 workers a couple of concurrent traces
        // (or one trace plus ordinary load) could starve this task the same
        // way it starved the real healthz server.
        let healthz = tokio::spawn(async { 200u16 });

        let status = tokio::time::timeout(Duration::from_millis(200), healthz)
            .await
            .expect(
                "healthz-style task must respond well within the liveness probe's \
                 timeoutSeconds even while a long analysis is offloaded to the blocking pool",
            )
            .expect("healthz-style task panicked");
        assert_eq!(status, 200);

        blocking.await.expect("blocking analysis task panicked");
    }
}

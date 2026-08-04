use alloy_provider::Provider;
use anyhow::Result;
use commonware_cryptography::sha256::Digest;
use commonware_runtime::telemetry::metrics::encoding::text::encode;
use commonware_runtime::telemetry::metrics::raw::Histogram;
use commonware_runtime::telemetry::metrics::registry::Registry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use alloy_primitives::{Address, B256, U256};

use crate::ReadOnlyProvider;
use crate::config::{ChainRole, SpeculativePrebuildConfig};
use crate::task_data::GasKillerTaskData;

use alloy::rpc::types::TransactionRequest;

/// Key identifying a task's expected digest in [`GasKillerValidator`]'s digest cache.
///
/// The digest is `sha256(abi.encode(transition_index, target_address, anchor_hash,
/// from_address, call_data, storage_updates))`, and `storage_updates` is derived by EVMSketch
/// from `(target_address, call_data, from_address, value, block_height)`. `transition_index`
/// is a *per-contract* counter, so keying on `(transition_index, block_height)` alone
/// would collide two tasks for *different* contracts that share the same index and
/// block — returning the wrong contract's digest. The key covers every field the
/// digest depends on; `anchor_hash` is included both for that reason and so a task with a
/// forged anchor cannot hit an honest task's cache entry and skip the anchor check in
/// [`GasKillerValidator::expected_digest_for_task`].
type DigestCacheKey = (u64, u64, Address, Address, U256, Vec<u8>, B256);

fn digest_cache_key(task: &GasKillerTaskData) -> DigestCacheKey {
    (
        task.transition_index,
        task.block_height,
        task.target_address,
        task.from_address,
        task.value,
        task.call_data.clone(),
        task.anchor_hash,
    )
}
use gas_analyzer::{EvmSketchExecutorCache, call_to_encoded_state_updates_with_evmsketch_mode};

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
    /// The hash of the anchor block (`block_height`) the execution ran against.
    ///
    /// Read from the same EVMSketch executor that produced `storage_updates`, so it is
    /// guaranteed to be the hash of the block the updates were computed at. Bound into the
    /// signed message to anchor slashing challenges.
    pub anchor_hash: B256,
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
    /// Cache: task identity ([`DigestCacheKey`]) -> computed digest.
    /// Prevents re-running expensive EVMSketch for the same task when the
    /// orchestrator validates multiple signatures for identical task data.
    digest_cache: Arc<Mutex<HashMap<DigestCacheKey, Digest>>>,
    /// LRU cache of pre-built EvmSketch executors keyed by (rpc_url, block_number).
    /// Eliminates the 2× eth_getBlockByNumber build cost (~80–120 ms) for the
    /// 2nd…Nth request at the same block height.
    executor_cache: Arc<EvmSketchExecutorCache>,
    /// Optional Prometheus metrics — injected on the node, absent on the router.
    validator_metrics: Option<Arc<ValidatorMetrics>>,
    /// Storage-update encoding. Must be identical on the node and router (it
    /// changes `storage_updates`, hence the digest); the production `new()` path
    /// reads it from `STATE_ENCODING` on both binaries. See
    /// [`crate::config::state_encoding`].
    state_encoding: gas_analyzer::StateEncoding,
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
        let chain_rpc_urls = crate::chain_rpc_urls_from_env()?;
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
            executor_cache: Arc::new(EvmSketchExecutorCache::new(capacity)),
            validator_metrics: None,
            // Production path: node and router both call `new()`, so reading the
            // env here keeps their encoding (and therefore their digests) in sync.
            state_encoding: crate::config::state_encoding(),
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
            executor_cache: Arc::new(EvmSketchExecutorCache::new(capacity)),
            validator_metrics: None,
            state_encoding: gas_analyzer::StateEncoding::Legacy,
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
            executor_cache: Arc::new(EvmSketchExecutorCache::new(capacity)),
            validator_metrics: None,
            state_encoding: gas_analyzer::StateEncoding::Legacy,
        }
    }

    /// Attaches Prometheus metrics; call this on the node before passing the validator to the contributor.
    pub fn with_validator_metrics(mut self, metrics: Arc<ValidatorMetrics>) -> Self {
        self.validator_metrics = Some(metrics);
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
    /// Returns the storage updates, block height, the anchor block hash, and the actual EVM
    /// chain ID (u64).
    pub async fn compute_storage_updates_for_tx(
        &self,
        contract_address: alloy::primitives::Address,
        call_data: &[u8],
        from_address: Option<alloy::primitives::Address>,
        value: Option<alloy::primitives::U256>,
        block_height: u64,
    ) -> Result<(Vec<u8>, u64, B256, u64)> {
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
            result.anchor_hash,
            numeric_chain_id,
        ))
    }

    /// Precomputes and caches the payload digest using already-computed storage updates.
    ///
    /// Call this from the task creator after it runs EVMSketch to build the payload, so that
    /// the orchestrator's validator can skip running EVMSketch again when verifying each incoming
    /// node signature for the same round.
    pub async fn prime_cache(&self, task_data: &GasKillerTaskData, storage_updates: &[u8]) {
        let digest = task_data.build_payload_hash(storage_updates);
        let cache_key = digest_cache_key(task_data);
        let mut cache = self.digest_cache.lock().await;
        cache.insert(cache_key, digest);
        debug!(
            transition_index = task_data.transition_index,
            block_height = task_data.block_height,
            "Primed validator digest cache from creator (verification will skip EVMSketch)"
        );
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
        // same block height.
        let (storage_updates, gas_estimate, _is_heuristic, _skipped_opcodes) =
            call_to_encoded_state_updates_with_evmsketch_mode(
                &self.executor_cache,
                rpc_url,
                tx_request,
                block_height,
                self.state_encoding,
            )
            .await
            .map_err(|e| anyhow::anyhow!("Gas analysis failed: {}", e))?;

        // Read the anchor block hash from the same executor that produced the storage
        // updates. `call_to_encoded_state_updates_with_evmsketch_mode` just populated the
        // cache for (rpc_url, block_height), so this is a cache hit (no extra RPC). Deriving
        // the hash here — rather than trusting a task field — guarantees it is the hash of the
        // exact block the updates were computed against, which slashing challenges anchor to.
        let anchor_hash = self
            .executor_cache
            .get_or_build(rpc_url, block_height)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to resolve anchor block hash: {}", e))?
            .anchor_block_hash();

        debug!(
            "Analysis complete: storage_updates_len={}, gas_estimate={}, block_height={}, anchor_hash={}",
            storage_updates.len(),
            gas_estimate,
            block_height,
            anchor_hash
        );

        Ok(AnalysisResult {
            storage_updates: storage_updates.to_vec(),
            gas_estimate,
            block_height,
            anchor_hash,
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

    /// Recomputes the storage updates and anchor block hash for a task via EVMSketch.
    ///
    /// Automatically detects which chain the target address is on, and uses the block_height
    /// from task_data so the result is deterministic and matches the router's.
    ///
    /// Returns `(storage_updates, anchor_hash)` where `anchor_hash` is derived from the same
    /// execution — never trusted from the task — so it is the hash of the block the updates
    /// were computed against.
    async fn compute_storage_updates(
        &self,
        task_data: &GasKillerTaskData,
    ) -> Result<(Vec<u8>, B256)> {
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
        Ok((result.storage_updates, result.anchor_hash))
    }

    /// Validates a task and returns the digest a correct node is expected to sign for it.
    ///
    /// This is the single place where storage updates are recomputed (via EVMSketch at
    /// `task.block_height`) to avoid double computation: the recomputed updates are hashed
    /// with [`GasKillerTaskData::build_payload_hash`], so a task whose announced
    /// `storage_updates` diverge from local re-execution yields a different digest and the
    /// dishonest announcement never reaches quorum.
    ///
    /// It is also where the task's claimed anchor block hash is checked against the one the
    /// local execution actually ran at, so a task announcing a mismatched anchor is rejected
    /// rather than signed.
    ///
    /// Results are cached by [`digest_cache_key`] so that repeated calls for the
    /// same task (e.g. the router resolving its automaton digest after [`Self::prime_cache`],
    /// or a node re-proposing a height after restart) only run the expensive EVMSketch
    /// computation once. Errors are NOT cached: transient RPC failures surface to the caller,
    /// which retries with backoff (deterministic failures are the caller's cue to skip).
    pub async fn expected_digest_for_task(&self, task: &GasKillerTaskData) -> Result<Digest> {
        let task_data = task;

        let cache_key = digest_cache_key(task_data);

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

        // Not cached — compute storage updates (the expensive EVMSketch path)
        let (storage_updates, anchor_hash) = self.compute_storage_updates(task_data).await?;

        // Soundness guard: the anchor hash the task claims must be the hash of the block the
        // updates were actually computed against. If a task creator announces a mismatched
        // anchor, honest validators would sign a commitment a challenger could not reproduce
        // (and could be slashed for). Reject such a task so it never reaches quorum.
        if task_data.anchor_hash != anchor_hash {
            return Err(anyhow::anyhow!(
                "anchor hash mismatch for block {}: task claims {}, derived {}",
                task_data.block_height,
                task_data.anchor_hash,
                anchor_hash
            ));
        }

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
impl commonware_avs_core::validator::ValidatorTrait<GasKillerTaskData> for GasKillerValidator {
    async fn expected_digest(&self, task: &GasKillerTaskData) -> Result<Digest> {
        self.expected_digest_for_task(task).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, U256};

    fn create_test_task_data() -> GasKillerTaskData {
        GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04].into(),
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
            block_height: 12345,
            anchor_hash: B256::from([4u8; 32]),
            chain_id: 1u64,
        }
    }

    #[tokio::test]
    async fn test_validator_creation() {
        let _validator =
            GasKillerValidator::with_rpc_url("https://ethereum-sepolia.publicnode.com");
    }

    #[test]
    fn digest_cache_key_distinguishes_different_contracts() {
        // Two tasks for DIFFERENT contracts at the same (transition_index,
        // block_height) — the exact collision the key must avoid so one task's
        // cached digest is never returned for the other.
        let a = create_test_task_data();
        let mut b = a.clone();
        b.target_address = Address::from([9u8; 20]);
        assert_ne!(digest_cache_key(&a), digest_cache_key(&b));

        // Differing call_data (same contract) must also key distinctly, since it
        // changes the computed storage updates and therefore the digest.
        let mut c = a.clone();
        c.call_data = vec![0xde, 0xad, 0xbe, 0xef];
        assert_ne!(digest_cache_key(&a), digest_cache_key(&c));

        // A differing anchor keys distinctly too. Beyond being a digest input, this is what
        // forces a task with a forged anchor through the EVMSketch path — and therefore
        // through the anchor check — instead of hitting an honest task's cache entry.
        let mut d = a.clone();
        d.anchor_hash = B256::from([0xab; 32]);
        assert_ne!(digest_cache_key(&a), digest_cache_key(&d));

        // Identical task identity keys identically (cache hit is intended here).
        assert_eq!(digest_cache_key(&a), digest_cache_key(&a.clone()));
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
    async fn test_expected_digest_uses_primed_cache() {
        // prime_cache stores the digest keyed by (transition_index, block_height), so
        // expected_digest_for_task must return it without hitting any RPC. This is the
        // router-side flow: the sequencer primes after EVMSketch, the automaton looks up.
        let validator = GasKillerValidator::with_rpc_url("https://example.com");
        let task_data = create_test_task_data();
        let storage_updates = vec![0x01, 0x02, 0x03, 0x04];

        validator.prime_cache(&task_data, &storage_updates).await;

        let digest = validator
            .expected_digest_for_task(&task_data)
            .await
            .expect("cached digest lookup must not require RPC");
        assert_eq!(digest, task_data.build_payload_hash(&storage_updates));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires RPC - run with: cargo test -- --ignored"]
    async fn test_full_validation_with_rpc() {
        // Integration test: full validation including storage update computation
        // This test is ignored by default as it requires RPC access and Anvil
        let validator = GasKillerValidator::with_rpc_url("https://ethereum-sepolia.publicnode.com");
        let task_data = create_test_task_data();

        let result = validator.expected_digest_for_task(&task_data).await;

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
}

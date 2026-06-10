use alloy_provider::Provider;
use anyhow::Result;
use commonware_codec::Read;
use commonware_cryptography::sha256::Digest;
use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::ReadOnlyProvider;
use crate::config::{ChainRole, SpeculativePrebuildConfig};
use crate::task_data::GasKillerTaskData;
use commonware_avs_router::validator::ValidatorTrait;
use commonware_avs_router::wire;

use alloy::rpc::types::TransactionRequest;
use gas_analyzer::{EvmSketchExecutorCache, call_to_encoded_state_updates_with_evmsketch};
use url::Url;

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
    /// LRU cache of pre-built EvmSketch executors keyed by (rpc_url, block_number).
    /// Eliminates the 2× eth_getBlockByNumber build cost (~80–120 ms) for the
    /// 2nd…Nth request at the same block height.
    executor_cache: Arc<EvmSketchExecutorCache>,
    /// Optional Prometheus metrics — injected on the node, absent on the router.
    validator_metrics: Option<Arc<ValidatorMetrics>>,
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
        use alloy_provider::ProviderBuilder;
        let rpc_url = self
            .rpc_url_for_chain(chain)
            .ok_or_else(|| anyhow::anyhow!("No RPC URL configured for chain role: {}", chain))?;
        let url = Url::parse(rpc_url)
            .map_err(|e| anyhow::anyhow!("Failed to parse RPC URL for chain {}: {}", chain, e))?;
        ProviderBuilder::new()
            .connect_http(url)
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
            call_to_encoded_state_updates_with_evmsketch(
                &self.executor_cache,
                rpc_url,
                tx_request,
                block_height,
            )
            .await
            .map_err(|e| anyhow::anyhow!("Gas analysis failed: {}", e))?;

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
}

use alloy::primitives::U256;
use alloy::sol_types::SolValue;
use alloy_provider::Provider;
use anyhow::Result;
use commonware_codec::Read;
use commonware_cryptography::sha256::Digest;
use commonware_cryptography::{Hasher, Sha256};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;
use url::Url;

use crate::config::ChainId;
use crate::task_data::GasKillerTaskData;
use commonware_avs_router::validator::ValidatorTrait;
use commonware_avs_router::wire;

use alloy::eips::BlockNumberOrTag;
use alloy::rpc::types::TransactionRequest;
use gas_analyzer::call_to_encoded_state_updates_with_evmsketch;

/// Result of gas analysis containing storage updates and gas information
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    /// The storage updates extracted from the transaction
    pub storage_updates: Vec<u8>,
    /// The gas estimate from gas-analyzer-rs
    #[allow(dead_code)]
    pub gas_estimate: u64,
    /// The block height at which the analysis was performed
    pub block_height: u64,
}

/// Validator implementation for the gas killer use case with multi-chain support
#[derive(Clone)]
pub struct GasKillerValidator {
    /// RPC URLs per chain for the gas analyzer
    chain_rpc_urls: HashMap<ChainId, String>,
    /// Default chain for backwards compatibility
    default_chain: ChainId,
    /// Cache: (transition_index, block_height) -> computed digest
    /// Prevents re-running expensive EVMSketch for the same round when the
    /// orchestrator validates multiple signatures for identical task data.
    digest_cache: Arc<Mutex<HashMap<(u64, u64), Digest>>>,
}

impl GasKillerValidator {
    /// Creates a new GasKillerValidator with multi-chain support.
    ///
    /// Reads RPC URLs from environment variables:
    /// - RPC_URL or HTTP_RPC for L1 (required)
    /// - L2_RPC_URL or L2_HTTP_RPC for L2 (optional)
    ///
    /// Returns an error if L1 RPC is not set.
    pub fn new() -> Result<Self> {
        let mut chain_rpc_urls = HashMap::new();

        // Load L1 RPC URL (required)
        let l1_rpc = env::var("RPC_URL")
            .or_else(|_| env::var("HTTP_RPC"))
            .map_err(|_| anyhow::anyhow!("RPC_URL or HTTP_RPC environment variable is not set"))?;
        chain_rpc_urls.insert(ChainId::Sepolia, l1_rpc);

        // Load L2 RPC URL (optional)
        if let Ok(l2_rpc) = env::var("L2_RPC_URL").or_else(|_| env::var("L2_HTTP_RPC"))
        {
            chain_rpc_urls.insert(ChainId::Gnosis, l2_rpc);
        }

        Ok(Self {
            chain_rpc_urls,
            default_chain: ChainId::Sepolia,
            digest_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Creates a new GasKillerValidator with a specific RPC URL (for default chain).
    ///
    /// Useful for testing without modifying environment variables.
    pub fn with_rpc_url(rpc_url: impl Into<String>) -> Self {
        let mut chain_rpc_urls = HashMap::new();
        chain_rpc_urls.insert(ChainId::Sepolia, rpc_url.into());
        Self {
            chain_rpc_urls,
            default_chain: ChainId::Sepolia,
            digest_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Creates a new GasKillerValidator with RPC URLs for multiple chains.
    pub fn with_chain_rpc_urls(chain_rpc_urls: HashMap<ChainId, String>) -> Self {
        Self {
            chain_rpc_urls,
            default_chain: ChainId::Sepolia,
            digest_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Returns the RPC URL for the default chain
    pub fn rpc_url(&self) -> &str {
        self.chain_rpc_urls
            .get(&self.default_chain)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    /// Returns the RPC URL for a specific chain
    pub fn rpc_url_for_chain(&self, chain_id: ChainId) -> Option<&str> {
        self.chain_rpc_urls.get(&chain_id).map(|s| s.as_str())
    }

    /// Returns whether a chain is supported
    pub fn supports_chain(&self, chain_id: ChainId) -> bool {
        self.chain_rpc_urls.contains_key(&chain_id)
    }

    /// Returns all supported chains
    pub fn supported_chains(&self) -> Vec<ChainId> {
        self.chain_rpc_urls.keys().copied().collect()
    }

    /// Detects which chain has code deployed at the given address.
    ///
    /// Checks each supported chain to see if the address has contract code.
    /// Returns the first chain where code is found, or an error if no chain has code.
    pub async fn detect_chain_for_address(
        &self,
        address: alloy::primitives::Address,
    ) -> Result<ChainId> {
        use alloy_provider::ProviderBuilder;

        debug!(
            address = %address,
            "Detecting chain for address"
        );

        let supported = self.supported_chains();
        // Clone the RPC URLs so the closure doesn't borrow self
        let chain_rpc_urls = self.chain_rpc_urls.clone();

        crate::config::detect_chain_for_address(address, &supported, |chain_id, addr| {
            let chain_rpc_urls = chain_rpc_urls.clone();
            async move {
                let rpc_url = chain_rpc_urls
                    .get(&chain_id)
                    .ok_or_else(|| anyhow::anyhow!("No RPC URL for chain {}", chain_id))?;
                let url = Url::parse(rpc_url)?;
                let provider = ProviderBuilder::new().connect_http(url);
                let code = provider.get_code_at(addr).await?;
                Ok(code)
            }
        })
        .await
    }

    /// Computes storage updates for a transaction using gas-analyzer-rs.
    ///
    /// Automatically detects which chain the contract is on, then computes storage updates.
    /// Returns the storage updates, block height, and detected chain ID.
    pub async fn compute_storage_updates_for_tx(
        &self,
        contract_address: alloy::primitives::Address,
        call_data: &[u8],
        from_address: Option<alloy::primitives::Address>,
        value: Option<alloy::primitives::U256>,
        block_height: u64,
    ) -> Result<(Vec<u8>, u64, ChainId)> {
        // Detect which chain has the contract
        let chain_id = self.detect_chain_for_address(contract_address).await?;

        debug!(
            chain = %chain_id,
            address = %contract_address,
            "Detected chain for contract"
        );

        let rpc_url = self
            .rpc_url_for_chain(chain_id)
            .ok_or_else(|| anyhow::anyhow!("No RPC URL configured for chain: {}", chain_id))?;

        let result = Self::analyze_transaction(
            rpc_url,
            contract_address,
            call_data,
            from_address,
            value,
            block_height,
        )
        .await?;
        Ok((result.storage_updates, result.block_height, chain_id))
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
        let digest = self.build_payload_hash(task_data, storage_updates);
        let cache_key = (task_data.transition_index, task_data.block_height);
        let mut cache = self.digest_cache.lock().await;
        cache.insert(cache_key, digest);
        debug!(
            transition_index = task_data.transition_index,
            block_height = task_data.block_height,
            "Primed validator digest cache from creator (verification will skip EVMSketch)"
        );
    }

    /// Builds the payload hash from task data and storage updates
    ///
    /// This method must produce the same hash as the on-chain expectedHash
    /// in GasKillerSDK.verifyAndUpdate to ensure consensus consistency.
    fn build_payload_hash(&self, task_data: &GasKillerTaskData, storage_updates: &[u8]) -> Digest {
        // IMPORTANT: This hash must match the on-chain expectedHash in GasKillerSDK.verifyAndUpdate:
        // sha256(abi.encode(transitionIndex, address(this), targetFunction, storageUpdates))

        let selector = task_data.function_selector();

        // Debug: Log hash of full storage_updates to detect any differences
        let mut storage_hasher = Sha256::new();
        storage_hasher.update(storage_updates);
        let storage_hash = storage_hasher.finalize();
        let storage_hash_hex: String = storage_hash
            .iter()
            .take(8)
            .map(|b| format!("{:02x}", b))
            .collect();

        debug!(
            transition_index = task_data.transition_index,
            target_address = %task_data.target_address,
            target_function = %selector,
            storage_updates_len = storage_updates.len(),
            storage_updates_hash = %storage_hash_hex,
            "Validator build_payload_hash inputs"
        );

        // Build flattened ABI encoding matching abi.encode(transitionIndex, address(this), selector, storageUpdates)
        // Heads (32 bytes each)
        let head_transition = U256::from(task_data.transition_index).abi_encode();
        let head_address = task_data.target_address.abi_encode();
        let head_selector = selector.abi_encode();
        // Offset to the dynamic bytes tail: 4 words (3 static + 1 offset) = 0x80
        let head_offset = U256::from(32u64 * 4u64).abi_encode();

        // Tail for dynamic bytes: length (u256) + data + padding
        let mut tail = Vec::with_capacity(32 + storage_updates.len() + 31);
        tail.extend_from_slice(&U256::from(storage_updates.len()).abi_encode());
        tail.extend_from_slice(storage_updates);
        let pad_len = (32 - (storage_updates.len() % 32)) % 32;
        if pad_len > 0 {
            tail.extend(std::iter::repeat_n(0u8, pad_len));
        }

        // Concatenate head and tail into final payload
        let mut payload = Vec::with_capacity(32 * 4 + tail.len());
        payload.extend_from_slice(&head_transition);
        payload.extend_from_slice(&head_address);
        payload.extend_from_slice(&head_selector);
        payload.extend_from_slice(&head_offset);
        payload.extend_from_slice(&tail);

        let mut hasher = Sha256::new();
        hasher.update(&payload);
        let payload_hash = hasher.finalize();

        debug!("Built payload hash: {:?}", payload_hash);
        payload_hash
    }

    /// Performs the core gas analysis using gas-analyzer-rs
    ///
    /// This method contains the core logic for:
    /// 1. Forking the blockchain state
    /// 2. Executing the transaction
    /// 3. Extracting storage changes and gas information
    ///
    /// Takes an explicit RPC URL parameter for flexibility.
    /// Forks at the specified block for deterministic results.
    pub async fn analyze_transaction(
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

        // Call gas-analyzer-rs to get storage updates and gas estimate using EvmSketch
        let (storage_updates, gas_estimate, _is_heuristic, _skipped_opcodes) =
            call_to_encoded_state_updates_with_evmsketch(
                rpc_url,
                tx_request,
                BlockNumberOrTag::Number(block_height),
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

        let result = Self::analyze_transaction(
            rpc_url,
            task_data.target_address,
            &task_data.call_data,
            Some(task_data.from_address),
            Some(task_data.value),
            task_data.block_height,
        )
        .await?;
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

        // Not cached — compute storage updates
        let storage_updates = self.compute_storage_updates(task_data).await?;

        // Build expected payload hash using computed storage updates
        let payload_hash = self.build_payload_hash(task_data, &storage_updates);

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
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
            block_height: 12345,
        }
    }

    #[tokio::test]
    async fn test_validator_creation() {
        let _validator =
            GasKillerValidator::with_rpc_url("https://ethereum-sepolia.publicnode.com");
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
        let validator = GasKillerValidator::with_rpc_url("https://ethereum-sepolia.publicnode.com");
        let task_data = create_test_task_data();
        let storage_updates = vec![0x01, 0x02, 0x03, 0x04];

        let hash1 = validator.build_payload_hash(&task_data, &storage_updates);
        let hash2 = validator.build_payload_hash(&task_data, &storage_updates);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, Digest::from([0u8; 32]));
    }

    #[test]
    fn test_build_payload_hash_different_inputs() {
        let validator = GasKillerValidator::with_rpc_url("https://ethereum-sepolia.publicnode.com");
        let task_data = create_test_task_data();

        let hash1 = validator.build_payload_hash(&task_data, &[0x01, 0x02]);
        let hash2 = validator.build_payload_hash(&task_data, &[0x03, 0x04]);

        assert_ne!(hash1, hash2);
    }
}

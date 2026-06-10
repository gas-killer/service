use crate::creator::{DispatchTime, take_dispatch_time};
use crate::metrics::MetricsCollector;
use gas_killer_common::ChainRole;
use gas_killer_common::bindings::GAS_KILLER_INTERFACE_ID;
use gas_killer_common::bindings::gaskillersdk::{BN254, GasKillerSDK, IBLSSignatureCheckerTypes as GasKillerIBLSTypes};
use commonware_avs_router::bindings::bls_sig_check_operator_state_retriever::BLSSigCheckOperatorStateRetriever::getNonSignerStakesAndSignatureReturn;
use commonware_avs_router::executor::bls::BlsSignatureVerificationHandler;
use commonware_avs_router::executor::ExecutionResult;
use crate::task_data::GasKillerTaskData;
use alloy::network::Ethereum;
use alloy_primitives::{Address, Bytes, FixedBytes, U256};
use alloy_provider::Provider;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Default receipt-wait timeout on L1. At ~12s/block this covers several blocks
/// plus mempool-replacement headroom before the round is abandoned.
const DEFAULT_RECEIPT_TIMEOUT_L1_SECS: u64 = 120;
/// Default receipt-wait timeout on L2, where blocks land in seconds or less.
const DEFAULT_RECEIPT_TIMEOUT_L2_SECS: u64 = 30;

/// Handler for executing verifyAndUpdate transactions with multi-chain support
pub struct GasKillerHandler<P> {
    /// Wallet providers keyed by EVM chain ID
    providers: HashMap<u64, P>,
    /// Maps each actual EVM chain ID to its gas-killer role, resolved once at startup.
    /// Lets the executor pick the per-role receipt timeout from the numeric chain ID
    /// carried in task data, without re-querying `eth_chainId`.
    chain_roles: HashMap<u64, ChainRole>,
    metrics: Option<Arc<MetricsCollector>>,
    /// Shared with the creator to measure P2P round-trip duration.
    dispatch_time: DispatchTime,
    /// Memoizes ERC-165 GasKiller interface support per target address. A deployed
    /// contract's supported interfaces are immutable, so entries never expire.
    interface_cache: Arc<RwLock<HashMap<Address, bool>>>,
    /// Optional override (seconds) for the verifyAndUpdate receipt-wait timeout,
    /// applied to every chain. When unset, per-chain defaults apply. Sourced from
    /// `EXECUTOR_RECEIPT_TIMEOUT_SECS`.
    receipt_timeout_override: Option<u64>,
}

impl<P: Provider<Ethereum> + Clone + Send + Sync + 'static> GasKillerHandler<P> {
    /// Creates a new handler with a single provider (defaults to Ethereum mainnet key)
    pub fn new(provider: P) -> Self {
        let mut providers = HashMap::new();
        providers.insert(1u64, provider);
        Self {
            providers,
            chain_roles: HashMap::new(),
            metrics: None,
            dispatch_time: Default::default(),
            interface_cache: Arc::new(RwLock::new(HashMap::new())),
            receipt_timeout_override: None,
        }
    }

    /// Creates a new handler with providers for multiple chains, keyed by actual EVM chain ID.
    pub fn with_providers(providers: HashMap<u64, P>) -> Self {
        Self {
            providers,
            chain_roles: HashMap::new(),
            metrics: None,
            dispatch_time: Default::default(),
            interface_cache: Arc::new(RwLock::new(HashMap::new())),
            receipt_timeout_override: None,
        }
    }

    /// Records the role (L1/L2) of each actual EVM chain ID, used to select the
    /// per-role receipt timeout for the chain referenced in task data.
    pub fn with_chain_roles(mut self, chain_roles: HashMap<u64, ChainRole>) -> Self {
        self.chain_roles = chain_roles;
        self
    }

    pub fn with_metrics(mut self, metrics: Arc<MetricsCollector>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn with_dispatch_time(mut self, dispatch_time: DispatchTime) -> Self {
        self.dispatch_time = dispatch_time;
        self
    }

    /// Overrides the receipt-wait timeout (seconds) for all chains. `None` keeps
    /// the per-chain defaults.
    pub fn with_receipt_timeout(mut self, timeout_secs: Option<u64>) -> Self {
        self.receipt_timeout_override = timeout_secs;
        self
    }

    /// Adds a provider for a specific chain
    pub fn add_provider(&mut self, chain_id: u64, provider: P) {
        self.providers.insert(chain_id, provider);
    }

    /// Gets the provider for a specific chain
    fn get_provider(&self, chain_id: u64) -> Option<&P> {
        self.providers.get(&chain_id)
    }

    /// Resolves the receipt-wait timeout for `chain_role`: the configured override
    /// if set, otherwise the per-role default.
    fn receipt_timeout(&self, chain_role: ChainRole) -> Duration {
        let secs = self.receipt_timeout_override.unwrap_or(match chain_role {
            ChainRole::L1 => DEFAULT_RECEIPT_TIMEOUT_L1_SECS,
            ChainRole::L2 => DEFAULT_RECEIPT_TIMEOUT_L2_SECS,
        });
        Duration::from_secs(secs)
    }

    /// Resolves whether `target_addr` implements the GasKiller ERC-165 interface,
    /// memoizing the result per address. Interface support is immutable for a
    /// deployed contract, so the first lookup is reused on every later round and
    /// the per-round `supportsInterface` RPC collapses to a hashmap read.
    async fn supports_gas_killer_interface(
        &self,
        provider: P,
        target_addr: Address,
    ) -> Result<bool> {
        if let Some(supported) = self.interface_cache.read().await.get(&target_addr).copied() {
            return Ok(supported);
        }

        let gas_killer_sdk = GasKillerSDK::new(target_addr, provider);
        let supports_interface_start = Instant::now();
        let supported = match gas_killer_sdk
            .supportsInterface(GAS_KILLER_INTERFACE_ID)
            .call()
            .await
        {
            Ok(supported) => supported,
            Err(e) => {
                warn!("supportsInterface call failed: {}", e);
                return Err(anyhow::anyhow!("supportsInterface call failed: {}", e));
            }
        };
        if let Some(m) = &self.metrics {
            m.executor_supports_interface_seconds
                .observe(supports_interface_start.elapsed().as_secs_f64());
        }
        self.interface_cache
            .write()
            .await
            .insert(target_addr, supported);
        Ok(supported)
    }

    async fn execute_verification(
        &mut self,
        msg_hash: FixedBytes<32>,
        quorum_numbers: Bytes,
        current_block_number: u32,
        non_signer_data: getNonSignerStakesAndSignatureReturn,
        task_data: Option<&GasKillerTaskData>,
    ) -> Result<ExecutionResult> {
        // Unwrap the return type to get the actual data
        let data = non_signer_data._0;

        // Convert the non-signer data to the format expected by the GasKillerSDK
        let non_signer_struct_data = GasKillerIBLSTypes::NonSignerStakesAndSignature {
            nonSignerQuorumBitmapIndices: data.nonSignerQuorumBitmapIndices,
            nonSignerPubkeys: data
                .nonSignerPubkeys
                .into_iter()
                .map(|p| BN254::G1Point { X: p.X, Y: p.Y })
                .collect(),
            quorumApks: data
                .quorumApks
                .into_iter()
                .map(|p| BN254::G1Point { X: p.X, Y: p.Y })
                .collect(),
            apkG2: BN254::G2Point {
                X: data.apkG2.X,
                Y: data.apkG2.Y,
            },
            sigma: BN254::G1Point {
                X: data.sigma.X,
                Y: data.sigma.Y,
            },
            quorumApkIndices: data.quorumApkIndices,
            totalStakeIndices: data.totalStakeIndices,
            nonSignerStakeIndices: data.nonSignerStakeIndices,
        };

        // Validate that task data is provided
        let task_data = task_data
            .ok_or_else(|| anyhow::anyhow!("Task data is required for gas killer verification"))?;

        let chain_id: u64 = task_data.chain_id;

        // Get the chain-specific provider
        let provider = self
            .get_provider(chain_id)
            .ok_or_else(|| anyhow::anyhow!("No provider configured for chain: {}", chain_id))?
            .clone();

        info!(
            storage_updates_len = task_data.storage_updates.len(),
            chain = %chain_id,
            "Using storage updates from task data on detected chain"
        );

        // Extract task data parameters - use pre-computed storage_updates from task data
        let storage_updates = task_data.storage_updates.clone();
        let transition_index = U256::from(task_data.transition_index);
        let target_function = task_data.function_selector();
        let target_addr = task_data.target_address;

        // Debug: Log exact inputs for hash comparison
        debug!(
            transition_index = %transition_index,
            target_address = %target_addr,
            target_function = %target_function,
            storage_updates_len = storage_updates.len(),
            storage_updates_first_32 = %hex::encode(&task_data.storage_updates[..std::cmp::min(32, task_data.storage_updates.len())]),
            detected_chain = %chain_id,
            "Executor payload hash inputs"
        );

        // The payload-hash preflight and the ERC-165 interface check are independent,
        // so run them concurrently. Once the interface result is cached the second
        // future collapses to a hashmap read, making the join effectively free.
        let metrics = self.metrics.clone();
        let (expected_hash, supports_result) = tokio::join!(
            async {
                let hash_preflight_start = Instant::now();
                let expected_hash = FixedBytes::<32>::from(
                    task_data.build_payload_hash(storage_updates.as_ref()).0,
                );
                if let Some(m) = &metrics {
                    m.executor_hash_preflight_seconds
                        .observe(hash_preflight_start.elapsed().as_secs_f64());
                }
                expected_hash
            },
            self.supports_gas_killer_interface(provider.clone(), target_addr),
        );

        // Confirm the locally computed payload hash matches the quorum's signed hash.
        if expected_hash != msg_hash {
            warn!(
                offchain_msg_hash = %msg_hash,
                local_expected_hash = %expected_hash,
                transition_index = %transition_index,
                target_address = %target_addr,
                target_function = %target_function,
                storage_updates_len = storage_updates.len(),
                "Message hash mismatch between aggregation and local computation"
            );
            return Err(anyhow::anyhow!(
                "Message hash mismatch: aggregation {} != local {}",
                msg_hash,
                expected_hash
            ));
        }
        info!("Message hash match confirmed");

        // Ensure the contract implements the GasKiller interface via the ERC-165 check.
        if !supports_result? {
            warn!(
                interface_id = %GAS_KILLER_INTERFACE_ID,
                "Target contract does not support GasKiller interface"
            );
            return Err(anyhow::anyhow!(
                "Target contract does not support GasKiller interface ({})",
                GAS_KILLER_INTERFACE_ID
            ));
        }

        let gas_killer_sdk = GasKillerSDK::new(target_addr, provider);

        // Execute the gas killer verifyAndUpdate
        // Use referenceBlockNumber = current_block_number - 1 so that eth_estimateGas (which
        // simulates at the current block) satisfies the on-chain check:
        //   require(referenceBlockNumber < block.number)
        // Without the decrement, eth_estimateGas at block N sees referenceBlockNumber == N
        // and reverts with FutureBlockNumber.
        info!("Sending verifyAndUpdate transaction");
        let tx_send_start = Instant::now();
        let send_result = gas_killer_sdk
            .verifyAndUpdate(
                msg_hash,
                quorum_numbers,
                current_block_number.saturating_sub(1),
                storage_updates,
                transition_index,
                target_function,
                non_signer_struct_data,
            )
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send verifyAndUpdate transaction: {}", e));
        if let Some(m) = &self.metrics {
            m.executor_tx_send_seconds
                .observe(tx_send_start.elapsed().as_secs_f64());
        }
        let call_return = send_result?;

        // Bound the receipt wait so L1 mempool congestion, RPC degradation, or a
        // dropped transaction can't stall the executor indefinitely. On timeout we
        // return an error so the orchestrator counts the round as failed and moves on.
        // Unknown chain IDs fall back to the L1 (longer) timeout.
        let chain_role = self.chain_roles.get(&chain_id).copied().unwrap_or_default();
        let receipt_timeout = self.receipt_timeout(chain_role);
        let receipt_start = Instant::now();
        let receipt = match tokio::time::timeout(receipt_timeout, call_return.get_receipt()).await {
            Ok(receipt_result) => {
                if let Some(m) = &self.metrics {
                    m.executor_receipt_confirmation_seconds
                        .observe(receipt_start.elapsed().as_secs_f64());
                }
                receipt_result
                    .map_err(|e| anyhow::anyhow!("Failed to get transaction receipt: {}", e))?
            }
            Err(_) => {
                warn!(
                    chain = %chain_id,
                    timeout_secs = receipt_timeout.as_secs(),
                    "get_receipt timed out waiting for transaction inclusion"
                );
                return Err(anyhow::anyhow!(
                    "get_receipt timed out after {}s on chain {}",
                    receipt_timeout.as_secs(),
                    chain_id
                ));
            }
        };
        info!(
            tx = %receipt.transaction_hash,
            block = receipt.block_number,
            status = ?receipt.status(),
            gas_used = ?receipt.gas_used,
            "verifyAndUpdate receipt"
        );

        Ok(ExecutionResult {
            transaction_hash: format!("{:?}", receipt.transaction_hash),
            block_number: receipt.block_number,
            gas_used: Some(receipt.gas_used),
            status: Some(receipt.status()),
            contract_address: receipt.contract_address.map(|addr| format!("{:?}", addr)),
        })
    }
}

#[async_trait]
impl<P: Provider<Ethereum> + Clone + Send + Sync + 'static> BlsSignatureVerificationHandler
    for GasKillerHandler<P>
{
    type TaskData = GasKillerTaskData;

    async fn handle_verification(
        &mut self,
        round: u64,
        msg_hash: FixedBytes<32>,
        quorum_numbers: Bytes,
        current_block_number: u32,
        non_signer_data: getNonSignerStakesAndSignatureReturn,
        task_data: Option<&Self::TaskData>,
    ) -> Result<ExecutionResult> {
        // Record P2P round-trip: time from this round's creator dispatch to threshold signatures
        // received. Consume the entry keyed by `round` so a failed earlier round (which never
        // reaches here) cannot contribute a stale, inflated sample.
        if let Some(start) = take_dispatch_time(&self.dispatch_time, round)
            && let Some(m) = &self.metrics
        {
            m.p2p_round_trip_seconds
                .observe(start.elapsed().as_secs_f64());
        }

        let exec_start = Instant::now();

        let result = self
            .execute_verification(
                msg_hash,
                quorum_numbers,
                current_block_number,
                non_signer_data,
                task_data,
            )
            .await;

        if let Some(m) = &self.metrics {
            m.execution_duration_seconds
                .observe(exec_start.elapsed().as_secs_f64());
            match &result {
                Ok(_) => {
                    m.aggregation_rounds_completed.inc();
                }
                Err(_) => {
                    m.aggregation_rounds_failed.inc();
                }
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolValue;
    use alloy_provider::{ProviderBuilder, mock::Asserter};

    // supportsInterface(bytes4) returns (bool); the eth_call result is the
    // ABI-encoded bool wrapped as Bytes. Responses are consumed FIFO, so each
    // queued entry corresponds to exactly one RPC.
    fn push_supports_interface(asserter: &Asserter, supported: bool) {
        asserter.push_success(&Bytes::from(supported.abi_encode()));
    }

    #[tokio::test]
    async fn test_supports_interface_cached_after_first_call() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        // Queue a single response: the first lookup must hit the RPC and the
        // second must be served from the cache. A cache miss on the second call
        // would drain the empty asserter and error.
        push_supports_interface(&asserter, true);

        let handler = GasKillerHandler::new(provider.clone());
        let target = Address::from([0x11u8; 20]);

        let first = handler
            .supports_gas_killer_interface(provider.clone(), target)
            .await
            .expect("first lookup should resolve over RPC");
        assert!(first);

        let second = handler
            .supports_gas_killer_interface(provider.clone(), target)
            .await
            .expect("second lookup should be served from cache");
        assert!(second);
    }

    #[tokio::test]
    async fn test_supports_interface_caches_unsupported_result() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        push_supports_interface(&asserter, false);

        let handler = GasKillerHandler::new(provider.clone());
        let target = Address::from([0x22u8; 20]);

        // A `false` result is immutable too, so it is cached and reused without a
        // second RPC.
        assert!(
            !handler
                .supports_gas_killer_interface(provider.clone(), target)
                .await
                .unwrap()
        );
        assert!(
            !handler
                .supports_gas_killer_interface(provider.clone(), target)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_supports_interface_caches_per_address() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        // Two distinct addresses each require their own RPC; queue one response
        // per address, ordered to match the call sequence below.
        push_supports_interface(&asserter, true);
        push_supports_interface(&asserter, false);

        let handler = GasKillerHandler::new(provider.clone());
        let supported_addr = Address::from([0x33u8; 20]);
        let unsupported_addr = Address::from([0x44u8; 20]);

        assert!(
            handler
                .supports_gas_killer_interface(provider.clone(), supported_addr)
                .await
                .unwrap()
        );
        assert!(
            !handler
                .supports_gas_killer_interface(provider.clone(), unsupported_addr)
                .await
                .unwrap()
        );
        // Both addresses are now cached, so neither repeat lookup issues an RPC.
        assert!(
            handler
                .supports_gas_killer_interface(provider.clone(), supported_addr)
                .await
                .unwrap()
        );
        assert!(
            !handler
                .supports_gas_killer_interface(provider.clone(), unsupported_addr)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_receipt_timeout_defaults_per_chain() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let handler = GasKillerHandler::new(provider);

        assert_eq!(
            handler.receipt_timeout(ChainRole::L1),
            Duration::from_secs(120)
        );
        assert_eq!(
            handler.receipt_timeout(ChainRole::L2),
            Duration::from_secs(30)
        );
    }

    #[tokio::test]
    async fn test_receipt_timeout_override_applies_to_all_chains() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let handler = GasKillerHandler::new(provider).with_receipt_timeout(Some(45));

        assert_eq!(
            handler.receipt_timeout(ChainRole::L1),
            Duration::from_secs(45)
        );
        assert_eq!(
            handler.receipt_timeout(ChainRole::L2),
            Duration::from_secs(45)
        );
    }
}

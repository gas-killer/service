use crate::metrics::MetricsCollector;
use crate::payload_revert::PayloadRevert;
use crate::sequencer::{DispatchedTask, Traced, set_task_failed, set_task_ready};
use crate::store::SqliteStore;
use crate::task_data::GasKillerTaskData;
use alloy::network::Ethereum;
use alloy_primitives::{Address, Bytes, FixedBytes, U256};
use alloy_provider::Provider;
use anyhow::{Result, bail};
use commonware_avs_router::executor::ExecutionResult;
use gas_killer_common::ChainRole;
use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;
use gas_killer_common::bindings::schnorrstakeregistry::ISchnorrStakeRegistry;
use gas_killer_common::bindings::{GAS_KILLER_INTERFACE_ID, GAS_KILLER_NESTED_INTERFACE_ID};
use gas_killer_common::{
    BundleProof, NestedBundle, NestedFrame, NestedSpec, PayloadView, TaskBundle, TreeTrace,
};
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Default receipt-wait timeout on L1. At ~12s/block this covers several blocks
/// plus mempool-replacement headroom before the round is abandoned.
const DEFAULT_RECEIPT_TIMEOUT_L1_SECS: u64 = 120;
/// Default receipt-wait timeout on L2, where blocks land in seconds or less.
const DEFAULT_RECEIPT_TIMEOUT_L2_SECS: u64 = 30;

/// Gas estimate recorded in a rendered payload when `eth_estimateGas` could not be reached. It is
/// advisory — the user re-fills gas when submitting — so a transport failure still yields a
/// submittable payload rather than failing an already-completed round. Sized as a generous ceiling
/// that clears a real `verifyAndUpdate` (signature verification plus a large batched state
/// transition) so a caller that submits it verbatim as the gas limit does not run out of gas; it
/// stays well under the block gas limit, and unused gas is refunded, so over-provisioning costs
/// the submitter nothing.
///
/// It never stands in for a call that executed and reverted: those fail the round outright, so a
/// payload carrying this estimate is one whose landability is unknown, not one known to fail.
const PAYLOAD_GAS_ESTIMATE_FALLBACK: u64 = 10_000_000;

/// Resolved inputs for a `verifyAndUpdate` call, assembled once by
/// [`GasKillerHandler::prepare_schnorr`] and consumed by either the render path or the retained
/// broadcast path.
struct PreparedSchnorr<P> {
    provider: P,
    chain_id: u64,
    target_addr: Address,
    from_address: Address,
    msg_hash: FixedBytes<32>,
    reference_block_number: u32,
    storage_updates: Bytes,
    transition_index: u64,
    target_function: FixedBytes<4>,
    s: U256,
    r_addr: Address,
    non_signers: Vec<Address>,
}

/// A completed round rendered for user execution: the ready-to-sign transaction request plus the
/// durable [`TaskBundle`] it was derived from.
struct RenderedRound {
    payload: PayloadView,
    bundle: TaskBundle,
}

/// The quorum signature's arguments as every render shapes them.
struct QuorumProof {
    msg_hash: FixedBytes<32>,
    current_block_number: u32,
    s: U256,
    r_addr: Address,
    non_signers: Vec<Address>,
}

/// What every frame contract of a tree must agree on before the tree is rendered.
struct TreePreflight {
    registry: Address,
    /// The smallest `blockStaleMeasure()` in the tree: the frame that goes stale first bounds
    /// how long the payload can land.
    min_stale_measure: u64,
}

/// Bound a rendered payload's validity so it expires before the operator set can change.
///
/// A payload is submittable until `valid_until_block`, but a Schnorr settlement only verifies while
/// the operator set still matches the one its signature was assembled against. `horizon` is the
/// registry's earliest possible mutation block, so validity must end strictly below it — a
/// settlement landing *on* that block can already see the mutated set.
///
/// `None` leaves the value untouched (the horizon could not be read). `u64::MAX` — which the
/// registry returns as `type(uint256).max` when nothing is scheduled — is the common case and
/// clamps nothing. A horizon at or below the existing validity yields a payload that is already
/// expired; that is deliberate, and `GET /tasks/{id}` answers it with the usual `PAYLOAD_EXPIRED`
/// re-request rather than handing over calldata that would revert.
fn clamp_to_mutation_horizon(valid_until_block: u64, horizon: Option<U256>) -> u64 {
    let Some(horizon) = horizon else {
        return valid_until_block;
    };
    // Saturating: an unscheduled horizon is `type(uint256).max`, far beyond u64.
    let horizon = u64::try_from(horizon).unwrap_or(u64::MAX);
    valid_until_block.min(horizon.saturating_sub(1))
}

/// The last block a tree referencing `reference` can land in: within the payload buffer, before
/// the stalest frame's `blockStaleMeasure` lapses, and no later than the signed expiry.
fn tree_valid_until(reference: u64, buffer: u64, min_stale_measure: u64, expiry_block: u64) -> u64 {
    reference
        .saturating_add(buffer.min(min_stale_measure))
        .min(expiry_block)
}

/// Handler for executing verifyAndUpdate transactions with multi-chain support
#[derive(Clone)]
pub struct GasKillerHandler<P> {
    /// Wallet providers keyed by EVM chain ID
    providers: HashMap<u64, P>,
    /// Maps each actual EVM chain ID to its gas-killer role, resolved once at startup.
    /// Lets the executor pick the per-role receipt timeout from the numeric chain ID
    /// carried in task data, without re-querying `eth_chainId`.
    chain_roles: HashMap<u64, ChainRole>,
    metrics: Option<Arc<MetricsCollector>>,
    /// Memoizes ERC-165 GasKiller interface support per target address. A deployed
    /// contract's supported interfaces are immutable, so entries never expire.
    interface_cache: Arc<RwLock<HashMap<Address, bool>>>,
    /// Optional override (seconds) for the verifyAndUpdate receipt-wait timeout,
    /// applied to every chain. When unset, per-chain defaults apply. Sourced from
    /// `EXECUTOR_RECEIPT_TIMEOUT_SECS`.
    receipt_timeout_override: Option<u64>,
    /// Durable store used to settle a task's terminal status once its height
    /// executes. `None` in store-less test/dev harnesses, where the transition is
    /// skipped.
    store: Option<SqliteStore>,
    /// Blocks past the reference block for which a rendered payload's `valid_until_block` is set.
    /// Sourced from `PAYLOAD_BLOCK_BUFFER`.
    payload_block_buffer: u64,
}

impl<P: Provider<Ethereum> + Clone + Send + Sync + 'static> GasKillerHandler<P> {
    /// Creates a new handler with a single provider for the given EVM chain ID.
    pub fn new(chain_id: u64, provider: P) -> Self {
        let mut providers = HashMap::new();
        providers.insert(chain_id, provider);
        Self {
            providers,
            chain_roles: HashMap::new(),
            metrics: None,
            interface_cache: Arc::new(RwLock::new(HashMap::new())),
            receipt_timeout_override: None,
            store: None,
            payload_block_buffer: gas_killer_common::DEFAULT_PAYLOAD_BLOCK_BUFFER,
        }
    }

    /// Creates a new handler with providers for multiple chains, keyed by actual EVM chain ID.
    pub fn with_providers(providers: HashMap<u64, P>) -> Self {
        Self {
            providers,
            chain_roles: HashMap::new(),
            metrics: None,
            interface_cache: Arc::new(RwLock::new(HashMap::new())),
            receipt_timeout_override: None,
            store: None,
            payload_block_buffer: gas_killer_common::DEFAULT_PAYLOAD_BLOCK_BUFFER,
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

    /// Overrides the receipt-wait timeout (seconds) for all chains. `None` keeps
    /// the per-chain defaults.
    pub fn with_receipt_timeout(mut self, timeout_secs: Option<u64>) -> Self {
        self.receipt_timeout_override = timeout_secs;
        self
    }

    /// Attaches the durable store so a settled height advances its task's terminal
    /// status.
    pub fn with_store(mut self, store: SqliteStore) -> Self {
        self.store = Some(store);
        self
    }

    /// Sets the block buffer used to compute a rendered payload's `valid_until_block`
    /// (`reference_block_number + buffer`).
    pub fn with_payload_block_buffer(mut self, buffer: u64) -> Self {
        self.payload_block_buffer = buffer;
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

    /// Resolves the advisory `estimated_gas` for a rendered payload from the outcome of
    /// `eth_estimateGas`, or fails the round when that call proved the payload cannot land.
    ///
    /// The estimate doubles as the router's only end-to-end submittability check: it executes the
    /// exact calldata the client would send, as the client's account, against the target's current
    /// state and through the same signature verification. A revert there is proof the transaction
    /// would revert for the client too, so the round fails with the decoded cause rather than
    /// returning calldata that is certain to burn the client's gas.
    ///
    /// Failures that never reached execution — transport errors, timeouts, rate limiting — carry
    /// no information about landability, so they keep [`PAYLOAD_GAS_ESTIMATE_FALLBACK`] and the
    /// payload is still returned.
    fn resolve_payload_gas(
        &self,
        outcome: alloy::contract::Result<u64>,
        target_addr: Address,
    ) -> Result<u64> {
        let error = match outcome {
            Ok(gas) => return Ok(gas),
            Err(error) => error,
        };

        match PayloadRevert::from_call_error(&error) {
            Some(revert) => {
                warn!(
                    target = %target_addr,
                    %error,
                    revert = %revert,
                    "verifyAndUpdate reverts at current state; failing the round instead of \
                     returning an unsubmittable payload"
                );
                if let Some(m) = &self.metrics {
                    m.payloads_rejected_reverting.inc();
                }
                bail!("rendered payload reverts and cannot be submitted: {revert}");
            }
            None => {
                warn!(
                    target = %target_addr,
                    %error,
                    fallback = PAYLOAD_GAS_ESTIMATE_FALLBACK,
                    "verifyAndUpdate gas estimation unreachable; using fallback estimate"
                );
                Ok(PAYLOAD_GAS_ESTIMATE_FALLBACK)
            }
        }
    }

    /// Resolves whether `target_addr` implements the Gas Killer ERC-165 interface,
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

        let sdk = GasKillerSDK::new(target_addr, provider);
        let supports_interface_start = Instant::now();
        let supported = match sdk.supportsInterface(GAS_KILLER_INTERFACE_ID).call().await {
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

    /// Runs the shared preflight for a round and resolves every `verifyAndUpdate` input.
    ///
    /// Resolves the chain provider, confirms the locally recomputed payload hash matches the
    /// quorum-signed hash, and gates on the target's ERC-165 Gas Killer interface.
    /// `reference_block_number = current_block_number - 1` so that a simulation at the current
    /// block satisfies the on-chain `require(referenceBlockNumber < block.number)`; without the
    /// decrement a simulation at block N would see `referenceBlockNumber == N` and revert with
    /// `FutureBlockNumber`.
    async fn prepare_schnorr(
        &self,
        msg_hash: FixedBytes<32>,
        current_block_number: u32,
        s: U256,
        r_addr: Address,
        non_signers: Vec<Address>,
        task_data: Option<&GasKillerTaskData>,
    ) -> Result<PreparedSchnorr<P>> {
        let task_data = task_data
            .ok_or_else(|| anyhow::anyhow!("Task data is required for gas killer verification"))?;

        let chain_id: u64 = task_data.chain_id;
        let provider = self
            .get_provider(chain_id)
            .ok_or_else(|| anyhow::anyhow!("No provider configured for chain: {}", chain_id))?
            .clone();

        let storage_updates = task_data.storage_updates.clone();
        let transition_index = task_data.transition_index;
        let target_function = task_data.function_selector();
        let target_addr = task_data.target_address;
        let from_address = task_data.from_address;

        // The payload-hash preflight and the ERC-165 interface check are
        // independent, so run them concurrently. Once the interface result is
        // cached the second future collapses to a hashmap read.
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
                transition_index,
                target_address = %target_addr,
                "Message hash mismatch between aggregation and local computation"
            );
            return Err(anyhow::anyhow!(
                "Message hash mismatch: aggregation {} != local {}",
                msg_hash,
                expected_hash
            ));
        }

        // Ensure the contract implements the Gas Killer interface (ERC-165).
        if !supports_result? {
            warn!(
                interface_id = %GAS_KILLER_INTERFACE_ID,
                "Target contract does not support the Gas Killer interface"
            );
            return Err(anyhow::anyhow!(
                "Target contract does not support the Gas Killer interface ({})",
                GAS_KILLER_INTERFACE_ID
            ));
        }

        Ok(PreparedSchnorr {
            provider,
            chain_id,
            target_addr,
            from_address,
            msg_hash,
            reference_block_number: current_block_number.saturating_sub(1),
            storage_updates,
            transition_index,
            target_function,
            s,
            r_addr,
            non_signers,
        })
    }

    /// The **auto-execute** broadcast path, retained for the per-API-key auto-execute /
    /// account-abstraction tier. The completion handler renders a
    /// user-signed payload via [`Self::render_schnorr_payload`]; both share
    /// [`Self::prepare_schnorr`].
    ///
    /// Concurrent sessions settle through clones sharing one wallet, and `SimpleNonceManager`
    /// reserves nothing, so a caller must serialize fill-through-send per chain wallet.
    pub async fn execute_schnorr_verification(
        &mut self,
        msg_hash: FixedBytes<32>,
        current_block_number: u32,
        s: U256,
        r_addr: Address,
        non_signers: Vec<Address>,
        task_data: Option<&GasKillerTaskData>,
    ) -> Result<ExecutionResult> {
        let prepared = self
            .prepare_schnorr(
                msg_hash,
                current_block_number,
                s,
                r_addr,
                non_signers,
                task_data,
            )
            .await?;
        let PreparedSchnorr {
            provider,
            chain_id,
            target_addr,
            msg_hash,
            reference_block_number,
            storage_updates,
            transition_index,
            target_function,
            s,
            r_addr,
            non_signers,
            ..
        } = prepared;

        let sdk = GasKillerSDK::new(target_addr, provider);

        info!(
            non_signers = non_signers.len(),
            "Sending Schnorr verifyAndUpdate transaction"
        );
        let tx_send_start = Instant::now();
        let send_result = sdk
            .verifyAndUpdate(
                msg_hash,
                reference_block_number,
                storage_updates,
                U256::from(transition_index),
                target_function,
                s,
                r_addr,
                non_signers,
            )
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send verifyAndUpdate transaction: {}", e));
        if let Some(m) = &self.metrics {
            m.executor_tx_send_seconds
                .observe(tx_send_start.elapsed().as_secs_f64());
        }
        let call_return = send_result?;

        // Bound the receipt wait so mempool congestion or a dropped transaction
        // can't stall the executor indefinitely. Unknown chain IDs fall back to
        // the L1 (longer) timeout.
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
            "Schnorr verifyAndUpdate receipt"
        );

        Ok(ExecutionResult {
            transaction_hash: format!("{:?}", receipt.transaction_hash),
            block_number: receipt.block_number,
            gas_used: Some(receipt.gas_used),
            status: Some(receipt.status()),
            contract_address: receipt.contract_address.map(|a| a.to_string()),
        })
    }

    /// Read the target's Schnorr registry horizon: the earliest block at which its operator set
    /// can change.
    ///
    /// `SchnorrStakeRegistry` holds a single current aggregate key rather than per-block history,
    /// so any operator-set change invalidates a signature an off-chain round already assembled
    /// against the previous set. Changes are announced ahead of time and this horizon publishes
    /// when the earliest one becomes applicable, which is what lets a rendered payload be given a
    /// validity that ends before the set can move underneath it.
    ///
    /// Returns `None` when the horizon cannot be established, in which case the payload keeps its
    /// unclamped validity: a transient RPC failure should not discard a completed signing round,
    /// and the registry's own fail-closed watermark still rejects a settlement assembled against a
    /// stale set. The guarantee is best-effort for the same reason it is only a guarantee for the
    /// announced path — `registerOperator` / `deregisterOperator` bypass the notice window
    /// entirely and emit `ForcedMutation`.
    ///
    /// Both reads happen per render. The registry address looks memoizable per target the way
    /// [`Self::interface_cache`] memoizes ERC-165 support, but the two differ: a contract's
    /// supported interfaces are immutable, whereas `GasKillerSDK` keeps its registry in
    /// storage behind an `internal` setter, so a target is free to expose an owner-gated path that
    /// re-points it. A memoized address would survive that for the lifetime of the process and read
    /// the horizon off a registry the target no longer uses — failing open, and silently. Rendering
    /// runs once per completed round, so the second call is not on a hot path.
    async fn schnorr_mutation_horizon(&self, provider: P, target_addr: Address) -> Option<U256> {
        let sdk = GasKillerSDK::new(target_addr, provider.clone());
        let registry_addr = match sdk.schnorrRegistry().call().await {
            Ok(addr) => addr,
            Err(error) => {
                warn!(
                    target = %target_addr,
                    %error,
                    "could not read schnorrRegistry; leaving payload validity unclamped"
                );
                return None;
            }
        };
        self.registry_horizon(provider, registry_addr).await
    }

    /// [`Self::schnorr_mutation_horizon`] for a registry already read.
    async fn registry_horizon(&self, provider: P, registry_addr: Address) -> Option<U256> {
        let registry = ISchnorrStakeRegistry::new(registry_addr, provider);
        match registry.nextPossibleMutationBlock().call().await {
            Ok(horizon) => Some(horizon),
            Err(error) => {
                warn!(
                    registry = %registry_addr,
                    %error,
                    "could not read nextPossibleMutationBlock; leaving payload validity unclamped"
                );
                None
            }
        }
    }

    /// Renders a completed round into a user-signable transaction request and the durable
    /// [`TaskBundle`] it derives from, without broadcasting.
    ///
    /// `data` is the full `verifyAndUpdate` calldata; `estimated_gas` comes from
    /// `eth_estimateGas` simulated as the requesting account, via
    /// [`Self::resolve_payload_gas`] — which also fails the round if that call reverts. `value` is
    /// fixed at zero and kept server-controlled, so a future on-chain fee is a server change, not
    /// an integrator client-code change.
    #[allow(clippy::too_many_arguments)]
    async fn render_schnorr_payload(
        &self,
        msg_hash: FixedBytes<32>,
        current_block_number: u32,
        s: U256,
        r_addr: Address,
        non_signers: Vec<Address>,
        task_data: Option<&GasKillerTaskData>,
    ) -> Result<RenderedRound> {
        let prepared = self
            .prepare_schnorr(
                msg_hash,
                current_block_number,
                s,
                r_addr,
                non_signers,
                task_data,
            )
            .await?;
        let PreparedSchnorr {
            provider,
            chain_id,
            target_addr,
            from_address,
            msg_hash,
            reference_block_number,
            storage_updates,
            transition_index,
            target_function,
            s,
            r_addr,
            non_signers,
        } = prepared;

        let value = U256::ZERO;
        let sdk = GasKillerSDK::new(target_addr, provider);
        let call = sdk
            .verifyAndUpdate(
                msg_hash,
                reference_block_number,
                storage_updates.clone(),
                U256::from(transition_index),
                target_function,
                s,
                r_addr,
                non_signers.clone(),
            )
            .from(from_address)
            .value(value);

        let data = call.calldata().clone();
        let estimated_gas = self.resolve_payload_gas(call.estimate_gas().await, target_addr)?;

        let unclamped_valid_until = reference_block_number as u64 + self.payload_block_buffer;
        let horizon = self
            .schnorr_mutation_horizon(sdk.provider().clone(), target_addr)
            .await;
        let valid_until_block = clamp_to_mutation_horizon(unclamped_valid_until, horizon);
        if valid_until_block < unclamped_valid_until {
            info!(
                target = %target_addr,
                reference_block = reference_block_number,
                valid_until_block,
                unclamped_valid_until,
                "payload validity shortened to expire before the operator set can change"
            );
        }

        let payload = PayloadView {
            to: target_addr,
            data,
            value,
            chain_id,
            estimated_gas,
            valid_until_block,
        };
        let bundle = TaskBundle {
            msg_hash,
            reference_block_number,
            transition_index,
            target_address: target_addr,
            target_function,
            storage_updates,
            chain_id,
            value,
            valid_until_block,
            proof: BundleProof::Schnorr {
                s,
                r_addr,
                non_signers,
            },
            nested: None,
        };
        Ok(RenderedRound { payload, bundle })
    }

    /// Renders a signed nested tree as `verifyAndUpdateTree`, after checking the router's own
    /// tree hashes to the signed root and that every frame contract can apply its frame.
    async fn render_tree_payload(
        &self,
        task: &GasKillerTaskData,
        tree: &TreeTrace,
        spec: NestedSpec,
        proof: QuorumProof,
    ) -> Result<RenderedRound> {
        let QuorumProof {
            msg_hash,
            current_block_number,
            s,
            r_addr,
            non_signers,
        } = proof;
        let encoded = tree
            .encoded
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("a one-frame trace settles flat"))?;
        let local_root = FixedBytes::<32>::from(encoded.root.0);
        if local_root != msg_hash {
            warn!(
                offchain_msg_hash = %msg_hash,
                local_root = %local_root,
                target_address = %task.target_address,
                "Tree root mismatch between aggregation and local computation"
            );
            bail!("Message hash mismatch: aggregation {msg_hash} != local {local_root}");
        }

        let chain_id = task.chain_id;
        let provider = self
            .get_provider(chain_id)
            .ok_or_else(|| anyhow::anyhow!("No provider configured for chain: {}", chain_id))?
            .clone();
        let contracts: BTreeSet<Address> = tree.frames.iter().map(|f| f.target).collect();
        let preflight = self
            .preflight_tree(&provider, task.target_address, &contracts)
            .await?;

        let reference_block_number = current_block_number.saturating_sub(1);
        let target_function = task.function_selector();
        let submission = GasKillerSDK::TreeSubmission {
            root: msg_hash,
            expiryBlock: U256::from(spec.expiry_block),
            expiryProof: encoded.expiry_proof.clone(),
            sig: GasKillerSDK::QuorumSignature {
                s,
                Raddr: r_addr,
                nonSigners: non_signers.clone(),
                refBlock: U256::from(reference_block_number),
            },
            transitionIndex: U256::from(task.transition_index),
            targetFunction: target_function,
            storageUpdates: tree.root_program.clone(),
            proof: encoded.root_proof.clone(),
            children: encoded.root_children.clone(),
        };

        let value = U256::ZERO;
        let sdk = GasKillerSDK::new(task.target_address, provider);
        let call = sdk
            .verifyAndUpdateTree(submission)
            .from(task.from_address)
            .value(value);
        let data = call.calldata().clone();
        let estimated_gas =
            self.resolve_payload_gas(call.estimate_gas().await, task.target_address)?;

        let reference = reference_block_number as u64;
        let unclamped_valid_until = tree_valid_until(
            reference,
            self.payload_block_buffer,
            preflight.min_stale_measure,
            spec.expiry_block,
        );
        let horizon = self
            .registry_horizon(sdk.provider().clone(), preflight.registry)
            .await;
        let valid_until_block = clamp_to_mutation_horizon(unclamped_valid_until, horizon);

        let payload = PayloadView {
            to: task.target_address,
            data,
            value,
            chain_id,
            estimated_gas,
            valid_until_block,
        };
        let bundle = TaskBundle {
            msg_hash,
            reference_block_number,
            transition_index: task.transition_index,
            target_address: task.target_address,
            target_function,
            storage_updates: tree.root_program.clone(),
            chain_id,
            value,
            valid_until_block,
            proof: BundleProof::Schnorr {
                s,
                r_addr,
                non_signers,
            },
            nested: Some(NestedBundle {
                expiry_block: spec.expiry_block,
                expiry_proof: encoded.expiry_proof.clone(),
                proof: encoded.root_proof.clone(),
                children: encoded.root_children.clone(),
                frames: tree
                    .frames
                    .iter()
                    .map(|frame| NestedFrame {
                        contract: frame.target,
                        transition_index: frame
                            .transition_index
                            .and_then(|i| u64::try_from(i).ok())
                            .unwrap_or_default(),
                    })
                    .collect(),
            }),
        };
        Ok(RenderedRound { payload, bundle })
    }

    /// Checks every frame contract reports the nested interface and shares the root's registry,
    /// whose approval is the only one a nested frame will find.
    async fn preflight_tree(
        &self,
        provider: &P,
        root: Address,
        contracts: &BTreeSet<Address>,
    ) -> Result<TreePreflight> {
        let root_registry = GasKillerSDK::new(root, provider.clone())
            .schnorrRegistry()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("schnorrRegistry call on {root} failed: {e}"))?;
        let mut min_stale_measure = u64::MAX;
        for &contract in contracts {
            let sdk = GasKillerSDK::new(contract, provider.clone());
            let supported = sdk
                .supportsInterface(GAS_KILLER_NESTED_INTERFACE_ID)
                .call()
                .await
                .map_err(|e| anyhow::anyhow!("supportsInterface call on {contract} failed: {e}"))?;
            if !supported {
                bail!(
                    "frame contract {contract} does not support the nested Gas Killer interface \
                     ({GAS_KILLER_NESTED_INTERFACE_ID})"
                );
            }
            let registry =
                sdk.schnorrRegistry().call().await.map_err(|e| {
                    anyhow::anyhow!("schnorrRegistry call on {contract} failed: {e}")
                })?;
            if registry != root_registry {
                bail!(
                    "frame contract {contract} uses schnorrRegistry {registry}, but the root \
                     {root} uses {root_registry}"
                );
            }
            let measure =
                sdk.blockStaleMeasure().call().await.map_err(|e| {
                    anyhow::anyhow!("blockStaleMeasure call on {contract} failed: {e}")
                })?;
            min_stale_measure = min_stale_measure.min(u64::try_from(measure).unwrap_or(u64::MAX));
        }
        Ok(TreePreflight {
            registry: root_registry,
            min_stale_measure,
        })
    }

    /// The certified task with the router's own trace, which started alongside the session.
    /// Rendering checks the trace against the digest the quorum signed, so the quorum's digest
    /// is never rendered unchecked.
    async fn traced_task(
        &self,
        dispatched: &DispatchedTask,
    ) -> Result<(GasKillerTaskData, Traced)> {
        let traced = dispatched
            .trace
            .wait()
            .await
            .map_err(|e| anyhow::anyhow!("task enrichment failed: {e}"))?;
        let task = GasKillerTaskData {
            storage_updates: traced.flat_program().cloned().unwrap_or_default(),
            ..dispatched.task.clone()
        };
        task.validate()?;
        Ok((task, traced))
    }

    async fn render(
        &self,
        dispatched: &DispatchedTask,
        proof: QuorumProof,
    ) -> Result<RenderedRound> {
        let (task, traced) = self.traced_task(dispatched).await?;
        match (&traced, dispatched.nested) {
            (Traced::Tree(tree), Some(spec)) if tree.is_nested() => {
                self.render_tree_payload(&task, tree, spec, proof).await
            }
            _ => {
                self.render_schnorr_payload(
                    proof.msg_hash,
                    proof.current_block_number,
                    proof.s,
                    proof.r_addr,
                    proof.non_signers,
                    Some(&task),
                )
                .await
            }
        }
    }

    /// Renders a signed session's payload, settling its task `ready` on success and returning
    /// the payload's last valid block. Called by [`crate::schnorr_submitter::SchnorrSubmitter`]
    /// once per attempt; `started` is when the session began, for the round-latency measurement.
    ///
    /// A failed attempt leaves the task untouched so a retry can still render it; the
    /// submitter settles the failure through [`Self::settle_failed`] once it stops retrying.
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_schnorr_verification(
        &mut self,
        dispatched: &DispatchedTask,
        started: Instant,
        msg_hash: FixedBytes<32>,
        current_block_number: u32,
        s: U256,
        r_addr: Address,
        non_signers: Vec<Address>,
    ) -> Result<u64> {
        let exec_start = Instant::now();

        let result = self
            .render(
                dispatched,
                QuorumProof {
                    msg_hash,
                    current_block_number,
                    s,
                    r_addr,
                    non_signers,
                },
            )
            .await;

        if let Some(m) = &self.metrics {
            m.execution_duration_seconds
                .observe(exec_start.elapsed().as_secs_f64());
        }
        let rendered = result?;

        if let Some(m) = &self.metrics {
            m.aggregation_rounds_completed.inc();
            m.round_latency_seconds
                .observe(started.elapsed().as_secs_f64());
        }

        // The on-chain submission is left to the user (or the future auto-execute tier).
        if let Some(store) = &self.store {
            set_task_ready(
                store,
                self.metrics.as_deref(),
                &dispatched.task_id,
                &rendered.payload,
                &rendered.bundle,
            )
            .await;
        }

        Ok(rendered.payload.valid_until_block)
    }

    /// Settles a signed session's task `failed` once its render attempts are exhausted.
    pub async fn settle_render_failed(&mut self, dispatched: &DispatchedTask, reason: &str) {
        if let Some(m) = &self.metrics {
            m.aggregation_rounds_failed.inc();
        }
        self.settle_failed(dispatched, reason).await;
    }

    /// Settles a session's task `failed` with `reason` once nothing more will be tried for it.
    pub async fn settle_failed(&mut self, dispatched: &DispatchedTask, reason: &str) {
        if let Some(store) = &self.store {
            set_task_failed(store, self.metrics.as_deref(), &dispatched.task_id, reason).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::RouterTrace;
    use alloy::sol_types::SolValue;
    use alloy_provider::{ProviderBuilder, mock::Asserter};

    // supportsInterface(bytes4) returns (bool); the eth_call result is the
    // ABI-encoded bool wrapped as Bytes. Responses are consumed FIFO, so each
    // queued entry corresponds to exactly one RPC.
    fn push_supports_interface(asserter: &Asserter, supported: bool) {
        asserter.push_success(&Bytes::from(supported.abi_encode()));
    }

    // An unscheduled horizon is `type(uint256).max`, which is the state of a registry with nothing
    // announced — by far the common case, and it must leave validity untouched.
    #[test]
    fn horizon_clamp_is_noop_when_no_change_is_scheduled() {
        assert_eq!(clamp_to_mutation_horizon(150, Some(U256::MAX)), 150);
    }

    // A horizon the router could not read must not silently shorten validity.
    #[test]
    fn horizon_clamp_is_noop_when_horizon_is_unknown() {
        assert_eq!(clamp_to_mutation_horizon(150, None), 150);
    }

    // A horizon beyond the payload's own expiry constrains nothing.
    #[test]
    fn horizon_clamp_is_noop_when_horizon_is_beyond_validity() {
        assert_eq!(clamp_to_mutation_horizon(150, Some(U256::from(400))), 150);
    }

    // The payload must stop being valid strictly *below* the horizon: a settlement landing on that
    // block can already see the mutated operator set.
    #[test]
    fn horizon_clamp_ends_validity_one_block_before_the_horizon() {
        assert_eq!(clamp_to_mutation_horizon(150, Some(U256::from(120))), 119);
        assert_eq!(clamp_to_mutation_horizon(150, Some(U256::from(151))), 150);
        assert_eq!(clamp_to_mutation_horizon(150, Some(U256::from(150))), 149);
    }

    // A change announced mid-round can leave the horizon at or behind the payload's reference
    // block. The payload is then born expired, which `GET /tasks/{id}` reports as PAYLOAD_EXPIRED —
    // preferable to handing over calldata that would revert.
    #[test]
    fn horizon_clamp_yields_an_already_expired_payload_when_a_change_is_imminent() {
        assert_eq!(clamp_to_mutation_horizon(150, Some(U256::from(10))), 9);
        // A zero horizon must not underflow into u64::MAX, which would defeat the clamp entirely.
        assert_eq!(clamp_to_mutation_horizon(150, Some(U256::ZERO)), 0);
    }

    #[tokio::test]
    async fn test_supports_interface_cached_after_first_call() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        // Queue a single response: the first lookup must hit the RPC and the
        // second must be served from the cache. A cache miss on the second call
        // would drain the empty asserter and error.
        push_supports_interface(&asserter, true);

        let handler = GasKillerHandler::new(1, provider.clone());
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

        let handler = GasKillerHandler::new(1, provider.clone());
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

        let handler = GasKillerHandler::new(1, provider.clone());
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
        let handler = GasKillerHandler::new(1, provider);

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
        let handler = GasKillerHandler::new(1, provider).with_receipt_timeout(Some(45));

        assert_eq!(
            handler.receipt_timeout(ChainRole::L1),
            Duration::from_secs(45)
        );
        assert_eq!(
            handler.receipt_timeout(ChainRole::L2),
            Duration::from_secs(45)
        );
    }

    // -- task lifecycle settlement --

    use crate::store::TaskStatus;

    async fn store() -> SqliteStore {
        SqliteStore::connect_in_memory()
            .await
            .expect("in-memory store should open and migrate")
    }

    async fn key_id(store: &SqliteStore) -> String {
        store
            .create_api_key(None, None)
            .await
            .expect("key creation should succeed")
            .id
    }

    fn request_body() -> crate::ingress::GasKillerTaskRequestBody {
        crate::ingress::GasKillerTaskRequestBody {
            target_address: Address::from([0x11; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78],
            transition_index: Some(0),
            from_address: Address::from([0x22; 20]),
            value: U256::ZERO,
            block_height: 1,
        }
    }

    /// A dispatched session for `task_id` whose router trace ended in `trace`, carrying
    /// `task_data` as announced (without storage updates).
    fn dispatched(
        task_id: &str,
        task_data: &GasKillerTaskData,
        trace: Result<Bytes, String>,
    ) -> DispatchedTask {
        DispatchedTask {
            task_id: task_id.to_owned(),
            task: GasKillerTaskData {
                storage_updates: Bytes::new(),
                ..task_data.clone()
            },
            nested: None,
            chain: gas_killer_common::ChainRole::L1,
            trace: RouterTrace::finished(trace.map(Traced::Flat)),
        }
    }

    /// A session whose router trace produced `task_data`'s own storage updates.
    fn traced(task_id: &str, task_data: &GasKillerTaskData) -> DispatchedTask {
        dispatched(task_id, task_data, Ok(task_data.storage_updates.clone()))
    }

    /// Task data whose signed hash matches its own storage updates, so the render preflight passes.
    fn matching_task_data() -> (GasKillerTaskData, FixedBytes<32>) {
        let storage_updates = Bytes::from(vec![0xaa, 0xbb, 0xcc, 0xdd]);
        let task_data = GasKillerTaskData {
            storage_updates: storage_updates.clone(),
            transition_index: 0,
            target_address: Address::from([0x11; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78],
            from_address: Address::from([0x22; 20]),
            value: U256::ZERO,
            block_height: 1,
            chain_id: 1,
        };
        let msg_hash =
            FixedBytes::<32>::from(task_data.build_payload_hash(storage_updates.as_ref()).0);
        (task_data, msg_hash)
    }

    #[tokio::test]
    async fn a_failed_attempt_leaves_the_task_for_the_submitter_to_settle() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request_body()).await.unwrap();
        let (task_data, msg_hash) = matching_task_data();
        let session = dispatched(&task.id, &task_data, Err("call reverted".to_owned()));

        let provider = ProviderBuilder::new().connect_mocked_client(Asserter::new());
        let mut handler = GasKillerHandler::new(1, provider).with_store(store.clone());

        let error = handler
            .handle_schnorr_verification(
                &session,
                Instant::now(),
                msg_hash,
                100,
                U256::ZERO,
                Address::ZERO,
                vec![],
            )
            .await
            .expect_err("an attempt whose trace failed must fail");
        assert_ne!(
            store.get_task(&task.id).await.unwrap().unwrap().status,
            TaskStatus::Failed,
            "only the submitter's last attempt settles a failure"
        );

        handler
            .settle_render_failed(&session, &format!("verification failed: {error}"))
            .await;
        let settled = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Failed);
        assert!(
            settled
                .error
                .as_deref()
                .is_some_and(|e| e.contains("verification failed"))
        );
    }

    /// Only a signed session whose render failed counts as a failed round; a session that never
    /// reached rendering is counted by its height outcome instead.
    #[tokio::test]
    async fn only_a_failed_render_counts_as_a_failed_round() {
        let store = store().await;
        let key = key_id(&store).await;
        let timed_out = store.create_task(&key, &request_body()).await.unwrap();
        let unrendered = store.create_task(&key, &request_body()).await.unwrap();
        let (task_data, _) = matching_task_data();

        let metrics = Arc::new(MetricsCollector::new());
        let provider = ProviderBuilder::new().connect_mocked_client(Asserter::new());
        let mut handler = GasKillerHandler::new(1, provider)
            .with_store(store.clone())
            .with_metrics(Arc::clone(&metrics));

        handler
            .settle_failed(&traced(&timed_out.id, &task_data), "timed out")
            .await;
        assert_eq!(metrics.aggregation_rounds_failed.get(), 0);

        handler
            .settle_render_failed(&traced(&unrendered.id, &task_data), "hash mismatch")
            .await;
        assert_eq!(metrics.aggregation_rounds_failed.get(), 1);

        for id in [&timed_out.id, &unrendered.id] {
            let settled = store.get_task(id).await.unwrap().unwrap();
            assert_eq!(settled.status, TaskStatus::Failed);
        }
    }

    #[tokio::test]
    async fn handle_schnorr_verification_settles_ready_with_rendered_payload_and_bundle() {
        use alloy::sol_types::SolCall;

        let store = store().await;
        let key = key_id(&store).await;
        let task = store
            .create_task(&key, &request_body())
            .await
            .expect("task creation should succeed");
        let (task_data, msg_hash) = matching_task_data();

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        // The only queued RPC answers the ERC-165 interface probe. The eth_estimateGas after it
        // drains the now-empty asserter and errors, exercising the fallback estimate, and so does
        // the mutation-horizon read, leaving validity unclamped — the round still renders.
        push_supports_interface(&asserter, true);

        let mut handler = GasKillerHandler::new(1, provider)
            .with_store(store.clone())
            .with_payload_block_buffer(50);

        let current_block = 100u32;
        let s = U256::from(42u64);
        let r_addr = Address::from([0x44; 20]);
        let non_signers = vec![Address::from([0x55; 20])];
        let result = handler
            .handle_schnorr_verification(
                &traced(&task.id, &task_data),
                Instant::now(),
                msg_hash,
                current_block,
                s,
                r_addr,
                non_signers.clone(),
            )
            .await;

        assert!(result.is_ok());

        let settled = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Ready);

        let payload: PayloadView =
            serde_json::from_str(settled.payload.as_deref().expect("payload persisted")).unwrap();
        assert_eq!(payload.to, task_data.target_address);
        assert_eq!(payload.value, U256::ZERO);
        assert_eq!(payload.chain_id, 1);
        assert_eq!(payload.estimated_gas, PAYLOAD_GAS_ESTIMATE_FALLBACK);
        // reference_block_number = current_block - 1; valid_until = reference + buffer.
        assert_eq!(payload.valid_until_block, (current_block as u64 - 1) + 50);

        // The rendered calldata ABI-decodes to a verifyAndUpdate call carrying the round inputs.
        let decoded = GasKillerSDK::verifyAndUpdateCall::abi_decode(payload.data.as_ref())
            .expect("payload data should decode as verifyAndUpdate");
        assert_eq!(decoded.msgHash, msg_hash);
        assert_eq!(decoded.referenceBlockNumber, current_block - 1);
        assert_eq!(decoded.storageUpdates, task_data.storage_updates);
        assert_eq!(decoded.transitionIndex, U256::ZERO);
        assert_eq!(decoded.targetFunction, task_data.function_selector());

        // The structured bundle persists alongside the payload and round-trips.
        let bundle: TaskBundle =
            serde_json::from_str(settled.bundle.as_deref().expect("bundle persisted")).unwrap();
        assert_eq!(bundle.msg_hash, msg_hash);
        assert_eq!(bundle.reference_block_number, current_block - 1);
        assert_eq!(bundle.transition_index, 0);
        assert_eq!(bundle.chain_id, 1);
        assert_eq!(
            bundle.proof,
            BundleProof::Schnorr {
                s,
                r_addr,
                non_signers
            }
        );
    }

    /// A transient failure on the first attempt must not cost the retry its router trace: the
    /// second attempt renders and settles the task ready.
    #[tokio::test]
    async fn a_retry_after_a_transient_failure_renders_the_round() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request_body()).await.unwrap();
        let (task_data, msg_hash) = matching_task_data();
        let session = traced(&task.id, &task_data);

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        asserter.push_failure_msg("connection reset");
        push_supports_interface(&asserter, true);

        let mut handler = GasKillerHandler::new(1, provider)
            .with_store(store.clone())
            .with_payload_block_buffer(50);

        for expect_render in [false, true] {
            let result = handler
                .handle_schnorr_verification(
                    &session,
                    Instant::now(),
                    msg_hash,
                    100,
                    U256::from(42u64),
                    Address::from([0x44; 20]),
                    vec![],
                )
                .await;
            if expect_render {
                result.expect("the retry should render");
            } else {
                let error = result.expect_err("the first attempt should fail");
                assert!(error.to_string().contains("supportsInterface"), "{error}");
            }
        }

        let settled = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Ready);
        assert!(settled.payload.is_some());
    }

    /// Settles a signed session whose router trace ended in `trace`, returning the failure the
    /// task recorded. No RPC is queued: both cases must fail before the chain is touched.
    async fn settle_with_trace(trace: Result<Bytes, String>) -> String {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request_body()).await.unwrap();
        let (task_data, msg_hash) = matching_task_data();
        let session = dispatched(&task.id, &task_data, trace);

        let provider = ProviderBuilder::new().connect_mocked_client(Asserter::new());
        let mut handler = GasKillerHandler::new(1, provider).with_store(store.clone());

        let result = handler
            .handle_schnorr_verification(
                &session,
                Instant::now(),
                msg_hash,
                100,
                U256::from(42u64),
                Address::from([0x44; 20]),
                vec![],
            )
            .await;

        let error = result.expect_err("the round must not render");
        handler
            .settle_render_failed(&session, &format!("verification failed: {error}"))
            .await;
        let settled = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Failed);
        assert!(settled.payload.is_none());
        settled.error.expect("a failure reason should be recorded")
    }

    #[tokio::test]
    async fn a_quorum_digest_the_router_trace_disagrees_with_is_not_rendered() {
        let error = settle_with_trace(Ok(Bytes::from(vec![0x01]))).await;
        assert!(error.contains("Message hash mismatch"), "{error}");
    }

    #[tokio::test]
    async fn a_failed_router_trace_fails_the_task_with_its_cause() {
        let error = settle_with_trace(Err("call reverted".to_owned())).await;
        assert!(
            error.contains("task enrichment failed: call reverted"),
            "{error}"
        );
    }

    // -- payload gas estimation --

    use crate::payload_revert::{execution_reverted, rpc_error};

    fn handler_for_estimation() -> GasKillerHandler<impl Provider<Ethereum> + Clone> {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        GasKillerHandler::new(1, provider)
    }

    // Queues the JSON-RPC error response a node returns for a call that executed and reverted, so
    // the provider fails the next RPC the way `eth_estimateGas` does against a target whose
    // registry cannot verify the round.
    fn push_execution_revert(asserter: &Asserter, data: &str) {
        asserter.push(alloy::rpc::json_rpc::ResponsePayload::Failure(
            alloy::rpc::json_rpc::ErrorPayload {
                code: 3,
                message: "execution reverted".into(),
                data: Some(
                    serde_json::value::RawValue::from_string(format!("\"{data}\"")).unwrap(),
                ),
            },
        ));
    }

    #[test]
    fn resolve_payload_gas_uses_the_node_estimate() {
        let handler = handler_for_estimation();
        assert_eq!(
            handler
                .resolve_payload_gas(Ok(257_468), Address::ZERO)
                .unwrap(),
            257_468
        );
    }

    // Estimation that never executed says nothing about landability, so the round still completes
    // and the payload carries the advisory fallback.
    #[test]
    fn resolve_payload_gas_falls_back_when_estimation_is_unreachable() {
        let handler = handler_for_estimation();
        assert_eq!(
            handler
                .resolve_payload_gas(Err(rpc_error(-32005, "rate limit exceeded")), Address::ZERO)
                .unwrap(),
            PAYLOAD_GAS_ESTIMATE_FALLBACK
        );
    }

    // A revert is proof the client's submission would revert too, so the round fails and the
    // cause travels in the error rather than being replaced by a fallback estimate.
    #[test]
    fn resolve_payload_gas_fails_the_round_when_the_call_reverts() {
        let handler = handler_for_estimation();
        let error = handler
            .resolve_payload_gas(Err(execution_reverted(Some("0x68477238"))), Address::ZERO)
            .expect_err("a reverting estimate must not yield a payload");
        let message = error.to_string();
        assert!(message.contains("InvalidQuorumSignature()"), "{message}");
        assert!(message.contains("schnorrRegistry"), "{message}");
    }

    #[test]
    fn resolve_payload_gas_counts_rejected_payloads() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let metrics = Arc::new(MetricsCollector::new());
        let handler = GasKillerHandler::new(1, provider).with_metrics(metrics.clone());

        assert!(
            handler
                .resolve_payload_gas(Err(execution_reverted(Some("0x68477238"))), Address::ZERO)
                .is_err()
        );
        assert!(
            metrics
                .encode()
                .contains("gas_killer_payloads_rejected_reverting_total 1"),
            "the rejection should be counted:\n{}",
            metrics.encode()
        );
    }

    // The failure the mis-wired integrator target hit: the round certifies, the payload renders,
    // and the estimate proves it cannot land. The task must settle failed with the cause instead
    // of ready with calldata that burns the client's gas.
    #[tokio::test]
    async fn handle_schnorr_verification_settles_failed_when_the_rendered_payload_reverts() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store
            .create_task(&key, &request_body())
            .await
            .expect("task creation should succeed");
        let (task_data, msg_hash) = matching_task_data();

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        // The ERC-165 probe passes, then eth_estimateGas reverts with the registry-mismatch
        // selector.
        push_supports_interface(&asserter, true);
        push_execution_revert(&asserter, "0x68477238");

        let session = traced(&task.id, &task_data);
        let mut handler = GasKillerHandler::new(1, provider)
            .with_store(store.clone())
            .with_payload_block_buffer(50);

        let result = handler
            .handle_schnorr_verification(
                &session,
                Instant::now(),
                msg_hash,
                100,
                U256::from(42u64),
                Address::from([0x44; 20]),
                vec![],
            )
            .await;

        let error = result.expect_err("a payload proven to revert must fail the attempt");
        handler
            .settle_render_failed(&session, &format!("verification failed: {error}"))
            .await;
        let settled = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Failed);
        assert!(
            settled.payload.is_none(),
            "a payload proven to revert must not be persisted"
        );
        let error = settled.error.expect("a failure reason should be recorded");
        assert!(error.contains("InvalidQuorumSignature()"), "{error}");
        assert!(error.contains("schnorrRegistry"), "{error}");
    }

    // -- nested trees --

    use gas_analyzer::nested::{FrameProgram, IStateUpdateTypes, StateUpdate};

    const REGISTRY: Address = Address::repeat_byte(0x77);
    const CALLEE: Address = Address::repeat_byte(0xbb);

    /// A root at [`matching_task_data`]'s target whose only frame op nests [`CALLEE`].
    fn two_frame_tree(task_data: &GasKillerTaskData, spec: &NestedSpec) -> TreeTrace {
        let frame = |target, caller, updates| FrameProgram {
            target,
            caller,
            value: U256::ZERO,
            calldata_hash: alloy_primitives::B256::ZERO,
            transition_index: Some(U256::ZERO),
            updates,
            children: Vec::new(),
        };
        let nest = StateUpdate::Nested(IStateUpdateTypes::Nested {
            target: CALLEE,
            value: U256::ZERO,
            childLeaf: alloy_primitives::B256::ZERO,
        });
        let mut root = frame(task_data.target_address, Address::ZERO, vec![nest]);
        root.children = vec![1];
        let callee = frame(CALLEE, task_data.target_address, Vec::new());
        gas_killer_common::build_tree_trace(
            task_data,
            vec![root, callee],
            BTreeSet::from([task_data.target_address, CALLEE]),
            spec,
        )
        .unwrap()
    }

    /// Answers the tree preflight: the root's registry, then each frame contract in address
    /// order with its interface support, registry and `blockStaleMeasure`.
    fn push_tree_preflight(asserter: &Asserter, frames: &[(Address, u64)]) {
        asserter.push_success(&Bytes::from(REGISTRY.abi_encode()));
        for (registry, measure) in frames {
            push_supports_interface(asserter, true);
            asserter.push_success(&Bytes::from(registry.abi_encode()));
            asserter.push_success(&Bytes::from(U256::from(*measure).abi_encode()));
        }
    }

    async fn render_tree(
        frames: &[(Address, u64)],
    ) -> (Result<u64>, SqliteStore, String, TreeTrace) {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request_body()).await.unwrap();
        let (task_data, _) = matching_task_data();
        let spec = NestedSpec { expiry_block: 140 };
        let tree = two_frame_tree(&task_data, &spec);
        let session = DispatchedTask {
            nested: Some(spec),
            trace: RouterTrace::finished(Ok(Traced::Tree(Arc::new(tree.clone())))),
            ..dispatched(&task.id, &task_data, Ok(Bytes::new()))
        };

        let asserter = Asserter::new();
        push_tree_preflight(&asserter, frames);
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let mut handler = GasKillerHandler::new(1, provider)
            .with_store(store.clone())
            .with_payload_block_buffer(50);
        let result = handler
            .handle_schnorr_verification(
                &session,
                Instant::now(),
                FixedBytes::from(tree.digest.0),
                100,
                U256::from(42u64),
                Address::from([0x44; 20]),
                vec![],
            )
            .await;
        (result, store, task.id, tree)
    }

    #[tokio::test]
    async fn a_signed_tree_renders_verify_and_update_tree() {
        use alloy::sol_types::SolCall;

        let (result, store, task_id, tree) = render_tree(&[(REGISTRY, 300), (REGISTRY, 300)]).await;
        assert_eq!(
            result.unwrap(),
            140,
            "the signed expiry caps reference + buffer"
        );

        let settled = store.get_task(&task_id).await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Ready);
        let payload: PayloadView =
            serde_json::from_str(settled.payload.as_deref().unwrap()).unwrap();
        let call = GasKillerSDK::verifyAndUpdateTreeCall::abi_decode(payload.data.as_ref())
            .expect("a tree renders verifyAndUpdateTree");
        let encoded = tree.encoded.as_ref().unwrap();
        assert_eq!(call.submission.root, encoded.root);
        assert_eq!(call.submission.expiryBlock, U256::from(140));
        assert_eq!(call.submission.sig.refBlock, U256::from(99));
        assert_eq!(call.submission.storageUpdates, tree.root_program);
        assert_eq!(call.submission.children, encoded.root_children);

        let bundle: TaskBundle = serde_json::from_str(settled.bundle.as_deref().unwrap()).unwrap();
        let nested = bundle
            .nested
            .expect("a tree bundle carries its nested arguments");
        assert_eq!(nested.expiry_block, 140);
        assert_eq!(
            nested.frames.iter().map(|f| f.contract).collect::<Vec<_>>(),
            [matching_task_data().0.target_address, CALLEE]
        );
    }

    /// The signed expiry is fixed; a frame contract that goes stale sooner shortens only how
    /// long the rendered payload is offered for.
    #[tokio::test]
    async fn the_stalest_frame_bounds_the_rendered_validity() {
        let (result, _, _, _) = render_tree(&[(REGISTRY, 300), (REGISTRY, 20)]).await;
        assert_eq!(result.unwrap(), 99 + 20);
    }

    #[tokio::test]
    async fn a_callee_on_another_registry_fails_the_render_with_its_cause() {
        let (result, _, _, _) =
            render_tree(&[(REGISTRY, 300), (Address::repeat_byte(0x99), 300)]).await;
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(&format!("{CALLEE} uses schnorrRegistry")),
            "{error}"
        );
    }

    #[test]
    fn tree_validity_never_outlasts_the_signed_expiry() {
        assert_eq!(tree_valid_until(99, 50, 300, 140), 140);
        assert_eq!(tree_valid_until(99, 50, 20, 140), 119);
        assert_eq!(tree_valid_until(99, 10, 300, 140), 109);
    }
}

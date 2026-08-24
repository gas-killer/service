use crate::metrics::MetricsCollector;
use crate::payload_revert::PayloadRevert;
use crate::sequencer::{InFlightTask, in_flight_task, set_task_failed, set_task_ready};
use crate::store::SqliteStore;
use crate::task_data::GasKillerTaskData;
use alloy::network::Ethereum;
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes, FixedBytes, U256};
use alloy_provider::Provider;
use anyhow::{Result, bail};
use commonware_avs_router::executor::{BlsSignatureVerificationHandler, ExecutionResult};
use commonware_avs_router::sequencer::{DispatchTime, take_dispatch_time};
use gas_killer_common::ChainRole;
use gas_killer_common::bindings::bls_sig_check_operator_state_retriever::IBLSSignatureCheckerTypes as RetrieverIBLSTypes;
use gas_killer_common::bindings::gaskillersdk::{
    BN254, GasKillerSDK, IBLSSignatureCheckerTypes as GasKillerIBLSTypes,
};
use gas_killer_common::bindings::schnorrgaskillersdk::SchnorrGasKillerSDK;
use gas_killer_common::bindings::schnorrstakeregistry::ISchnorrStakeRegistry;
use gas_killer_common::bindings::{GAS_KILLER_INTERFACE_ID, SCHNORR_GAS_KILLER_INTERFACE_ID};
use gas_killer_common::{BundleProof, PayloadView, TaskBundle};
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

/// Rebuilds the operator-state-retriever `NonSignerStakesAndSignature` into the distinct
/// `GasKillerSDK` binding type. Each `sol!` invocation mints its own Rust type, so the fields
/// are copied across even though the ABI layout is identical.
fn reshape_non_signer(
    data: RetrieverIBLSTypes::NonSignerStakesAndSignature,
) -> GasKillerIBLSTypes::NonSignerStakesAndSignature {
    GasKillerIBLSTypes::NonSignerStakesAndSignature {
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
    }
}

/// The [`ExecutionResult`] a completion handler returns for a rendered (non-broadcast) round.
///
/// A rendered round persists the payload/bundle and submits no transaction, so there is no
/// receipt to report; the submitter only needs an `Ok` to mark the height settled.
fn rendered_execution_result() -> ExecutionResult {
    ExecutionResult {
        transaction_hash: String::new(),
        block_number: None,
        gas_used: None,
        status: None,
        contract_address: None,
    }
}

/// Resolved inputs for a BLS `verifyAndUpdate` call, assembled once by
/// [`GasKillerHandler::prepare_bls`] and consumed by either the render path
/// ([`GasKillerHandler::render_bls_payload`]) or the retained broadcast path
/// ([`GasKillerHandler::execute_verification`]).
struct PreparedBls<P> {
    provider: P,
    chain_id: u64,
    target_addr: Address,
    from_address: Address,
    msg_hash: FixedBytes<32>,
    quorum_numbers: Bytes,
    /// `current_block_number - 1`; see [`GasKillerHandler::prepare_bls`].
    reference_block_number: u32,
    storage_updates: Bytes,
    transition_index: u64,
    target_function: FixedBytes<4>,
    non_signer: GasKillerIBLSTypes::NonSignerStakesAndSignature,
}

/// Resolved inputs for a Schnorr `verifyAndUpdate` call; the Schnorr twin of [`PreparedBls`],
/// swapping the BN254 non-signer struct for the aggregate `(s, Raddr)` and the ascending
/// `non_signers`.
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
    /// Durable store used to settle a task's terminal status once its height
    /// executes. `None` in store-less test/dev harnesses, where the transition is
    /// skipped.
    store: Option<SqliteStore>,
    /// Shared with [`crate::sequencer::GasKillerTaskSource`]; see
    /// [`crate::sequencer::InFlightTask`].
    in_flight: InFlightTask,
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
            dispatch_time: Default::default(),
            interface_cache: Arc::new(RwLock::new(HashMap::new())),
            receipt_timeout_override: None,
            store: None,
            in_flight: in_flight_task(),
            payload_block_buffer: gas_killer_common::DEFAULT_PAYLOAD_BLOCK_BUFFER,
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
            store: None,
            in_flight: in_flight_task(),
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

    /// Attaches the durable store so a settled height advances its task's terminal
    /// status.
    pub fn with_store(mut self, store: SqliteStore) -> Self {
        self.store = Some(store);
        self
    }

    /// Shares the in-flight task slot with [`crate::sequencer::GasKillerTaskSource`].
    /// Must be the same handle the task source is built with, or a settled height
    /// won't map back to its task.
    pub fn with_in_flight_task(mut self, in_flight: InFlightTask) -> Self {
        self.in_flight = in_flight;
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

    /// Runs the shared preflight for a BLS round and resolves every `verifyAndUpdate` input.
    ///
    /// Reshapes the non-signer struct into the SDK binding type, resolves the chain provider,
    /// confirms the locally recomputed payload hash matches the quorum-signed hash, and gates on
    /// the target's ERC-165 GasKiller interface. `reference_block_number = current_block_number
    /// - 1` so that a simulation at the current block satisfies the on-chain
    /// `require(referenceBlockNumber < block.number)`; without the decrement a simulation at
    /// block N would see `referenceBlockNumber == N` and revert with `FutureBlockNumber`.
    async fn prepare_bls(
        &self,
        msg_hash: FixedBytes<32>,
        quorum_numbers: Bytes,
        current_block_number: u32,
        non_signer_data: RetrieverIBLSTypes::NonSignerStakesAndSignature,
        task_data: Option<&GasKillerTaskData>,
    ) -> Result<PreparedBls<P>> {
        let non_signer = reshape_non_signer(non_signer_data);

        let task_data = task_data
            .ok_or_else(|| anyhow::anyhow!("Task data is required for gas killer verification"))?;

        let chain_id: u64 = task_data.chain_id;

        let provider = self
            .get_provider(chain_id)
            .ok_or_else(|| anyhow::anyhow!("No provider configured for chain: {}", chain_id))?
            .clone();

        info!(
            storage_updates_len = task_data.storage_updates.len(),
            chain = %chain_id,
            "Using storage updates from task data on detected chain"
        );

        let storage_updates = task_data.storage_updates.clone();
        let transition_index = task_data.transition_index;
        let target_function = task_data.function_selector();
        let target_addr = task_data.target_address;
        let from_address = task_data.from_address;

        debug!(
            transition_index,
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
                transition_index,
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

        Ok(PreparedBls {
            provider,
            chain_id,
            target_addr,
            from_address,
            msg_hash,
            quorum_numbers,
            reference_block_number: current_block_number.saturating_sub(1),
            storage_updates,
            transition_index,
            target_function,
            non_signer,
        })
    }

    /// Broadcasts a BLS `verifyAndUpdate` on-chain from the router's funded wallet and waits for
    /// the receipt.
    ///
    /// This is the **auto-execute** path: the entry point for the per-API-key auto-execute /
    /// account-abstraction tier that submits the round on the user's behalf. The completion
    /// handler renders a user-signed payload via [`Self::render_bls_payload`]; both share
    /// [`Self::prepare_bls`], so a per-key branch between rendering and broadcasting stays
    /// localized.
    pub async fn execute_verification(
        &mut self,
        msg_hash: FixedBytes<32>,
        quorum_numbers: Bytes,
        current_block_number: u32,
        non_signer_data: RetrieverIBLSTypes::NonSignerStakesAndSignature,
        task_data: Option<&GasKillerTaskData>,
    ) -> Result<ExecutionResult> {
        let prepared = self
            .prepare_bls(
                msg_hash,
                quorum_numbers,
                current_block_number,
                non_signer_data,
                task_data,
            )
            .await?;
        let PreparedBls {
            provider,
            chain_id,
            target_addr,
            msg_hash,
            quorum_numbers,
            reference_block_number,
            storage_updates,
            transition_index,
            target_function,
            non_signer,
            ..
        } = prepared;

        let gas_killer_sdk = GasKillerSDK::new(target_addr, provider);

        info!("Sending verifyAndUpdate transaction");
        let tx_send_start = Instant::now();
        let send_result = gas_killer_sdk
            .verifyAndUpdate(
                msg_hash,
                quorum_numbers,
                reference_block_number,
                storage_updates,
                U256::from(transition_index),
                target_function,
                non_signer,
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
        // return an error so the submitter counts the height as failed and moves on.
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
            contract_address: receipt.contract_address.map(|a| a.to_string()),
        })
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

    /// Renders a completed BLS round into a user-signable transaction request and the durable
    /// [`TaskBundle`] it derives from, without broadcasting.
    ///
    /// `data` is the full `verifyAndUpdate` calldata; `estimated_gas` comes from
    /// `eth_estimateGas` simulated as the requesting account, via
    /// [`Self::resolve_payload_gas`] — which also fails the round if that call reverts. `value` is
    /// fixed at zero: `verifyAndUpdate` is not payable in beta and the value is kept
    /// server-controlled so a future on-chain fee is a server change, not an integrator client-code
    /// change.
    async fn render_bls_payload(
        &self,
        msg_hash: FixedBytes<32>,
        quorum_numbers: Bytes,
        current_block_number: u32,
        non_signer_data: RetrieverIBLSTypes::NonSignerStakesAndSignature,
        task_data: Option<&GasKillerTaskData>,
    ) -> Result<RenderedRound> {
        let prepared = self
            .prepare_bls(
                msg_hash,
                quorum_numbers,
                current_block_number,
                non_signer_data,
                task_data,
            )
            .await?;
        let PreparedBls {
            provider,
            chain_id,
            target_addr,
            from_address,
            msg_hash,
            quorum_numbers,
            reference_block_number,
            storage_updates,
            transition_index,
            target_function,
            non_signer,
        } = prepared;

        // Capture the ABI-encoded proof for the bundle before the struct is moved into the call.
        let non_signer_abi = Bytes::from(non_signer.abi_encode());
        let value = U256::ZERO;

        let sdk = GasKillerSDK::new(target_addr, provider);
        let call = sdk
            .verifyAndUpdate(
                msg_hash,
                quorum_numbers.clone(),
                reference_block_number,
                storage_updates.clone(),
                U256::from(transition_index),
                target_function,
                non_signer,
            )
            .from(from_address)
            .value(value);

        let data = call.calldata().clone();
        let estimated_gas = self.resolve_payload_gas(call.estimate_gas().await, target_addr)?;

        let valid_until_block = reference_block_number as u64 + self.payload_block_buffer;

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
            proof: BundleProof::Bls {
                quorum_numbers,
                non_signer_stakes_and_signature: non_signer_abi,
            },
        };
        Ok(RenderedRound { payload, bundle })
    }

    /// Schnorr twin of [`Self::supports_gas_killer_interface`], checking the
    /// `ISchnorrGasKillerSDK` interface ID instead. Shares the same memo cache: a
    /// process runs exactly one signature scheme, so a given target address is
    /// only ever probed for one of the two interface IDs.
    async fn supports_schnorr_interface(&self, provider: P, target_addr: Address) -> Result<bool> {
        if let Some(supported) = self.interface_cache.read().await.get(&target_addr).copied() {
            return Ok(supported);
        }

        let sdk = SchnorrGasKillerSDK::new(target_addr, provider);
        let supports_interface_start = Instant::now();
        let supported = match sdk
            .supportsInterface(SCHNORR_GAS_KILLER_INTERFACE_ID)
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

    /// Schnorr twin of [`Self::prepare_bls`]: identical preflights (payload-hash match, ERC-165
    /// gate, `reference_block_number = head − 1`), resolving the aggregate `(s, Raddr)` and the
    /// strictly ascending `non_signers` instead of the BN254 non-signer struct.
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
            self.supports_schnorr_interface(provider.clone(), target_addr),
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

        // Ensure the contract implements the Schnorr GasKiller interface (ERC-165).
        if !supports_result? {
            warn!(
                interface_id = %SCHNORR_GAS_KILLER_INTERFACE_ID,
                "Target contract does not support the Schnorr GasKiller interface"
            );
            return Err(anyhow::anyhow!(
                "Target contract does not support the Schnorr GasKiller interface ({})",
                SCHNORR_GAS_KILLER_INTERFACE_ID
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

    /// Schnorr twin of [`Self::execute_verification`] — the **auto-execute** broadcast path for
    /// the per-API-key auto-execute / account-abstraction tier. The completion handler renders a
    /// user-signed payload via [`Self::render_schnorr_payload`]; both share
    /// [`Self::prepare_schnorr`].
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

        let sdk = SchnorrGasKillerSDK::new(target_addr, provider);

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
    /// supported interfaces are immutable, whereas `SchnorrGasKillerSDK` keeps its registry in
    /// storage behind an `internal` setter, so a target is free to expose an owner-gated path that
    /// re-points it. A memoized address would survive that for the lifetime of the process and read
    /// the horizon off a registry the target no longer uses — failing open, and silently. Rendering
    /// runs once per completed round, so the second call is not on a hot path.
    async fn schnorr_mutation_horizon(&self, provider: P, target_addr: Address) -> Option<U256> {
        let sdk = SchnorrGasKillerSDK::new(target_addr, provider.clone());
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

    /// Schnorr twin of [`Self::render_bls_payload`]: renders a completed Schnorr round into a
    /// user-signable transaction request and its durable [`TaskBundle`] without broadcasting.
    /// The outer payload is scheme-agnostic — only the encoded `data` and the bundle's proof
    /// differ from the BLS rendering.
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
        let sdk = SchnorrGasKillerSDK::new(target_addr, provider);
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
        };
        Ok(RenderedRound { payload, bundle })
    }

    /// Schnorr twin of [`BlsSignatureVerificationHandler::handle_verification`]:
    /// same metrics envelope and task settlement, the proof arguments swap the
    /// BN254 non-signer struct for the aggregate `(s, Raddr)` and the strictly
    /// ascending `non_signers`. Called by
    /// [`crate::schnorr_submitter::SchnorrSubmitter`].
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_schnorr_verification(
        &mut self,
        height: u64,
        msg_hash: FixedBytes<32>,
        current_block_number: u32,
        s: U256,
        r_addr: Address,
        non_signers: Vec<Address>,
        task_data: Option<&GasKillerTaskData>,
    ) -> Result<ExecutionResult> {
        let dispatch_start = take_dispatch_time(&self.dispatch_time, height);
        if let Some(start) = dispatch_start
            && let Some(m) = &self.metrics
        {
            m.p2p_round_trip_seconds
                .observe(start.elapsed().as_secs_f64());
        }

        let exec_start = Instant::now();

        let result = self
            .render_schnorr_payload(
                msg_hash,
                current_block_number,
                s,
                r_addr,
                non_signers,
                task_data,
            )
            .await;

        if let Some(m) = &self.metrics {
            m.execution_duration_seconds
                .observe(exec_start.elapsed().as_secs_f64());
            match &result {
                Ok(_) => {
                    m.aggregation_rounds_completed.inc();
                    if let Some(start) = dispatch_start {
                        m.round_latency_seconds
                            .observe(start.elapsed().as_secs_f64());
                    }
                }
                Err(_) => {
                    m.aggregation_rounds_failed.inc();
                }
            }
        }

        // Settle the task this height was executing. `GasKillerTaskSource::next_task`
        // set the in-flight slot when it dispatched this task; taking it here both
        // records the outcome and clears the slot so a later skipped height is not
        // mistaken for this one. A successful round persists the rendered payload and its
        // bundle; the on-chain submission is left to the user (or the future auto-execute tier).
        if let Some(store) = &self.store
            && let Some(task_id) = self.in_flight.lock().ok().and_then(|mut slot| slot.take())
        {
            match &result {
                Ok(rendered) => {
                    set_task_ready(store, &task_id, &rendered.payload, &rendered.bundle).await
                }
                Err(e) => {
                    set_task_failed(store, &task_id, &format!("verification failed: {e}")).await
                }
            }
        }

        result.map(|_| rendered_execution_result())
    }
}

#[async_trait::async_trait]
impl<P: Provider<Ethereum> + Clone + Send + Sync + 'static> BlsSignatureVerificationHandler
    for GasKillerHandler<P>
{
    type TaskData = GasKillerTaskData;

    /// Submits `verifyAndUpdate` for a certified height, recording round-trip
    /// and execution metrics. Called by [`commonware_avs_router::submitter::Submitter`]
    /// with the aggregation height as the metric key.
    async fn handle_verification(
        &mut self,
        height: u64,
        msg_hash: FixedBytes<32>,
        quorum_numbers: Bytes,
        current_block_number: u32,
        non_signer_data: RetrieverIBLSTypes::NonSignerStakesAndSignature,
        task_data: Option<&GasKillerTaskData>,
    ) -> Result<ExecutionResult> {
        // Record P2P round-trip: time from this height's sequencer dispatch to a
        // certificate reaching the submitter. Consume the entry keyed by `height`
        // so a failed earlier height (which never reaches here) cannot contribute
        // a stale, inflated sample. The dispatch instant is kept so the
        // end-to-end latency can be observed once execution completes.
        let dispatch_start = take_dispatch_time(&self.dispatch_time, height);
        if let Some(start) = dispatch_start
            && let Some(m) = &self.metrics
        {
            m.p2p_round_trip_seconds
                .observe(start.elapsed().as_secs_f64());
        }

        let exec_start = Instant::now();

        let result = self
            .render_bls_payload(
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
                    // End-to-end latency: sequencer dispatch through round completion.
                    // Failed heights are skipped — a failure sample would distort the percentiles.
                    if let Some(start) = dispatch_start {
                        m.round_latency_seconds
                            .observe(start.elapsed().as_secs_f64());
                    }
                }
                Err(_) => {
                    m.aggregation_rounds_failed.inc();
                }
            }
        }

        // Settle the task this height was executing. `GasKillerTaskSource::next_task`
        // set the in-flight slot when it dispatched this task; taking it here both
        // records the outcome and clears the slot so a later skipped height is not
        // mistaken for this one. A successful round persists the rendered payload and its
        // bundle; the on-chain submission is left to the user (or the future auto-execute tier).
        if let Some(store) = &self.store
            && let Some(task_id) = self.in_flight.lock().ok().and_then(|mut slot| slot.take())
        {
            match &result {
                Ok(rendered) => {
                    set_task_ready(store, &task_id, &rendered.payload, &rendered.bundle).await
                }
                Err(e) => {
                    set_task_failed(store, &task_id, &format!("verification failed: {e}")).await
                }
            }
        }

        result.map(|_| rendered_execution_result())
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
    use gas_killer_common::bindings::bls_sig_check_operator_state_retriever::BN254 as RetrieverBN254;

    fn empty_non_signer_data() -> RetrieverIBLSTypes::NonSignerStakesAndSignature {
        RetrieverIBLSTypes::NonSignerStakesAndSignature {
            nonSignerQuorumBitmapIndices: vec![],
            nonSignerPubkeys: vec![],
            quorumApks: vec![],
            apkG2: RetrieverBN254::G2Point {
                X: [U256::ZERO, U256::ZERO],
                Y: [U256::ZERO, U256::ZERO],
            },
            sigma: RetrieverBN254::G1Point {
                X: U256::ZERO,
                Y: U256::ZERO,
            },
            quorumApkIndices: vec![],
            totalStakeIndices: vec![],
            nonSignerStakeIndices: vec![],
        }
    }

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

    #[tokio::test]
    async fn handle_verification_settles_task_failed_when_task_data_missing() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store
            .create_task(&key, &request_body())
            .await
            .expect("task creation should succeed");

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let in_flight = in_flight_task();
        *in_flight.lock().unwrap() = Some(task.id.clone());

        let mut handler = GasKillerHandler::new(1, provider)
            .with_store(store.clone())
            .with_in_flight_task(in_flight.clone());

        // `task_data: None` makes `render_bls_payload` fail immediately in `prepare_bls`
        // (before any provider call), exercising the settlement wiring without needing a real
        // chain, ABI-encoded certificate, or registered operator set.
        let result = handler
            .handle_verification(
                0,
                FixedBytes::<32>::ZERO,
                Bytes::new(),
                0,
                empty_non_signer_data(),
                None,
            )
            .await;

        assert!(result.is_err());
        assert!(
            in_flight.lock().unwrap().is_none(),
            "the slot must be cleared once the task settles"
        );
        let settled = store
            .get_task(&task.id)
            .await
            .unwrap()
            .expect("task should still exist");
        assert_eq!(settled.status, TaskStatus::Failed);
        assert!(
            settled
                .error
                .as_deref()
                .is_some_and(|e| e.contains("verification failed"))
        );
    }

    #[tokio::test]
    async fn handle_verification_settles_ready_with_rendered_payload_and_bundle() {
        use alloy::sol_types::SolCall;
        use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;

        let store = store().await;
        let key = key_id(&store).await;
        let task = store
            .create_task(&key, &request_body())
            .await
            .expect("task creation should succeed");

        // Task data whose signed hash matches its own storage updates, so the render preflight
        // passes and a payload is produced.
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

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        // The only queued RPC answers the ERC-165 interface probe. The subsequent
        // eth_estimateGas drains the now-empty asserter and errors, exercising the fallback
        // estimate — the round still renders a payload rather than failing.
        push_supports_interface(&asserter, true);

        let in_flight = in_flight_task();
        *in_flight.lock().unwrap() = Some(task.id.clone());
        let mut handler = GasKillerHandler::new(1, provider)
            .with_store(store.clone())
            .with_in_flight_task(in_flight.clone())
            .with_payload_block_buffer(50);

        let current_block = 100u32;
        let quorum_numbers = Bytes::from(vec![0x00]);
        let result = handler
            .handle_verification(
                0,
                msg_hash,
                quorum_numbers.clone(),
                current_block,
                empty_non_signer_data(),
                Some(&task_data),
            )
            .await;

        assert!(result.is_ok());
        assert!(
            in_flight.lock().unwrap().is_none(),
            "the slot must be cleared once the task settles"
        );

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
        assert_eq!(decoded.quorumNumbers, quorum_numbers);
        assert_eq!(decoded.referenceBlockNumber, current_block - 1);
        assert_eq!(decoded.storageUpdates, storage_updates);
        assert_eq!(decoded.transitionIndex, U256::ZERO);
        assert_eq!(decoded.targetFunction, task_data.function_selector());

        // The structured bundle persists alongside the payload and round-trips.
        let bundle: TaskBundle =
            serde_json::from_str(settled.bundle.as_deref().expect("bundle persisted")).unwrap();
        assert_eq!(bundle.msg_hash, msg_hash);
        assert_eq!(bundle.reference_block_number, current_block - 1);
        assert_eq!(bundle.transition_index, 0);
        assert_eq!(bundle.chain_id, 1);
        assert!(matches!(bundle.proof, BundleProof::Bls { .. }));
    }

    #[tokio::test]
    async fn handle_verification_is_noop_on_store_when_slot_empty() {
        let store = store().await;
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let mut handler = GasKillerHandler::new(1, provider).with_store(store);

        // No task was recorded as in flight (e.g. a certificate for an unassigned
        // height); settlement must not panic when there is nothing to settle.
        let result = handler
            .handle_verification(
                0,
                FixedBytes::<32>::ZERO,
                Bytes::new(),
                0,
                empty_non_signer_data(),
                None,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn handle_schnorr_verification_settles_task_failed_when_task_data_missing() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store
            .create_task(&key, &request_body())
            .await
            .expect("task creation should succeed");

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let in_flight = in_flight_task();
        *in_flight.lock().unwrap() = Some(task.id.clone());

        let mut handler = GasKillerHandler::new(1, provider)
            .with_store(store.clone())
            .with_in_flight_task(in_flight.clone());

        // `task_data: None` makes `render_schnorr_payload` fail immediately in
        // `prepare_schnorr` (before any provider call), exercising the settlement wiring
        // without needing a real chain or a valid aggregate signature.
        let result = handler
            .handle_schnorr_verification(
                0,
                FixedBytes::<32>::ZERO,
                0,
                U256::ZERO,
                Address::ZERO,
                vec![],
                None,
            )
            .await;

        assert!(result.is_err());
        assert!(
            in_flight.lock().unwrap().is_none(),
            "the slot must be cleared once the task settles"
        );
        let settled = store
            .get_task(&task.id)
            .await
            .unwrap()
            .expect("task should still exist");
        assert_eq!(settled.status, TaskStatus::Failed);
        assert!(
            settled
                .error
                .as_deref()
                .is_some_and(|e| e.contains("verification failed"))
        );
    }

    #[tokio::test]
    async fn handle_schnorr_verification_is_noop_on_store_when_slot_empty() {
        let store = store().await;
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let mut handler = GasKillerHandler::new(1, provider).with_store(store);

        // No task was recorded as in flight; settlement must not panic when there
        // is nothing to settle.
        let result = handler
            .handle_schnorr_verification(
                0,
                FixedBytes::<32>::ZERO,
                0,
                U256::ZERO,
                Address::ZERO,
                vec![],
                None,
            )
            .await;

        assert!(result.is_err());
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
    // signature checker cannot verify the round.
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
            .resolve_payload_gas(Err(execution_reverted(Some("0xe1310aed"))), Address::ZERO)
            .expect_err("a reverting estimate must not yield a payload");
        let message = error.to_string();
        assert!(message.contains("InvalidQuorumApkHash()"), "{message}");
        assert!(message.contains("blsSignatureChecker"), "{message}");
    }

    #[test]
    fn resolve_payload_gas_counts_rejected_payloads() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let metrics = Arc::new(MetricsCollector::new());
        let handler = GasKillerHandler::new(1, provider).with_metrics(metrics.clone());

        assert!(
            handler
                .resolve_payload_gas(Err(execution_reverted(Some("0xe1310aed"))), Address::ZERO)
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
    async fn handle_verification_settles_failed_when_the_rendered_payload_reverts() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store
            .create_task(&key, &request_body())
            .await
            .expect("task creation should succeed");

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

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        // The ERC-165 probe passes, then eth_estimateGas reverts with the signature-checker
        // mismatch selector.
        push_supports_interface(&asserter, true);
        push_execution_revert(&asserter, "0xe1310aed");

        let in_flight = in_flight_task();
        *in_flight.lock().unwrap() = Some(task.id.clone());
        let mut handler = GasKillerHandler::new(1, provider)
            .with_store(store.clone())
            .with_in_flight_task(in_flight.clone())
            .with_payload_block_buffer(50);

        let result = handler
            .handle_verification(
                0,
                msg_hash,
                Bytes::from(vec![0x00]),
                100,
                empty_non_signer_data(),
                Some(&task_data),
            )
            .await;

        assert!(result.is_err());
        let settled = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Failed);
        assert!(
            settled.payload.is_none(),
            "a payload proven to revert must not be persisted"
        );
        let error = settled.error.expect("a failure reason should be recorded");
        assert!(error.contains("InvalidQuorumApkHash()"), "{error}");
        assert!(error.contains("blsSignatureChecker"), "{error}");
    }
}

//! Ethereum JSON-RPC compatible ingress.
//!
//! Lets users point a standard Ethereum client (wallet, ethers/viem/web3 script, `cast`) at the
//! router as if it were a regular RPC node. `eth_sendRawTransaction` is intercepted: the raw
//! signed transaction is decoded, the sender is recovered from its ECDSA signature, and the
//! transaction is converted into a Gas Killer task that flows through the same queue as
//! `POST /trigger`. All other `eth_*`/`net_*`/`web3_*` read methods are proxied verbatim to the
//! upstream chain RPC so nonce fetching, gas estimation, chain-id discovery, and block queries
//! keep working — a client can switch its RPC URL over and function unmodified.
//!
//! Routes:
//! - `POST /rpc` — API key via `Authorization: Bearer gk_...` (programmatic clients).
//! - `POST /rpc/:key` — API key embedded in the path, Infura-style, for wallets that cannot set
//!   headers. Both accept an optional `?chain=l1|l2` query parameter selecting which configured
//!   chain read methods are proxied to (default `l1`).
//!
//! Sender identity: `from_address` is not taken from the caller's word — it is recovered from the
//! transaction signature, so submitting as an address requires holding its key. Operators simulate
//! the call with that address as `msg.sender`, and what lands on-chain via `verifyAndUpdate` is
//! the storage diff of that simulation, preserving `msg.sender` semantics off-chain. The
//! transaction itself is never broadcast: its nonce is not consumed and no receipt will exist for
//! the returned hash. Duplicate submissions of the same raw bytes are deduplicated best-effort by
//! transaction hash (bounded, per-process): once the original is queued, duplicates return the
//! same hash idempotently; a duplicate racing a still-in-flight original gets the geth-style
//! "already known" error, since the original may yet fail.
//!
//! Error contract: HTTP status is always 200 for well-formed HTTP requests; failures are reported
//! as JSON-RPC 2.0 error objects. Standard codes (-32700 parse, -32600 invalid request, -32601
//! method not found, -32602 invalid params) plus server codes: -32000 generic/validation,
//! -32001 unauthorized, -32002 upstream RPC unavailable, -32005 queue at capacity.

use crate::error::{ApiError, ErrorCode};
use crate::ingress::{GasKillerTaskRequest, GasKillerTaskRequestBody};
use crate::ingress::{IngressState, check_queue_capacity, enqueue_task, resolve_task_token};
use alloy::consensus::Transaction;
use alloy::consensus::TxEnvelope;
use alloy::consensus::transaction::SignerRecoverable;
use alloy::eips::eip2718::Decodable2718;
use alloy_primitives::{Address, B256, U256};
use alloy_provider::Provider;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use gas_killer_common::ChainRole;
use gas_killer_common::config::CHAIN_DETECTION_ORDER;
use gas_killer_common::task_data::MAX_EVM_TX_CALLDATA_SIZE;
use serde::{Deserialize, Serialize};
use serde_json::value::{RawValue, to_raw_value};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::env;
use tracing::{info, warn};

// -- configuration -----------------------------------------------------------------------------

/// Default maximum number of entries accepted in one JSON-RPC batch request.
pub const DEFAULT_RPC_MAX_BATCH_SIZE: usize = 100;

/// Maximum number of entries accepted in one JSON-RPC batch request (`RPC_MAX_BATCH_SIZE`).
pub fn rpc_max_batch_size() -> usize {
    env::var("RPC_MAX_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v: &usize| v > 0)
        .unwrap_or(DEFAULT_RPC_MAX_BATCH_SIZE)
}

/// Default capacity of the seen-transaction-hash cache used for duplicate suppression.
pub const DEFAULT_RPC_TX_DEDUP_CAPACITY: usize = 8192;

/// Capacity of the seen-transaction-hash cache (`RPC_TX_DEDUP_CAPACITY`).
pub fn rpc_tx_dedup_capacity() -> usize {
    env::var("RPC_TX_DEDUP_CAPACITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v: &usize| v > 0)
        .unwrap_or(DEFAULT_RPC_TX_DEDUP_CAPACITY)
}

/// Default overall deadline for the upstream RPC work of one `eth_sendRawTransaction`.
pub const DEFAULT_RPC_UPSTREAM_TIMEOUT_SECS: u64 = 15;

/// Overall deadline in seconds for the upstream RPC work of one `eth_sendRawTransaction`
/// (`RPC_UPSTREAM_TIMEOUT_SECS`). Bounds how long a pending dedup reservation can exist when the
/// upstream hangs, so retries are never blocked indefinitely by a stuck first attempt.
pub fn rpc_upstream_timeout() -> std::time::Duration {
    let secs = env::var("RPC_UPSTREAM_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v: &u64| v > 0)
        .unwrap_or(DEFAULT_RPC_UPSTREAM_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

// -- duplicate suppression ---------------------------------------------------------------------

/// State of a transaction hash in the dedup cache.
///
/// `Pending` marks a submission whose validation/enqueue is still in flight; only `Committed`
/// entries — whose task is actually queued — answer duplicates with an idempotent success. A
/// duplicate racing a `Pending` first attempt gets a geth-style "already known" error instead,
/// so a first attempt that ultimately fails can never have handed a duplicate caller a success
/// for work that never happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxSeen {
    Pending,
    Committed,
}

/// Bounded FIFO map of recently submitted transaction hashes.
///
/// Makes wallet retry loops idempotent: a resubmission of already-queued raw bytes returns the
/// same hash without enqueueing a second task. Best-effort only: the map is per-process and
/// bounded, so it is not a consensus-level replay guard — nothing on-chain consumes the
/// transaction's nonce.
pub struct SeenTxCache {
    entries: HashMap<B256, TxSeen>,
    order: VecDeque<B256>,
    capacity: usize,
}

impl SeenTxCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    /// Reserves `hash` as `Pending`, evicting the oldest entry at capacity. Returns the existing
    /// state instead if the hash is already present.
    pub fn reserve(&mut self, hash: B256) -> Option<TxSeen> {
        if let Some(existing) = self.entries.get(&hash) {
            return Some(*existing);
        }
        self.entries.insert(hash, TxSeen::Pending);
        self.order.push_back(hash);
        if self.order.len() > self.capacity
            && let Some(evicted) = self.order.pop_front()
        {
            self.entries.remove(&evicted);
        }
        None
    }

    /// Marks a reservation as committed once its task is enqueued, enabling idempotent
    /// duplicate responses.
    pub fn commit(&mut self, hash: &B256) {
        if let Some(state) = self.entries.get_mut(hash) {
            *state = TxSeen::Committed;
        }
    }

    /// Rolls back a reservation whose submission failed, so a later retry is processed fresh.
    pub fn remove(&mut self, hash: &B256) {
        if self.entries.remove(hash).is_some() {
            self.order.retain(|h| h != hash);
        }
    }
}

// -- JSON-RPC wire types -----------------------------------------------------------------------

/// A single JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
pub struct RpcErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<RawValue>>,
}

impl RpcErrorObject {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    fn server(message: impl Into<String>) -> Self {
        Self::new(-32000, message)
    }

    fn upstream_unavailable() -> Self {
        Self::new(-32002, "upstream RPC unavailable")
    }
}

/// Maps ingress-layer [`ApiError`]s (shared with `/trigger`) onto JSON-RPC server error codes.
impl From<ApiError> for RpcErrorObject {
    fn from(e: ApiError) -> Self {
        let code = match e.code {
            ErrorCode::QueueFull | ErrorCode::RateLimited => -32005,
            ErrorCode::Unauthorized => -32001,
            ErrorCode::RpcUnavailable => -32002,
            _ => -32000,
        };
        Self::new(code, e.message)
    }
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Box<RawValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcErrorObject>,
}

impl RpcResponse {
    fn result(id: serde_json::Value, result: Box<RawValue>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: serde_json::Value, error: RpcErrorObject) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RpcQuery {
    /// Which configured chain read methods are proxied to: `l1` (default) or `l2`.
    chain: Option<String>,
}

// -- HTTP handlers -----------------------------------------------------------------------------

/// `POST /rpc` — credential from the `Authorization: Bearer` header.
pub async fn rpc_handler(
    State(state): State<IngressState>,
    Query(query): Query<RpcQuery>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    serve_rpc(state, query, headers, None, body).await
}

/// `POST /rpc/:key` — credential embedded in the URL path for clients that cannot set headers
/// (browser wallets). The key is the same `gk_`-prefixed API key `/trigger` accepts.
pub async fn rpc_with_key_handler(
    State(state): State<IngressState>,
    Path(key): Path<String>,
    Query(query): Query<RpcQuery>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    serve_rpc(state, query, headers, Some(key), body).await
}

async fn serve_rpc(
    state: IngressState,
    query: RpcQuery,
    headers: HeaderMap,
    path_key: Option<String>,
    body: bytes::Bytes,
) -> Response {
    // Authorize before touching the payload. Path key (wallet-style) takes precedence over the
    // Authorization header; both feed the same store-backed verification as /trigger.
    if let Err(e) = resolve_task_token(&state, &headers, path_key.as_deref()).await {
        return single_error_response(RpcErrorObject::from(e));
    }

    let chain = match query.chain.as_deref() {
        None => ChainRole::L1,
        Some(c) if c.eq_ignore_ascii_case("l1") => ChainRole::L1,
        Some(c) if c.eq_ignore_ascii_case("l2") => ChainRole::L2,
        Some(other) => {
            return single_error_response(RpcErrorObject::new(
                -32600,
                format!("unknown chain {other:?}: expected \"l1\" or \"l2\""),
            ));
        }
    };

    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return single_error_response(RpcErrorObject::new(-32700, format!("parse error: {e}")));
        }
    };

    match parsed {
        serde_json::Value::Object(_) => {
            let response = handle_entry(&state, chain, parsed).await;
            json_response(&response)
        }
        serde_json::Value::Array(entries) => {
            if entries.is_empty() {
                return single_error_response(RpcErrorObject::new(-32600, "empty batch"));
            }
            let max = rpc_max_batch_size();
            if entries.len() > max {
                return single_error_response(RpcErrorObject::new(
                    -32600,
                    format!("batch of {} exceeds the limit of {max}", entries.len()),
                ));
            }
            // Entries run concurrently (upstream reads dominate latency); response order matches
            // request order as JSON-RPC batch clients expect.
            let responses = futures::future::join_all(
                entries.into_iter().map(|e| handle_entry(&state, chain, e)),
            )
            .await;
            json_response(&responses)
        }
        _ => single_error_response(RpcErrorObject::new(
            -32600,
            "request must be a JSON object or a non-empty array",
        )),
    }
}

fn json_response<T: Serialize>(value: &T) -> Response {
    axum::Json(value).into_response()
}

fn single_error_response(error: RpcErrorObject) -> Response {
    json_response(&RpcResponse::error(serde_json::Value::Null, error))
}

/// Handles one JSON-RPC request object (either standalone or a batch element).
async fn handle_entry(
    state: &IngressState,
    chain: ChainRole,
    entry: serde_json::Value,
) -> RpcResponse {
    let serde_json::Value::Object(obj) = entry else {
        return RpcResponse::error(
            serde_json::Value::Null,
            RpcErrorObject::new(-32600, "batch entry must be a JSON object"),
        );
    };
    let id = obj.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let Some(method) = obj
        .get("method")
        .and_then(|m| m.as_str())
        .map(str::to_owned)
    else {
        return RpcResponse::error(
            id,
            RpcErrorObject::new(-32600, "missing or non-string \"method\""),
        );
    };
    let params = obj.get("params").cloned();

    match dispatch(state, chain, &method, params).await {
        Ok(result) => RpcResponse::result(id, result),
        Err(e) => RpcResponse::error(id, e),
    }
}

/// Methods with prefixes we otherwise proxy that must never be forwarded upstream.
///
/// Signing methods would ask the upstream node to sign with accounts it does not have (and would
/// leak intent), and subscriptions require a WebSocket transport this endpoint does not provide.
const PROXY_DENYLIST: &[&str] = &[
    "eth_sendTransaction",
    "eth_sendRawTransaction", // handled locally; listed for clarity
    "eth_sign",
    "eth_signTransaction",
    "eth_signTypedData",
    "eth_signTypedData_v1",
    "eth_signTypedData_v3",
    "eth_signTypedData_v4",
    "eth_subscribe",
    "eth_unsubscribe",
];

fn is_proxyable(method: &str) -> bool {
    (method.starts_with("eth_") || method.starts_with("net_") || method.starts_with("web3_"))
        && !PROXY_DENYLIST.contains(&method)
}

async fn dispatch(
    state: &IngressState,
    chain: ChainRole,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<Box<RawValue>, RpcErrorObject> {
    match method {
        "eth_sendRawTransaction" => handle_send_raw_transaction(state, params).await,
        // No unlocked accounts exist server-side; answering locally keeps wallets and dapp
        // libraries (which call this at startup) working instead of erroring.
        "eth_accounts" => to_raw_value(&Vec::<Address>::new())
            .map_err(|_| RpcErrorObject::server("serialization failed")),
        "eth_sendTransaction"
        | "eth_sign"
        | "eth_signTransaction"
        | "eth_signTypedData"
        | "eth_signTypedData_v1"
        | "eth_signTypedData_v3"
        | "eth_signTypedData_v4" => Err(RpcErrorObject::new(
            -32601,
            format!(
                "{method} is not supported: this endpoint holds no accounts; sign locally and submit via eth_sendRawTransaction"
            ),
        )),
        m if is_proxyable(m) => proxy_request(state, chain, m, params).await,
        _ => Err(RpcErrorObject::new(
            -32601,
            format!("the method {method} does not exist/is not available"),
        )),
    }
}

/// Forwards a read method verbatim to the upstream provider for `chain`, preserving the upstream
/// result or JSON-RPC error byte-for-byte where possible.
async fn proxy_request(
    state: &IngressState,
    chain: ChainRole,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<Box<RawValue>, RpcErrorObject> {
    let provider = state
        .providers
        .get(&chain)
        .ok_or_else(|| RpcErrorObject::server(format!("chain {chain} is not configured")))?;

    let params_raw = match &params {
        Some(v) => to_raw_value(v),
        None => to_raw_value(&[(); 0]),
    }
    .map_err(|_| RpcErrorObject::invalid_params("params are not serializable"))?;

    provider
        .raw_request_dyn(Cow::Owned(method.to_owned()), &params_raw)
        .await
        .map_err(|e| match e {
            alloy::transports::RpcError::ErrorResp(payload) => RpcErrorObject {
                code: payload.code,
                message: payload.message.into_owned(),
                data: payload.data,
            },
            other => {
                warn!(method, error = %other, "upstream RPC proxy call failed");
                RpcErrorObject::upstream_unavailable()
            }
        })
}

/// Locks a mutex, recovering the inner value if a previous holder panicked. The guarded
/// structures here are plain collections whose operations cannot leave them logically
/// inconsistent, so poisoning must not wedge the endpoint into panicking on every request.
fn lock_ignore_poison<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Returns the EVM chain id of `chain`'s provider, memoized per process (chain ids are static
/// for a given RPC endpoint).
async fn chain_id_for(state: &IngressState, chain: ChainRole) -> Result<u64, RpcErrorObject> {
    if let Some(id) = lock_ignore_poison(&state.chain_id_cache).get(&chain) {
        return Ok(*id);
    }
    let provider = state
        .providers
        .get(&chain)
        .ok_or_else(|| RpcErrorObject::server(format!("chain {chain} is not configured")))?;
    let id = provider.get_chain_id().await.map_err(|e| {
        warn!(chain = %chain, error = %e, "failed to fetch chain id");
        RpcErrorObject::upstream_unavailable()
    })?;
    lock_ignore_poison(&state.chain_id_cache).insert(chain, id);
    Ok(id)
}

/// Decodes, authenticates (via signature recovery), converts, and enqueues a raw signed
/// transaction as a Gas Killer task. Returns the transaction hash, as a real node would.
async fn handle_send_raw_transaction(
    state: &IngressState,
    params: Option<serde_json::Value>,
) -> Result<Box<RawValue>, RpcErrorObject> {
    let raw = extract_raw_tx_param(params)?;

    let envelope = TxEnvelope::decode_2718_exact(&raw).map_err(|e| {
        RpcErrorObject::invalid_params(format!("could not decode transaction: {e}"))
    })?;

    match &envelope {
        TxEnvelope::Eip4844(_) => {
            return Err(RpcErrorObject::server(
                "transaction type not supported: blob (EIP-4844) transactions cannot be converted to Gas Killer tasks",
            ));
        }
        TxEnvelope::Eip7702(_) => {
            return Err(RpcErrorObject::server(
                "transaction type not supported: set-code (EIP-7702) transactions cannot be converted to Gas Killer tasks",
            ));
        }
        _ => {}
    }

    // The recovered signer is the task's from_address: operators simulate the call with it as
    // msg.sender, so acting as an address requires its private key — same proof a real node
    // demands. recover_signer enforces EIP-2 low-s, correct for newly signed transactions.
    let from = envelope
        .recover_signer()
        .map_err(|e| RpcErrorObject::server(format!("invalid transaction signature: {e}")))?;

    let Some(target) = envelope.to() else {
        return Err(RpcErrorObject::server(
            "contract creation transactions are not supported",
        ));
    };

    let input = envelope.input();
    if input.is_empty() {
        return Err(RpcErrorObject::server(
            "transaction has no calldata: only contract function calls can become Gas Killer tasks",
        ));
    }
    if input.len() < 4 {
        return Err(RpcErrorObject::server(
            "transaction calldata is shorter than a 4-byte function selector",
        ));
    }
    if input.len() > MAX_EVM_TX_CALLDATA_SIZE {
        return Err(RpcErrorObject::server(format!(
            "transaction calldata of {} bytes exceeds the maximum of {MAX_EVM_TX_CALLDATA_SIZE}",
            input.len()
        )));
    }

    if state.providers.is_empty() {
        return Err(RpcErrorObject::server(
            "no upstream RPC providers are configured",
        ));
    }

    // Load-shed before any RPC round-trips, mirroring /trigger.
    check_queue_capacity(state).map_err(RpcErrorObject::from)?;

    let tx_hash = *envelope.tx_hash();
    let hash_result =
        to_raw_value(&tx_hash).map_err(|_| RpcErrorObject::server("serialization failed"))?;

    // Reserve the hash before the async validation work so a concurrent identical submission
    // cannot double-enqueue. Only a Committed entry (task actually queued) answers duplicates
    // with an idempotent success; a duplicate racing a still-Pending first attempt gets the
    // geth "already known" error, because that first attempt may yet fail and roll back — a
    // success here would report work that never happened.
    match lock_ignore_poison(&state.rpc_seen_txs).reserve(tx_hash) {
        Some(TxSeen::Committed) => {
            info!(tx_hash = %tx_hash, "duplicate raw transaction; returning existing hash");
            return Ok(hash_result);
        }
        Some(TxSeen::Pending) => {
            return Err(RpcErrorObject::server("already known"));
        }
        None => {}
    }

    // The overall deadline bounds how long the Pending reservation can block retries when the
    // upstream RPC hangs; on expiry the reservation is rolled back like any other failure.
    let converted = tokio::time::timeout(
        rpc_upstream_timeout(),
        convert_and_enqueue(state, &envelope, from, target),
    )
    .await
    .unwrap_or_else(|_| {
        warn!(tx_hash = %tx_hash, "upstream RPC calls timed out during raw tx conversion");
        Err(RpcErrorObject::upstream_unavailable())
    });

    match converted {
        Ok(()) => {
            lock_ignore_poison(&state.rpc_seen_txs).commit(&tx_hash);
            Ok(hash_result)
        }
        Err(e) => {
            lock_ignore_poison(&state.rpc_seen_txs).remove(&tx_hash);
            Err(e)
        }
    }
}

/// Resolves which configured chain a raw transaction's task must run on.
///
/// When the transaction carries an EIP-155 chain id, that signed intent is authoritative: the
/// chain whose id matches is selected, the target must have code there, and — because the
/// creator and operator nodes re-detect the chain by bytecode presence in `CHAIN_DETECTION_ORDER`
/// — any earlier-ordered chain must NOT also host code at that address, otherwise downstream
/// resolution would diverge from the signed intent and the task is refused as ambiguous.
///
/// Pre-EIP-155 transactions (no chain id) fall back to detection order. In both modes a failed
/// `eth_getCode` probe is a hard, retryable error rather than a skipped chain: silently skipping
/// a chain that later recovers is exactly the divergence window this function exists to close.
async fn resolve_target_chain(
    state: &IngressState,
    target: Address,
    tx_chain_id: Option<u64>,
) -> Result<ChainRole, RpcErrorObject> {
    let code_at = |role: ChainRole| {
        let provider = state.providers.get(&role);
        async move {
            let provider = provider
                .ok_or_else(|| RpcErrorObject::server(format!("chain {role} is not configured")))?;
            provider.get_code_at(target).await.map_err(|e| {
                warn!(chain = %role, error = %e, "eth_getCode probe failed");
                RpcErrorObject::upstream_unavailable()
            })
        }
    };

    let configured: Vec<ChainRole> = CHAIN_DETECTION_ORDER
        .iter()
        .copied()
        .filter(|role| state.providers.contains_key(role))
        .collect();

    let Some(tx_chain_id) = tx_chain_id else {
        // Legacy pre-EIP-155: no signed chain intent; first configured chain with code wins.
        for role in &configured {
            if !code_at(*role).await?.is_empty() {
                return Ok(*role);
            }
        }
        return Err(RpcErrorObject::server(format!(
            "no contract code at {target} on any supported chain"
        )));
    };

    let mut served_ids = Vec::with_capacity(configured.len());
    let mut selected = None;
    for role in &configured {
        let id = chain_id_for(state, *role).await?;
        served_ids.push(id);
        if id == tx_chain_id && selected.is_none() {
            selected = Some(*role);
        }
    }
    let Some(selected) = selected else {
        return Err(RpcErrorObject::server(format!(
            "transaction chain id {tx_chain_id} is not served by this endpoint (serving chain ids: {served_ids:?})"
        )));
    };

    if code_at(selected).await?.is_empty() {
        return Err(RpcErrorObject::server(format!(
            "no contract code at {target} on chain id {tx_chain_id}"
        )));
    }

    for role in &configured {
        if *role == selected {
            break;
        }
        if !code_at(*role).await?.is_empty() {
            return Err(RpcErrorObject::server(format!(
                "target {target} has code on multiple configured chains; refusing the transaction because task execution resolves chains by bytecode presence ({}-first) and would not honor the signed chain id {tx_chain_id}",
                CHAIN_DETECTION_ORDER[0]
            )));
        }
    }

    Ok(selected)
}

/// Validates the decoded transaction against live chain state and enqueues the task.
async fn convert_and_enqueue(
    state: &IngressState,
    envelope: &TxEnvelope,
    from: Address,
    target: Address,
) -> Result<(), RpcErrorObject> {
    let chain = resolve_target_chain(state, target, envelope.chain_id()).await?;

    let provider = state
        .providers
        .get(&chain)
        .ok_or_else(|| RpcErrorObject::server(format!("chain {chain} is not configured")))?;

    // Operators simulate with real balances and no overrides: a value transfer the sender cannot
    // fund would fail every node's simulation later. Reject it up front with the error text
    // wallets already know how to display.
    let value = envelope.value();
    if value > U256::ZERO {
        let balance = provider.get_balance(from).await.map_err(|e| {
            warn!(error = %e, "balance check failed");
            RpcErrorObject::upstream_unavailable()
        })?;
        if balance < value {
            return Err(RpcErrorObject::server(format!(
                "insufficient funds for transfer: address {from} has balance {balance}, tx value {value}"
            )));
        }
    }

    // Anchor the deterministic simulation at the current head: trivially fresh, and the
    // speculative prebuild cache targets head so validation usually hits a warm executor.
    let block_height = provider.get_block_number().await.map_err(|e| {
        warn!(error = %e, "block number fetch failed");
        RpcErrorObject::upstream_unavailable()
    })?;

    let request = GasKillerTaskRequest {
        body: GasKillerTaskRequestBody {
            target_address: target,
            call_data: input_bytes(envelope),
            // Auto: the creator resolves the next transition slot at dequeue time, so wallet
            // users need no knowledge of Gas Killer state and parallel submissions are safe.
            transition_index: None,
            from_address: from,
            value,
            block_height,
        },
    };

    request.validate().map_err(|e| {
        warn!(error = %e, "converted transaction failed task validation");
        RpcErrorObject::server(e.to_string())
    })?;

    info!(
        target_address = %target,
        from_address = %from,
        block_height,
        call_data_len = request.body.call_data.len(),
        chain = %chain,
        "raw transaction accepted as task"
    );

    enqueue_task(state, request).map_err(RpcErrorObject::from)
}

fn input_bytes(envelope: &TxEnvelope) -> Vec<u8> {
    envelope.input().to_vec()
}

/// Extracts the single `"0x..."` string parameter of `eth_sendRawTransaction`.
fn extract_raw_tx_param(params: Option<serde_json::Value>) -> Result<Vec<u8>, RpcErrorObject> {
    let Some(serde_json::Value::Array(items)) = params else {
        return Err(RpcErrorObject::invalid_params(
            "eth_sendRawTransaction expects an array of one hex-encoded transaction",
        ));
    };
    let [item] = items.as_slice() else {
        return Err(RpcErrorObject::invalid_params(format!(
            "eth_sendRawTransaction expects exactly 1 parameter, got {}",
            items.len()
        )));
    };
    let Some(hex_str) = item.as_str() else {
        return Err(RpcErrorObject::invalid_params(
            "transaction must be a hex string",
        ));
    };
    let bytes = alloy_primitives::hex::decode(hex_str)
        .map_err(|e| RpcErrorObject::invalid_params(format!("invalid hex: {e}")))?;
    if bytes.is_empty() {
        return Err(RpcErrorObject::invalid_params("empty transaction data"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingress::{IngressState, build_app};
    use crate::store::SqliteStore;
    use alloy::consensus::{SignableTransaction, TxEip1559, TxEip7702, TxLegacy};
    use alloy::eips::eip2718::Encodable2718;
    use alloy::network::TxSignerSync;
    use alloy_primitives::{Bytes, TxKind, U64, address, hex};
    use alloy_provider::{ProviderBuilder, mock::Asserter};
    use alloy_signer_local::PrivateKeySigner;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use gas_killer_common::ReadOnlyProvider;
    use std::collections::HashMap;
    use tower::util::ServiceExt;

    /// anvil account 0
    const TEST_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_ADDR: Address = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
    const TARGET: Address = address!("00000000000000000000000000000000000000A1");
    const CHAIN_ID: u64 = 31337;

    fn signer() -> PrivateKeySigner {
        TEST_KEY.parse().unwrap()
    }

    fn eip1559_tx(chain_id: u64, input: Vec<u8>, value: U256, to: TxKind) -> TxEnvelope {
        let mut tx = TxEip1559 {
            chain_id,
            nonce: 0,
            gas_limit: 1_000_000,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            to,
            value,
            input: input.into(),
            ..Default::default()
        };
        let sig = signer().sign_transaction_sync(&mut tx).unwrap();
        TxEnvelope::from(tx.into_signed(sig))
    }

    fn call_tx() -> TxEnvelope {
        eip1559_tx(
            CHAIN_ID,
            vec![0xAB, 0xCD, 0xEF, 0x01],
            U256::ZERO,
            TxKind::Call(TARGET),
        )
    }

    fn raw_hex(envelope: &TxEnvelope) -> String {
        format!("0x{}", hex::encode(envelope.encoded_2718()))
    }

    fn rpc_body(method: &str, params: serde_json::Value) -> String {
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
            .to_string()
    }

    fn send_raw_body(envelope: &TxEnvelope) -> String {
        rpc_body(
            "eth_sendRawTransaction",
            serde_json::json!([raw_hex(envelope)]),
        )
    }

    fn mock_provider() -> (ReadOnlyProvider, Asserter) {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        (provider, asserter)
    }

    /// State with a mocked L1 provider and open (store-less) auth.
    fn make_state() -> (IngressState, crate::creator::TaskReceiver, Asserter) {
        let (sender, receiver) = crate::creator::task_channel();
        let queue_depth = crate::creator::task_queue_depth();
        let mut state = IngressState::without_metrics(sender, queue_depth);
        let (provider, asserter) = mock_provider();
        let mut providers = HashMap::new();
        providers.insert(ChainRole::L1, provider);
        state.providers = std::sync::Arc::new(providers);
        (state, receiver, asserter)
    }

    fn make_app() -> (Router, crate::creator::TaskReceiver, Asserter) {
        let (state, receiver, asserter) = make_state();
        (build_app().with_state(state), receiver, asserter)
    }

    fn rpc_request(uri: &str, body: String) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    async fn response_json(resp: axum::response::Response) -> serde_json::Value {
        assert_eq!(resp.status(), StatusCode::OK, "JSON-RPC always answers 200");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Queues the standard happy-path mock responses for a CHAIN_ID-signed transaction:
    /// eth_chainId (resolves the signed chain), eth_getCode (contract exists), eth_blockNumber.
    fn push_happy_path(asserter: &Asserter) {
        asserter.push_success(&U64::from(CHAIN_ID)); // eth_chainId
        asserter.push_success(&Bytes::from(vec![0x60u8])); // eth_getCode: non-empty
        asserter.push_success(&U64::from(1234u64)); // eth_blockNumber
    }

    // -- eth_sendRawTransaction: happy path ----------------------------------------------------

    #[tokio::test]
    async fn send_raw_tx_queues_task_with_recovered_sender() {
        let (app, mut receiver, asserter) = make_app();
        push_happy_path(&asserter);

        let envelope = call_tx();
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;

        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert_eq!(
            json["result"].as_str().unwrap().to_lowercase(),
            format!("{:#x}", envelope.tx_hash()),
            "result must be the transaction hash"
        );

        let task = receiver.try_recv().expect("task should be queued");
        assert_eq!(task.body.target_address, TARGET);
        assert_eq!(
            task.body.from_address, TEST_ADDR,
            "from_address must be the ecrecovered signer"
        );
        assert_eq!(task.body.call_data, vec![0xAB, 0xCD, 0xEF, 0x01]);
        assert_eq!(task.body.value, U256::ZERO);
        assert_eq!(task.body.block_height, 1234);
        assert_eq!(
            task.body.transition_index, None,
            "transition index must be auto-resolved"
        );
    }

    #[tokio::test]
    async fn send_raw_tx_legacy_eip155_works() {
        let (app, mut receiver, asserter) = make_app();
        push_happy_path(&asserter);

        let mut tx = TxLegacy {
            chain_id: Some(CHAIN_ID),
            nonce: 7,
            gas_price: 1_000_000_000,
            gas_limit: 500_000,
            to: TxKind::Call(TARGET),
            value: U256::ZERO,
            input: vec![1, 2, 3, 4, 5].into(),
        };
        let sig = signer().sign_transaction_sync(&mut tx).unwrap();
        let envelope = TxEnvelope::from(tx.into_signed(sig));

        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert!(
            json["error"].is_null(),
            "unexpected error: {}",
            json["error"]
        );

        let task = receiver.try_recv().expect("task should be queued");
        assert_eq!(task.body.from_address, TEST_ADDR);
        assert_eq!(task.body.call_data, vec![1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn send_raw_tx_pre_eip155_skips_chain_id_check() {
        let (app, mut receiver, asserter) = make_app();
        // Only getCode + blockNumber are consumed: no chain id fetch for a pre-155 tx.
        asserter.push_success(&Bytes::from(vec![0x60u8]));
        asserter.push_success(&U64::from(1234u64));

        let mut tx = TxLegacy {
            chain_id: None,
            nonce: 0,
            gas_price: 1_000_000_000,
            gas_limit: 500_000,
            to: TxKind::Call(TARGET),
            value: U256::ZERO,
            input: vec![9, 9, 9, 9].into(),
        };
        let sig = signer().sign_transaction_sync(&mut tx).unwrap();
        let envelope = TxEnvelope::from(tx.into_signed(sig));

        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert!(
            json["error"].is_null(),
            "unexpected error: {}",
            json["error"]
        );
        assert!(receiver.try_recv().is_ok());
    }

    #[tokio::test]
    async fn send_raw_tx_with_value_checks_balance() {
        let (app, mut receiver, asserter) = make_app();
        asserter.push_success(&U64::from(CHAIN_ID)); // chainId
        asserter.push_success(&Bytes::from(vec![0x60u8])); // getCode
        asserter.push_success(&U256::from(10_000_000_000_000_000_000u128)); // getBalance: 10 ETH
        asserter.push_success(&U64::from(1234u64)); // blockNumber

        let envelope = eip1559_tx(
            CHAIN_ID,
            vec![1, 2, 3, 4],
            U256::from(1_000_000_000_000_000_000u128), // 1 ETH
            TxKind::Call(TARGET),
        );
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert!(
            json["error"].is_null(),
            "unexpected error: {}",
            json["error"]
        );
        let task = receiver.try_recv().unwrap();
        assert_eq!(task.body.value, U256::from(1_000_000_000_000_000_000u128));
    }

    #[tokio::test]
    async fn send_raw_tx_insufficient_balance_rejected() {
        let (app, mut receiver, asserter) = make_app();
        asserter.push_success(&U64::from(CHAIN_ID));
        asserter.push_success(&Bytes::from(vec![0x60u8]));
        asserter.push_success(&U256::from(1u64)); // balance: 1 wei

        let envelope = eip1559_tx(
            CHAIN_ID,
            vec![1, 2, 3, 4],
            U256::from(2u64),
            TxKind::Call(TARGET),
        );
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32000);
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("insufficient funds"),
            "wallets pattern-match this message: {}",
            json["error"]["message"]
        );
        assert!(receiver.try_recv().is_err(), "nothing must be queued");
    }

    // -- eth_sendRawTransaction: rejections ----------------------------------------------------

    #[tokio::test]
    async fn send_raw_tx_wrong_chain_id_rejected() {
        let (app, mut receiver, asserter) = make_app();
        asserter.push_success(&U64::from(CHAIN_ID)); // chainId (differs from tx's 1)

        let envelope = eip1559_tx(1, vec![1, 2, 3, 4], U256::ZERO, TxKind::Call(TARGET));
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32000);
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("chain id")
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_raw_tx_contract_creation_rejected() {
        let (app, mut receiver, _asserter) = make_app();
        let envelope = eip1559_tx(CHAIN_ID, vec![1, 2, 3, 4], U256::ZERO, TxKind::Create);
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32000);
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("contract creation")
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_raw_tx_empty_calldata_rejected() {
        let (app, mut receiver, _asserter) = make_app();
        let envelope = eip1559_tx(CHAIN_ID, vec![], U256::from(1u64), TxKind::Call(TARGET));
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32000);
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("no calldata")
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_raw_tx_short_calldata_rejected() {
        let (app, mut receiver, _asserter) = make_app();
        let envelope = eip1559_tx(CHAIN_ID, vec![1, 2], U256::ZERO, TxKind::Call(TARGET));
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32000);
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("4-byte")
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_raw_tx_eip7702_rejected() {
        let (app, mut receiver, _asserter) = make_app();
        let mut tx = TxEip7702 {
            chain_id: CHAIN_ID,
            nonce: 0,
            gas_limit: 500_000,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            to: TARGET,
            value: U256::ZERO,
            input: vec![1, 2, 3, 4].into(),
            ..Default::default()
        };
        let sig = signer().sign_transaction_sync(&mut tx).unwrap();
        let envelope = TxEnvelope::from(tx.into_signed(sig));

        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32000);
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("type not supported")
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_raw_tx_garbage_bytes_rejected() {
        let (app, mut receiver, _asserter) = make_app();
        let resp = app
            .oneshot(rpc_request(
                "/rpc",
                rpc_body("eth_sendRawTransaction", serde_json::json!(["0xdeadbeef"])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32602);
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_raw_tx_bad_params_rejected() {
        let (app, _receiver, _asserter) = make_app();
        for params in [
            serde_json::json!([]),
            serde_json::json!(["0x1", "0x2"]),
            serde_json::json!([42]),
            serde_json::json!("0x1234"),
            serde_json::json!(null),
            serde_json::json!(["not hex"]),
            serde_json::json!([""]),
        ] {
            let resp = app
                .clone()
                .oneshot(rpc_request(
                    "/rpc",
                    rpc_body("eth_sendRawTransaction", params.clone()),
                ))
                .await
                .unwrap();
            let json = response_json(resp).await;
            assert_eq!(
                json["error"]["code"], -32602,
                "params {params} should be invalid, got: {json}"
            );
        }
    }

    #[tokio::test]
    async fn send_raw_tx_no_contract_at_target_rejected() {
        let (app, mut receiver, asserter) = make_app();
        asserter.push_success(&U64::from(CHAIN_ID)); // chainId
        asserter.push_success(&Bytes::new()); // getCode: empty → not a contract

        let envelope = call_tx();
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32000);
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_raw_tx_upstream_down_returns_unavailable_and_allows_retry() {
        let (app, mut receiver, asserter) = make_app();
        asserter.push_failure_msg("connection refused"); // chainId fetch fails

        let envelope = call_tx();
        let resp = app
            .clone()
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32002);
        assert!(receiver.try_recv().is_err());

        // The dedup reservation must have been rolled back: a retry after upstream recovery
        // succeeds instead of being swallowed as a duplicate.
        push_happy_path(&asserter);
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert!(
            json["error"].is_null(),
            "retry should succeed: {}",
            json["error"]
        );
        assert!(receiver.try_recv().is_ok());
    }

    // -- duplicate suppression -------------------------------------------------------------------

    #[tokio::test]
    async fn duplicate_raw_tx_is_idempotent() {
        let (app, mut receiver, asserter) = make_app();
        push_happy_path(&asserter);

        let envelope = call_tx();
        let first = response_json(
            app.clone()
                .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
                .await
                .unwrap(),
        )
        .await;
        // No mock responses queued for the second call: it must not touch the upstream at all.
        let second = response_json(
            app.oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
                .await
                .unwrap(),
        )
        .await;

        assert_eq!(first["result"], second["result"]);
        assert!(receiver.try_recv().is_ok(), "first submission queues");
        assert!(
            receiver.try_recv().is_err(),
            "duplicate must not enqueue a second task"
        );
    }

    #[test]
    fn seen_tx_cache_two_state_lifecycle_and_eviction() {
        let mut cache = SeenTxCache::new(2);
        let (a, b, c) = (
            B256::with_last_byte(1),
            B256::with_last_byte(2),
            B256::with_last_byte(3),
        );
        assert_eq!(cache.reserve(a), None, "fresh hash reserves as pending");
        assert_eq!(
            cache.reserve(a),
            Some(TxSeen::Pending),
            "in-flight duplicate sees pending"
        );
        cache.commit(&a);
        assert_eq!(
            cache.reserve(a),
            Some(TxSeen::Committed),
            "committed duplicate sees committed"
        );
        assert_eq!(cache.reserve(b), None);
        assert_eq!(cache.reserve(c), None, "evicts a (capacity 2)");
        assert_eq!(
            cache.reserve(a),
            None,
            "a was evicted and can be re-reserved"
        );
        cache.remove(&c);
        assert_eq!(cache.reserve(c), None, "removed entries can be re-reserved");
    }

    #[tokio::test]
    async fn duplicate_of_pending_submission_returns_already_known() {
        // A duplicate that races a still-in-flight first submission must NOT get a success —
        // the first attempt may fail and roll back. Simulate the race by pre-marking the hash
        // as Pending, exactly the state a concurrent first request would have left.
        let (state, mut receiver, _asserter) = make_state();
        let envelope = call_tx();
        assert_eq!(
            lock_ignore_poison(&state.rpc_seen_txs).reserve(*envelope.tx_hash()),
            None
        );
        let app = build_app().with_state(state);

        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32000);
        assert_eq!(json["error"]["message"], "already known");
        assert!(receiver.try_recv().is_err(), "nothing must be enqueued");
    }

    #[tokio::test]
    async fn multi_chain_ambiguous_deployment_rejected() {
        // Contract deployed at the same address on L1 and L2, tx signed for L2: downstream
        // bytecode-first detection would pick L1, diverging from the signed intent → reject.
        let (sender, mut receiver) = crate::creator::task_channel();
        let queue_depth = crate::creator::task_queue_depth();
        let mut state = IngressState::without_metrics(sender, queue_depth);
        let (l1, l1_asserter) = mock_provider();
        let (l2, l2_asserter) = mock_provider();
        let mut providers = HashMap::new();
        providers.insert(ChainRole::L1, l1);
        providers.insert(ChainRole::L2, l2);
        state.providers = std::sync::Arc::new(providers);
        let app = build_app().with_state(state);

        l1_asserter.push_success(&U64::from(1u64)); // L1 chainId
        l2_asserter.push_success(&U64::from(10u64)); // L2 chainId → matches tx
        l2_asserter.push_success(&Bytes::from(vec![0x60u8])); // code on L2
        l1_asserter.push_success(&Bytes::from(vec![0x60u8])); // code on L1 too → ambiguous

        let envelope = eip1559_tx(10, vec![1, 2, 3, 4], U256::ZERO, TxKind::Call(TARGET));
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32000);
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("multiple configured chains"),
            "should explain the ambiguity: {}",
            json["error"]["message"]
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn l2_signed_tx_resolves_to_l2_when_unambiguous() {
        let (sender, mut receiver) = crate::creator::task_channel();
        let queue_depth = crate::creator::task_queue_depth();
        let mut state = IngressState::without_metrics(sender, queue_depth);
        let (l1, l1_asserter) = mock_provider();
        let (l2, l2_asserter) = mock_provider();
        let mut providers = HashMap::new();
        providers.insert(ChainRole::L1, l1);
        providers.insert(ChainRole::L2, l2);
        state.providers = std::sync::Arc::new(providers);
        let app = build_app().with_state(state);

        l1_asserter.push_success(&U64::from(1u64)); // L1 chainId
        l2_asserter.push_success(&U64::from(10u64)); // L2 chainId → matches tx
        l2_asserter.push_success(&Bytes::from(vec![0x60u8])); // code on L2
        l1_asserter.push_success(&Bytes::new()); // no code on L1 → unambiguous
        l2_asserter.push_success(&U64::from(777u64)); // L2 blockNumber

        let envelope = eip1559_tx(10, vec![1, 2, 3, 4], U256::ZERO, TxKind::Call(TARGET));
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert!(
            json["error"].is_null(),
            "unexpected error: {}",
            json["error"]
        );
        let task = receiver.try_recv().expect("task should be queued");
        assert_eq!(
            task.body.block_height, 777,
            "block height must come from L2"
        );
    }

    // -- queue interaction -----------------------------------------------------------------------

    #[tokio::test]
    async fn send_raw_tx_queue_full_returns_capacity_error() {
        let (mut state, mut receiver, _asserter) = make_state();
        state.max_queue_depth = 1;
        state
            .queue_depth
            .store(1, std::sync::atomic::Ordering::Relaxed);
        let app = build_app().with_state(state);

        let envelope = call_tx();
        let resp = app
            .oneshot(rpc_request("/rpc", send_raw_body(&envelope)))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32005);
        assert!(receiver.try_recv().is_err());
    }

    // -- proxying --------------------------------------------------------------------------------

    #[tokio::test]
    async fn proxied_method_passes_through_result() {
        let (app, _receiver, asserter) = make_app();
        asserter.push_success(&U64::from(0x7a69u64)); // eth_chainId

        let resp = app
            .oneshot(rpc_request(
                "/rpc",
                rpc_body("eth_chainId", serde_json::json!([])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["result"], "0x7a69");
    }

    #[tokio::test]
    async fn proxied_upstream_json_rpc_error_passes_through() {
        // Upstream returned a JSON-RPC error object: code and message must pass through
        // verbatim so clients see exactly what the node said.
        let (app, _receiver, asserter) = make_app();
        asserter.push_failure_msg("boom"); // ErrorPayload::internal_error_message → -32603

        let resp = app
            .oneshot(rpc_request(
                "/rpc",
                rpc_body("eth_blockNumber", serde_json::json!([])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32603);
        assert_eq!(json["error"]["message"], "boom");
        assert!(json["result"].is_null());
    }

    #[tokio::test]
    async fn proxied_transport_failure_maps_to_32002() {
        // Transport-level failure (here: the mock's empty response queue, which surfaces as a
        // custom transport error, not a JSON-RPC error response) must map to the documented
        // -32002 "upstream RPC unavailable" so clients can distinguish outage from node error.
        let (app, _receiver, _asserter) = make_app();
        let resp = app
            .oneshot(rpc_request(
                "/rpc",
                rpc_body("eth_blockNumber", serde_json::json!([])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32002);
    }

    #[tokio::test]
    async fn eth_accounts_answers_empty_locally() {
        let (app, _receiver, asserter) = make_app();
        // Nothing pushed: must not hit upstream.
        let _ = asserter;
        let resp = app
            .oneshot(rpc_request(
                "/rpc",
                rpc_body("eth_accounts", serde_json::json!([])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["result"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn signing_methods_rejected_with_guidance() {
        let (app, _receiver, _asserter) = make_app();
        for method in ["eth_sendTransaction", "eth_sign", "eth_signTypedData_v4"] {
            let resp = app
                .clone()
                .oneshot(rpc_request("/rpc", rpc_body(method, serde_json::json!([]))))
                .await
                .unwrap();
            let json = response_json(resp).await;
            assert_eq!(json["error"]["code"], -32601, "{method}");
            assert!(
                json["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("eth_sendRawTransaction"),
                "{method} error should point at eth_sendRawTransaction"
            );
        }
    }

    #[tokio::test]
    async fn non_ethereum_namespaces_are_not_proxied() {
        let (app, _receiver, _asserter) = make_app();
        for method in [
            "debug_traceCall",
            "admin_peers",
            "txpool_status",
            "eth_subscribe",
        ] {
            let resp = app
                .clone()
                .oneshot(rpc_request("/rpc", rpc_body(method, serde_json::json!([]))))
                .await
                .unwrap();
            let json = response_json(resp).await;
            assert_eq!(
                json["error"]["code"], -32601,
                "{method} must not be proxied"
            );
        }
    }

    #[tokio::test]
    async fn unknown_chain_query_param_rejected() {
        let (app, _receiver, _asserter) = make_app();
        let resp = app
            .oneshot(rpc_request(
                "/rpc?chain=l3",
                rpc_body("eth_chainId", serde_json::json!([])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn l2_chain_param_without_l2_provider_errors() {
        let (app, _receiver, _asserter) = make_app();
        let resp = app
            .oneshot(rpc_request(
                "/rpc?chain=l2",
                rpc_body("eth_chainId", serde_json::json!([])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32000);
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not configured")
        );
    }

    // -- JSON-RPC envelope handling ----------------------------------------------------------------

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let (app, _receiver, _asserter) = make_app();
        let resp = app
            .oneshot(rpc_request("/rpc", "{not json".to_string()))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32700);
        assert_eq!(json["id"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn non_object_request_rejected() {
        let (app, _receiver, _asserter) = make_app();
        let resp = app
            .oneshot(rpc_request("/rpc", "\"hello\"".to_string()))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn missing_method_rejected_with_id_echoed() {
        let (app, _receiver, _asserter) = make_app();
        let resp = app
            .oneshot(rpc_request(
                "/rpc",
                r#"{"jsonrpc":"2.0","id":7}"#.to_string(),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32600);
        assert_eq!(json["id"], 7);
    }

    #[tokio::test]
    async fn request_without_id_answers_null_id() {
        let (app, _receiver, _asserter) = make_app();
        let resp = app
            .oneshot(rpc_request(
                "/rpc",
                r#"{"jsonrpc":"2.0","method":"eth_accounts"}"#.to_string(),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["id"], serde_json::Value::Null);
        assert_eq!(json["result"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn string_ids_are_echoed_back() {
        let (app, _receiver, _asserter) = make_app();
        let resp = app
            .oneshot(rpc_request(
                "/rpc",
                r#"{"jsonrpc":"2.0","id":"abc-123","method":"eth_accounts","params":[]}"#
                    .to_string(),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["id"], "abc-123");
    }

    // -- batches -----------------------------------------------------------------------------------

    #[tokio::test]
    async fn batch_preserves_order_and_ids() {
        let (app, _receiver, asserter) = make_app();
        asserter.push_success(&U64::from(0x7a69u64)); // for the eth_chainId entry

        let body = serde_json::json!([
            {"jsonrpc": "2.0", "id": 10, "method": "eth_accounts", "params": []},
            {"jsonrpc": "2.0", "id": 11, "method": "eth_chainId", "params": []},
            {"jsonrpc": "2.0", "id": 12, "method": "no_such_method"},
            "not-an-object"
        ])
        .to_string();

        let resp = app.oneshot(rpc_request("/rpc", body)).await.unwrap();
        let json = response_json(resp).await;
        let entries = json.as_array().expect("batch answer must be an array");
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0]["id"], 10);
        assert_eq!(entries[0]["result"], serde_json::json!([]));
        assert_eq!(entries[1]["id"], 11);
        assert_eq!(entries[1]["result"], "0x7a69");
        assert_eq!(entries[2]["id"], 12);
        assert_eq!(entries[2]["error"]["code"], -32601);
        assert_eq!(entries[3]["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn empty_batch_rejected() {
        let (app, _receiver, _asserter) = make_app();
        let resp = app
            .oneshot(rpc_request("/rpc", "[]".to_string()))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn oversized_batch_rejected() {
        let (app, _receiver, _asserter) = make_app();
        let entries: Vec<_> = (0..=DEFAULT_RPC_MAX_BATCH_SIZE)
            .map(|i| serde_json::json!({"jsonrpc":"2.0","id":i,"method":"eth_accounts"}))
            .collect();
        let resp = app
            .oneshot(rpc_request(
                "/rpc",
                serde_json::to_string(&entries).unwrap(),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32600);
        assert!(json["error"]["message"].as_str().unwrap().contains("batch"));
    }

    // -- auth ---------------------------------------------------------------------------------------

    async fn app_with_store() -> (Router, SqliteStore, crate::creator::TaskReceiver) {
        let (sender, receiver) = crate::creator::task_channel();
        let queue_depth = crate::creator::task_queue_depth();
        let store = SqliteStore::connect_in_memory().await.unwrap();
        let mut state =
            IngressState::without_metrics(sender, queue_depth).with_store(store.clone());
        state.allow_unauthenticated = false;
        let app = build_app().with_state(state);
        (app, store, receiver)
    }

    #[tokio::test]
    async fn rpc_requires_auth_when_store_present() {
        let (app, _store, _rx) = app_with_store().await;
        let resp = app
            .oneshot(rpc_request(
                "/rpc",
                rpc_body("eth_accounts", serde_json::json!([])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32001);
    }

    #[tokio::test]
    async fn rpc_path_key_authenticates() {
        let (app, store, _rx) = app_with_store().await;
        let created = store.create_api_key(None, None).await.unwrap();

        let resp = app
            .clone()
            .oneshot(rpc_request(
                &format!("/rpc/{}", created.key),
                rpc_body("eth_accounts", serde_json::json!([])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["result"], serde_json::json!([]));

        // Wrong path key is rejected.
        let resp = app
            .oneshot(rpc_request(
                "/rpc/gk_wrong",
                rpc_body("eth_accounts", serde_json::json!([])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32001);
    }

    #[tokio::test]
    async fn rpc_bearer_header_authenticates() {
        let (app, store, _rx) = app_with_store().await;
        let created = store.create_api_key(None, None).await.unwrap();

        let req = Request::builder()
            .method(Method::POST)
            .uri("/rpc")
            .header("content-type", "application/json")
            .header("Authorization", format!("Bearer {}", created.key))
            .body(Body::from(rpc_body("eth_accounts", serde_json::json!([]))))
            .unwrap();
        let json = response_json(app.oneshot(req).await.unwrap()).await;
        assert_eq!(json["result"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn rpc_revoked_key_rejected() {
        let (app, store, _rx) = app_with_store().await;
        let created = store.create_api_key(None, None).await.unwrap();
        store.revoke_api_key(&created.id).await.unwrap();

        let resp = app
            .oneshot(rpc_request(
                &format!("/rpc/{}", created.key),
                rpc_body("eth_accounts", serde_json::json!([])),
            ))
            .await
            .unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32001);
    }

    // -- CORS preflight ------------------------------------------------------------------------------

    #[tokio::test]
    async fn cors_preflight_succeeds_without_auth() {
        let (app, _store, _rx) = app_with_store().await;
        let req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/rpc")
            .header("Origin", "https://dapp.example")
            .header("Access-Control-Request-Method", "POST")
            .header(
                "Access-Control-Request-Headers",
                "authorization,content-type",
            )
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status().is_success(),
            "preflight must succeed, got {}",
            resp.status()
        );
        assert!(
            resp.headers().contains_key("access-control-allow-origin"),
            "CORS headers must be present"
        );
        // The Fetch spec excludes Authorization from a `*` wildcard, so the header must be
        // allowed by name or Bearer auth from browsers breaks.
        let allow_headers = resp
            .headers()
            .get("access-control-allow-headers")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(
            allow_headers.contains("authorization"),
            "allow-headers must name authorization explicitly, got {allow_headers:?}"
        );
    }
}

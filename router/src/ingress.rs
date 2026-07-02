use crate::creator::{TaskQueueDepth, TaskSender};
use crate::error::{ApiError, ApiErrorBody, ApiErrorEnvelope, ApiJson, ErrorCode};
use crate::metrics::MetricsCollector;
use crate::store::{ApiKeyMetadata, CreatedApiKey, SqliteStore};
use alloy_primitives::{Address, U256};
use alloy_provider::Provider;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
    routing::{delete, get, post},
};
use gas_killer_common::ChainRole;
use gas_killer_common::ReadOnlyProvider;
use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;
use gas_killer_common::config::CHAIN_DETECTION_ORDER;
use gas_killer_common::task_data::MAX_EVM_TX_CALLDATA_SIZE;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tracing::{info, warn};

/// AVS identity metadata served at `GET /avs-metadata`.
///
/// The EigenLayer indexer fetches the URL passed to `updateAVSMetadataURI`
/// and expects this exact JSON shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AvsMetadata {
    pub name: String,
    pub website: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twitter: Option<String>,
    #[serde(rename = "operatorSets", skip_serializing_if = "Option::is_none")]
    pub operator_sets: Option<Vec<AvsOperatorSetMetadata>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvsOperatorSetMetadata {
    pub name: String,
    pub id: String,
    pub description: String,
    pub software: Vec<AvsOperatorSetSoftware>,
    #[serde(rename = "slashingConditions", skip_serializing_if = "Vec::is_empty")]
    pub slashing_conditions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvsOperatorSetSoftware {
    pub name: String,
    pub description: String,
    pub url: String,
}

#[derive(Clone)]
pub struct IngressState {
    pub sender: TaskSender,
    pub queue_depth: TaskQueueDepth,
    /// Maximum number of tasks allowed in the queue before the ingress starts returning 503.
    pub max_queue_depth: usize,
    pub metrics: Option<Arc<MetricsCollector>>,
    pub providers: Arc<HashMap<ChainRole, ReadOnlyProvider>>,
    /// Shared secret guarding the `/admin/keys` endpoints (`ADMIN_KEY`). `None` disables the
    /// admin API, so keys cannot be managed until it is set.
    pub admin_key: Option<String>,
    pub avs_metadata: AvsMetadata,
    /// Durable SQLite store shared with the orchestrator. `None` when persistence is not
    /// configured (e.g. in tests); the admin API and API-key auth require it.
    pub store: Option<SqliteStore>,
}

impl IngressState {
    pub fn new(
        sender: TaskSender,
        queue_depth: TaskQueueDepth,
        max_queue_depth: usize,
        metrics: Arc<MetricsCollector>,
        providers: HashMap<ChainRole, ReadOnlyProvider>,
        avs_metadata: AvsMetadata,
    ) -> Self {
        Self {
            sender,
            queue_depth,
            max_queue_depth,
            metrics: Some(metrics),
            providers: Arc::new(providers),
            admin_key: None,
            avs_metadata,
            store: None,
        }
    }

    pub fn without_metrics(sender: TaskSender, queue_depth: TaskQueueDepth) -> Self {
        Self {
            sender,
            queue_depth,
            max_queue_depth: gas_killer_common::p2p_message_backlog(),
            metrics: None,
            providers: Arc::new(HashMap::new()),
            admin_key: None,
            avs_metadata: AvsMetadata::default(),
            store: None,
        }
    }

    /// Attaches the durable SQLite store, returning the updated state for chained construction.
    pub fn with_store(mut self, store: SqliteStore) -> Self {
        self.store = Some(store);
        self
    }

    /// Sets the shared secret guarding the admin API, returning the updated state for chained
    /// construction.
    pub fn with_admin_key(mut self, admin_key: Option<String>) -> Self {
        self.admin_key = admin_key;
        self
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Extracts the token from an `Authorization: Bearer <token>` header, if present and valid.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Whether the request carries a Bearer token matching `expected`, compared in constant time
/// so a mismatch's position cannot be inferred from timing. Used for the `ADMIN_KEY` shared
/// secret, which is compared directly rather than looked up by hash.
fn check_bearer_auth(headers: &HeaderMap, expected: &str) -> bool {
    bearer_token(headers)
        .is_some_and(|token| constant_time_eq(token.as_bytes(), expected.as_bytes()))
}

/// Authorizes a task-submission request. When a durable store is configured — always the case
/// in production — a valid, unrevoked API key is required. When no store is configured
/// (development and tests) the endpoint is open.
async fn authorize_task_request(state: &IngressState, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(store) = &state.store else {
        return Ok(());
    };

    if let Some(token) = bearer_token(headers) {
        match store.verify_api_key(token).await {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(e) => {
                tracing::error!(error = %e, "api key verification failed");
                return Err(ApiError::internal("Internal error during authentication"));
            }
        }
    }

    Err(ApiError::unauthorized())
}

/// Guards the `/admin/keys` endpoints with the `ADMIN_KEY` shared secret. Returns a 503 when
/// the admin API is not configured, so an operator who has not set `ADMIN_KEY` gets a clear
/// signal rather than a locked door with no key, and a 401 when the credential is wrong.
fn authorize_admin(state: &IngressState, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(admin_key) = &state.admin_key else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::NotConfigured,
            "Admin API is not configured (set ADMIN_KEY)",
        ));
    };
    if check_bearer_auth(headers, admin_key) {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

/// Borrows the durable store, or returns a 503 when persistence is not configured. The admin
/// API and API-key auth cannot function without it.
fn require_store(state: &IngressState) -> Result<&SqliteStore, ApiError> {
    state.store.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::NotConfigured,
            "Persistence is not configured",
        )
    })
}

/// Onchain validation errors for incoming task requests.
#[derive(Debug)]
pub enum OnchainValidationError {
    ContractNotFound,
    TransitionIndexMismatch {
        provided: u64,
        current: u64,
    },
    BlockHeightInFuture {
        provided: u64,
        current: u64,
    },
    BlockHeightTooStale {
        provided: u64,
        current: u64,
        max_age: u64,
    },
    RpcError(String),
}

impl fmt::Display for OnchainValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContractNotFound => write!(f, "no contract found at target_address on any chain"),
            Self::TransitionIndexMismatch { provided, current } => write!(
                f,
                "transition_index {provided} does not match current onchain state {current}"
            ),
            Self::BlockHeightInFuture { provided, current } => write!(
                f,
                "block_height {provided} is ahead of current chain height {current}"
            ),
            Self::BlockHeightTooStale {
                provided,
                current,
                max_age,
            } => write!(
                f,
                "block_height {provided} is older than the staleness window ({max_age} blocks) relative to current chain height {current}"
            ),
            Self::RpcError(msg) => write!(f, "RPC error during onchain validation: {msg}"),
        }
    }
}

impl std::error::Error for OnchainValidationError {}

impl From<OnchainValidationError> for ApiError {
    fn from(e: OnchainValidationError) -> Self {
        let (status, code) = match e {
            OnchainValidationError::ContractNotFound => {
                (StatusCode::BAD_REQUEST, ErrorCode::ContractNotFound)
            }
            OnchainValidationError::TransitionIndexMismatch { .. } => {
                (StatusCode::BAD_REQUEST, ErrorCode::TransitionMismatch)
            }
            OnchainValidationError::BlockHeightInFuture { .. } => {
                (StatusCode::BAD_REQUEST, ErrorCode::InvalidRequest)
            }
            OnchainValidationError::BlockHeightTooStale { .. } => {
                (StatusCode::BAD_REQUEST, ErrorCode::StaleBlock)
            }
            OnchainValidationError::RpcError(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, ErrorCode::RpcUnavailable)
            }
        };
        // RPC failures are transient and their detail is internal; surface a generic message
        // to clients rather than leaking the upstream error string.
        let message = match code {
            ErrorCode::RpcUnavailable => "Service temporarily unavailable".to_string(),
            _ => e.to_string(),
        };
        ApiError::new(status, code, message)
    }
}

async fn detect_contract_chain<P: Provider + Clone>(
    providers: &HashMap<ChainRole, P>,
    address: Address,
) -> Result<ChainRole, OnchainValidationError> {
    let mut rpc_error: Option<String> = None;
    for &chain_id in &CHAIN_DETECTION_ORDER {
        if let Some(provider) = providers.get(&chain_id) {
            match provider.get_code_at(address).await {
                Ok(code) if !code.is_empty() => return Ok(chain_id),
                Ok(_) => {}
                Err(e) => {
                    warn!(chain = %chain_id, error = %e, "RPC error checking contract code");
                    rpc_error = Some(e.to_string());
                }
            }
        }
    }
    Err(match rpc_error {
        Some(e) => OnchainValidationError::RpcError(e),
        None => OnchainValidationError::ContractNotFound,
    })
}

async fn validate_onchain<P: Provider + Clone>(
    providers: &HashMap<ChainRole, P>,
    body: &GasKillerTaskRequestBody,
) -> Result<(), OnchainValidationError> {
    let chain_id = detect_contract_chain(providers, body.target_address).await?;

    let provider = providers.get(&chain_id).unwrap();

    let current_block = provider
        .get_block_number()
        .await
        .map_err(|e| OnchainValidationError::RpcError(e.to_string()))?;

    if body.block_height > current_block {
        return Err(OnchainValidationError::BlockHeightInFuture {
            provided: body.block_height,
            current: current_block,
        });
    }

    // Reject analyses anchored too far behind head. This is an off-chain policy bound (the
    // contract bounds the operator-set reference block, not this gas-analysis block_height):
    // it keeps requests within the speculative executor cache's window and rejects analyses
    // old enough to likely hit a transition_index mismatch. age == max_age stays valid, matching
    // the contract's `referenceBlockNumber + BLOCK_STALE_MEASURE >= block.number` convention.
    let max_age = gas_killer_common::block_stale_measure();
    if current_block.saturating_sub(body.block_height) > max_age {
        return Err(OnchainValidationError::BlockHeightTooStale {
            provided: body.block_height,
            current: current_block,
            max_age,
        });
    }

    // Only validate the transition index if explicitly provided.
    // None means "auto" — the server resolves the index at dequeue time.
    if let Some(provided) = body.transition_index {
        let contract = GasKillerSDK::new(body.target_address, provider.clone());
        let count = contract
            .stateTransitionCount()
            .call()
            .await
            .map_err(|e| OnchainValidationError::RpcError(e.to_string()))?;
        let current_count: u64 = count.try_into().map_err(|_| {
            OnchainValidationError::RpcError("stateTransitionCount overflow".into())
        })?;

        if provided != current_count {
            return Err(OnchainValidationError::TransitionIndexMismatch {
                provided,
                current: current_count,
            });
        }
    }

    Ok(())
}

/// Validation errors for incoming task requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    ZeroTargetAddress,
    ZeroFromAddress,
    EmptyCallData,
    CallDataTooShort { len: usize },
    CallDataTooLarge { len: usize, max: usize },
    ZeroBlockHeight,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroTargetAddress => write!(f, "target_address is zero"),
            Self::ZeroFromAddress => write!(f, "from_address is zero"),
            Self::EmptyCallData => write!(f, "call_data is empty"),
            Self::CallDataTooShort { len } => {
                write!(f, "call_data too short ({len} bytes, minimum 4)")
            }
            Self::CallDataTooLarge { len, max } => {
                write!(f, "call_data too large ({len} bytes, maximum {max})")
            }
            Self::ZeroBlockHeight => write!(f, "block_height is zero"),
        }
    }
}

impl std::error::Error for ValidationError {}

impl From<ValidationError> for ApiError {
    fn from(e: ValidationError) -> Self {
        let code = match e {
            ValidationError::ZeroTargetAddress | ValidationError::ZeroFromAddress => {
                ErrorCode::InvalidAddress
            }
            ValidationError::CallDataTooLarge { .. } => ErrorCode::CalldataTooLarge,
            ValidationError::EmptyCallData
            | ValidationError::CallDataTooShort { .. }
            | ValidationError::ZeroBlockHeight => ErrorCode::InvalidRequest,
        };
        ApiError::new(StatusCode::BAD_REQUEST, code, e.to_string())
    }
}

/// Deserializes `transition_index` from JSON.
///
/// Accepted values:
/// - `null` or missing field → `None` (auto: server assigns the next slot)
/// - `"auto"` → `None`
/// - non-negative integer → `Some(n)` (explicit fixed index)
///
/// Any other string or non-integer type is rejected with a descriptive error.
fn deserialize_transition_index<'de, D>(d: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{Error, Unexpected, Visitor};

    struct TransitionIndexVisitor;

    impl<'de> Visitor<'de> for TransitionIndexVisitor {
        type Value = Option<u64>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, r#"a non-negative integer, "auto", or null"#)
        }

        fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2: serde::Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(self)
        }

        fn visit_u64<E: Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: Error>(self, v: i64) -> Result<Self::Value, E> {
            u64::try_from(v)
                .map(Some)
                .map_err(|_| E::invalid_value(Unexpected::Signed(v), &self))
        }

        fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
            if v == "auto" {
                Ok(None)
            } else {
                Err(E::invalid_value(Unexpected::Str(v), &self))
            }
        }
    }

    d.deserialize_option(TransitionIndexVisitor)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GasKillerTaskRequestBody {
    pub target_address: Address,
    pub call_data: Vec<u8>,
    /// `None`, JSON `null`, `"auto"`, or a missing field all mean "auto":
    /// the server resolves the next available slot at dequeue time,
    /// enabling safe parallel submissions.
    #[serde(default, deserialize_with = "deserialize_transition_index")]
    pub transition_index: Option<u64>,
    pub from_address: Address,
    pub value: U256,
    pub block_height: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GasKillerTaskRequest {
    pub body: GasKillerTaskRequestBody,
}

impl GasKillerTaskRequest {
    /// Validates all request fields, returning the first error found.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let body = &self.body;

        if body.target_address.is_zero() {
            return Err(ValidationError::ZeroTargetAddress);
        }
        if body.from_address.is_zero() {
            return Err(ValidationError::ZeroFromAddress);
        }
        if body.call_data.is_empty() {
            return Err(ValidationError::EmptyCallData);
        }
        // minimum 4 bytes for function selector
        if body.call_data.len() < 4 {
            return Err(ValidationError::CallDataTooShort {
                len: body.call_data.len(),
            });
        }
        // maximum 128 KB for call data
        if body.call_data.len() > MAX_EVM_TX_CALLDATA_SIZE {
            return Err(ValidationError::CallDataTooLarge {
                len: body.call_data.len(),
                max: MAX_EVM_TX_CALLDATA_SIZE,
            });
        }
        if body.block_height == 0 {
            return Err(ValidationError::ZeroBlockHeight);
        }

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GasKillerTaskResponse {
    pub success: bool,
    pub message: String,
}

// Handler for POST /trigger
pub async fn trigger_task_handler(
    State(state): State<IngressState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<GasKillerTaskRequest>,
) -> Result<(StatusCode, Json<GasKillerTaskResponse>), ApiError> {
    authorize_task_request(&state, &headers).await?;

    // Load-shed before any validation work. Onchain validation costs multiple RPC
    // round-trips, so rejecting at-capacity requests up front keeps an overloaded
    // service from amplifying its own load; a request that would have failed
    // validation gets a 503 instead of a 400 while the queue is full.
    let current_depth = state.queue_depth.load(Ordering::Relaxed);
    if current_depth >= state.max_queue_depth {
        warn!(
            queue_depth = current_depth,
            max_queue_depth = state.max_queue_depth,
            "Task rejected: queue at capacity"
        );
        if let Some(m) = &state.metrics {
            m.ingress_at_capacity.inc();
        }
        return Err(ApiError::queue_full(
            "Service at capacity, please try again in a few minutes",
        ));
    }

    if let Err(e) = request.validate() {
        warn!(
            target_address = %request.body.target_address,
            from_address = %request.body.from_address,
            block_height = request.body.block_height,
            error = %e,
            "Task rejected"
        );
        if let Some(m) = &state.metrics {
            m.ingress_rejected.inc();
        }
        return Err(e.into());
    }

    if !state.providers.is_empty()
        && let Err(e) = validate_onchain(&*state.providers, &request.body).await
    {
        warn!(
            target_address = %request.body.target_address,
            from_address = %request.body.from_address,
            block_height = request.body.block_height,
            transition_index = ?request.body.transition_index,
            error = %e,
            "Task rejected (onchain)"
        );
        if let Some(m) = &state.metrics {
            m.ingress_rejected.inc();
        }
        return Err(e.into());
    }

    info!(
        target_address = %request.body.target_address,
        from_address = %request.body.from_address,
        block_height = request.body.block_height,
        call_data_len = request.body.call_data.len(),
        "Task accepted"
    );
    let depth = state.queue_depth.fetch_add(1, Ordering::Relaxed) + 1;
    if state.sender.send(request).is_err() {
        state.queue_depth.fetch_sub(1, Ordering::Relaxed);
        tracing::error!("task channel closed, dropping request");
        return Err(ApiError::internal("Internal error: task queue unavailable"));
    }
    if let Some(m) = &state.metrics {
        m.ingress_accepted.inc();
        m.task_queue_depth.set(depth as i64);
    }
    Ok((
        StatusCode::OK,
        Json(GasKillerTaskResponse {
            success: true,
            message: "Task queued".to_string(),
        }),
    ))
}

/// Request body for `POST /admin/keys`. The whole body is optional; an empty body creates an
/// Current unix time in seconds. Falls back to 0 if the system clock is before the epoch (never
/// in practice), which only makes the past-expiry check maximally permissive.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// unlabeled key. `invalid_at` is an optional unix timestamp at which the key expires; omit or
/// send `null` for a key that never expires. It must be in the future.
#[derive(Debug, Default, Deserialize)]
pub struct CreateApiKeyRequest {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub invalid_at: Option<i64>,
}

/// `POST /admin/keys` — issues a new API key. Admin-only. The response carries the raw key
/// value exactly once; it is not persisted in the clear and cannot be retrieved later.
async fn create_api_key_handler(
    State(state): State<IngressState>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Result<(StatusCode, Json<CreatedApiKey>), ApiError> {
    authorize_admin(&state, &headers)?;
    let store = require_store(&state)?;

    // The body is an optional JSON object; an empty body means an unlabeled, never-expiring key.
    let request = if body.is_empty() {
        CreateApiKeyRequest::default()
    } else {
        serde_json::from_slice::<CreateApiKeyRequest>(&body).map_err(|e| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidRequest,
                format!("invalid request body: {e}"),
            )
        })?
    };

    // Blank labels are normalized away so listings never show empty strings.
    let label = request.label.filter(|l| !l.trim().is_empty());

    // Reject an expiry that is already in the past — such a key would be born dead.
    if let Some(invalid_at) = request.invalid_at
        && invalid_at <= unix_now()
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::InvalidRequest,
            "invalid_at must be a unix timestamp in the future",
        ));
    }

    let created = store
        .create_api_key(label, request.invalid_at)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "failed to create api key");
            ApiError::internal("Failed to create API key")
        })?;
    info!(id = %created.id, "api key created");
    Ok((StatusCode::CREATED, Json(created)))
}

/// `GET /admin/keys` — lists metadata for active keys. Admin-only. Never returns key values.
async fn list_api_keys_handler(
    State(state): State<IngressState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ApiKeyMetadata>>, ApiError> {
    authorize_admin(&state, &headers)?;
    let store = require_store(&state)?;

    let keys = store.list_api_keys().await.map_err(|e| {
        tracing::error!(error = %e, "failed to list api keys");
        ApiError::internal("Failed to list API keys")
    })?;
    Ok(Json(keys))
}

/// `DELETE /admin/keys/:id` — revokes a key, taking effect immediately. Admin-only. Returns
/// 204 on success and 404 when no active key with that id exists.
async fn revoke_api_key_handler(
    State(state): State<IngressState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    authorize_admin(&state, &headers)?;
    let store = require_store(&state)?;

    let revoked = store.revoke_api_key(&id).await.map_err(|e| {
        tracing::error!(error = %e, "failed to revoke api key");
        ApiError::internal("Failed to revoke API key")
    })?;

    if revoked {
        info!(id = %id, "api key revoked");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
            "API key not found",
        ))
    }
}

async fn healthz_handler() -> StatusCode {
    StatusCode::OK
}

async fn avs_metadata_handler(State(state): State<IngressState>) -> Json<AvsMetadata> {
    Json(state.avs_metadata.clone())
}

/// Gives axum's built-in 404 (no matching route) and 405 (method not allowed) responses the same
/// `{ "error": { "code", "message" } }` body as handler errors, so every error the ingress emits
/// shares one contract.
///
/// Only framework-generated errors are rewritten: a 404/405 with no `Content-Type`. Handler errors
/// and the API's own envelopes always set `application/json`, so they pass through untouched. The
/// body is replaced in place rather than rebuilt, preserving the headers axum computed — in
/// particular the `Allow` header that a spec-compliant 405 must carry.
async fn wrap_framework_error(mut resp: Response) -> Response {
    let status = resp.status();
    let is_framework_error = matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ) && resp.headers().get(header::CONTENT_TYPE).is_none();
    if !is_framework_error {
        return resp;
    }

    let (code, message) = if status == StatusCode::METHOD_NOT_ALLOWED {
        (ErrorCode::MethodNotAllowed, "Method not allowed")
    } else {
        (ErrorCode::NotFound, "Not found")
    };
    let envelope = ApiErrorEnvelope {
        error: ApiErrorBody {
            code,
            message: message.to_string(),
        },
    };
    let body = match serde_json::to_vec(&envelope) {
        Ok(bytes) => bytes,
        // A fixed-shape struct cannot realistically fail to serialize; if it somehow does, leave
        // axum's original response untouched rather than panicking.
        Err(_) => return resp,
    };

    let len = body.len();
    *resp.body_mut() = axum::body::Body::from(body);
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    resp
}

pub fn build_app() -> Router<IngressState> {
    Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/avs-metadata", get(avs_metadata_handler))
        .route("/trigger", post(trigger_task_handler))
        .route(
            "/admin/keys",
            post(create_api_key_handler).get(list_api_keys_handler),
        )
        .route("/admin/keys/:id", delete(revoke_api_key_handler))
        .layer(axum::middleware::map_response(wrap_framework_error))
}

// Start the HTTP server in a background task
pub async fn start_gas_killer_http_server(state: IngressState, addr: &str) {
    let app = build_app().with_state(state);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind HTTP server");
    info!("Creator HTTP server running on {}", addr);
    axum::serve(listener, app)
        .await
        .expect("HTTP server failed");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a valid request that passes all checks.
    fn valid_request() -> GasKillerTaskRequest {
        GasKillerTaskRequest {
            body: GasKillerTaskRequestBody {
                target_address: "0x0000000000000000000000000000000000000001"
                    .parse()
                    .unwrap(),
                from_address: "0x0000000000000000000000000000000000000002"
                    .parse()
                    .unwrap(),
                call_data: vec![0xAB, 0xCD, 0xEF, 0x01], // 4-byte selector
                transition_index: Some(0),
                value: U256::ZERO,
                block_height: 1,
            },
        }
    }

    // -- baseline --

    #[test]
    fn test_valid_request_passes() {
        assert!(valid_request().validate().is_ok());
    }

    // -- individual validation checks --

    #[test]
    fn test_zero_target_address() {
        let mut req = valid_request();
        req.body.target_address = Address::ZERO;
        assert_eq!(
            req.validate().unwrap_err(),
            ValidationError::ZeroTargetAddress
        );
    }

    #[test]
    fn test_zero_from_address() {
        let mut req = valid_request();
        req.body.from_address = Address::ZERO;
        assert_eq!(
            req.validate().unwrap_err(),
            ValidationError::ZeroFromAddress
        );
    }

    #[test]
    fn test_empty_call_data() {
        let mut req = valid_request();
        req.body.call_data = vec![];
        assert_eq!(req.validate().unwrap_err(), ValidationError::EmptyCallData);
    }

    #[test]
    fn test_call_data_too_short() {
        let mut req = valid_request();
        req.body.call_data = vec![0x01, 0x02, 0x03]; // 3 bytes, need 4
        assert_eq!(
            req.validate().unwrap_err(),
            ValidationError::CallDataTooShort { len: 3 }
        );
    }

    #[test]
    fn test_call_data_at_exactly_4_bytes() {
        let mut req = valid_request();
        req.body.call_data = vec![0x01, 0x02, 0x03, 0x04];
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_call_data_too_large() {
        let mut req = valid_request();
        req.body.call_data = vec![0u8; MAX_EVM_TX_CALLDATA_SIZE + 1];
        assert_eq!(
            req.validate().unwrap_err(),
            ValidationError::CallDataTooLarge {
                len: MAX_EVM_TX_CALLDATA_SIZE + 1,
                max: MAX_EVM_TX_CALLDATA_SIZE,
            }
        );
    }

    #[test]
    fn test_call_data_at_max_size() {
        let mut req = valid_request();
        req.body.call_data = vec![0u8; MAX_EVM_TX_CALLDATA_SIZE];
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_zero_block_height() {
        let mut req = valid_request();
        req.body.block_height = 0;
        assert_eq!(
            req.validate().unwrap_err(),
            ValidationError::ZeroBlockHeight
        );
    }

    // -- Display --

    #[test]
    fn test_validation_error_display() {
        assert_eq!(
            ValidationError::ZeroTargetAddress.to_string(),
            "target_address is zero"
        );
        assert_eq!(
            ValidationError::ZeroFromAddress.to_string(),
            "from_address is zero"
        );
        assert_eq!(
            ValidationError::EmptyCallData.to_string(),
            "call_data is empty"
        );
        assert_eq!(
            ValidationError::CallDataTooShort { len: 2 }.to_string(),
            "call_data too short (2 bytes, minimum 4)"
        );
        assert_eq!(
            ValidationError::CallDataTooLarge {
                len: 200_000,
                max: 131_072
            }
            .to_string(),
            "call_data too large (200000 bytes, maximum 131072)"
        );
        assert_eq!(
            ValidationError::ZeroBlockHeight.to_string(),
            "block_height is zero"
        );
    }

    // -- priority ordering --

    #[test]
    fn test_first_failure_wins() {
        // Request that fails multiple checks: target=zero, from=zero, call_data empty, block=0
        let req = GasKillerTaskRequest {
            body: GasKillerTaskRequestBody {
                target_address: Address::ZERO,
                from_address: Address::ZERO,
                call_data: vec![],
                transition_index: Some(u64::MAX),
                value: U256::MAX,
                block_height: 0,
            },
        };
        // First check is target_address
        assert_eq!(
            req.validate().unwrap_err(),
            ValidationError::ZeroTargetAddress
        );
    }

    // -- HTTP handler integration tests --
    //
    // These tests call trigger_task_handler through the real Axum router using
    // tower::ServiceExt::oneshot, so they exercise JSON extraction, status codes,
    // and queue interaction end-to-end without binding a port.

    mod http {
        use super::*;
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::util::ServiceExt; // for `oneshot`

        fn make_app() -> (Router, crate::creator::TaskReceiver) {
            let (sender, receiver) = crate::creator::task_channel();
            let queue_depth = crate::creator::task_queue_depth();
            let state = IngressState::without_metrics(sender, queue_depth);
            let app = build_app().with_state(state);
            (app, receiver)
        }

        /// Builds an app backed by an in-memory store, optionally with an admin key. Returns the
        /// store handle so tests can mint/revoke keys directly, and the receiver so accepted
        /// tasks have somewhere to land (a dropped receiver would make `/trigger` fail on send).
        async fn make_app_with_store(
            admin_key: Option<&str>,
        ) -> (Router, SqliteStore, crate::creator::TaskReceiver) {
            let (sender, receiver) = crate::creator::task_channel();
            let queue_depth = crate::creator::task_queue_depth();
            let store = SqliteStore::connect_in_memory()
                .await
                .expect("in-memory store should open");
            let state = IngressState::without_metrics(sender, queue_depth)
                .with_store(store.clone())
                .with_admin_key(admin_key.map(str::to_string));
            let app = build_app().with_state(state);
            (app, store, receiver)
        }

        fn admin_request(
            method: Method,
            uri: &str,
            token: Option<&str>,
            body: &str,
        ) -> Request<Body> {
            let mut builder = Request::builder().method(method).uri(uri);
            if let Some(token) = token {
                builder = builder.header("Authorization", format!("Bearer {token}"));
            }
            builder
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        }

        fn bearer_request(body: &str, token: &str) -> Request<Body> {
            Request::builder()
                .method(Method::POST)
                .uri("/trigger")
                .header("content-type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap()
        }

        fn json_request(body: &str) -> Request<Body> {
            Request::builder()
                .method(Method::POST)
                .uri("/trigger")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        }

        fn valid_body() -> String {
            serde_json::json!({
                "body": {
                    "target_address": "0x0000000000000000000000000000000000000001",
                    "from_address":   "0x0000000000000000000000000000000000000002",
                    "call_data":      [0xAB, 0xCD, 0xEF, 0x01],
                    "transition_index": 0,
                    "value": "0x0",
                    "block_height": 1
                }
            })
            .to_string()
        }

        async fn response_body(resp: axum::response::Response) -> GasKillerTaskResponse {
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            serde_json::from_slice(&bytes).unwrap()
        }

        async fn error_envelope(resp: axum::response::Response) -> crate::error::ApiErrorEnvelope {
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            serde_json::from_slice(&bytes)
                .expect("error response should deserialize as the ApiError envelope")
        }

        #[tokio::test]
        async fn test_healthz_returns_200() {
            let (app, _queue) = make_app();
            let req = Request::builder()
                .method(Method::GET)
                .uri("/healthz")
                .body(Body::empty())
                .unwrap();

            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn test_valid_request_returns_200_and_queues_task() {
            let (app, mut receiver) = make_app();
            let resp = app.oneshot(json_request(&valid_body())).await.unwrap();

            assert_eq!(resp.status(), StatusCode::OK);
            let body = response_body(resp).await;
            assert!(body.success);
            assert_eq!(body.message, "Task queued");
            assert!(
                receiver.try_recv().is_ok(),
                "task should have been pushed to queue"
            );
        }

        #[tokio::test]
        async fn test_zero_target_address_returns_400() {
            let (app, _queue) = make_app();
            let payload = serde_json::json!({
                "body": {
                    "target_address": "0x0000000000000000000000000000000000000000",
                    "from_address":   "0x0000000000000000000000000000000000000002",
                    "call_data":      [0xAB, 0xCD, 0xEF, 0x01],
                    "transition_index": 0,
                    "value": "0x0",
                    "block_height": 1
                }
            })
            .to_string();

            let resp = app.oneshot(json_request(&payload)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::InvalidAddress);
            assert!(body.error.message.contains("target_address is zero"));
        }

        #[tokio::test]
        async fn test_zero_from_address_returns_400() {
            let (app, _queue) = make_app();
            let payload = serde_json::json!({
                "body": {
                    "target_address": "0x0000000000000000000000000000000000000001",
                    "from_address":   "0x0000000000000000000000000000000000000000",
                    "call_data":      [0xAB, 0xCD, 0xEF, 0x01],
                    "transition_index": 0,
                    "value": "0x0",
                    "block_height": 1
                }
            })
            .to_string();

            let resp = app.oneshot(json_request(&payload)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::InvalidAddress);
            assert!(body.error.message.contains("from_address is zero"));
        }

        #[tokio::test]
        async fn test_empty_call_data_returns_400() {
            let (app, _queue) = make_app();
            let payload = serde_json::json!({
                "body": {
                    "target_address": "0x0000000000000000000000000000000000000001",
                    "from_address":   "0x0000000000000000000000000000000000000002",
                    "call_data":      [],
                    "transition_index": 0,
                    "value": "0x0",
                    "block_height": 1
                }
            })
            .to_string();

            let resp = app.oneshot(json_request(&payload)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::InvalidRequest);
            assert!(body.error.message.contains("call_data is empty"));
        }

        #[tokio::test]
        async fn test_call_data_too_short_returns_400() {
            let (app, _queue) = make_app();
            let payload = serde_json::json!({
                "body": {
                    "target_address": "0x0000000000000000000000000000000000000001",
                    "from_address":   "0x0000000000000000000000000000000000000002",
                    "call_data":      [0x01, 0x02, 0x03],
                    "transition_index": 0,
                    "value": "0x0",
                    "block_height": 1
                }
            })
            .to_string();

            let resp = app.oneshot(json_request(&payload)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::InvalidRequest);
            assert!(body.error.message.contains("call_data too short"));
        }

        #[tokio::test]
        async fn test_call_data_too_large_returns_400() {
            let (app, _queue) = make_app();
            let oversized = vec![0u8; MAX_EVM_TX_CALLDATA_SIZE + 1];
            let payload = serde_json::json!({
                "body": {
                    "target_address": "0x0000000000000000000000000000000000000001",
                    "from_address":   "0x0000000000000000000000000000000000000002",
                    "call_data":      oversized,
                    "transition_index": 0,
                    "value": "0x0",
                    "block_height": 1
                }
            })
            .to_string();

            let resp = app.oneshot(json_request(&payload)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::CalldataTooLarge);
            assert!(body.error.message.contains("call_data too large"));
        }

        #[tokio::test]
        async fn test_zero_block_height_returns_400() {
            let (app, _queue) = make_app();
            let payload = serde_json::json!({
                "body": {
                    "target_address": "0x0000000000000000000000000000000000000001",
                    "from_address":   "0x0000000000000000000000000000000000000002",
                    "call_data":      [0xAB, 0xCD, 0xEF, 0x01],
                    "transition_index": 0,
                    "value": "0x0",
                    "block_height": 0
                }
            })
            .to_string();

            let resp = app.oneshot(json_request(&payload)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::InvalidRequest);
            assert!(body.error.message.contains("block_height is zero"));
        }

        #[tokio::test]
        async fn test_malformed_json_returns_4xx() {
            let (app, _queue) = make_app();
            let req = Request::builder()
                .method(Method::POST)
                .uri("/trigger")
                .header("content-type", "application/json")
                .body(Body::from("not json at all {{{"))
                .unwrap();

            let resp = app.oneshot(req).await.unwrap();
            assert!(
                resp.status().is_client_error(),
                "malformed JSON should return 4xx, got {}",
                resp.status()
            );
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::InvalidRequest);
        }

        #[tokio::test]
        async fn test_missing_required_field_returns_422() {
            let (app, _queue) = make_app();
            // `block_height` is missing
            let payload = serde_json::json!({
                "body": {
                    "target_address": "0x0000000000000000000000000000000000000001",
                    "from_address":   "0x0000000000000000000000000000000000000002",
                    "call_data":      [0xAB, 0xCD, 0xEF, 0x01],
                    "transition_index": 0,
                    "value": "0x0"
                }
            })
            .to_string();

            let resp = app.oneshot(json_request(&payload)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::InvalidRequest);
        }

        #[tokio::test]
        async fn test_empty_body_returns_4xx() {
            let (app, _queue) = make_app();
            let req = Request::builder()
                .method(Method::POST)
                .uri("/trigger")
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();

            let resp = app.oneshot(req).await.unwrap();
            assert!(
                resp.status().is_client_error(),
                "empty body should return 4xx, got {}",
                resp.status()
            );
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::InvalidRequest);
        }

        #[tokio::test]
        async fn test_wrong_method_returns_405() {
            let (app, _queue) = make_app();
            let req = Request::builder()
                .method(Method::GET)
                .uri("/trigger")
                .body(Body::empty())
                .unwrap();

            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
            // A spec-compliant 405 must advertise the supported methods; rewriting the body into
            // the error envelope must not drop the Allow header axum computes for the route.
            let allow = resp
                .headers()
                .get(axum::http::header::ALLOW)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert!(
                allow.contains("POST"),
                "Allow should list POST, got {allow:?}"
            );
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::MethodNotAllowed);
        }

        #[tokio::test]
        async fn test_unknown_path_returns_404() {
            let (app, _queue) = make_app();
            let req = Request::builder()
                .method(Method::GET)
                .uri("/does-not-exist")
                .body(Body::empty())
                .unwrap();

            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::NotFound);
        }

        // Pins the documented contract of `wrap_framework_error`: a handler that emits a bare,
        // bodyless `StatusCode::NOT_FOUND`/`METHOD_NOT_ALLOWED` (no Content-Type) is rewritten into
        // the error envelope, while a handler that already returns an envelope (Content-Type
        // application/json) keeps its specific status, code, and message untouched. Guards against a
        // future handler's error shape silently diverging from — or being clobbered by — the layer.
        #[tokio::test]
        async fn test_bare_status_handler_is_wrapped_but_envelope_handler_is_preserved() {
            use crate::error::ErrorCode;

            let app: Router = Router::new()
                .route("/bare", get(|| async { StatusCode::NOT_FOUND }))
                .route(
                    "/typed",
                    get(|| async {
                        ApiError::new(
                            StatusCode::NOT_FOUND,
                            ErrorCode::NotFound,
                            "widget 7 not found",
                        )
                    }),
                )
                .layer(axum::middleware::map_response(wrap_framework_error));

            // Bare StatusCode from a handler → wrapped into the generic envelope.
            let bare = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/bare")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(bare.status(), StatusCode::NOT_FOUND);
            let bare_body = error_envelope(bare).await;
            assert_eq!(bare_body.error.code, ErrorCode::NotFound);
            assert_eq!(bare_body.error.message, "Not found");

            // Handler-built envelope (application/json) → passed through with its specific message.
            let typed = app
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/typed")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(typed.status(), StatusCode::NOT_FOUND);
            let typed_body = error_envelope(typed).await;
            assert_eq!(typed_body.error.code, ErrorCode::NotFound);
            assert_eq!(typed_body.error.message, "widget 7 not found");
        }

        #[tokio::test]
        async fn test_valid_request_does_not_leave_extra_tasks() {
            // Two sequential valid requests → queue should hold exactly two tasks
            let (sender, mut receiver) = crate::creator::task_channel();
            let queue_depth = crate::creator::task_queue_depth();
            let app1 = build_app().with_state(IngressState::without_metrics(
                sender.clone(),
                queue_depth.clone(),
            ));
            let app2 = build_app().with_state(IngressState::without_metrics(
                sender.clone(),
                queue_depth.clone(),
            ));

            app1.oneshot(json_request(&valid_body())).await.unwrap();
            app2.oneshot(json_request(&valid_body())).await.unwrap();

            assert!(receiver.try_recv().is_ok());
            assert!(receiver.try_recv().is_ok());
            assert!(
                receiver.try_recv().is_err(),
                "queue should be empty after two recvs"
            );
        }

        // -- auth tests --
        //
        // Task-submission auth against the store is covered in the admin/API-key section below
        // (valid, revoked, and unknown keys). These cover the store-absent development path and
        // the always-open utility endpoints.

        #[tokio::test]
        async fn test_no_store_configured_accepts_unauthenticated() {
            // With no durable store (development / tests) the endpoint is open.
            let (app, _queue) = make_app();
            let resp = app.oneshot(json_request(&valid_body())).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn test_healthz_unauthenticated_with_store_configured() {
            let (app, _store, _rx) = make_app_with_store(Some("admin-secret")).await;
            let req = Request::builder()
                .method(Method::GET)
                .uri("/healthz")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn test_avs_metadata_returns_200_with_valid_json() {
            let (sender, _receiver) = crate::creator::task_channel();
            let queue_depth = crate::creator::task_queue_depth();
            let mut state = IngressState::without_metrics(sender, queue_depth);
            state.avs_metadata = AvsMetadata {
                name: "Gas Killer".to_string(),
                website: "https://gaskiller.xyz".to_string(),
                description: "Test AVS".to_string(),
                logo: Some("https://example.com/logo.png".to_string()),
                twitter: Some("https://x.com/gaskiller".to_string()),
                operator_sets: None,
            };
            let app = build_app().with_state(state);
            let req = Request::builder()
                .method(Method::GET)
                .uri("/avs-metadata")
                .body(Body::empty())
                .unwrap();

            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let metadata: AvsMetadata =
                serde_json::from_slice(&bytes).expect("response should be valid AvsMetadata JSON");
            assert_eq!(metadata.name, "Gas Killer");
            assert_eq!(metadata.website, "https://gaskiller.xyz");
        }

        #[tokio::test]
        async fn test_avs_metadata_accessible_without_auth_with_store_configured() {
            let (app, _store, _rx) = make_app_with_store(Some("admin-secret")).await;
            let req = Request::builder()
                .method(Method::GET)
                .uri("/avs-metadata")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        // -- admin API + API-key auth tests --

        async fn created_key_json(resp: axum::response::Response) -> serde_json::Value {
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            serde_json::from_slice(&bytes).expect("create response should be JSON")
        }

        #[tokio::test]
        async fn admin_create_returns_503_when_admin_key_unset() {
            let (app, _store, _rx) = make_app_with_store(None).await;
            let req = admin_request(Method::POST, "/admin/keys", None, "{}");
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::NotConfigured);
        }

        #[tokio::test]
        async fn admin_create_rejects_missing_or_wrong_credential() {
            let (app, _store, _rx) = make_app_with_store(Some("admin-secret")).await;
            // No credential.
            let resp = app
                .clone()
                .oneshot(admin_request(Method::POST, "/admin/keys", None, "{}"))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
            // Wrong credential.
            let resp = app
                .oneshot(admin_request(
                    Method::POST,
                    "/admin/keys",
                    Some("wrong"),
                    "{}",
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn admin_create_issues_key_with_prefix() {
            let (app, _store, _rx) = make_app_with_store(Some("admin-secret")).await;
            let req = admin_request(
                Method::POST,
                "/admin/keys",
                Some("admin-secret"),
                r#"{"label":"client-a"}"#,
            );
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
            let json = created_key_json(resp).await;
            assert!(
                json["key"].as_str().unwrap().starts_with("gk_"),
                "created key should carry the gk_ prefix"
            );
            assert_eq!(json["label"], "client-a");
            assert!(json["id"].as_str().is_some());
            assert!(json["created_at"].as_i64().unwrap() > 0);
            assert!(json["invalid_at"].is_null(), "no expiry was requested");
        }

        #[tokio::test]
        async fn admin_create_honors_future_expiry() {
            let (app, _store, _rx) = make_app_with_store(Some("admin-secret")).await;
            // Year 2100 — comfortably in the future.
            let req = admin_request(
                Method::POST,
                "/admin/keys",
                Some("admin-secret"),
                r#"{"invalid_at":4102444800}"#,
            );
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
            let json = created_key_json(resp).await;
            assert_eq!(json["invalid_at"].as_i64(), Some(4_102_444_800));
        }

        #[tokio::test]
        async fn admin_create_rejects_past_expiry() {
            let (app, _store, _rx) = make_app_with_store(Some("admin-secret")).await;
            // A 1970 timestamp is already in the past.
            let req = admin_request(
                Method::POST,
                "/admin/keys",
                Some("admin-secret"),
                r#"{"invalid_at":1}"#,
            );
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::InvalidRequest);
        }

        #[tokio::test]
        async fn admin_create_accepts_empty_body_as_unlabeled() {
            let (app, _store, _rx) = make_app_with_store(Some("admin-secret")).await;
            let req = Request::builder()
                .method(Method::POST)
                .uri("/admin/keys")
                .header("Authorization", "Bearer admin-secret")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
            let json = created_key_json(resp).await;
            assert!(json["label"].is_null(), "empty body should yield no label");
        }

        #[tokio::test]
        async fn admin_list_returns_metadata_without_key_value() {
            let (app, store, _rx) = make_app_with_store(Some("admin-secret")).await;
            let created = store
                .create_api_key(Some("client-a".to_string()), None)
                .await
                .unwrap();

            let resp = app
                .oneshot(admin_request(
                    Method::GET,
                    "/admin/keys",
                    Some("admin-secret"),
                    "",
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let entries = list.as_array().expect("list should be a JSON array");
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0]["id"], created.id);
            assert_eq!(entries[0]["label"], "client-a");
            assert!(
                entries[0].get("key").is_none(),
                "listing must never expose the key value"
            );
            assert!(
                entries[0].get("key_hash").is_none(),
                "listing must never expose the key hash"
            );
        }

        #[tokio::test]
        async fn admin_revoke_returns_204_then_404() {
            let (app, store, _rx) = make_app_with_store(Some("admin-secret")).await;
            let created = store.create_api_key(None, None).await.unwrap();

            let uri = format!("/admin/keys/{}", created.id);
            let resp = app
                .clone()
                .oneshot(admin_request(
                    Method::DELETE,
                    &uri,
                    Some("admin-secret"),
                    "",
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NO_CONTENT);

            // Revoking again reports 404 — no active key with that id remains.
            let resp = app
                .oneshot(admin_request(
                    Method::DELETE,
                    &uri,
                    Some("admin-secret"),
                    "",
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn trigger_accepts_valid_api_key() {
            let (app, store, mut rx) = make_app_with_store(None).await;
            let created = store.create_api_key(None, None).await.unwrap();

            let resp = app
                .oneshot(bearer_request(&valid_body(), &created.key))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert!(rx.try_recv().is_ok(), "valid key should queue the task");
        }

        #[tokio::test]
        async fn trigger_rejects_revoked_api_key() {
            let (app, store, _rx) = make_app_with_store(None).await;
            let created = store.create_api_key(None, None).await.unwrap();
            store.revoke_api_key(&created.id).await.unwrap();

            let resp = app
                .oneshot(bearer_request(&valid_body(), &created.key))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn trigger_rejects_unknown_key_when_store_present() {
            let (app, _store, _rx) = make_app_with_store(None).await;
            let resp = app
                .oneshot(bearer_request(&valid_body(), "gk_unknown"))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn trigger_without_token_rejected_when_store_present() {
            let (app, _store, _rx) = make_app_with_store(None).await;
            let resp = app.oneshot(json_request(&valid_body())).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }

        // -- queue capacity tests --

        #[tokio::test]
        async fn test_full_queue_returns_503() {
            let (sender, _receiver) = crate::creator::task_channel();
            let queue_depth = crate::creator::task_queue_depth();
            let mut state = IngressState::without_metrics(sender, queue_depth.clone());
            state.max_queue_depth = 1;
            queue_depth.store(1, std::sync::atomic::Ordering::Relaxed);
            let app = build_app().with_state(state);

            let resp = app.oneshot(json_request(&valid_body())).await.unwrap();
            assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
            let body = error_envelope(resp).await;
            assert_eq!(body.error.code, crate::error::ErrorCode::QueueFull);
            assert!(body.error.message.to_lowercase().contains("capacity"));
        }

        #[tokio::test]
        async fn test_full_queue_increments_at_capacity_metric() {
            let (sender, _receiver) = crate::creator::task_channel();
            let queue_depth = crate::creator::task_queue_depth();
            let metrics = Arc::new(MetricsCollector::new());
            let mut state = IngressState::without_metrics(sender, queue_depth.clone());
            state.metrics = Some(Arc::clone(&metrics));
            state.max_queue_depth = 1;
            queue_depth.store(1, std::sync::atomic::Ordering::Relaxed);
            let app = build_app().with_state(state);

            let resp = app.oneshot(json_request(&valid_body())).await.unwrap();
            assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(metrics.ingress_at_capacity.get(), 1);
            assert_eq!(metrics.ingress_rejected.get(), 0);
        }

        #[tokio::test]
        async fn test_queue_one_below_limit_still_accepts() {
            let (sender, _receiver) = crate::creator::task_channel();
            let queue_depth = crate::creator::task_queue_depth();
            let mut state = IngressState::without_metrics(sender, queue_depth.clone());
            state.max_queue_depth = 2;
            queue_depth.store(1, std::sync::atomic::Ordering::Relaxed);
            let app = build_app().with_state(state);

            let resp = app.oneshot(json_request(&valid_body())).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn test_rejected_request_does_not_enqueue() {
            let (app, mut receiver) = make_app();
            let payload = serde_json::json!({
                "body": {
                    "target_address": "0x0000000000000000000000000000000000000000",
                    "from_address":   "0x0000000000000000000000000000000000000002",
                    "call_data":      [0xAB, 0xCD, 0xEF, 0x01],
                    "transition_index": 0,
                    "value": "0x0",
                    "block_height": 1
                }
            })
            .to_string();

            app.oneshot(json_request(&payload)).await.unwrap();
            assert!(
                receiver.try_recv().is_err(),
                "invalid task must not be pushed to queue"
            );
        }
    }

    // -- transition_index deserialization tests --

    mod transition_index_deser {
        use crate::ingress::GasKillerTaskRequestBody;

        fn deser(json: &str) -> Result<Option<u64>, serde_json::Error> {
            let body: GasKillerTaskRequestBody = serde_json::from_str(json)?;
            Ok(body.transition_index)
        }

        fn body_with(transition_index_json: &str) -> String {
            format!(
                r#"{{"target_address":"0x0000000000000000000000000000000000000001","call_data":[1,2,3,4],"from_address":"0x0000000000000000000000000000000000000002","value":"0x0","block_height":1,"transition_index":{}}}"#,
                transition_index_json
            )
        }

        #[test]
        fn test_numeric_gives_some() {
            assert_eq!(deser(&body_with("42")).unwrap(), Some(42));
        }

        #[test]
        fn test_zero_gives_some_zero() {
            assert_eq!(deser(&body_with("0")).unwrap(), Some(0));
        }

        #[test]
        fn test_null_gives_none() {
            assert_eq!(deser(&body_with("null")).unwrap(), None);
        }

        #[test]
        fn test_auto_string_gives_none() {
            assert_eq!(deser(&body_with(r#""auto""#)).unwrap(), None);
        }

        #[test]
        fn test_missing_field_gives_none() {
            let json = r#"{"target_address":"0x0000000000000000000000000000000000000001","call_data":[1,2,3,4],"from_address":"0x0000000000000000000000000000000000000002","value":"0x0","block_height":1}"#;
            assert_eq!(deser(json).unwrap(), None);
        }

        #[test]
        fn test_unknown_string_is_rejected() {
            assert!(deser(&body_with(r#""foo""#)).is_err());
        }

        #[test]
        fn test_empty_string_is_rejected() {
            assert!(deser(&body_with(r#""""#)).is_err());
        }

        #[test]
        fn test_negative_integer_is_rejected() {
            assert!(deser(&body_with("-1")).is_err());
        }

        #[test]
        fn test_boolean_is_rejected() {
            assert!(deser(&body_with("true")).is_err());
        }
    }

    // -- onchain validation unit tests --
    //
    // These tests exercise validate_onchain / detect_contract_chain directly using
    // alloy's built-in mock transport (alloy_provider::mock::Asserter).  No live
    // chain or forked node is required; responses are queued FIFO and consumed by
    // each RPC call in order:
    //   1. eth_getCode        (detect_contract_chain)
    //   2. eth_blockNumber    (block-height check)
    //   3. eth_call           (stateTransitionCount view call)

    mod onchain {
        use super::*;
        use alloy::sol_types::SolValue;
        use alloy_primitives::{Bytes, U64};
        use alloy_provider::{ProviderBuilder, mock::Asserter};

        fn mock_provider() -> (impl Provider + Clone, Asserter) {
            let asserter = Asserter::new();
            let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
            (provider, asserter)
        }

        fn valid_body() -> GasKillerTaskRequestBody {
            GasKillerTaskRequestBody {
                target_address: "0x0000000000000000000000000000000000000001"
                    .parse()
                    .unwrap(),
                from_address: "0x0000000000000000000000000000000000000002"
                    .parse()
                    .unwrap(),
                call_data: vec![0xAB, 0xCD, 0xEF, 0x01],
                transition_index: Some(5),
                value: U256::ZERO,
                block_height: 50,
            }
        }

        fn push_code_exists(asserter: &Asserter) {
            asserter.push_success(&Bytes::from(vec![0x60u8]));
        }

        fn push_code_empty(asserter: &Asserter) {
            asserter.push_success(&Bytes::new());
        }

        fn push_block_number(asserter: &Asserter, n: u64) {
            asserter.push_success(&U64::from(n));
        }

        fn push_state_transition_count(asserter: &Asserter, count: u64) {
            asserter.push_success(&Bytes::from(U256::from(count).abi_encode()));
        }

        #[tokio::test]
        async fn test_contract_not_found() {
            let (provider, asserter) = mock_provider();
            push_code_empty(&asserter);

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            let err = validate_onchain(&providers, &valid_body())
                .await
                .unwrap_err();
            assert!(
                matches!(err, OnchainValidationError::ContractNotFound),
                "expected ContractNotFound, got {err}"
            );
        }

        #[tokio::test]
        async fn test_block_height_in_future() {
            let (provider, asserter) = mock_provider();
            push_code_exists(&asserter);
            push_block_number(&asserter, 40); // chain is at 40, request wants 50

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            let err = validate_onchain(&providers, &valid_body())
                .await
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    OnchainValidationError::BlockHeightInFuture {
                        provided: 50,
                        current: 40
                    }
                ),
                "expected BlockHeightInFuture, got {err}"
            );
        }

        #[tokio::test]
        async fn test_block_height_too_stale() {
            let body = valid_body(); // block_height = 50
            let measure = gas_killer_common::block_stale_measure();
            // head one block past the staleness window → age = measure + 1, rejected
            let head = body.block_height + measure + 1;

            let (provider, asserter) = mock_provider();
            push_code_exists(&asserter);
            push_block_number(&asserter, head);

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            let err = validate_onchain(&providers, &body).await.unwrap_err();
            assert!(
                matches!(
                    err,
                    OnchainValidationError::BlockHeightTooStale { provided: 50, .. }
                ),
                "expected BlockHeightTooStale, got {err}"
            );
        }

        #[tokio::test]
        async fn test_block_height_at_staleness_boundary_passes() {
            let body = valid_body(); // block_height = 50, transition_index = Some(5)
            let measure = gas_killer_common::block_stale_measure();
            // head exactly at the window edge → age == measure, still valid
            let head = body.block_height + measure;

            let (provider, asserter) = mock_provider();
            push_code_exists(&asserter);
            push_block_number(&asserter, head);
            push_state_transition_count(&asserter, 5); // matches transition_index → passes

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            assert!(
                validate_onchain(&providers, &body).await.is_ok(),
                "age == staleness window should be accepted"
            );
        }

        #[tokio::test]
        async fn test_transition_index_behind() {
            let (provider, asserter) = mock_provider();
            push_code_exists(&asserter);
            push_block_number(&asserter, 100); // chain at 100, request wants 50 ✓
            push_state_transition_count(&asserter, 10); // contract at 10, request provides 5 ✗

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            let err = validate_onchain(&providers, &valid_body())
                .await
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    OnchainValidationError::TransitionIndexMismatch {
                        provided: 5,
                        current: 10
                    }
                ),
                "expected TransitionIndexMismatch, got {err}"
            );
        }

        #[tokio::test]
        async fn test_valid_onchain_state_passes() {
            let (provider, asserter) = mock_provider();
            push_code_exists(&asserter);
            push_block_number(&asserter, 100); // chain at 100 >= request 50 ✓
            push_state_transition_count(&asserter, 5); // contract at 5, request at 5 ✓

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            validate_onchain(&providers, &valid_body())
                .await
                .expect("valid onchain state should pass");
        }

        #[tokio::test]
        async fn test_transition_index_ahead() {
            let (provider, asserter) = mock_provider();
            push_code_exists(&asserter);
            push_block_number(&asserter, 100);
            push_state_transition_count(&asserter, 3); // contract at 3, request at 5 → ahead ✗

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            let err = validate_onchain(&providers, &valid_body())
                .await
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    OnchainValidationError::TransitionIndexMismatch {
                        provided: 5,
                        current: 3
                    }
                ),
                "expected TransitionIndexMismatch, got {err}"
            );
        }

        #[tokio::test]
        async fn test_rpc_error_on_get_code_treated_as_rpc_error() {
            let (provider, asserter) = mock_provider();
            asserter.push_failure_msg("connection refused");

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            let err = validate_onchain(&providers, &valid_body())
                .await
                .unwrap_err();
            assert!(
                matches!(err, OnchainValidationError::RpcError(_)),
                "expected RpcError, got {err}"
            );
        }

        #[tokio::test]
        async fn test_rpc_error_on_block_number() {
            let (provider, asserter) = mock_provider();
            push_code_exists(&asserter);
            asserter.push_failure_msg("node overloaded");

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            let err = validate_onchain(&providers, &valid_body())
                .await
                .unwrap_err();
            assert!(
                matches!(err, OnchainValidationError::RpcError(_)),
                "expected RpcError, got {err}"
            );
        }

        #[tokio::test]
        async fn test_rpc_error_on_state_transition_count() {
            let (provider, asserter) = mock_provider();
            push_code_exists(&asserter);
            push_block_number(&asserter, 100);
            asserter.push_failure_msg("call reverted");

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            let err = validate_onchain(&providers, &valid_body())
                .await
                .unwrap_err();
            assert!(
                matches!(err, OnchainValidationError::RpcError(_)),
                "expected RpcError, got {err}"
            );
        }

        #[tokio::test]
        async fn test_auto_transition_index_skips_count_check() {
            let (provider, asserter) = mock_provider();
            push_code_exists(&asserter);
            push_block_number(&asserter, 100);
            // No push_state_transition_count — the mock asserter would fail if it were called.

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, provider);

            let mut body = valid_body();
            body.transition_index = None;

            validate_onchain(&providers, &body)
                .await
                .expect("auto transition_index should skip count check and pass");
        }

        #[tokio::test]
        async fn test_l2_fallback_when_l1_has_no_code() {
            let (l1_provider, l1_asserter) = mock_provider();
            let (l2_provider, l2_asserter) = mock_provider();

            push_code_empty(&l1_asserter); // L1 has no code
            push_code_exists(&l2_asserter); // L2 does
            push_block_number(&l2_asserter, 100);
            push_state_transition_count(&l2_asserter, 5);

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, l1_provider);
            providers.insert(ChainRole::L2, l2_provider);

            validate_onchain(&providers, &valid_body())
                .await
                .expect("should find contract on L2");
        }

        #[tokio::test]
        async fn test_not_found_when_all_chains_empty() {
            let (l1_provider, l1_asserter) = mock_provider();
            let (l2_provider, l2_asserter) = mock_provider();

            push_code_empty(&l1_asserter);
            push_code_empty(&l2_asserter);

            let mut providers = HashMap::new();
            providers.insert(ChainRole::L1, l1_provider);
            providers.insert(ChainRole::L2, l2_provider);

            let err = validate_onchain(&providers, &valid_body())
                .await
                .unwrap_err();
            assert!(
                matches!(err, OnchainValidationError::ContractNotFound),
                "expected ContractNotFound, got {err}"
            );
        }
    }

    // -- error envelope mapping --
    //
    // Locks the status code and machine-readable ErrorCode each validation error maps to,
    // including that transient RPC failures are sanitized so internal detail never reaches
    // the client.

    mod error_mapping {
        use super::*;
        use crate::error::ErrorCode;

        #[test]
        fn validation_errors_map_to_code_and_400() {
            let cases = [
                (
                    ValidationError::ZeroTargetAddress,
                    ErrorCode::InvalidAddress,
                ),
                (ValidationError::ZeroFromAddress, ErrorCode::InvalidAddress),
                (ValidationError::EmptyCallData, ErrorCode::InvalidRequest),
                (
                    ValidationError::CallDataTooShort { len: 2 },
                    ErrorCode::InvalidRequest,
                ),
                (
                    ValidationError::CallDataTooLarge { len: 1, max: 0 },
                    ErrorCode::CalldataTooLarge,
                ),
                (ValidationError::ZeroBlockHeight, ErrorCode::InvalidRequest),
            ];
            for (err, code) in cases {
                let api = ApiError::from(err);
                assert_eq!(api.status, StatusCode::BAD_REQUEST);
                assert_eq!(api.code, code);
            }
        }

        #[test]
        fn onchain_errors_map_to_code_and_status() {
            let cases = [
                (
                    OnchainValidationError::ContractNotFound,
                    StatusCode::BAD_REQUEST,
                    ErrorCode::ContractNotFound,
                ),
                (
                    OnchainValidationError::TransitionIndexMismatch {
                        provided: 5,
                        current: 6,
                    },
                    StatusCode::BAD_REQUEST,
                    ErrorCode::TransitionMismatch,
                ),
                (
                    OnchainValidationError::BlockHeightInFuture {
                        provided: 10,
                        current: 9,
                    },
                    StatusCode::BAD_REQUEST,
                    ErrorCode::InvalidRequest,
                ),
                (
                    OnchainValidationError::BlockHeightTooStale {
                        provided: 1,
                        current: 500,
                        max_age: 300,
                    },
                    StatusCode::BAD_REQUEST,
                    ErrorCode::StaleBlock,
                ),
            ];
            for (err, status, code) in cases {
                let api = ApiError::from(err);
                assert_eq!(api.status, status);
                assert_eq!(api.code, code);
            }
        }

        #[test]
        fn rpc_error_is_503_and_message_is_sanitized() {
            let api = ApiError::from(OnchainValidationError::RpcError(
                "connection refused at 10.0.0.5".to_string(),
            ));
            assert_eq!(api.status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(api.code, ErrorCode::RpcUnavailable);
            assert_eq!(api.message, "Service temporarily unavailable");
            assert!(
                !api.message.contains("10.0.0.5"),
                "internal RPC detail must not leak to clients"
            );
        }
    }
}

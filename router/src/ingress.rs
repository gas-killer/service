use crate::creator::{SimpleTaskQueue, TaskQueue};
use crate::metrics::MetricsCollector;
use alloy_primitives::{Address, U256};
use alloy_provider::Provider;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use gas_killer_common::ChainId;
use gas_killer_common::ReadOnlyProvider;
use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;
use gas_killer_common::config::CHAIN_DETECTION_ORDER;
use gas_killer_common::task_data::MAX_EVM_TX_CALLDATA_SIZE;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Clone)]
pub struct IngressState {
    pub queue: Arc<SimpleTaskQueue>,
    pub metrics: Option<Arc<MetricsCollector>>,
    pub providers: Arc<HashMap<ChainId, ReadOnlyProvider>>,
}

impl IngressState {
    pub fn new(
        queue: Arc<SimpleTaskQueue>,
        metrics: Arc<MetricsCollector>,
        providers: HashMap<ChainId, ReadOnlyProvider>,
    ) -> Self {
        Self {
            queue,
            metrics: Some(metrics),
            providers: Arc::new(providers),
        }
    }

    pub fn without_metrics(queue: Arc<SimpleTaskQueue>) -> Self {
        Self {
            queue,
            metrics: None,
            providers: Arc::new(HashMap::new()),
        }
    }
}

/// Onchain validation errors for incoming task requests.
#[derive(Debug)]
pub enum OnchainValidationError {
    ContractNotFound,
    TransitionIndexMismatch { provided: u64, current: u64 },
    BlockHeightInFuture { provided: u64, current: u64 },
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
            Self::RpcError(msg) => write!(f, "RPC error during onchain validation: {msg}"),
        }
    }
}

impl std::error::Error for OnchainValidationError {}

async fn detect_contract_chain<P: Provider + Clone>(
    providers: &HashMap<ChainId, P>,
    address: Address,
) -> Result<ChainId, OnchainValidationError> {
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
    providers: &HashMap<ChainId, P>,
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

    let contract = GasKillerSDK::new(body.target_address, provider.clone());
    let count = contract
        .stateTransitionCount()
        .call()
        .await
        .map_err(|e| OnchainValidationError::RpcError(e.to_string()))?;
    let current_count: u64 = count
        .try_into()
        .map_err(|_| OnchainValidationError::RpcError("stateTransitionCount overflow".into()))?;

    if body.transition_index != current_count {
        return Err(OnchainValidationError::TransitionIndexMismatch {
            provided: body.transition_index,
            current: current_count,
        });
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GasKillerTaskRequestBody {
    pub target_address: Address,
    pub call_data: Vec<u8>,
    pub transition_index: u64,
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
    Json(request): Json<GasKillerTaskRequest>,
) -> (StatusCode, Json<GasKillerTaskResponse>) {
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
        return (
            StatusCode::BAD_REQUEST,
            Json(GasKillerTaskResponse {
                success: false,
                message: format!("Task rejected: {e}"),
            }),
        );
    }

    if !state.providers.is_empty()
        && let Err(e) = validate_onchain(&*state.providers, &request.body).await
    {
        let status = if matches!(e, OnchainValidationError::RpcError(_)) {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::BAD_REQUEST
        };
        warn!(
            target_address = %request.body.target_address,
            from_address = %request.body.from_address,
            block_height = request.body.block_height,
            transition_index = request.body.transition_index,
            error = %e,
            "Task rejected (onchain)"
        );
        if let Some(m) = &state.metrics {
            m.ingress_rejected.inc();
        }
        let client_message = if matches!(e, OnchainValidationError::RpcError(_)) {
            "Service temporarily unavailable".to_string()
        } else {
            format!("Task rejected: {e}")
        };
        return (
            status,
            Json(GasKillerTaskResponse {
                success: false,
                message: client_message,
            }),
        );
    }

    info!(
        target_address = %request.body.target_address,
        from_address = %request.body.from_address,
        block_height = request.body.block_height,
        call_data_len = request.body.call_data.len(),
        "Task accepted"
    );
    if let Some(m) = &state.metrics {
        m.ingress_accepted.inc();
    }
    state.queue.push(request);
    (
        StatusCode::OK,
        Json(GasKillerTaskResponse {
            success: true,
            message: "Task queued".to_string(),
        }),
    )
}

async fn healthz_handler() -> StatusCode {
    StatusCode::OK
}

pub fn build_app() -> Router<IngressState> {
    Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/trigger", post(trigger_task_handler))
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
                transition_index: 0,
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
                transition_index: u64::MAX,
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

        fn make_app() -> (Router, Arc<SimpleTaskQueue>) {
            let queue = Arc::new(SimpleTaskQueue::new());
            let state = IngressState::without_metrics(queue.clone());
            let app = build_app().with_state(state);
            (app, queue)
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
            let (app, queue) = make_app();
            let resp = app.oneshot(json_request(&valid_body())).await.unwrap();

            assert_eq!(resp.status(), StatusCode::OK);
            let body = response_body(resp).await;
            assert!(body.success);
            assert_eq!(body.message, "Task queued");
            assert!(
                queue.pop().is_some(),
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
            let body = response_body(resp).await;
            assert!(!body.success);
            assert!(body.message.contains("target_address is zero"));
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
            let body = response_body(resp).await;
            assert!(!body.success);
            assert!(body.message.contains("from_address is zero"));
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
            let body = response_body(resp).await;
            assert!(!body.success);
            assert!(body.message.contains("call_data is empty"));
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
            let body = response_body(resp).await;
            assert!(!body.success);
            assert!(body.message.contains("call_data too short"));
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
            let body = response_body(resp).await;
            assert!(!body.success);
            assert!(body.message.contains("call_data too large"));
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
            let body = response_body(resp).await;
            assert!(!body.success);
            assert!(body.message.contains("block_height is zero"));
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
        }

        #[tokio::test]
        async fn test_valid_request_does_not_leave_extra_tasks() {
            // Two sequential valid requests → queue should hold exactly two tasks
            let queue = Arc::new(SimpleTaskQueue::new());
            let app1 = build_app().with_state(IngressState::without_metrics(queue.clone()));
            let app2 = build_app().with_state(IngressState::without_metrics(queue.clone()));

            app1.oneshot(json_request(&valid_body())).await.unwrap();
            app2.oneshot(json_request(&valid_body())).await.unwrap();

            assert!(queue.pop().is_some());
            assert!(queue.pop().is_some());
            assert!(
                queue.pop().is_none(),
                "queue should be empty after two pops"
            );
        }

        #[tokio::test]
        async fn test_rejected_request_does_not_enqueue() {
            let (app, queue) = make_app();
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
                queue.pop().is_none(),
                "invalid task must not be pushed to queue"
            );
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
                transition_index: 5,
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
            providers.insert(ChainId::L1, provider);

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
            providers.insert(ChainId::L1, provider);

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
        async fn test_transition_index_behind() {
            let (provider, asserter) = mock_provider();
            push_code_exists(&asserter);
            push_block_number(&asserter, 100); // chain at 100, request wants 50 ✓
            push_state_transition_count(&asserter, 10); // contract at 10, request provides 5 ✗

            let mut providers = HashMap::new();
            providers.insert(ChainId::L1, provider);

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
            providers.insert(ChainId::L1, provider);

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
            providers.insert(ChainId::L1, provider);

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
            providers.insert(ChainId::L1, provider);

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
            providers.insert(ChainId::L1, provider);

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
            providers.insert(ChainId::L1, provider);

            let err = validate_onchain(&providers, &valid_body())
                .await
                .unwrap_err();
            assert!(
                matches!(err, OnchainValidationError::RpcError(_)),
                "expected RpcError, got {err}"
            );
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
            providers.insert(ChainId::L1, l1_provider);
            providers.insert(ChainId::L2, l2_provider);

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
            providers.insert(ChainId::L1, l1_provider);
            providers.insert(ChainId::L2, l2_provider);

            let err = validate_onchain(&providers, &valid_body())
                .await
                .unwrap_err();
            assert!(
                matches!(err, OnchainValidationError::ContractNotFound),
                "expected ContractNotFound, got {err}"
            );
        }
    }
}

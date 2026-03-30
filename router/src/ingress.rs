use crate::creator::{SimpleTaskQueue, TaskQueue};
use alloy_primitives::{Address, U256};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use gas_killer_common::task_data::MAX_EVM_TX_CALLDATA_SIZE;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use tracing::{info, warn};

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
    State(queue): State<Arc<SimpleTaskQueue>>,
    Json(request): Json<GasKillerTaskRequest>,
) -> (StatusCode, Json<GasKillerTaskResponse>) {
    match request.validate() {
        Ok(()) => {
            info!(
                target_address = %request.body.target_address,
                from_address = %request.body.from_address,
                block_height = request.body.block_height,
                call_data_len = request.body.call_data.len(),
                "Task accepted"
            );
            queue.push(request);
            (
                StatusCode::OK,
                Json(GasKillerTaskResponse {
                    success: true,
                    message: "Task queued".to_string(),
                }),
            )
        }
        Err(e) => {
            warn!(
                target_address = %request.body.target_address,
                from_address = %request.body.from_address,
                block_height = request.body.block_height,
                error = %e,
                "Task rejected"
            );
            (
                StatusCode::BAD_REQUEST,
                Json(GasKillerTaskResponse {
                    success: false,
                    message: format!("Task rejected: {e}"),
                }),
            )
        }
    }
}

async fn healthz_handler() -> StatusCode {
    StatusCode::OK
}

pub fn build_app(queue: Arc<SimpleTaskQueue>) -> Router {
    Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/trigger", post(trigger_task_handler))
        .with_state(queue)
}

// Start the HTTP server in a background task
pub async fn start_gas_killer_http_server(queue: Arc<SimpleTaskQueue>, addr: &str) {
    let app = build_app(queue);
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
            let app = build_app(queue.clone());
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
            let app1 = build_app(queue.clone());
            let app2 = build_app(queue.clone());

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
}

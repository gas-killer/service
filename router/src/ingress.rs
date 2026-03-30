use crate::creator::{SimpleTaskQueue, TaskQueue};
use alloy_primitives::{Address, U256};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
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

// Start the HTTP server in a background task
pub async fn start_gas_killer_http_server(queue: Arc<SimpleTaskQueue>, addr: &str) {
    let app = Router::new()
        .route("/trigger", post(trigger_task_handler))
        .with_state(queue);
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
}

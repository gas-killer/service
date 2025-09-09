use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::usecases::gas_killer::types::{
    EnrichedGasKillerRequest, GasKillerTransactionRequest, GasKillerTransactionResponse,
    RequestMetadata, RequestStatus, ValidationError,
};

pub struct GasKillerIngressState {
    pub queue: Arc<Mutex<Vec<EnrichedGasKillerRequest>>>,
    pub max_queue_size: usize,
}

impl GasKillerIngressState {
    pub fn new(max_queue_size: usize) -> Self {
        Self {
            queue: Arc::new(Mutex::new(Vec::new())),
            max_queue_size,
        }
    }
}

/// Handler for POST /api/v1/gas-killer/transaction
pub async fn gas_killer_transaction_handler(
    State(state): State<Arc<GasKillerIngressState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<GasKillerTransactionRequest>,
) -> Result<(StatusCode, Json<GasKillerTransactionResponse>), (StatusCode, Json<ValidationError>)> {
    // Validate the request
    if let Err(e) = req.validate() {
        warn!("Request validation failed: {:?}", e);
        return Err((StatusCode::BAD_REQUEST, Json(e)));
    }

    // Extract metadata from request
    let ip_address = Some(addr.ip().to_string());
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let metadata = RequestMetadata {
        ip_address,
        user_agent,
        additional: Default::default(),
    };

    // Create enriched request
    let request_id = Uuid::new_v4();
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let enriched_request = EnrichedGasKillerRequest {
        request_id,
        request: req,
        metadata,
        created_at,
    };

    // Try to add to queue
    {
        let mut queue = match state.queue.lock() {
            Ok(q) => q,
            Err(e) => {
                error!("Failed to acquire queue lock: {:?}", e);
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(ValidationError {
                        code: "QUEUE_ERROR".to_string(),
                        message: "Service temporarily unavailable".to_string(),
                        details: None,
                    }),
                ));
            }
        };

        // Check if queue is full
        if queue.len() >= state.max_queue_size {
            warn!("Queue is full, rejecting request");
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ValidationError {
                    code: "QUEUE_FULL".to_string(),
                    message: "Service queue is full, please retry later".to_string(),
                    details: None,
                }),
            ));
        }

        queue.push(enriched_request.clone());
        info!(
            "Gas Killer request queued: {} (queue size: {})",
            request_id,
            queue.len()
        );
    }

    // Return success response
    Ok((
        StatusCode::OK,
        Json(GasKillerTransactionResponse {
            request_id: request_id.to_string(),
            status: RequestStatus::Queued,
            estimated_time: 30, // Default estimate
            message: Some("Transaction request queued for gas optimization".to_string()),
        }),
    ))
}

/// Handler for GET /api/v1/gas-killer/status/{request_id}
pub async fn gas_killer_status_handler(
    State(_state): State<Arc<GasKillerIngressState>>,
    axum::extract::Path(request_id): axum::extract::Path<String>,
) -> (StatusCode, Json<GasKillerTransactionResponse>) {
    // For now, return a simple response
    // In production, this would check the actual status of the request
    (
        StatusCode::OK,
        Json(GasKillerTransactionResponse {
            request_id,
            status: RequestStatus::Processing,
            estimated_time: 15,
            message: Some("Request is being processed".to_string()),
        }),
    )
}

/// Handler for GET /api/v1/gas-killer/health
pub async fn gas_killer_health_handler(
    State(state): State<Arc<GasKillerIngressState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let queue_size = state.queue.lock().map(|q| q.len()).unwrap_or(0);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "healthy",
            "queue_size": queue_size,
            "max_queue_size": state.max_queue_size,
        })),
    )
}

/// Start the Gas Killer HTTP server
pub async fn start_gas_killer_http_server(
    state: Arc<GasKillerIngressState>,
    addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        .route(
            "/api/v1/gas-killer/transaction",
            post(gas_killer_transaction_handler),
        )
        .route(
            "/api/v1/gas-killer/status/:request_id",
            axum::routing::get(gas_killer_status_handler),
        )
        .route(
            "/api/v1/gas-killer/health",
            axum::routing::get(gas_killer_health_handler),
        )
        // Keep backward compatibility with original endpoint
        .route("/trigger", post(legacy_trigger_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Gas Killer ingress HTTP server running on {}", addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// Mock handler for backward compatibility - not used in Gas Killer
async fn legacy_trigger_handler(
    State(_state): State<Arc<GasKillerIngressState>>,
    ConnectInfo(_addr): ConnectInfo<SocketAddr>,
    _headers: HeaderMap,
    Json(_req): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Mock response for legacy endpoint
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "success": true,
            "message": "Legacy endpoint - use /api/v1/gas-killer/transaction instead"
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_valid_gas_killer_request() {
        let request = GasKillerTransactionRequest {
            target_contract_address: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
            target_method: "transfer(address,uint256)".to_string(),
            target_chain_id: 1,
            params: "0x1234567890".to_string(),
            caller_address: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        };

        // Validate request should succeed
        assert!(request.validate().is_ok());
    }

    #[tokio::test]
    async fn test_invalid_address_request() {
        let request = GasKillerTransactionRequest {
            target_contract_address: "invalid_address".to_string(),
            target_method: "transfer(address,uint256)".to_string(),
            target_chain_id: 1,
            params: "0x1234567890".to_string(),
            caller_address: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        };

        // Validate request should fail due to invalid address
        assert!(request.validate().is_err());
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let state = Arc::new(GasKillerIngressState::new(100));
        // Just test that we can create the state and access queue size
        let queue_size = state.queue.lock().unwrap().len();
        assert_eq!(queue_size, 0);
        assert_eq!(state.max_queue_size, 100);
    }
}

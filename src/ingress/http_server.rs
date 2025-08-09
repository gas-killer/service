use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

use crate::ingress::types::{TaskRequest, TaskResponse};

// Handler for POST /trigger
pub async fn trigger_task_handler(
    State(state): State<Arc<Mutex<Vec<TaskRequest>>>>,
    Json(req): Json<TaskRequest>,
) -> (StatusCode, Json<TaskResponse>) {
    // Validate target_contract is a valid hex address
    if !req.body.target_contract.starts_with("0x") || req.body.target_contract.len() != 42 {
        return (
            StatusCode::BAD_REQUEST,
            Json(TaskResponse {
                success: false,
                message: "Invalid target contract address format".to_string(),
            }),
        );
    }
    
    // Validate target_function is a valid hex selector (4 bytes = 8 hex chars + 0x)
    if !req.body.target_function.starts_with("0x") || req.body.target_function.len() != 10 {
        return (
            StatusCode::BAD_REQUEST,
            Json(TaskResponse {
                success: false,
                message: "Invalid function selector format (should be 0x followed by 8 hex chars)".to_string(),
            }),
        );
    }
    
    // Validate function_params is hex-encoded (even number of chars after 0x)
    if !req.body.function_params.starts_with("0x") || (req.body.function_params.len() - 2) % 2 != 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(TaskResponse {
                success: false,
                message: "Invalid function parameters format (should be hex-encoded)".to_string(),
            }),
        );
    }
    
    // Add to queue
    {
        let mut queue = state.lock().await;
        queue.push(req.clone());
    }
    (
        StatusCode::OK,
        Json(TaskResponse {
            success: true,
            message: "Task queued".to_string(),
        }),
    )
}

// Start the HTTP server in a background task
pub async fn start_http_server(queue: Arc<Mutex<Vec<TaskRequest>>>, addr: &str) {
    let app = Router::new()
        .route("/trigger", post(trigger_task_handler))
        .with_state(queue);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind HTTP server");
    info!("ListeningCreator HTTP server running on {}", addr);
    axum::serve(listener, app)
        .await
        .expect("HTTP server failed");
}

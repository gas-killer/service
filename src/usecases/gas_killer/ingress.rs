#![allow(dead_code)]
use crate::usecases::gas_killer::creator::{SimpleTaskQueue, TaskQueue};
use alloy_primitives::{Address, U256};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GasKillerTaskRequestBody {
    pub target_address: Address,
    pub call_data: Vec<u8>,
    pub storage_updates: Vec<u8>,
    pub transition_index: u64,
    pub from_address: Address,
    pub value: U256,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GasKillerTaskRequest {
    pub body: GasKillerTaskRequestBody,
}

#[allow(dead_code)]
impl GasKillerTaskRequest {
    pub fn is_valid(&self) -> bool {
        let body = &self.body;
        if body.target_address.is_zero()
            || body.call_data.is_empty()
            || body.storage_updates.is_empty()
        {
            // TODO: add additional checks
            return false;
        }
        true
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
    if request.is_valid() {
        queue.push(request);
        return (
            StatusCode::OK,
            Json(GasKillerTaskResponse {
                success: true,
                message: "Task queued".to_string(),
            }),
        );
    }

    (
        StatusCode::BAD_REQUEST,
        Json(GasKillerTaskResponse {
            success: false,
            message: "Task rejected: invalid task".to_string(),
        }),
    )
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

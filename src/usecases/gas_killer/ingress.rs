use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use std::sync::{Arc, Mutex};
use tracing::info;

use crate::usecases::gas_killer::structs::{GasKillerTaskRequest, GasKillerTaskResponse};

// Handler for POST /trigger
pub async fn trigger_task_handler(
    State(state): State<Arc<Mutex<Vec<GasKillerTaskRequest>>>>,
    Json(request): Json<GasKillerTaskRequest>,
) -> (StatusCode, Json<GasKillerTaskResponse>) {
    if let Ok(mut queue) = state.lock()
        && request.is_valid()
    {
        queue.push(request.clone());
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
pub async fn start_gas_killer_http_server(
    queue: Arc<Mutex<Vec<GasKillerTaskRequest>>>,
    addr: &str,
) {
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

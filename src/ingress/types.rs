use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TaskRequestBody {
    pub target_contract: String,    // Target contract address (hex string, e.g., "0x...")
    pub target_function: String,    // Target function selector (hex string, e.g., "0xa9059cbb" for transfer)
    pub function_params: String,    // Hex-encoded function parameters
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TaskRequest {
    pub body: TaskRequestBody,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskResponse {
    pub success: bool,
    pub message: String,
}

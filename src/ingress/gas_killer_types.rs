use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GasKillerTaskRequestBody {
    pub target_contract: String,    // Target contract address (hex string, e.g., "0x...")
    pub target_function: String,    // Target function selector (hex string, e.g., "0xa9059cbb" for transfer)
    pub function_params: String,    // Hex-encoded function parameters
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GasKillerTaskRequest {
    pub body: GasKillerTaskRequestBody,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GasKillerTaskResponse {
    pub success: bool,
    pub message: String,
}
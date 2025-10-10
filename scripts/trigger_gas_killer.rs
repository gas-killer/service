use alloy::primitives::{hex, Address, U256};
use gas_killer_router::usecases::gas_killer::ingress::{
    GasKillerTaskRequest, GasKillerTaskRequestBody,
};
use serde_json::json;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenv::dotenv().ok();

    // Required env vars
    let target_address: Address = env_var("GAS_KILLER_TARGET_ADDRESS")?.parse()?;
    let call_data_hex = env_var("GAS_KILLER_CALL_DATA")?;
    let from_address: Address = env_var("GAS_KILLER_FROM_ADDRESS")?.parse()?;
    let transition_index: u64 = env_var("GAS_KILLER_TRANSITION_INDEX")?.parse()?;

    // Optional env vars
    let value: U256 = env::var("GAS_KILLER_VALUE")
        .ok()
        .unwrap_or_else(|| "0".to_string())
        .parse()?;
    let storage_updates_hex =
        env::var("GAS_KILLER_STORAGE_UPDATES").unwrap_or_else(|_| "0x".to_string());

    // Decode hex inputs to bytes
    let call_data = hex::decode(call_data_hex.trim_start_matches("0x"))?;
    let storage_updates = if storage_updates_hex.len() > 2 {
        hex::decode(storage_updates_hex.trim_start_matches("0x"))?
    } else {
        Vec::new()
    };

    // Build request
    let body = GasKillerTaskRequestBody {
        target_address,
        call_data,
        storage_updates,
        transition_index,
        from_address,
        value,
    };
    let request = GasKillerTaskRequest { body };

    // Serialize via serde to match axum Json extractor expectations
    let payload = json!({
        "body": {
            "target_address": format!("{:?}", request.body.target_address),
            "call_data": request.body.call_data,
            "storage_updates": request.body.storage_updates,
            "transition_index": request.body.transition_index,
            "from_address": format!("{:?}", request.body.from_address),
            "value": format!("{}", request.body.value),
        }
    });

    let url = env::var("GAS_KILLER_TRIGGER_URL")
        .unwrap_or_else(|_| "http://localhost:8080/trigger".to_string());
    println!("Posting GasKiller task to {}", url);

    let client = reqwest::Client::new();
    let resp = client.post(&url).json(&payload).send().await?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    println!("Response: {}\n{}", status, text);

    if !status.is_success() {
        Err(format!("Trigger failed with status {}", status).into())
    } else {
        Ok(())
    }
}

fn env_var(name: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    env::var(name).map_err(|_| format!("{} environment variable is required", name).into())
}



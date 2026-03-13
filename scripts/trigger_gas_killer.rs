use alloy::primitives::{Address, U256, hex};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol_types::SolCall;
use bindings::arraysummation::ArraySummation::sumCall;
use gas_killer_router::ingress::{GasKillerTaskRequest, GasKillerTaskRequestBody};
use serde_json::json;
use std::env;
use std::fs;
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenv::dotenv().ok();

    // Default to mock if any required env vars are missing
    let missing_required_env = [
        "GAS_KILLER_TARGET_ADDRESS",
        "GAS_KILLER_CALL_DATA",
        "GAS_KILLER_FROM_ADDRESS",
        "GAS_KILLER_TRANSITION_INDEX",
    ]
    .iter()
    .any(|k| env::var(k).is_err());

    let request = if missing_required_env {
        build_mock_request().await?
    } else {
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

        // Decode hex inputs to bytes
        let call_data = hex::decode(call_data_hex.trim_start_matches("0x"))?;

        // Get RPC URL for fetching block number if needed
        let rpc_for_block =
            env::var("HTTP_RPC").map_err(|_| "HTTP_RPC required to fetch block number")?;
        let rpc_url_for_block = Url::parse(&rpc_for_block)?;
        let provider_for_block = ProviderBuilder::new().connect_http(rpc_url_for_block);

        // Resolve block_height for deterministic execution
        let block_height = resolve_block_height(&provider_for_block).await?;

        // Build request
        let body = GasKillerTaskRequestBody {
            target_address,
            call_data,
            transition_index,
            from_address,
            value,
            block_height,
        };
        GasKillerTaskRequest { body }
    };

    // Serialize via serde to match axum Json extractor expectations
    let body_json = json!({
        "target_address": format!("{:?}", request.body.target_address),
        "call_data": request.body.call_data,
        "transition_index": request.body.transition_index,
        "from_address": format!("{:?}", request.body.from_address),
        "value": format!("{}", request.body.value),
        "block_height": request.body.block_height,
    });

    let payload = json!({
        "body": body_json
    });

    // Debug summary of the request prior to sending
    let selector_hex = if request.body.call_data.len() >= 4 {
        hex::encode(&request.body.call_data[0..4])
    } else {
        String::from("")
    };
    println!(
        "Debug request summary:\n  target_address: {:?}\n  from_address: {:?}\n  transition_index: {}\n  value: {}\n  block_height: {}\n  call_data_len: {} (selector: 0x{})",
        request.body.target_address,
        request.body.from_address,
        request.body.transition_index,
        request.body.value,
        request.body.block_height,
        request.body.call_data.len(),
        selector_hex
    );

    // Prepare provider and contract for verification of currentSum
    let rpc_for_read = env::var("HTTP_RPC")?;
    let rpc_url_for_read = Url::parse(&rpc_for_read)?;
    let provider = ProviderBuilder::new().connect_http(rpc_url_for_read);
    let array_contract = bindings::arraysummation::ArraySummation::new(
        request.body.target_address,
        provider.clone(),
    );

    // Ensure target has code deployed
    let code = provider
        .get_code_at(request.body.target_address)
        .await
        .map_err(|e| {
            format!(
                "Failed to read code at target {}: {}",
                request.body.target_address, e
            )
        })?;
    if code.as_ref().is_empty() {
        return Err(format!(
            "Target address {} has no code deployed. Aborting trigger.",
            request.body.target_address
        )
        .into());
    }

    // Capture initial currentSum before posting task
    // Each trigger uses different indexes, so currentSum will change each time
    let initial_sum = array_contract
        .currentSum()
        .call()
        .await
        .map_err(|e| format!("Failed to read currentSum before trigger: {}", e))?
        .to::<u64>();

    let url = env::var("GAS_KILLER_TRIGGER_URL")
        .unwrap_or_else(|_| "http://localhost:8080/trigger".to_string());
    println!("Posting GasKiller task to {}", url);

    let client = reqwest::Client::new();
    let resp = client.post(&url).json(&payload).send().await?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    println!("Response: {}\n{}", status, text);

    if !status.is_success() {
        eprintln!(
            "Trigger failed with status {}. Reprinting request summary to aid debugging...\n  target_address: {:?}\n  from_address: {:?}\n  transition_index: {}\n  value: {}\n  block_height: {}\n  call_data_len: {} (selector: 0x{})",
            status,
            request.body.target_address,
            request.body.from_address,
            request.body.transition_index,
            request.body.value,
            request.body.block_height,
            request.body.call_data.len(),
            selector_hex
        );
        Err(format!("Trigger failed with status {}", status).into())
    } else {
        // Poll currentSum until it changes or timeout
        use tokio::time::{Duration, Instant, sleep};
        let max_wait_time = Duration::from_secs(150);
        let check_interval = Duration::from_secs(10);
        let start_time = Instant::now();

        loop {
            let current_sum = array_contract
                .currentSum()
                .call()
                .await
                .map_err(|e| format!("Failed to read currentSum after trigger: {}", e))?
                .to::<u64>();

            println!(
                "currentSum: {}, Initial: {}, Elapsed: {:.1}s",
                current_sum,
                initial_sum,
                start_time.elapsed().as_secs_f64()
            );

            if current_sum != initial_sum {
                println!(
                    "✅ SUCCESS: currentSum changed from {} to {}",
                    initial_sum, current_sum
                );
                return Ok(());
            }

            if start_time.elapsed() >= max_wait_time {
                println!(
                    "❌ TIMEOUT: currentSum unchanged ({}), waited {:.1} seconds",
                    current_sum,
                    max_wait_time.as_secs_f64()
                );
                return Err("Timeout waiting for currentSum to change".into());
            }

            sleep(check_interval).await;
        }
    }
}

fn env_var(name: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    env::var(name).map_err(|_| format!("{} environment variable is required", name).into())
}

/// Resolves the block height to use for deterministic execution.
async fn resolve_block_height<P: Provider>(
    provider: &P,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let current_block = provider
        .get_block_number()
        .await
        .map_err(|e| format!("Failed to get current block number: {}", e))?;
    println!("Using current block: {}", current_block);
    Ok(current_block)
}

async fn build_mock_request()
-> Result<GasKillerTaskRequest, Box<dyn std::error::Error + Send + Sync>> {
    // Try to source a real deployed ArraySummation address from AVS_DEPLOYMENT_PATH; fallback to placeholder
    let target_address: Address = match env::var("AVS_DEPLOYMENT_PATH") {
        Ok(path) => {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(deployment) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(addr) = deployment
                        .get("addresses")
                        .and_then(|a| a.get("arraySummation"))
                        .and_then(|v| v.as_str())
                    {
                        addr.parse()?
                    } else {
                        "0x0000000000000000000000000000000000000001".parse()?
                    }
                } else {
                    "0x0000000000000000000000000000000000000001".parse()?
                }
            } else {
                "0x0000000000000000000000000000000000000001".parse()?
            }
        }
        Err(_) => "0x0000000000000000000000000000000000000001".parse()?,
    };
    // Use Anvil's default first unlocked account to ensure a signing credential exists in the spawned fork
    let from_address: Address = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266".parse()?;
    let value = U256::from(0);

    // Derive RPC URL to read current stateTransitionCount
    let rpc = env::var("HTTP_RPC")
        .map_err(|_| "HTTP_RPC environment variable is required for mock mode")?;
    let rpc_url = Url::parse(&rpc)?;

    // Read current stateTransitionCount to compute correct transition_index
    let provider = ProviderBuilder::new().connect_http(rpc_url.clone());
    let array_contract =
        bindings::arraysummation::ArraySummation::new(target_address, provider.clone());
    let current_count = array_contract
        .stateTransitionCount()
        .call()
        .await
        .map_err(|e| format!("Failed to read stateTransitionCount: {}", e))?
        .to::<u64>();

    // Use different indexes based on transition_index to get different sums each time
    // Offset by 3 for each new trigger: [0,1,2], [3,4,5], [6,7,8], etc.
    // Array has 100 elements, so we can do ~33 unique triggers
    let base_idx = (current_count * 3) % 97; // Stay within bounds of 100 element array
    let indexes = vec![
        U256::from(base_idx),
        U256::from(base_idx + 1),
        U256::from(base_idx + 2),
    ];
    println!(
        "Using indexes [{}, {}, {}] for transition_index={}",
        base_idx,
        base_idx + 1,
        base_idx + 2,
        current_count
    );
    let call = sumCall { indexes };
    let call_data = call.abi_encode().to_vec();

    // Resolve block_height for deterministic execution
    let block_height = resolve_block_height(&provider).await?;

    let body = GasKillerTaskRequestBody {
        target_address,
        call_data,
        // transitionIndex must equal the current stateTransitionCount() at call time
        transition_index: current_count,
        from_address,
        value,
        block_height,
    };

    Ok(GasKillerTaskRequest { body })
}

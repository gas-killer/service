use alloy::primitives::{Address, U256, hex};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol_types::SolCall;
use gas_killer_router::ingress::{GasKillerTaskRequest, GasKillerTaskRequestBody};
use scripts::bindings::arraysummation::ArraySummation::sumCall;
use scripts::bindings::reentrantcheckpoint::ReentrantCheckpoint::advanceCall;
use scripts::task_payload::{
    DEFAULT_READY_TIMEOUT_SECS, submit_payload, submitter_key, task_status_url,
    wait_for_ready_payload,
};
use serde_json::json;
use std::env;
use std::fs;
use url::Url;

/// True when `E2E_EXAMPLE=reentrant` selects the re-entrancy demonstration target
/// (a `ReentrantCheckpoint`, task `advance()`, progress read via `counter()`) instead of
/// the default array-summation one (`sum(uint256[])`, progress via `currentSum()`).
fn e2e_example_is_reentrant() -> bool {
    matches!(
        env::var("E2E_EXAMPLE").map(|v| v.trim().to_ascii_lowercase()),
        Ok(ref v) if v == "reentrant" || v == "reentrant-checkpoint"
    )
}

/// Read the target's "progress" value — the state the e2e watches for change to confirm a
/// task settled. `counter()` for the re-entrancy target, `currentSum()` otherwise.
async fn read_progress_value<P: Provider>(
    target: Address,
    provider: &P,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    if e2e_example_is_reentrant() {
        Ok(
            scripts::bindings::reentrantcheckpoint::ReentrantCheckpoint::new(target, provider)
                .counter()
                .call()
                .await
                .map_err(|e| format!("Failed to read counter(): {}", e))?
                .to::<u64>(),
        )
    } else {
        Ok(
            scripts::bindings::arraysummation::ArraySummation::new(target, provider)
                .currentSum()
                .call()
                .await
                .map_err(|e| format!("Failed to read currentSum(): {}", e))?
                .to::<u64>(),
        )
    }
}

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
        let transition_index: Option<u64> = {
            let raw = env_var("GAS_KILLER_TRANSITION_INDEX")?;
            if raw == "auto" || raw == "null" {
                None
            } else {
                Some(raw.parse::<u64>()?)
            }
        };

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

        // Build request (None = auto, resolved server-side)
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
    let transition_index_display = match request.body.transition_index {
        Some(idx) => idx.to_string(),
        None => "auto".to_string(),
    };
    println!(
        "Debug request summary:\n  target_address: {:?}\n  from_address: {:?}\n  transition_index: {}\n  value: {}\n  block_height: {}\n  call_data_len: {} (selector: 0x{})",
        request.body.target_address,
        request.body.from_address,
        transition_index_display,
        request.body.value,
        request.body.block_height,
        request.body.call_data.len(),
        selector_hex
    );

    // Prepare provider for reading the target's progress value.
    let rpc_for_read = env::var("HTTP_RPC")?;
    let rpc_url_for_read = Url::parse(&rpc_for_read)?;
    let provider = ProviderBuilder::new().connect_http(rpc_url_for_read);

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

    // Capture the target's progress value before posting the task; a settled task changes
    // it (currentSum for array-summation, counter for the re-entrancy target).
    let initial_sum = read_progress_value(request.body.target_address, &provider).await?;

    let url = env::var("GAS_KILLER_TRIGGER_URL")
        .unwrap_or_else(|_| "http://localhost:8080/tasks".to_string());
    println!("Posting GasKiller task to {}", url);

    let client = reqwest::Client::new();
    let mut req = client.post(&url).json(&payload);
    if let Ok(api_key) = env::var("GAS_KILLER_API_KEY")
        && !api_key.is_empty()
    {
        req = req.header("Authorization", format!("Bearer {api_key}"));
    }
    let resp = req.send().await?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    println!("Response: {}\n{}", status, text);

    if !status.is_success() {
        eprintln!(
            "Trigger failed with status {}. Reprinting request summary to aid debugging...\n  target_address: {:?}\n  from_address: {:?}\n  transition_index: {}\n  value: {}\n  block_height: {}\n  call_data_len: {} (selector: 0x{})",
            status,
            request.body.target_address,
            request.body.from_address,
            transition_index_display,
            request.body.value,
            request.body.block_height,
            request.body.call_data.len(),
            selector_hex
        );
        return Err(format!("Trigger failed with status {}", status).into());
    }

    let task_id = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("task_id")
                .and_then(|t| t.as_str())
                .map(str::to_string)
        })
        .ok_or("Accepted response did not carry a task_id")?;
    let status_url = task_status_url(&url, &task_id)?;
    let api_key = env::var("GAS_KILLER_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());

    // Poll until the task is `ready`, extract the rendered payload, submit it with a funded key,
    // then confirm the on-chain effect by checking `currentSum` moved.
    let http_rpc = env::var("HTTP_RPC")?;
    let submit_key = submitter_key()?;

    let payload = wait_for_ready_payload(
        &client,
        &status_url,
        api_key.as_deref(),
        &task_id,
        DEFAULT_READY_TIMEOUT_SECS,
    )
    .await?;

    submit_payload(&payload, &http_rpc, &submit_key).await?;

    // Confirm the on-chain effect: the target's progress value must move after the
    // user-submitted verifyAndUpdate (currentSum for array-summation, counter for the
    // re-entrancy target).
    let final_sum = read_progress_value(request.body.target_address, &provider).await?;
    if final_sum == initial_sum {
        return Err(format!(
            "target progress unchanged ({final_sum}) after submitting the payload; verifyAndUpdate had no effect"
        )
        .into());
    }
    println!(
        "✅ SUCCESS: target progress changed {} → {} after the user-submitted verifyAndUpdate (task {})",
        initial_sum, final_sum, task_id
    );
    Ok(())
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
        scripts::bindings::arraysummation::ArraySummation::new(target_address, provider.clone());
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
    // The re-entrancy target's task is the no-arg `advance()`, which re-enters itself
    // mid-transition; the array-summation target's task is `sum(indexes)`.
    let call_data = if e2e_example_is_reentrant() {
        println!("Using ReentrantCheckpoint.advance() for transition_index={current_count}");
        advanceCall {}.abi_encode().to_vec()
    } else {
        sumCall { indexes }.abi_encode().to_vec()
    };

    // Resolve block_height for deterministic execution
    let block_height = resolve_block_height(&provider).await?;

    let body = GasKillerTaskRequestBody {
        target_address,
        call_data,
        transition_index: Some(current_count),
        from_address,
        value,
        block_height,
    };

    Ok(GasKillerTaskRequest { body })
}

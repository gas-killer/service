use alloy::primitives::{Address, U256, hex};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol_types::SolCall;
use gas_killer_router::ingress::{GasKillerTaskRequest, GasKillerTaskRequestBody};
use scripts::bindings::arraysummation::ArraySummation::sumCall;
use scripts::bindings::onchainlife::OnchainLife::stepCall;
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

/// True when `E2E_EXAMPLE=onchain-life` selects the Game of Life target (an `OnchainLife`,
/// task `step(uint32)`, progress read via `generation()`).
fn e2e_example_is_onchain_life() -> bool {
    matches!(
        env::var("E2E_EXAMPLE").map(|v| v.trim().to_ascii_lowercase()),
        Ok(ref v) if v == "onchain-life" || v == "onchainlife"
    )
}

/// Generations per `step` for the Game of Life target when `ONCHAIN_LIFE_GENERATIONS` is unset.
/// At ~16.5M gas each, three puts a direct call above a 30M block while the diff it produces
/// stays at 16 board words plus the generation counter — the property the unbounded-profile leg
/// asserts.
const DEFAULT_ONCHAIN_LIFE_GENERATIONS: u32 = 3;

/// Generations to `step`, read from `ONCHAIN_LIFE_GENERATIONS`.
///
/// `run_e2e_test.sh` exports this and estimates the same generation count in its step 7a, so the
/// call it proves unmineable is the call submitted here. The two halves of the unbounded claim
/// only mean anything if they measure one transition, which is why the count is read rather than
/// hardcoded on both sides.
fn onchain_life_generations() -> u32 {
    parse_onchain_life_generations(env::var("ONCHAIN_LIFE_GENERATIONS").ok().as_deref())
}

/// Parses the `ONCHAIN_LIFE_GENERATIONS` value (trimmed). `None` / empty →
/// [`DEFAULT_ONCHAIN_LIFE_GENERATIONS`]. Panics on a non-numeric value rather than falling back:
/// silently substituting the default would settle a different transition than step 7a estimated,
/// leaving the proof measuring two different calls while still passing.
fn parse_onchain_life_generations(raw: Option<&str>) -> u32 {
    match raw.map(str::trim) {
        None | Some("") => DEFAULT_ONCHAIN_LIFE_GENERATIONS,
        Some(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("ONCHAIN_LIFE_GENERATIONS must be a u32, got '{value}'")),
    }
}

/// Read the target's "progress" value — the state the e2e watches for change to confirm a
/// task settled. `counter()` for the re-entrancy target, `generation()` for the Game of Life
/// one, `currentSum()` otherwise.
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
    } else if e2e_example_is_onchain_life() {
        Ok(
            scripts::bindings::onchainlife::OnchainLife::new(target, provider)
                .generation()
                .call()
                .await
                .map_err(|e| format!("Failed to read generation(): {}", e))?
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
    // Resolve the task target from the deploy JSON's scheme-agnostic `arraySummation` key,
    // which whichever deploy ran writes (aliasing it for the schnorr and re-entrancy targets).
    //
    // Every failure here names its actual cause — a missing file, an unparseable file, or an
    // absent key — so a mis-wired deploy is distinguishable from a chain that never received one.
    let target_address: Address = {
        let path = env::var("AVS_DEPLOYMENT_PATH")
            .map_err(|_| "AVS_DEPLOYMENT_PATH is required to resolve the task target")?;
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read the deployment JSON at '{path}': {e}"))?;
        let deployment: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse the deployment JSON at '{path}': {e}"))?;
        let raw = deployment
            .get("addresses")
            .and_then(|a| a.get("arraySummation"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                format!(
                    "addresses.arraySummation is missing from '{path}' — deploy a target first \
                     (deploy_example writes this key, aliased when the example is not \
                     array-summation), or set GAS_KILLER_TARGET_ADDRESS to bypass this lookup"
                )
            })?;
        raw.parse()
            .map_err(|_| format!("addresses.arraySummation in '{path}' is not an address: {raw}"))?
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
    // mid-transition; the Game of Life target's is `step(generations)`; the array-summation
    // target's is `sum(indexes)`.
    let call_data = if e2e_example_is_reentrant() {
        println!("Using ReentrantCheckpoint.advance() for transition_index={current_count}");
        advanceCall {}.abi_encode().to_vec()
    } else if e2e_example_is_onchain_life() {
        let generations = onchain_life_generations();
        println!("Using OnchainLife.step({generations}) for transition_index={current_count}");
        stepCall { generations }.abi_encode().to_vec()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onchain_life_generations_parsing() {
        assert_eq!(
            parse_onchain_life_generations(None),
            DEFAULT_ONCHAIN_LIFE_GENERATIONS
        );
        assert_eq!(
            parse_onchain_life_generations(Some("")),
            DEFAULT_ONCHAIN_LIFE_GENERATIONS
        );
        assert_eq!(
            parse_onchain_life_generations(Some("   ")),
            DEFAULT_ONCHAIN_LIFE_GENERATIONS
        );
        assert_eq!(parse_onchain_life_generations(Some("5")), 5);
        assert_eq!(parse_onchain_life_generations(Some(" 12 ")), 12);
    }

    /// A non-numeric value must not fall back to the default: step 7a estimates the count it was
    /// given while this binary would settle a different one, and the leg would pass while its two
    /// halves measured different transitions.
    #[test]
    #[should_panic(expected = "ONCHAIN_LIFE_GENERATIONS must be a u32")]
    fn onchain_life_generations_rejects_a_non_numeric_value() {
        let _ = parse_onchain_life_generations(Some("abc"));
    }
}

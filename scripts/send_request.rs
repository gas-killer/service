//! Submits one Gas Killer task to the router, then submits the payload it renders and confirms the
//! transition landed on-chain.
//!
//! Two modes:
//!
//! - **Explicit** — with `GAS_KILLER_TARGET_ADDRESS`, `GAS_KILLER_CALL_DATA`,
//!   `GAS_KILLER_FROM_ADDRESS`, and `GAS_KILLER_TRANSITION_INDEX` all set, it triggers exactly that
//!   call against that target. Nothing here is contract-specific: the calldata comes from the
//!   caller and the on-chain check reads `stateTransitionCount()`, which the SDK declares on every
//!   target.
//! - **Default** — with any of those unset, it resolves the deployed target from the deployment
//!   JSON and triggers the array-summation exercise (`sum` over a rotating slice), which is what
//!   makes it a one-command smoke test of a local or Helm stack.
//!
//! For driving any other example, prefer `run_scenario` with the scenario `deploy_example` renders
//! from `scripts/examples/examples.toml` — that path takes its calldata from the manifest, so no
//! example's transition is defined twice.

use alloy::primitives::{Address, U256, hex};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol_types::SolCall;
use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;
use gas_killer_router::ingress::{GasKillerTaskRequest, GasKillerTaskRequestBody};
use scripts::bindings::arraysummation::ArraySummation::sumCall;
use scripts::deployment::{TARGET_ADDRESS_KEY, target_address};
use scripts::task_payload::{
    DEFAULT_READY_TIMEOUT_SECS, submit_payload, submitter_key, task_status_url,
    wait_for_ready_payload,
};
use serde_json::json;
use std::env;
use std::fs;
use url::Url;

/// Reads the target's settled-transition counter — the state this binary watches to confirm a task
/// landed.
///
/// `stateTransitionCount()` is declared by the SDK every target inherits, so the check works for
/// whatever contract the caller points at. It is also the stricter signal: a contract-specific
/// getter can legitimately hold its value across a transition (summing a zero-valued slice, say),
/// while the counter advances on every settlement.
async fn read_transition_count<P: Provider>(
    target: Address,
    provider: &P,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(GasKillerSDK::new(target, provider)
        .stateTransitionCount()
        .call()
        .await
        .map_err(|e| format!("Failed to read stateTransitionCount(): {e}"))?
        .to::<u64>())
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

    // Prepare provider for reading the target's transition counter.
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

    // Capture the counter before posting the task; a settled task advances it.
    let initial_count = read_transition_count(request.body.target_address, &provider).await?;

    let url = env::var("GAS_KILLER_TASKS_URL")
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
    // then confirm the on-chain effect by checking the transition counter advanced.
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

    // Confirm the on-chain effect: the counter must advance after the user-submitted
    // verifyAndUpdate.
    let final_count = read_transition_count(request.body.target_address, &provider).await?;
    if final_count == initial_count {
        return Err(format!(
            "stateTransitionCount unchanged ({final_count}) after submitting the payload; verifyAndUpdate had no effect"
        )
        .into());
    }
    println!(
        "✅ SUCCESS: stateTransitionCount advanced {} → {} after the user-submitted verifyAndUpdate (task {})",
        initial_count, final_count, task_id
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
    // Resolve the task target from the deploy JSON's scheme-agnostic target key,
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
        let raw = target_address(&deployment).ok_or_else(|| {
            format!(
                "addresses.{TARGET_ADDRESS_KEY} is missing from '{path}' — deploy a target first \
                 (deploy_example records it there for whichever example it deployed), or set \
                 GAS_KILLER_TARGET_ADDRESS to bypass this lookup"
            )
        })?;
        raw.parse().map_err(|_| {
            format!("addresses.{TARGET_ADDRESS_KEY} in '{path}' is not an address: {raw}")
        })?
    };
    // Use Anvil's default first unlocked account to ensure a signing credential exists in the spawned fork
    let from_address: Address = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266".parse()?;
    let value = U256::from(0);

    // Derive RPC URL to read current stateTransitionCount
    let rpc = env::var("HTTP_RPC")
        .map_err(|_| "HTTP_RPC environment variable is required for mock mode")?;
    let rpc_url = Url::parse(&rpc)?;

    // Read current stateTransitionCount to compute correct transition_index. It is declared by
    // the SDK every target inherits, so this reads through the SDK binding rather than any one
    // example's.
    let provider = ProviderBuilder::new().connect_http(rpc_url.clone());
    let current_count = GasKillerSDK::new(target_address, provider.clone())
        .stateTransitionCount()
        .call()
        .await
        .map_err(|e| format!("Failed to read stateTransitionCount: {}", e))?
        .to::<u64>();

    // Offset by 3 per trigger — [0,1,2], [3,4,5], … — so repeated runs against one deployment
    // produce different sums. The deployed array holds 100 elements.
    let base_idx = (current_count * 3) % 97;
    let indexes = vec![
        U256::from(base_idx),
        U256::from(base_idx + 1),
        U256::from(base_idx + 2),
    ];
    println!(
        "Using ArraySummation.sum([{}, {}, {}]) for transition_index={current_count}",
        base_idx,
        base_idx + 1,
        base_idx + 2
    );
    let call_data = sumCall { indexes }.abi_encode().to_vec();

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

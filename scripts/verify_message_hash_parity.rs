//! Asserts that `GasKillerTaskData::build_payload_hash` is byte-identical to the deployed
//! contract's `getMessageHash` across a range of payload shapes.
//!
//! Run against a chain with a GasKiller target contract deployed (the e2e stack deploys
//! ArraySummation). Reads `HTTP_RPC` and the target address from `GAS_KILLER_TARGET_ADDRESS`
//! (falling back to `addresses.arraySummation` in `AVS_DEPLOYMENT_PATH`).
//!
//! Exits non-zero on any mismatch so the e2e workflow fails.

use alloy::primitives::{Address, Bytes, FixedBytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use gas_killer_common::GasKillerTaskData;
use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;
use std::env;
use std::fs;
use url::Url;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    dotenv::dotenv().ok();

    let rpc = env::var("HTTP_RPC").map_err(|_| "HTTP_RPC environment variable is required")?;
    let target_address = resolve_target_address()?;

    println!("Verifying message-hash parity against {target_address} via {rpc}");

    let provider = ProviderBuilder::new().connect_http(Url::parse(&rpc)?);

    let code = provider
        .get_code_at(target_address)
        .await
        .map_err(|e| format!("failed to read code at {target_address}: {e}"))?;
    if code.is_empty() {
        return Err(format!("target address {target_address} has no code deployed").into());
    }

    let contract = GasKillerSDK::new(target_address, provider);

    let mut checked = 0usize;
    let mut mismatches = 0usize;

    for (transition_index, anchor_hash, caller, contract_calldata, storage_updates) in test_vectors()
    {
        let onchain = contract
            .getMessageHash(
                U256::from(transition_index),
                anchor_hash,
                caller,
                Bytes::from(contract_calldata.clone()),
                Bytes::from(storage_updates.clone()),
            )
            .call()
            .await
            .map_err(|e| format!("getMessageHash call failed: {e}"))?;

        let task_data = GasKillerTaskData {
            storage_updates: Bytes::from(storage_updates.clone()),
            transition_index,
            target_address,
            call_data: contract_calldata.clone(),
            from_address: caller,
            value: U256::ZERO,
            block_height: 0,
            anchor_hash,
            chain_id: 0,
        };
        let local = FixedBytes::<32>::from(task_data.build_payload_hash(&storage_updates).0);

        checked += 1;
        if local == onchain {
            println!(
                "  ok    ti={transition_index:<20} anchor={anchor_hash} caller={caller} calldata_len={:<4} updates_len={:<4} hash={local}",
                contract_calldata.len(),
                storage_updates.len(),
            );
        } else {
            mismatches += 1;
            eprintln!(
                "  FAIL  ti={transition_index:<20} anchor={anchor_hash} caller={caller} calldata_len={:<4} updates_len={:<4}\n        local   = {local}\n        onchain = {onchain}",
                contract_calldata.len(),
                storage_updates.len(),
            );
        }
    }

    if mismatches > 0 {
        return Err(format!(
            "{mismatches}/{checked} message-hash parity checks failed: local build_payload_hash diverges from on-chain getMessageHash"
        )
        .into());
    }

    println!("✅ All {checked} message-hash parity checks passed");
    Ok(())
}

/// Resolves the target contract address from `GAS_KILLER_TARGET_ADDRESS`, falling back to the
/// `addresses.arraySummation` field of the deployment JSON at `AVS_DEPLOYMENT_PATH`.
fn resolve_target_address() -> Result<Address, BoxError> {
    if let Ok(addr) = env::var("GAS_KILLER_TARGET_ADDRESS")
        && !addr.is_empty()
    {
        return Ok(addr.parse()?);
    }

    let path = env::var("AVS_DEPLOYMENT_PATH").map_err(
        |_| "set GAS_KILLER_TARGET_ADDRESS or AVS_DEPLOYMENT_PATH to locate the target contract",
    )?;
    let content = fs::read_to_string(&path).map_err(|e| format!("failed to read {path}: {e}"))?;
    let deployment: serde_json::Value = serde_json::from_str(&content)?;
    let addr = deployment
        .get("addresses")
        .and_then(|a| a.get("arraySummation"))
        .and_then(|v| v.as_str())
        .ok_or("addresses.arraySummation not found in deployment JSON")?;
    Ok(addr.parse()?)
}

/// `(transition_index, anchor_hash, caller, contract_calldata, storage_updates)` vectors.
///
/// Both dynamic fields (`contract_calldata` and `storage_updates`) are swept across the
/// dynamic-bytes padding boundaries (0, sub-word, exactly one word, word+1, multi-word) so the
/// two-tail ABI layout is exercised, along with anchor/caller/index edges.
fn test_vectors() -> Vec<(u64, B256, Address, Vec<u8>, Vec<u8>)> {
    let anchors = [
        B256::ZERO,
        B256::from([0x11; 32]),
        B256::from([0xde; 32]),
        B256::from([0xff; 32]),
    ];
    let callers = [
        Address::ZERO,
        Address::from([0x22; 20]),
        Address::from([0xca; 20]),
    ];
    let calldata_lengths = [0usize, 4, 36, 33];
    let update_lengths = [0usize, 1, 4, 31, 32, 33, 64, 100];
    let indices = [0u64, 1, 7, 1_000_000, u64::MAX];

    let mut vectors = Vec::new();
    for (i, updates_len) in update_lengths.iter().enumerate() {
        let anchor_hash = anchors[i % anchors.len()];
        let caller = callers[i % callers.len()];
        let transition_index = indices[i % indices.len()];
        let calldata_len = calldata_lengths[i % calldata_lengths.len()];
        let contract_calldata = (0..calldata_len)
            .map(|b| (b as u8).wrapping_mul(13).wrapping_add(3))
            .collect();
        let storage_updates = (0..*updates_len)
            .map(|b| (b as u8).wrapping_mul(7).wrapping_add(1))
            .collect();
        vectors.push((
            transition_index,
            anchor_hash,
            caller,
            contract_calldata,
            storage_updates,
        ));
    }
    vectors
}

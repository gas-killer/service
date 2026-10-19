//! Asserts that `GasKillerTaskData::build_payload_hash` is byte-identical to the deployed
//! contract's `getMessageHash` across a range of payload shapes, and, for a target whose SDK
//! binds `msg.value` into the digest, to its `getMessageHashWithValue` for value-bearing tasks.
//! A target built from an older SDK has no `getMessageHashWithValue`; its value-bearing checks
//! are skipped and reported as such.
//!
//! Run against a chain with a GasKiller target contract deployed (the e2e stack deploys one).
//! Reads `HTTP_RPC` and the target address from `GAS_KILLER_TARGET_ADDRESS` (falling back to the
//! deployed-target key in `AVS_DEPLOYMENT_PATH`).
//!
//! Exits non-zero on any mismatch so the e2e workflow fails.

use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol_types::SolCall;
use gas_killer_common::GasKillerTaskData;
use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;
use gas_killer_common::bindings::valueboundsdk::IValueBoundDigest;
use scripts::deployment::{TARGET_ADDRESS_KEY, target_address};
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

    // A function's selector appears in the runtime code's dispatcher, so its absence means the
    // target was built from an SDK that predates the value-bound digest.
    let binds_value = code
        .windows(4)
        .any(|w| w == IValueBoundDigest::getMessageHashWithValueCall::SELECTOR);
    let valued = IValueBoundDigest::new(target_address, provider.clone());
    let contract = GasKillerSDK::new(target_address, provider);

    let mut checked = 0usize;
    let mut mismatches = 0usize;

    for (transition_index, selector, storage_updates) in test_vectors() {
        let onchain = contract
            .getMessageHash(
                U256::from(transition_index),
                FixedBytes::<4>::from(selector),
                Bytes::from(storage_updates.clone()),
            )
            .call()
            .await
            .map_err(|e| format!("getMessageHash call failed: {e}"))?;

        let task_data = GasKillerTaskData {
            storage_updates: Bytes::from(storage_updates.clone()),
            transition_index,
            target_address,
            call_data: selector.to_vec(),
            from_address: Address::ZERO,
            value: U256::ZERO,
            block_height: 0,
            chain_id: 0,
        };
        let local = FixedBytes::<32>::from(task_data.build_payload_hash(&storage_updates).0);

        checked += 1;
        if !report(
            transition_index,
            &selector,
            &storage_updates,
            U256::ZERO,
            local,
            onchain,
        ) {
            mismatches += 1;
        }
    }

    if binds_value {
        for ((transition_index, selector, storage_updates), value) in test_vectors()
            .into_iter()
            .zip(test_values().into_iter().cycle())
        {
            let onchain = valued
                .getMessageHashWithValue(
                    U256::from(transition_index),
                    FixedBytes::<4>::from(selector),
                    Bytes::from(storage_updates.clone()),
                    value,
                )
                .call()
                .await
                .map_err(|e| format!("getMessageHashWithValue call failed: {e}"))?;

            let task_data = GasKillerTaskData {
                storage_updates: Bytes::from(storage_updates.clone()),
                transition_index,
                target_address,
                call_data: selector.to_vec(),
                from_address: Address::ZERO,
                value,
                block_height: 0,
                chain_id: 0,
            };
            let local = FixedBytes::<32>::from(task_data.build_payload_hash(&storage_updates).0);

            checked += 1;
            if !report(
                transition_index,
                &selector,
                &storage_updates,
                value,
                local,
                onchain,
            ) {
                mismatches += 1;
            }
        }
    } else {
        println!(
            "  skip  value-bearing checks: {target_address} has no getMessageHashWithValue, so its SDK predates the value-bound digest"
        );
    }

    if mismatches > 0 {
        return Err(format!(
            "{mismatches}/{checked} message-hash parity checks failed: local build_payload_hash diverges from the on-chain digest"
        )
        .into());
    }

    println!("✅ All {checked} message-hash parity checks passed");
    Ok(())
}

/// Prints one parity result and returns whether the local and on-chain digests match.
fn report(
    transition_index: u64,
    selector: &[u8; 4],
    storage_updates: &[u8],
    value: U256,
    local: FixedBytes<32>,
    onchain: FixedBytes<32>,
) -> bool {
    let matched = local == onchain;
    if matched {
        println!(
            "  ok    ti={transition_index:<20} selector=0x{} len={:<4} value={value} hash={local}",
            hex_selector(selector),
            storage_updates.len(),
        );
    } else {
        eprintln!(
            "  FAIL  ti={transition_index:<20} selector=0x{} len={:<4} value={value}\n        local   = {local}\n        onchain = {onchain}",
            hex_selector(selector),
            storage_updates.len(),
        );
    }
    matched
}

/// Resolves the target contract address from `GAS_KILLER_TARGET_ADDRESS`, falling back to the
/// deployed-target key of the deployment JSON at `AVS_DEPLOYMENT_PATH`.
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
    let addr = target_address(&deployment).ok_or_else(|| {
        format!("addresses.{TARGET_ADDRESS_KEY} not found in the deployment JSON at {path}")
    })?;
    Ok(addr.parse()?)
}

/// `(transition_index, function_selector, storage_updates)` vectors spanning the dynamic-bytes
/// padding boundaries (0, sub-word, exactly one word, word+1, multi-word) and selector/index edges.
fn test_vectors() -> Vec<(u64, [u8; 4], Vec<u8>)> {
    let selectors = [
        [0x00, 0x00, 0x00, 0x00],
        [0x12, 0x34, 0x56, 0x78],
        [0xde, 0xad, 0xbe, 0xef],
        [0xff, 0xff, 0xff, 0xff],
    ];
    let lengths = [0usize, 1, 4, 31, 32, 33, 64, 100];
    let indices = [0u64, 1, 7, 1_000_000, u64::MAX];

    let mut vectors = Vec::new();
    for (i, len) in lengths.iter().enumerate() {
        let selector = selectors[i % selectors.len()];
        let transition_index = indices[i % indices.len()];
        let storage_updates = (0..*len)
            .map(|b| (b as u8).wrapping_mul(7).wrapping_add(1))
            .collect();
        vectors.push((transition_index, selector, storage_updates));
    }
    vectors
}

/// Non-zero `msg.value`s for the value-bearing checks: one wei, one ether, and the largest
/// value the word can hold.
fn test_values() -> Vec<U256> {
    vec![
        U256::from(1u64),
        U256::from(10u64).pow(U256::from(18u64)),
        U256::MAX,
    ]
}

fn hex_selector(selector: &[u8; 4]) -> String {
    selector.iter().map(|b| format!("{b:02x}")).collect()
}

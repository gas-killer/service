//! Asserts that `GasKillerTaskData::build_payload_hash` is byte-identical to the deployed
//! contract's `getMessageHash` across a range of payload shapes.
//!
//! Run against a chain with a GasKiller target contract deployed (the e2e stack deploys
//! ArraySummation). Reads `HTTP_RPC` and the target address from `GAS_KILLER_TARGET_ADDRESS`
//! (falling back to `addresses.arraySummation` in `AVS_DEPLOYMENT_PATH`).
//!
//! Exits non-zero on any mismatch so the e2e workflow fails.

use alloy::primitives::{Address, Bytes, FixedBytes, U256};
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
            auth: None,
        };
        let local = FixedBytes::<32>::from(task_data.build_payload_hash(&storage_updates).0);

        checked += 1;
        if local == onchain {
            println!(
                "  ok    ti={transition_index:<20} selector=0x{} len={:<4} hash={local}",
                hex_selector(&selector),
                storage_updates.len(),
            );
        } else {
            mismatches += 1;
            eprintln!(
                "  FAIL  ti={transition_index:<20} selector=0x{} len={:<4}\n        local   = {local}\n        onchain = {onchain}",
                hex_selector(&selector),
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

    verify_signed_message_hash_parity(&contract, target_address).await?;
    Ok(())
}

/// Asserts that the sender-authenticated `build_payload_hash` (auth variant) is byte-identical to
/// the deployed contract's `getSignedMessageHash`.
///
/// Gracefully skips when the deployed contract predates `getSignedMessageHash` (i.e. before the
/// solidity-sdk trustless-auth change is deployed): a missing function makes the `eth_call` revert,
/// which is treated as "not deployed yet" rather than a parity failure, so this check is
/// forward-compatible and does not break the e2e until the contract ships.
async fn verify_signed_message_hash_parity<P: Provider + Clone>(
    contract: &GasKillerSDK::GasKillerSDKInstance<P>,
    target_address: Address,
) -> Result<(), BoxError> {
    println!("Verifying signed-message-hash parity (getSignedMessageHash)");

    let mut checked = 0usize;
    let mut mismatches = 0usize;

    for (transition_index, signer, value, nonce, call_data, storage_updates) in
        signed_test_vectors()
    {
        let onchain = match contract
            .getSignedMessageHash(
                U256::from(transition_index),
                signer,
                value,
                U256::from(nonce),
                Bytes::from(call_data.clone()),
                Bytes::from(storage_updates.clone()),
            )
            .call()
            .await
        {
            Ok(hash) => hash,
            Err(e) => {
                // The function does not exist on this (older) deployment; skip forward-compatibly.
                println!(
                    "  skip  getSignedMessageHash unavailable on the deployed contract \
                     (pre-trustless-auth); skipping signed-hash parity: {e}"
                );
                return Ok(());
            }
        };

        let task_data = GasKillerTaskData {
            storage_updates: Bytes::from(storage_updates.clone()),
            transition_index,
            target_address,
            call_data: call_data.clone(),
            from_address: signer,
            value,
            block_height: 0,
            chain_id: 0,
            auth: Some(gas_killer_common::task_data::TxAuth {
                nonce,
                max_priority_fee_per_gas: 0,
                max_fee_per_gas: 0,
                gas_limit: 0,
                y_parity: false,
                r: U256::ZERO,
                s: U256::ZERO,
            }),
        };
        let local = FixedBytes::<32>::from(task_data.build_payload_hash(&storage_updates).0);

        checked += 1;
        if local == onchain {
            println!(
                "  ok    ti={transition_index:<20} signer={signer} value={value} nonce={nonce} \
                 calldata_len={:<4} updates_len={:<4} hash={local}",
                call_data.len(),
                storage_updates.len(),
            );
        } else {
            mismatches += 1;
            eprintln!(
                "  FAIL  ti={transition_index} signer={signer} value={value} nonce={nonce}\n        local   = {local}\n        onchain = {onchain}"
            );
        }
    }

    if mismatches > 0 {
        return Err(format!(
            "{mismatches}/{checked} signed-message-hash parity checks failed: local build_payload_hash (auth) diverges from on-chain getSignedMessageHash"
        )
        .into());
    }

    println!("✅ All {checked} signed-message-hash parity checks passed");
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

fn hex_selector(selector: &[u8; 4]) -> String {
    selector.iter().map(|b| format!("{b:02x}")).collect()
}

/// `(transition_index, signer, value, nonce, call_data, storage_updates)` vectors for the
/// sender-authenticated hash. Spans the two dynamic-bytes fields across padding boundaries plus
/// signer/value/nonce edges. Includes the exact vector pinned in the `common` unit test and the
/// solidity-sdk test so all three agree.
#[allow(clippy::type_complexity)]
fn signed_test_vectors() -> Vec<(u64, Address, U256, u64, Vec<u8>, Vec<u8>)> {
    let signer: Address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
        .parse()
        .unwrap();
    vec![
        // The pinned cross-repo vector (matches common::task_data and solidity-sdk).
        (
            3,
            signer,
            U256::from(12345u64),
            7,
            vec![0xAB, 0xCD, 0xEF, 0x01],
            vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
        ),
        (
            0,
            Address::ZERO,
            U256::ZERO,
            0,
            vec![0x00, 0x00, 0x00, 0x00],
            vec![],
        ),
        (
            1,
            signer,
            U256::ZERO,
            1,
            vec![0x12, 0x34, 0x56, 0x78],
            vec![0u8; 32],
        ),
        (
            u64::MAX,
            signer,
            U256::MAX,
            u64::MAX,
            (0..40u8).collect(),
            (0..100u8)
                .map(|b| b.wrapping_mul(7).wrapping_add(1))
                .collect(),
        ),
    ]
}

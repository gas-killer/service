//! E2E test for the Ethereum JSON-RPC ingress (`/rpc`).
//!
//! Simulates a user who switched their RPC URL over to the Gas Killer router: every read the
//! flow needs (chain id, nonce, gas price, block number) is fetched *through the router*, a real
//! EIP-1559 transaction is signed locally with `PRIVATE_KEY`, submitted via
//! `eth_sendRawTransaction`, and success is asserted by watching `ArraySummation.currentSum()`
//! change on-chain — proving the raw transaction was converted into a Gas Killer task, signed by
//! the operator quorum, and executed through `verifyAndUpdate`.
//!
//! Also asserts endpoint resilience: batch requests, duplicate-submission idempotency (no double
//! state transition), and clean JSON-RPC errors for garbage bytes, wrong-chain signatures, and
//! unsupported methods.
//!
//! Required env: `HTTP_RPC`, `PRIVATE_KEY`, `GAS_KILLER_TARGET_ADDRESS`, `GAS_KILLER_API_KEY`.
//! Optional: `GAS_KILLER_ROUTER_URL` (default `http://localhost:8080`).

use alloy::consensus::{SignableTransaction, TxEip1559};
use alloy::eips::eip2718::Encodable2718;
use alloy::network::TxSignerSync;
use alloy::primitives::{Address, TxKind, U256, hex};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolCall;
use bindings::arraysummation::ArraySummation::sumCall;
use serde_json::{Value, json};
use std::env;
use std::time::Duration;
use url::Url;

type AnyErr = Box<dyn std::error::Error + Send + Sync>;

struct RpcClient {
    http: reqwest::Client,
    url: String,
    next_id: std::cell::Cell<u64>,
}

impl RpcClient {
    fn new(url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            url,
            next_id: std::cell::Cell::new(1),
        }
    }

    /// Sends one JSON-RPC request and returns the full response object.
    async fn call_raw(&self, method: &str, params: Value) -> Result<Value, AnyErr> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let resp = self.http.post(&self.url).json(&body).send().await?;
        let status = resp.status();
        let json: Value = resp
            .json()
            .await
            .map_err(|e| format!("{method}: response was not JSON (HTTP {status}): {e}"))?;
        Ok(json)
    }

    /// Sends one JSON-RPC request and returns its `result`, failing on any `error`.
    async fn call(&self, method: &str, params: Value) -> Result<Value, AnyErr> {
        let resp = self.call_raw(method, params).await?;
        if !resp["error"].is_null() {
            return Err(format!("{method} returned error: {}", resp["error"]).into());
        }
        Ok(resp["result"].clone())
    }

    /// Sends a request expected to fail; returns the JSON-RPC error object.
    async fn call_expect_error(&self, method: &str, params: Value) -> Result<Value, AnyErr> {
        let resp = self.call_raw(method, params).await?;
        if resp["error"].is_null() {
            return Err(format!("{method} unexpectedly succeeded: {}", resp["result"]).into());
        }
        Ok(resp["error"].clone())
    }
}

fn hex_to_u64(v: &Value, what: &str) -> Result<u64, AnyErr> {
    let s = v
        .as_str()
        .ok_or_else(|| format!("{what} is not a string: {v}"))?;
    Ok(u64::from_str_radix(s.trim_start_matches("0x"), 16)?)
}

fn hex_to_u128(v: &Value, what: &str) -> Result<u128, AnyErr> {
    let s = v
        .as_str()
        .ok_or_else(|| format!("{what} is not a string: {v}"))?;
    Ok(u128::from_str_radix(s.trim_start_matches("0x"), 16)?)
}

fn env_var(name: &str) -> Result<String, AnyErr> {
    env::var(name).map_err(|_| format!("{name} environment variable is required").into())
}

/// Resolves the ArraySummation target: `GAS_KILLER_TARGET_ADDRESS` when it parses, otherwise
/// `addresses.arraySummation` from the `AVS_DEPLOYMENT_PATH` JSON (same fallback `send_request`
/// uses). The env var can carry garbage when the e2e script's jq-less extraction fallback runs,
/// so an unparseable value falls through to the deployment file instead of failing the run.
fn resolve_target_address() -> Result<Address, AnyErr> {
    if let Ok(raw) = env::var("GAS_KILLER_TARGET_ADDRESS") {
        match raw.trim().parse::<Address>() {
            Ok(addr) => return Ok(addr),
            Err(e) => eprintln!(
                "GAS_KILLER_TARGET_ADDRESS {raw:?} is not an address ({e}); falling back to AVS_DEPLOYMENT_PATH"
            ),
        }
    }
    let path = env::var("AVS_DEPLOYMENT_PATH")
        .map_err(|_| "set GAS_KILLER_TARGET_ADDRESS or AVS_DEPLOYMENT_PATH")?;
    let deployment: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let addr = deployment["addresses"]["arraySummation"]
        .as_str()
        .ok_or_else(|| format!("no addresses.arraySummation in {path}"))?;
    Ok(addr.parse()?)
}

#[tokio::main]
async fn main() -> Result<(), AnyErr> {
    dotenv::dotenv().ok();

    let router_url =
        env::var("GAS_KILLER_ROUTER_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let api_key = env_var("GAS_KILLER_API_KEY")?;
    let target_address = resolve_target_address()?;
    let signer: PrivateKeySigner = env_var("PRIVATE_KEY")?.parse()?;
    let from = signer.address();

    // Wallet-style path-key auth: the URL is the only thing a wallet user configures.
    let rpc = RpcClient::new(format!(
        "{}/rpc/{}",
        router_url.trim_end_matches('/'),
        api_key
    ));

    // Direct chain connection, used only for ground-truth assertions.
    let direct = ProviderBuilder::new().connect_http(Url::parse(&env_var("HTTP_RPC")?)?);
    let contract = bindings::arraysummation::ArraySummation::new(target_address, direct.clone());

    println!("== Step 1: read methods through the router RPC (drop-in check) ==");
    let chain_id = hex_to_u64(&rpc.call("eth_chainId", json!([])).await?, "eth_chainId")?;
    let direct_chain_id = direct.get_chain_id().await?;
    assert_eq!(
        chain_id, direct_chain_id,
        "router-proxied chain id must match the chain"
    );
    println!("   eth_chainId: {chain_id} ✓ (matches direct RPC)");

    let block_number = hex_to_u64(
        &rpc.call("eth_blockNumber", json!([])).await?,
        "eth_blockNumber",
    )?;
    assert!(block_number > 0, "block number should be nonzero");
    println!("   eth_blockNumber: {block_number} ✓");

    let nonce = hex_to_u64(
        &rpc.call("eth_getTransactionCount", json!([from, "latest"]))
            .await?,
        "eth_getTransactionCount",
    )?;
    println!("   eth_getTransactionCount({from}): {nonce} ✓");

    let gas_price = hex_to_u128(&rpc.call("eth_gasPrice", json!([])).await?, "eth_gasPrice")?;
    println!("   eth_gasPrice: {gas_price} ✓");

    let accounts = rpc.call("eth_accounts", json!([])).await?;
    assert_eq!(accounts, json!([]), "eth_accounts must be empty");
    println!("   eth_accounts: [] ✓");

    println!("== Step 2: batch request ==");
    let batch_body = json!([
        {"jsonrpc": "2.0", "id": 100, "method": "eth_chainId", "params": []},
        {"jsonrpc": "2.0", "id": 101, "method": "eth_blockNumber", "params": []},
    ]);
    let batch_resp: Value = rpc
        .http
        .post(&rpc.url)
        .json(&batch_body)
        .send()
        .await?
        .json()
        .await?;
    let entries = batch_resp
        .as_array()
        .ok_or("batch response must be an array")?;
    assert_eq!(entries.len(), 2, "batch must answer both entries");
    assert_eq!(entries[0]["id"], 100);
    assert_eq!(entries[1]["id"], 101);
    assert!(!entries[0]["result"].is_null() && !entries[1]["result"].is_null());
    println!("   2-entry batch answered in order ✓");

    println!("== Step 3: negative checks ==");
    let err = rpc
        .call_expect_error("eth_sendRawTransaction", json!(["0xdeadbeef"]))
        .await?;
    assert_eq!(err["code"], -32602, "garbage bytes must be -32602: {err}");
    println!("   garbage raw tx rejected with -32602 ✓");

    let err = rpc.call_expect_error("debug_traceCall", json!([])).await?;
    assert_eq!(err["code"], -32601, "debug_ must not be proxied: {err}");
    println!("   debug_traceCall rejected with -32601 ✓");

    let err = rpc
        .call_expect_error(
            "eth_sendTransaction",
            json!([{"from": from, "to": target_address}]),
        )
        .await?;
    assert_eq!(err["code"], -32601);
    println!("   eth_sendTransaction rejected with guidance ✓");

    // A transaction signed for a different chain must be refused, not silently executed here.
    let mut wrong_chain_tx =
        build_sum_tx(&contract, 999_999, nonce, gas_price, target_address).await?;
    let sig = signer.sign_transaction_sync(&mut wrong_chain_tx)?;
    let wrong_raw =
        alloy::consensus::TxEnvelope::from(wrong_chain_tx.into_signed(sig)).encoded_2718();
    let err = rpc
        .call_expect_error(
            "eth_sendRawTransaction",
            json!([format!("0x{}", hex::encode(wrong_raw))]),
        )
        .await?;
    assert_eq!(err["code"], -32000, "wrong-chain tx must be -32000: {err}");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("chain id"),
        "error should explain the chain id mismatch: {err}"
    );
    println!("   wrong-chain-id signed tx rejected ✓");

    println!("== Step 4: sign and submit a real transaction through the router ==");
    let initial_sum = contract.currentSum().call().await?.to::<u64>();
    let initial_count = contract.stateTransitionCount().call().await?.to::<u64>();
    println!("   initial currentSum={initial_sum} stateTransitionCount={initial_count}");

    let mut tx = build_sum_tx(&contract, chain_id, nonce, gas_price, target_address).await?;
    let sig = signer.sign_transaction_sync(&mut tx)?;
    let envelope = alloy::consensus::TxEnvelope::from(tx.into_signed(sig));
    let expected_hash = format!("{:#x}", envelope.tx_hash());
    let raw = format!("0x{}", hex::encode(envelope.encoded_2718()));

    let result = rpc
        .call("eth_sendRawTransaction", json!([raw.clone()]))
        .await?;
    let returned_hash = result.as_str().ok_or("result must be a hash string")?;
    assert_eq!(
        returned_hash.to_lowercase(),
        expected_hash,
        "returned hash must be the transaction hash"
    );
    println!("   eth_sendRawTransaction accepted, hash {returned_hash} ✓");

    println!("== Step 5: duplicate submission is idempotent ==");
    let dup = rpc.call("eth_sendRawTransaction", json!([raw])).await?;
    assert_eq!(
        dup.as_str().map(str::to_lowercase),
        Some(expected_hash.clone()),
        "duplicate must return the same hash"
    );
    println!("   duplicate returned the same hash ✓");

    println!("== Step 6: wait for on-chain execution ==");
    let max_wait = Duration::from_secs(150);
    let interval = Duration::from_secs(10);
    let start = tokio::time::Instant::now();
    loop {
        let sum = contract.currentSum().call().await?.to::<u64>();
        println!(
            "   currentSum: {sum} (initial {initial_sum}), elapsed {:.0}s",
            start.elapsed().as_secs_f64()
        );
        if sum != initial_sum {
            println!("✅ currentSum changed: raw transaction executed through Gas Killer");
            break;
        }
        if start.elapsed() >= max_wait {
            return Err("timeout waiting for currentSum to change".into());
        }
        tokio::time::sleep(interval).await;
    }

    println!("== Step 7: duplicate did not double-execute ==");
    let count = contract.stateTransitionCount().call().await?.to::<u64>();
    assert_eq!(
        count,
        initial_count + 1,
        "exactly one state transition expected"
    );
    // A duplicated task would follow shortly after the first round; give it a window to show up.
    tokio::time::sleep(Duration::from_secs(30)).await;
    let count_after = contract.stateTransitionCount().call().await?.to::<u64>();
    assert_eq!(
        count_after,
        initial_count + 1,
        "duplicate submission must not cause a second state transition"
    );
    println!("   stateTransitionCount stayed at {count_after} ✓");

    println!("✅ SUCCESS: JSON-RPC ingress e2e passed");
    Ok(())
}

/// Builds an unsigned EIP-1559 `sum(uint256[])` call whose indexes vary with the current state
/// transition count, so each successful execution changes `currentSum`.
async fn build_sum_tx<P: Provider + Clone>(
    contract: &bindings::arraysummation::ArraySummation::ArraySummationInstance<P>,
    chain_id: u64,
    nonce: u64,
    gas_price: u128,
    target_address: Address,
) -> Result<TxEip1559, AnyErr> {
    let current_count = contract.stateTransitionCount().call().await?.to::<u64>();
    let base_idx = (current_count * 3) % 97;
    let call = sumCall {
        indexes: vec![
            U256::from(base_idx),
            U256::from(base_idx + 1),
            U256::from(base_idx + 2),
        ],
    };
    println!(
        "   calldata: sum([{}, {}, {}])",
        base_idx,
        base_idx + 1,
        base_idx + 2
    );
    Ok(TxEip1559 {
        chain_id,
        nonce,
        gas_limit: 1_000_000,
        max_fee_per_gas: gas_price.saturating_mul(2).max(1_000_000_000),
        max_priority_fee_per_gas: 1_000_000_000,
        to: TxKind::Call(target_address),
        value: U256::ZERO,
        input: call.abi_encode().into(),
        ..Default::default()
    })
}

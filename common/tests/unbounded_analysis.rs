//! Native end-to-end test of the unbounded analysis path.
//!
//! Proves the service-side half of unbounded mode without the docker stack:
//! an ArraySummation whose direct `sum()` costs more than the mainnet block
//! gas limit is analyzable by `GasKillerValidator` **only** under
//! `GK_SIM_PROFILE=unbounded-v1`, and the resulting signed payload is the
//! small O(1) state diff. The full-stack proof (3 nodes + router + anvil +
//! `verifyAndUpdate` on-chain) is `scripts/run_unbounded_e2e_test.sh`.
//!
//! Requires `anvil` (foundry) on PATH; the test soft-skips otherwise so plain
//! `cargo test` environments stay green.

use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, U256, address, hex};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use gas_killer_common::validator::GasKillerValidator;

sol! {
    interface IArraySummation {
        /// Empty `indexes` sums the whole array — one cold SLOAD per element.
        function sum(uint256[] calldata indexes) external;
    }
}

/// Creation bytecode built from gas-killer/solidity-sdk `main`
/// (`out/ArraySummation.sol/ArraySummation.json`, solc 0.8.x via forge).
const CREATION_HEX: &str = include_str!("fixtures/ArraySummation.creation.hex");

const MAINNET_BLOCK_GAS_LIMIT: u64 = 30_000_000;
/// ~20k elements ≈ one cold SLOAD each in `sum()` → ~43M gas, safely above the limit.
const ARRAY_SIZE: u64 = 20_000;

struct LocalAnvil {
    child: std::process::Child,
    url: String,
}

impl LocalAnvil {
    async fn spawn() -> Option<LocalAnvil> {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("bind to pick a free port")
            .local_addr()
            .expect("local_addr")
            .port();
        let child = match std::process::Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--silent",
                "--disable-block-gas-limit",
                // gas-analyzer's executor supports mainnet/sepolia/gnosis chain
                // ids only; the docker stack forks Sepolia, mirror that here.
                "--chain-id",
                "11155111",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return None, // anvil not installed — soft-skip
        };
        let anvil = LocalAnvil {
            child,
            url: format!("http://127.0.0.1:{port}"),
        };
        let provider = ProviderBuilder::new().connect_http(anvil.url.parse().unwrap());
        for _ in 0..100 {
            if provider.get_chain_id().await.is_ok() {
                return Some(anvil);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("anvil did not become ready within 5s");
    }
}

impl Drop for LocalAnvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Deploys ArraySummation(avs, checker, ARRAY_SIZE, maxValue, seed). The
/// constructor writes one storage slot per element (~450M gas at 20k
/// elements) — only possible because anvil runs without a block gas limit.
async fn deploy_big_array(provider: &impl Provider, from: Address) -> Address {
    let mut initcode = hex::decode(CREATION_HEX.trim()).expect("valid fixture hex");
    // abi.encode(address avs, address checker, uint256 size, uint256 maxValue, uint256 seed)
    let mut word = [0u8; 32];
    let args: [U256; 5] = [
        U256::from_be_slice(address!("0x0000000000000000000000000000000000000a75").as_slice()),
        U256::from_be_slice(address!("0x0000000000000000000000000000000000000b15").as_slice()),
        U256::from(ARRAY_SIZE),
        U256::from(1_000u64),
        U256::from(42u64),
    ];
    for arg in args {
        word.copy_from_slice(&arg.to_be_bytes::<32>());
        initcode.extend_from_slice(&word);
    }

    let tx = TransactionRequest::default()
        .with_from(from)
        .with_deploy_code(initcode)
        .with_gas_limit(2_000_000_000);
    let receipt = provider
        .send_transaction(tx)
        .await
        .expect("send deploy tx")
        .get_receipt()
        .await
        .expect("deploy receipt");
    assert!(receipt.status(), "ArraySummation deploy reverted");
    receipt
        .contract_address
        .expect("deploy receipt has contract address")
}

#[tokio::test(flavor = "multi_thread")]
async fn unbounded_profile_analyzes_above_block_gas_limit_sum() {
    let Some(anvil) = LocalAnvil::spawn().await else {
        eprintln!("skipping: anvil (foundry) not on PATH");
        return;
    };
    let provider = ProviderBuilder::new().connect_http(anvil.url.parse().unwrap());
    // anvil's first funded dev account.
    let caller = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

    let contract = deploy_big_array(&provider, caller).await;
    let call_data = IArraySummation::sumCall { indexes: vec![] }.abi_encode();

    // The premise: direct execution cannot fit in a real block.
    let direct_gas = provider
        .estimate_gas(
            TransactionRequest::default()
                .with_from(caller)
                .with_to(contract)
                .with_input(call_data.clone()),
        )
        .await
        .expect("estimate direct sum()");
    assert!(
        direct_gas > MAINNET_BLOCK_GAS_LIMIT,
        "premise failed: direct sum() must exceed the block gas limit, got {direct_gas}"
    );

    let block_height = provider.get_block_number().await.expect("block number");

    // GK_SIM_PROFILE is read at validator construction. Only the unbounded
    // profile is exercised here: the Chain-profile failure mode (tracer OOG at
    // the node's real limits → analysis error) is node-configuration-dependent
    // — this anvil runs without a block gas limit so its `debug_traceCall`
    // defaults to unlimited call gas — and is covered by gas-analyzer's own
    // anvil-backed profile tests.
    unsafe { std::env::set_var("GK_SIM_PROFILE", "unbounded-v1") };
    let unbounded_validator = GasKillerValidator::with_rpc_url(&anvil.url);
    let analysis = unbounded_validator
        .analyze_transaction(
            &anvil.url,
            contract,
            &call_data,
            Some(caller),
            None,
            block_height,
        )
        .await
        .expect("unbounded-profile analysis must succeed on an above-block-limit call");

    assert!(
        !analysis.storage_updates.is_empty(),
        "analysis must produce a signed payload"
    );
    assert!(
        analysis.storage_updates.len() < 4_096,
        "the payload must be an O(1) diff, got {} bytes for a {direct_gas}-gas execution",
        analysis.storage_updates.len()
    );
    assert!(
        analysis.gas_estimate < MAINNET_BLOCK_GAS_LIMIT,
        "applying the diff on-chain must fit a real block, estimated {}",
        analysis.gas_estimate
    );

    println!(
        "direct sum(): {direct_gas} gas (> {MAINNET_BLOCK_GAS_LIMIT} block limit); \
         gas-killer payload: {} bytes, applies for ~{} gas",
        analysis.storage_updates.len(),
        analysis.gas_estimate
    );
}

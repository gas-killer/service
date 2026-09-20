//! Native end-to-end test of the UNBOUNDED_V3 (gkvm) analysis path.
//!
//! Proves the service-side half of guest-VM tasks without the docker stack:
//! an operator that installed guest programs through `GK_GUEST_PROGRAM[_N]`
//! analyzes tracked functions that STATICCALL the gkvm precompile, under
//! `GK_SIM_EXECUTOR=local`, and the payload it would sign is byte-identical
//! to what forge recorded for the same consumer bytecode + calldata against
//! the ffi shim (`fixtures/gkvm/parity_tasks.json`, written by
//! gas-killer/solidity-sdk `make -C tools/gk parity`; the two ELFs are
//! gas-analyzer's committed M1 guests). An operator WITHOUT the program gets
//! an analysis error — it abstains, it never signs. An operator on the `rpc`
//! executor does not get that error by itself (it would sign the consumer's
//! `GkVmUnavailable` revert transition): the `requiresGuestVm` registry
//! (`GK_GUEST_VM_CONSUMERS`) is what makes it abstain, and the last phase
//! shows both halves.
//!
//! The chain node stays vanilla: anvil knows nothing about the precompile.
//!
//! Requires `anvil` (foundry) on PATH; the test soft-skips otherwise so plain
//! `cargo test` environments stay green. One `#[test]` on purpose: the phases
//! reconfigure the process environment, which concurrent tests would race.

use std::sync::Arc;

use alloy::primitives::{Address, B256, Bytes, address, keccak256};
use alloy::providers::{Provider, ProviderBuilder};
use gas_killer_common::local_exec_shim::{GkvmHost, GuestProgramSet, GuestVmConsumers};
use gas_killer_common::validator::GasKillerValidator;

const PARITY_TASKS: &str = include_str!("fixtures/gkvm/parity_tasks.json");
const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gkvm");

/// anvil's first funded dev account.
const CALLER: Address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
const CONSUMER: Address = address!("0x00000000000000000000000000000000000050a1");

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParityFixture {
    consumer_code: Bytes,
    tasks: Vec<ParityTask>,
}

/// One forge-recorded task: `calldata` sent to a fresh consumer, and the
/// encoded `StateUpdate` payload forge observed.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParityTask {
    vector: String,
    #[serde(rename = "fn")]
    function: String,
    calldata: Bytes,
    success: bool,
    storage_updates: Bytes,
}

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

fn program_hash(elf: &str) -> B256 {
    keccak256(std::fs::read(format!("{FIXTURE_DIR}/{elf}")).expect("fixture ELF"))
}

/// Same 2-worker runtime as router/node main.rs (commonware's default), the
/// one the blocking-pool offload in `analyze_transaction` exists for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_guest_programs_serve_gkvm_tasks_and_a_missing_one_abstains() {
    let Some(anvil) = LocalAnvil::spawn().await else {
        eprintln!("skipping: anvil (foundry) not on PATH");
        return;
    };
    let provider = ProviderBuilder::new().connect_http(anvil.url.parse().unwrap());

    let fixture: ParityFixture = serde_json::from_str(PARITY_TASKS).expect("parity_tasks.json");
    assert_eq!(fixture.tasks.len(), 17);
    // The consumer's production runtime code (GKVM = the canonical address),
    // fresh storage — what the forge leg ran every task against.
    provider
        .raw_request::<_, ()>(
            "anvil_setCode".into(),
            (CONSUMER, fixture.consumer_code.clone()),
        )
        .await
        .expect("anvil_setCode");
    let block_height = provider.get_block_number().await.expect("block number");

    let hello = program_hash("hello-c.elf");
    let bench = program_hash("bench-c.elf");

    // ---- an operator with both programs installed through the environment
    // (read at validator construction) --------------------------------------
    unsafe {
        std::env::set_var("GK_SIM_EXECUTOR", "local");
        std::env::set_var("GK_GUEST_PROGRAM", format!("{FIXTURE_DIR}/hello-c.elf"));
        std::env::set_var("GK_GUEST_PROGRAM_HASH", hello.to_string());
        std::env::set_var("GK_GUEST_PROGRAM_1", format!("{FIXTURE_DIR}/bench-c.elf"));
        std::env::set_var("GK_GUEST_PROGRAM_HASH_1", bench.to_string());
    }
    let mut installed = [hello, bench];
    installed.sort_unstable();

    // `unbounded-v1` is the profile guest-VM consumers run under. NOT `chain`:
    // there the local executor sends the block gas limit as the tx gas, which
    // an Osaka chain (this anvil; Sepolia) rejects above the 2^24 tx cap
    // (`TxGasLimitGreaterThanCap`) before any code runs — a property of the
    // local executor's Chain profile, nothing to do with the guest VM.
    {
        let profile = "unbounded-v1";
        unsafe { std::env::set_var("GK_SIM_PROFILE", profile) };
        let validator = GasKillerValidator::with_rpc_url(&anvil.url);
        let host = validator.gkvm_host().expect("GK_GUEST_* configured a host");
        assert_eq!(host.programs().program_hashes(), installed);

        for task in &fixture.tasks {
            let analysis = validator
                .analyze_transaction(
                    &anvil.url,
                    CONSUMER,
                    &task.calldata,
                    Some(CALLER),
                    None,
                    block_height,
                )
                .await
                .unwrap_or_else(|e| {
                    panic!("{} via {} ({profile}): {e:#}", task.vector, task.function)
                });
            assert_eq!(
                Bytes::from(analysis.storage_updates),
                task.storage_updates,
                "{} via {} ({profile}): the payload this operator would sign diverged from \
                 the forge leg",
                task.vector,
                task.function
            );
        }
        // The tasks really reached the guest VM. One host serves the whole
        // validator, so `ask` and `probe` of one vector share a memo entry —
        // fewer guest runs than tasks is expected.
        assert!(host.guest_runs() > 0);
        assert!(host.guest_runs() + host.memo_hits() >= fixture.tasks.len() as u64);
        println!(
            "{profile}: {} tasks byte-identical to the forge leg ({} guest runs, {} memo hits)",
            fixture.tasks.len(),
            host.guest_runs(),
            host.memo_hits()
        );
    }

    let answered = fixture
        .tasks
        .iter()
        .find(|task| task.success && task.function == "ask")
        .expect("an answered ask() task");

    // ---- an operator with a guest VM but NOT this program: abstains --------
    let other = GasKillerValidator::with_rpc_url(&anvil.url)
        .with_gkvm_host(Arc::new(GkvmHost::new(GuestProgramSet::default())));
    let err = other
        .analyze_transaction(
            &anvil.url,
            CONSUMER,
            &answered.calldata,
            Some(CALLER),
            None,
            block_height,
        )
        .await
        .expect_err("an operator without the program must not produce a payload");
    assert!(format!("{err:#}").contains("is not installed"), "{err:#}");

    // ---- an operator with no guest VM configured at all: abstains ----------
    unsafe {
        for var in [
            "GK_GUEST_PROGRAM",
            "GK_GUEST_PROGRAM_HASH",
            "GK_GUEST_PROGRAM_1",
            "GK_GUEST_PROGRAM_HASH_1",
        ] {
            std::env::remove_var(var);
        }
    }
    let bare = GasKillerValidator::with_rpc_url(&anvil.url);
    assert!(bare.gkvm_host().is_none());
    let err = bare
        .analyze_transaction(
            &anvil.url,
            CONSUMER,
            &answered.calldata,
            Some(CALLER),
            None,
            block_height,
        )
        .await
        .expect_err("an operator without a guest VM must not produce a payload");
    assert!(
        format!("{err:#}").contains("no guest programs are configured"),
        "{err:#}"
    );

    // ---- the `requiresGuestVm` gate -----------------------------------------
    // What it exists for: an operator on the rpc executor does NOT abstain by
    // itself. The precompile address is an empty account behind
    // `debug_traceCall`, the consumer reverts `GkVmUnavailable`, and the
    // analysis succeeds with a payload the local operators never produce.
    unsafe { std::env::set_var("GK_SIM_EXECUTOR", "rpc") };
    let registry = GuestVmConsumers::default().with_consumer(CONSUMER, hello);
    let ungated_rpc = GasKillerValidator::with_rpc_url(&anvil.url);
    let divergent = ungated_rpc
        .analyze_transaction(
            &anvil.url,
            CONSUMER,
            &answered.calldata,
            Some(CALLER),
            None,
            block_height,
        )
        .await
        .expect("without the registry the rpc executor analyzes the task");
    println!(
        "rpc, ungated: a {}-byte payload (keccak {}) vs the local operators' {} bytes (keccak {})",
        divergent.storage_updates.len(),
        keccak256(&divergent.storage_updates),
        answered.storage_updates.len(),
        keccak256(&answered.storage_updates)
    );
    assert_ne!(
        Bytes::from(divergent.storage_updates),
        answered.storage_updates,
        "the rpc executor has no guest VM: it cannot produce the local operators' payload"
    );

    // Registered, the same operator refuses before simulating anything.
    let gated_rpc =
        GasKillerValidator::with_rpc_url(&anvil.url).with_guest_vm_consumers(registry.clone());
    let err = gated_rpc
        .analyze_transaction(
            &anvil.url,
            CONSUMER,
            &answered.calldata,
            Some(CALLER),
            None,
            block_height,
        )
        .await
        .expect_err("a registered guest-VM consumer must not be analyzed under rpc");
    assert!(
        format!("{err:#}").contains("guest-VM gate") && format!("{err:#}").contains("is rpc"),
        "{err:#}"
    );
    // The gate is per consumer: the same code at an unregistered address is
    // still analyzed (and still diverges — registration is what protects).
    let unregistered = address!("0x00000000000000000000000000000000000050a2");
    provider
        .raw_request::<_, ()>(
            "anvil_setCode".into(),
            (unregistered, fixture.consumer_code.clone()),
        )
        .await
        .expect("anvil_setCode");
    let block_height = provider.get_block_number().await.expect("block number");
    gated_rpc
        .analyze_transaction(
            &anvil.url,
            unregistered,
            &answered.calldata,
            Some(CALLER),
            None,
            block_height,
        )
        .await
        .expect("an unregistered consumer is not gated");

    // Under local the gate asks for the consumer's program: read from the
    // environment like an operator would configure it.
    unsafe {
        std::env::set_var("GK_SIM_EXECUTOR", "local");
        std::env::set_var("GK_GUEST_PROGRAM", format!("{FIXTURE_DIR}/bench-c.elf"));
        std::env::set_var("GK_GUEST_PROGRAM_HASH", bench.to_string());
        std::env::set_var("GK_GUEST_VM_CONSUMERS", format!("{CONSUMER}={hello}"));
    }
    let wrong_program = GasKillerValidator::with_rpc_url(&anvil.url);
    let err = wrong_program
        .analyze_transaction(
            &anvil.url,
            CONSUMER,
            &answered.calldata,
            Some(CALLER),
            None,
            block_height,
        )
        .await
        .expect_err("a local operator without the registered program must abstain");
    assert!(
        format!("{err:#}").contains("guest-VM gate")
            && format!("{err:#}").contains(&hello.to_string()),
        "{err:#}"
    );
    let host = wrong_program.gkvm_host().expect("bench-c is installed");
    assert_eq!(
        host.guest_runs(),
        0,
        "the gate refuses before anything runs"
    );

    unsafe {
        std::env::set_var("GK_GUEST_PROGRAM", format!("{FIXTURE_DIR}/hello-c.elf"));
        std::env::set_var("GK_GUEST_PROGRAM_HASH", hello.to_string());
    }
    let gated_local = GasKillerValidator::with_rpc_url(&anvil.url);
    let analysis = gated_local
        .analyze_transaction(
            &anvil.url,
            CONSUMER,
            &answered.calldata,
            Some(CALLER),
            None,
            block_height,
        )
        .await
        .expect("registered consumer, local executor, program installed");
    assert_eq!(
        Bytes::from(analysis.storage_updates),
        answered.storage_updates
    );
    unsafe {
        for var in [
            "GK_GUEST_PROGRAM",
            "GK_GUEST_PROGRAM_HASH",
            "GK_GUEST_VM_CONSUMERS",
        ] {
            std::env::remove_var(var);
        }
    }
}

//! Native end-to-end test of UNBOUNDED_V3 program distribution: the
//! hash-addressed path (`guest_programs`) through the real validator.
//!
//! The operator here has NO `GK_GUEST_PROGRAM` slot. What it has is a mirror
//! (`GK_GUEST_MIRRORS`, a local HTTP server serving `<hash>.elf`) and a cache
//! dir (`GK_GUEST_CACHE_DIR`). The first task naming a program makes the
//! validator fetch it by hash, verify it and install it on the live gkvm
//! host — and the payload it then signs is byte-identical to what forge
//! recorded for the same consumer bytecode + calldata (the parity fixture
//! `gkvm_analysis.rs` also replays). A mirror serving the wrong bytes under
//! the right name is skipped; when no mirror serves the program, the operator
//! abstains exactly like one without the slot.
//!
//! The binding comes from the chain when the consumer exposes it: a contract
//! answering `programHash()` resolves to that program with no registry entry,
//! while the parity consumer (which exposes nothing) resolves to `None` and
//! stays bound by `GK_GUEST_VM_CONSUMERS`.
//!
//! Requires `anvil` (foundry) on PATH; the test soft-skips otherwise. One
//! `#[test]` on purpose: the phases reconfigure the process environment.

use std::collections::HashMap;
use std::sync::Arc;

use alloy::primitives::{Address, B256, Bytes, address, keccak256};
use alloy::providers::{Provider, ProviderBuilder};
use gas_killer_common::guest_programs::{GuestProgramFetcher, GuestProgramResolver, Installed};
use gas_killer_common::local_exec_shim::GuestVmConsumers;
use gas_killer_common::validator::GasKillerValidator;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PARITY_TASKS: &str = include_str!("fixtures/gkvm/parity_tasks.json");
const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gkvm");

/// anvil's first funded dev account.
const CALLER: Address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
/// The parity consumer: exposes no program getter.
const CONSUMER: Address = address!("0x00000000000000000000000000000000000050a1");
/// A contract that answers every call with one word: a `programHash()` getter.
const EXPOSER: Address = address!("0x00000000000000000000000000000000000050b1");

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParityFixture {
    consumer_code: Bytes,
    tasks: Vec<ParityTask>,
}

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
                "--chain-id",
                "11155111",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return None,
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

/// Runtime code returning `word` to every call: `PUSH32 word PUSH1 0 MSTORE
/// PUSH1 32 PUSH1 0 RETURN`.
fn constant_getter(word: B256) -> Bytes {
    let mut code = vec![0x7f];
    code.extend_from_slice(word.as_slice());
    code.extend_from_slice(&[0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3]);
    Bytes::from(code)
}

/// A mirror: serves `objects` by path (`/<name>` → bytes), 404 otherwise.
/// Returns its base URL.
async fn mirror(objects: HashMap<String, Vec<u8>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let objects = Arc::new(objects);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let objects = Arc::clone(&objects);
            tokio::spawn(async move {
                let mut request = vec![0u8; 8192];
                let read = socket.read(&mut request).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&request[..read]).to_string();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .trim_start_matches('/')
                    .to_string();
                let (status, body) = match objects.get(&path) {
                    Some(bytes) => ("200 OK", bytes.clone()),
                    None => ("404 Not Found", b"no such object".to_vec()),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.write_all(&body).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{addr}")
}

fn fixture_elf(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURE_DIR}/{name}")).expect("fixture ELF")
}

/// Same 2-worker runtime as router/node main.rs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn programs_are_fetched_by_hash_at_first_use_and_bound_by_the_consumer() {
    let Some(anvil) = LocalAnvil::spawn().await else {
        eprintln!("skipping: anvil (foundry) not on PATH");
        return;
    };
    let provider = ProviderBuilder::new().connect_http(anvil.url.parse().unwrap());
    let fixture: ParityFixture = serde_json::from_str(PARITY_TASKS).expect("parity_tasks.json");

    let hello_elf = fixture_elf("hello-c.elf");
    let bench_elf = fixture_elf("bench-c.elf");
    let hello = keccak256(&hello_elf);
    let bench = keccak256(&bench_elf);

    provider
        .raw_request::<_, ()>(
            "anvil_setCode".into(),
            (CONSUMER, fixture.consumer_code.clone()),
        )
        .await
        .expect("anvil_setCode consumer");
    provider
        .raw_request::<_, ()>("anvil_setCode".into(), (EXPOSER, constant_getter(hello)))
        .await
        .expect("anvil_setCode exposer");
    let block_height = provider.get_block_number().await.expect("block number");

    // ---- the resolver: the chain names the program ---------------------------
    let resolver = GuestProgramResolver::new();
    assert_eq!(
        resolver
            .program_hash(&provider, EXPOSER, block_height)
            .await
            .expect("chain answers"),
        Some(hello),
        "a consumer exposing programHash() resolves to it"
    );
    assert_eq!(
        resolver
            .program_hash(&provider, CONSUMER, block_height)
            .await
            .expect("chain answers"),
        None,
        "the parity consumer exposes nothing"
    );
    assert_eq!(
        resolver
            .program_hash(&provider, Address::repeat_byte(0x77), block_height)
            .await
            .expect("chain answers"),
        None,
        "an empty account exposes nothing"
    );

    // ---- an operator with NO program slot, a mirror and a cache dir ----------
    let good_mirror = mirror(HashMap::from([
        (GuestProgramFetcher::object_name(&hello), hello_elf.clone()),
        (GuestProgramFetcher::object_name(&bench), bench_elf.clone()),
    ]))
    .await;
    let lying_mirror = mirror(HashMap::from([(
        GuestProgramFetcher::object_name(&hello),
        b"the wrong program under the right name".to_vec(),
    )]))
    .await;
    let cache = tempfile::tempdir().expect("cache dir");

    unsafe {
        std::env::set_var("GK_SIM_EXECUTOR", "local");
        std::env::set_var("GK_SIM_PROFILE", "unbounded-v1");
        // The lying mirror comes first: it must be skipped, not trusted.
        std::env::set_var("GK_GUEST_MIRRORS", format!("{lying_mirror}, {good_mirror}"));
        std::env::set_var("GK_GUEST_CACHE_DIR", cache.path().display().to_string());
        for var in [
            "GK_GUEST_PROGRAM",
            "GK_GUEST_PROGRAM_HASH",
            "GK_GUEST_VM_CONSUMERS",
        ] {
            std::env::remove_var(var);
        }
    }
    // The parity consumer runs both M1 guests; the registry names both, and the
    // gate installs every program it names for the consumer at the first task.
    let registry = GuestVmConsumers::default()
        .with_consumer(CONSUMER, hello)
        .with_consumer(CONSUMER, bench);
    let fetching = GasKillerValidator::with_rpc_url(&anvil.url).with_guest_vm_consumers(registry);
    let host = fetching
        .gkvm_host()
        .expect("mirrors make a (still empty) host exist");
    assert_eq!(host.programs().program_count(), 0);
    assert!(fetching.guest_program_fetcher().is_some());

    let answered = fixture
        .tasks
        .iter()
        .find(|task| task.success && task.function == "ask")
        .expect("an answered ask() task");
    let analysis = fetching
        .analyze_transaction(
            &anvil.url,
            CONSUMER,
            &answered.calldata,
            Some(CALLER),
            None,
            block_height,
        )
        .await
        .expect("the program is fetched by hash at first use");
    assert_eq!(
        Bytes::from(analysis.storage_updates),
        answered.storage_updates,
        "a program fetched by hash signs the same bytes as a boot-time slot"
    );
    assert!(host.has_program(&hello));
    assert!(host.has_program(&bench), "every program the registry names is installed");
    let cached = cache.path().join(GuestProgramFetcher::object_name(&hello));
    assert_eq!(std::fs::read(&cached).expect("cached copy"), hello_elf);

    // Every task of the fixture, same operator: the install happened once.
    for task in &fixture.tasks {
        let analysis = fetching
            .analyze_transaction(
                &anvil.url,
                CONSUMER,
                &task.calldata,
                Some(CALLER),
                None,
                block_height,
            )
            .await
            .unwrap_or_else(|e| panic!("{} via {}: {e:#}", task.vector, task.function));
        assert_eq!(
            Bytes::from(analysis.storage_updates),
            task.storage_updates,
            "{} via {}",
            task.vector,
            task.function
        );
    }
    println!(
        "{} tasks byte-identical to the forge leg with the program fetched from {good_mirror}",
        fixture.tasks.len()
    );

    // ---- the chain as the binding: a consumer that exposes its program needs
    // no registry entry, and the gate fetches what it names ----------------------
    let exposed = GasKillerValidator::with_rpc_url(&anvil.url);
    let exposed_host = exposed.gkvm_host().expect("host");
    assert!(!exposed_host.has_program(&hello));
    // What the EXPOSER does when called is irrelevant here (it is not a Gas
    // Killer consumer); the gate ran before the simulation and installed what
    // the chain named.
    let _ = exposed
        .analyze_transaction(
            &anvil.url,
            EXPOSER,
            &[0u8; 4],
            Some(CALLER),
            None,
            block_height,
        )
        .await;
    assert!(
        exposed_host.has_program(&hello),
        "the gate installed the program the consumer exposes on-chain"
    );

    // ---- a warm restart: the cache serves, no mirror needed ------------------
    unsafe { std::env::remove_var("GK_GUEST_MIRRORS") };
    let offline = GasKillerValidator::with_rpc_url(&anvil.url)
        .with_guest_vm_consumers(GuestVmConsumers::default().with_consumer(CONSUMER, hello));
    let offline_host = offline.gkvm_host().expect("a cache dir makes a host exist");
    assert_eq!(
        offline
            .guest_program_fetcher()
            .expect("fetcher")
            .ensure_installed(&offline_host, hello)
            .await
            .expect("from the cache"),
        Installed::FromCache(cached.clone())
    );
    let analysis = offline
        .analyze_transaction(
            &anvil.url,
            CONSUMER,
            &answered.calldata,
            Some(CALLER),
            None,
            block_height,
        )
        .await
        .expect("served from the cache");
    assert_eq!(
        Bytes::from(analysis.storage_updates),
        answered.storage_updates
    );

    // ---- no mirror serves the program: abstain, never a different payload ----
    unsafe {
        std::env::set_var("GK_GUEST_MIRRORS", lying_mirror.clone());
        std::env::remove_var("GK_GUEST_CACHE_DIR");
    }
    let starved = GasKillerValidator::with_rpc_url(&anvil.url)
        .with_guest_vm_consumers(GuestVmConsumers::default().with_consumer(CONSUMER, hello));
    let err = starved
        .analyze_transaction(
            &anvil.url,
            CONSUMER,
            &answered.calldata,
            Some(CALLER),
            None,
            block_height,
        )
        .await
        .expect_err("an operator that cannot fetch the program must not produce a payload");
    let text = format!("{err:#}");
    assert!(
        text.contains("guest-VM gate") && text.contains("no mirror served it"),
        "{text}"
    );
    assert!(
        !starved.gkvm_host().expect("host").has_program(&hello),
        "the wrong bytes were never installed"
    );

    // ---- the registry is still honoured when the consumer exposes nothing
    // and no mirror is configured: the pre-distribution behaviour ------------
    unsafe {
        for var in ["GK_GUEST_MIRRORS", "GK_GUEST_CACHE_DIR", "GK_SIM_PROFILE"] {
            std::env::remove_var(var);
        }
    }
    let bare = GasKillerValidator::with_rpc_url(&anvil.url)
        .with_guest_vm_consumers(GuestVmConsumers::default().with_consumer(CONSUMER, hello));
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
        .expect_err("no guest VM at all");
    assert!(
        format!("{err:#}").contains("no guest programs are configured"),
        "{err:#}"
    );
    unsafe { std::env::remove_var("GK_SIM_EXECUTOR") };
}

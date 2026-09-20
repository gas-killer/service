//! UNBOUNDED_V3 negative test, at the quorum level and without the docker
//! stack: an operator without the guest program abstains — the round
//! degrades, but no divergent transition is ever signed.
//!
//! Five operators run the node's real signing loop (`Contributor::run`, the
//! one `main.rs` spawns) over the real `GasKillerValidator`, wired to each
//! other and to a test orchestrator through in-memory channels instead of the
//! authenticated p2p network. The chain is a vanilla anvil carrying the sdk's
//! gkvm consumer (`common/tests/fixtures/gkvm`, the fixtures of
//! `common/tests/gkvm_analysis.rs`); the task is a forge-recorded, answered
//! `ask()`.
//!
//!   A, B  local executor, program installed, consumer registered
//!   C     local executor, NO guest VM,       consumer registered
//!   D     rpc executor,                      consumer registered
//!   E     rpc executor,                      consumer NOT registered
//!
//! A and B sign the digest of the forge leg's payload and their aggregate
//! verifies: with a threshold of 2 the round completes without C and D (with
//! 4 of 5 it would stall — either way on signature COUNT, never on
//! disagreement). C and D refuse at the `requiresGuestVm` gate and put nothing
//! on the wire. E is the contrast that gives the assertions teeth, and the
//! residual risk stated as a test: an operator whose registry lacks the
//! consumer DOES sign, and what it signs is the `GkVmUnavailable` revert
//! transition — a digest the honest operators reject, which can only reach a
//! quorum if `threshold` operators are misconfigured the same way.
//! `GK_GUEST_VM_CONSUMERS` belongs on every operator, whatever its executor.
//!
//! Observed, printed, NOT asserted (it is upstream behavior this repo does not
//! own): commonware-avs-node's `Contributor::run` propagates a validation
//! error on a round's `Start` with `?`, so an abstaining operator's signing
//! loop EXITS — the process stays up, but signs nothing again until restarted.
//!
//! Requires `anvil` (foundry) on PATH; soft-skips otherwise. One `#[test]`:
//! validator construction reads the process environment.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use alloy::primitives::{Address, B256, Bytes, U256, address, keccak256};
use alloy::providers::{Provider, ProviderBuilder};
use commonware_actor::{Feedback, Unreliable};
use commonware_avs_core::bn254::{
    Bn254, PublicKey, Signature, aggregate_signatures, aggregate_verify, get_signer,
};
use commonware_avs_core::validator::ValidatorTrait;
use commonware_avs_core::wire::{self, aggregation::Payload};
use commonware_avs_node::contributor::{AggregationInput, Contribute, Contributor};
use commonware_codec::{EncodeSize, Read, Write};
use commonware_cryptography::Signer;
use commonware_cryptography::sha256::Digest;
use commonware_p2p::{CheckedSender, LimitedSender, Message, Receiver, Recipients};
use commonware_runtime::{IoBuf, IoBufs};
use gas_killer_common::local_exec_shim::{
    GkvmHost, GuestProgramSet, GuestVmConsumers, LoadedGuestProgram,
};
use gas_killer_common::{GasKillerTaskData, GasKillerValidator};
use tokio::sync::mpsc;

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../common/tests/fixtures/gkvm");
const PARITY_TASKS: &str = include_str!("../../common/tests/fixtures/gkvm/parity_tasks.json");

/// anvil's first funded dev account.
const CALLER: Address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
const CONSUMER: Address = address!("0x00000000000000000000000000000000000050a1");
const ROUND: u64 = 1;
/// A guest run of hello-c is milliseconds; this only bounds a hung test.
const DEADLINE: Duration = Duration::from_secs(180);

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParityFixture {
    consumer_code: Bytes,
    tasks: Vec<ParityTask>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParityTask {
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
            tokio::time::sleep(Duration::from_millis(50)).await;
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

/// Every message an operator put on the wire, as the orchestrator sees it.
type Seen = Arc<Mutex<Vec<(PublicKey, Vec<u8>)>>>;
type Inboxes = Arc<HashMap<PublicKey, mpsc::UnboundedSender<Message<PublicKey>>>>;

/// The operator's side of the p2p channel: a broadcast reaches every other
/// operator's inbox (so the peers' own verify + aggregate branch runs) and
/// the orchestrator's record.
#[derive(Clone)]
struct Outbox {
    me: PublicKey,
    peers: Inboxes,
    seen: Seen,
}

struct CheckedOutbox {
    outbox: Outbox,
    recipients: Vec<PublicKey>,
}

impl LimitedSender for Outbox {
    type PublicKey = PublicKey;
    type Checked<'a>
        = CheckedOutbox
    where
        Self: 'a;

    fn check(
        &mut self,
        recipients: Recipients<Self::PublicKey>,
    ) -> Result<Self::Checked<'_>, SystemTime> {
        let recipients = match recipients {
            Recipients::All => self
                .peers
                .keys()
                .filter(|peer| **peer != self.me)
                .cloned()
                .collect(),
            Recipients::Some(recipients) => recipients,
            Recipients::One(recipient) => vec![recipient],
        };
        Ok(CheckedOutbox {
            outbox: self.clone(),
            recipients,
        })
    }
}

impl CheckedSender for CheckedOutbox {
    type PublicKey = PublicKey;

    fn recipients(&self) -> Vec<Self::PublicKey> {
        self.recipients.clone()
    }

    fn send(self, message: impl Into<IoBufs> + Send, _priority: bool) -> Unreliable<Feedback> {
        let bytes = message.into().coalesce().as_ref().to_vec();
        self.outbox
            .seen
            .lock()
            .unwrap()
            .push((self.outbox.me.clone(), bytes.clone()));
        for recipient in &self.recipients {
            if let Some(inbox) = self.outbox.peers.get(recipient) {
                // A peer whose signing loop has exited has dropped its inbox.
                let _ = inbox.send((self.outbox.me.clone(), IoBuf::from(bytes.clone())));
            }
        }
        Unreliable::new(Feedback::Ok)
    }
}

#[derive(Debug)]
struct Inbox(mpsc::UnboundedReceiver<Message<PublicKey>>);

#[derive(Debug)]
struct InboxClosed;

impl std::fmt::Display for InboxClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "inbox closed")
    }
}

impl std::error::Error for InboxClosed {}

impl Receiver for Inbox {
    type Error = InboxClosed;
    type PublicKey = PublicKey;

    async fn recv(&mut self) -> Result<Message<Self::PublicKey>, Self::Error> {
        self.0.recv().await.ok_or(InboxClosed)
    }
}

/// The operator's validator, with its verdict on the round's `Start` message
/// recorded — the test's only view into an operator that stays silent.
struct Recorded {
    inner: GasKillerValidator,
    start_verdict: Mutex<Option<Result<Digest, String>>>,
}

impl Recorded {
    fn record(&self, msg: &[u8], verdict: &anyhow::Result<Digest>) {
        let mut buf = msg;
        let is_start = wire::Aggregation::<GasKillerTaskData>::read_cfg(&mut buf, &())
            .is_ok_and(|m| matches!(m.payload, Some(Payload::Start)));
        if is_start {
            let verdict = match verdict {
                Ok(digest) => Ok(*digest),
                Err(e) => Err(format!("{e:#}")),
            };
            self.start_verdict.lock().unwrap().get_or_insert(verdict);
        }
    }

    fn start_verdict(&self) -> Option<Result<Digest, String>> {
        self.start_verdict.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ValidatorTrait for Recorded {
    async fn validate_and_return_expected_hash(&self, msg: &[u8]) -> anyhow::Result<Digest> {
        let verdict = self.inner.validate_and_return_expected_hash(msg).await;
        self.record(msg, &verdict);
        verdict
    }

    async fn get_payload_from_message(&self, msg: &[u8]) -> anyhow::Result<Digest> {
        self.inner.get_payload_from_message(msg).await
    }
}

struct Operator {
    name: &'static str,
    signer: Bn254,
    validator: Arc<Recorded>,
}

fn operator(name: &'static str, key: &str, validator: GasKillerValidator) -> Operator {
    Operator {
        name,
        signer: get_signer(key),
        validator: Arc::new(Recorded {
            inner: validator,
            start_verdict: Mutex::new(None),
        }),
    }
}

/// Same 2-worker runtime as router/node main.rs (commonware's default).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_operator_without_the_guest_program_abstains_and_no_divergent_transition_is_signed() {
    let Some(anvil) = LocalAnvil::spawn().await else {
        eprintln!("skipping: anvil (foundry) not on PATH");
        return;
    };
    let provider = ProviderBuilder::new().connect_http(anvil.url.parse().unwrap());

    let fixture: ParityFixture = serde_json::from_str(PARITY_TASKS).expect("parity_tasks.json");
    provider
        .raw_request::<_, ()>(
            "anvil_setCode".into(),
            (CONSUMER, fixture.consumer_code.clone()),
        )
        .await
        .expect("anvil_setCode");
    // A round at block 0 is refused outright ("block_height is required").
    provider
        .raw_request::<_, String>("evm_mine".into(), ())
        .await
        .expect("evm_mine");
    let block_height = provider.get_block_number().await.expect("block number");
    assert!(block_height > 0);
    let answered = fixture
        .tasks
        .iter()
        .find(|task| task.success && task.function == "ask")
        .expect("an answered ask() task");

    let hello_path = format!("{FIXTURE_DIR}/hello-c.elf");
    let hello: B256 = keccak256(std::fs::read(&hello_path).expect("fixture ELF"));
    let with_hello = || {
        let mut programs = GuestProgramSet::default();
        programs.insert_program(
            LoadedGuestProgram::from_file(std::path::Path::new(&hello_path), hello)
                .expect("hello-c.elf mounts against its keccak"),
        );
        Arc::new(GkvmHost::new(programs))
    };
    let registry = GuestVmConsumers::default().with_consumer(CONSUMER, hello);

    // The executor is read from the environment at validator construction;
    // everything per-operator after that goes through the builders. See
    // gkvm_analysis.rs for why the profile is `unbounded-v1`, not `chain`.
    unsafe {
        for var in [
            "GK_GUEST_PROGRAM",
            "GK_GUEST_PROGRAM_HASH",
            "GK_GUEST_VM_CONSUMERS",
        ] {
            std::env::remove_var(var);
        }
        std::env::set_var("GK_SIM_PROFILE", "unbounded-v1");
        std::env::set_var("GK_SIM_EXECUTOR", "local");
    }
    let local = || GasKillerValidator::with_rpc_url(&anvil.url);
    let a = operator(
        "A",
        "11",
        local()
            .with_gkvm_host(with_hello())
            .with_guest_vm_consumers(registry.clone()),
    );
    let b = operator(
        "B",
        "12",
        local()
            .with_gkvm_host(with_hello())
            .with_guest_vm_consumers(registry.clone()),
    );
    let c = operator("C", "13", local().with_guest_vm_consumers(registry.clone()));
    assert!(c.validator.inner.gkvm_host().is_none());
    unsafe { std::env::set_var("GK_SIM_EXECUTOR", "rpc") };
    let rpc = || GasKillerValidator::with_rpc_url(&anvil.url);
    let d = operator("D", "14", rpc().with_guest_vm_consumers(registry.clone()));
    let e = operator("E", "15", rpc());
    let operators = [a, b, c, d, e];

    let orchestrator = get_signer("7").public_key();
    let contributors: Vec<PublicKey> = operators.iter().map(|o| o.signer.public_key()).collect();
    let g1_map: HashMap<_, _> = operators
        .iter()
        .map(|o| (o.signer.public_key(), o.signer.public_g1()))
        .collect();
    let threshold = 2;

    let seen: Seen = Arc::default();
    let mut senders = HashMap::new();
    let mut receivers = Vec::new();
    for key in &contributors {
        let (tx, rx) = mpsc::unbounded_channel();
        senders.insert(key.clone(), tx);
        receivers.push(Inbox(rx));
    }
    let inboxes: Inboxes = Arc::new(senders);

    let mut loops = Vec::new();
    for (op, inbox) in operators.iter().zip(receivers) {
        let contributor = Contributor::<GasKillerTaskData>::new(
            orchestrator.clone(),
            op.signer.clone(),
            contributors.clone(),
            Some(AggregationInput::new(threshold, g1_map.clone())),
        )
        .with_validator(op.validator.clone());
        let outbox = Outbox {
            me: op.signer.public_key(),
            peers: inboxes.clone(),
            seen: seen.clone(),
        };
        loops.push(tokio::spawn(contributor.run(outbox, inbox)));
    }

    // The round, as the router's creator broadcasts it: the task, carrying
    // the storage updates the router itself computed.
    let task = GasKillerTaskData {
        storage_updates: answered.storage_updates.to_vec().into(),
        transition_index: 0,
        target_address: CONSUMER,
        call_data: answered.calldata.to_vec(),
        from_address: CALLER,
        value: U256::ZERO,
        block_height,
        chain_id: 11155111,
    };
    let expected = task.build_payload_hash(&answered.storage_updates);
    let start = wire::Aggregation::<GasKillerTaskData>::new(ROUND, task, Some(Payload::Start));
    let mut start_bytes = Vec::with_capacity(start.encode_size());
    start.write(&mut start_bytes);
    for inbox in inboxes.values() {
        inbox
            .send((orchestrator.clone(), IoBuf::from(start_bytes.clone())))
            .expect("every signing loop is up");
    }

    // Every operator reaches a verdict on the round; the ones that accepted it
    // put a signature on the wire.
    let signatures = || -> Vec<(PublicKey, Signature)> {
        seen.lock()
            .unwrap()
            .iter()
            .filter_map(|(from, bytes)| {
                let mut buf = bytes.as_slice();
                let message = wire::Aggregation::<GasKillerTaskData>::read_cfg(&mut buf, &())
                    .expect("operators speak the aggregation wire format");
                assert_eq!(message.round, ROUND);
                match message.payload {
                    Some(Payload::Signature(sig)) => Some((
                        from.clone(),
                        Signature::try_from(sig).expect("a well-formed signature"),
                    )),
                    _ => None,
                }
            })
            .collect()
    };
    let waited = tokio::time::timeout(DEADLINE, async {
        loop {
            let verdicts: Vec<_> = operators
                .iter()
                .map(|o| o.validator.start_verdict())
                .collect();
            if verdicts.iter().all(Option::is_some) {
                let accepted = verdicts.iter().flatten().filter(|v| v.is_ok()).count();
                if signatures().len() >= accepted {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        waited.is_ok(),
        "the round did not settle within {DEADLINE:?}"
    );
    // Nothing is owed anymore (one signature per accepted verdict is in), so
    // this only gives a would-be stray message the time to show up.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let verdict = |i: usize| operators[i].validator.start_verdict().unwrap();
    let key = |i: usize| operators[i].signer.public_key();
    let signed = signatures();
    let signature_of = |i: usize| -> Option<Signature> {
        let mut found = signed.iter().filter(|(from, _)| *from == key(i));
        let first = found.next().map(|(_, sig)| sig.clone());
        assert!(found.next().is_none(), "{} signed twice", operators[i].name);
        first
    };

    // A, B: the forge leg's payload, signed.
    for i in [0, 1] {
        assert_eq!(
            verdict(i),
            Ok(expected),
            "{} must analyze the task into the forge leg's payload",
            operators[i].name
        );
        let sig = signature_of(i).expect("an operator with the program signs");
        assert!(aggregate_verify(&[key(i)], None, expected.as_ref(), &sig));
    }
    // C, D: refused at the gate, nothing on the wire.
    let (c_refusal, d_refusal) = (
        verdict(2).expect_err("C has no guest VM"),
        verdict(3).expect_err("D runs the rpc executor"),
    );
    assert!(
        c_refusal.contains("guest-VM gate") && c_refusal.contains("no guest programs"),
        "{c_refusal}"
    );
    assert!(
        d_refusal.contains("guest-VM gate") && d_refusal.contains("is rpc"),
        "{d_refusal}"
    );
    assert!(signature_of(2).is_none() && signature_of(3).is_none());
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|(from, _)| *from != key(2) && *from != key(3)),
        "an abstaining operator puts nothing on the wire"
    );

    // Degraded, not stalled, at this threshold: the two signatures aggregate
    // and verify against the two signers — what the router lands on-chain.
    let quorum = [signature_of(0).unwrap(), signature_of(1).unwrap()];
    let aggregate = aggregate_signatures(&quorum).expect("aggregate");
    assert!(aggregate_verify(
        &[key(0), key(1)],
        None,
        expected.as_ref(),
        &aggregate
    ));

    // E, the contrast: unregistered, the rpc executor analyzes the task into
    // the GkVmUnavailable revert transition and SIGNS it. The honest digest
    // does not verify under its signature, and no honest signature verifies
    // over its digest: it stays a minority of one.
    let divergent = verdict(4).expect("ungated, the rpc executor analyzes the task");
    assert_ne!(divergent, expected);
    let stray = signature_of(4).expect("an ungated operator signs what it computed");
    assert!(aggregate_verify(
        &[key(4)],
        None,
        divergent.as_ref(),
        &stray
    ));
    assert!(!aggregate_verify(
        &[key(4)],
        None,
        expected.as_ref(),
        &stray
    ));
    assert_eq!(signed.len(), 3, "A, B and the ungated E — nobody else");
    for (from, sig) in &signed {
        let over_expected =
            aggregate_verify(std::slice::from_ref(from), None, expected.as_ref(), sig);
        assert_eq!(
            over_expected,
            *from != key(4),
            "every signature of a gated operator is over the one honest digest"
        );
    }

    // Upstream behavior, reported only (module docs): what abstaining costs
    // the operator today.
    for (op, signing_loop) in operators.iter().zip(&loops) {
        println!(
            "operator {}: start verdict {}, signing loop {}",
            op.name,
            match op.validator.start_verdict().unwrap() {
                Ok(digest) if digest == expected => "= the forge leg's digest".to_string(),
                Ok(_) => "= a DIVERGENT digest".to_string(),
                Err(e) => format!("refused ({e})"),
            },
            if signing_loop.is_finished() {
                "EXITED"
            } else {
                "running"
            }
        );
    }
    assert!(
        !loops[0].is_finished() && !loops[1].is_finished(),
        "the signing operators keep serving"
    );
    for signing_loop in loops {
        signing_loop.abort();
    }
}

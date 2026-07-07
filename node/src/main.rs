//! Gas Killer Node - Aggregation participant for the Gas Killer AVS
//!
//! The node runs the commonware-consensus aggregation engine with the secp256k1
//! ECDSA multisig scheme: for every height the router announces a task on p2p
//! channel 1, the node validates it via EVMSketch, signs the expected task digest
//! (or the skip digest when the router abandons the height) with its operator
//! ECDSA key, and gossips TipAcks on channel 0 until the height certifies.

mod automaton;
mod digest;
mod precommit;
mod reporter;
mod schnorr_participant;
mod task_book;

use ::tokio::net::TcpListener;
use axum::{
    Router, extract::State, http::StatusCode, http::header, response::IntoResponse, routing::get,
};
use clap::{Arg, Command};
use commonware_consensus::aggregation::{Config as AggregationConfig, Engine};
use commonware_consensus::types::{Epoch, EpochDelta, HeightDelta};
use commonware_cryptography::Signer as _;
use commonware_cryptography::certificate::ConstantProvider;
use commonware_p2p::authenticated::lookup::{self, Network};
use commonware_p2p::{Address, AddressableManager as _};
use commonware_parallel::Sequential;
use commonware_runtime::buffer::paged::CacheRef;
use commonware_runtime::{Metrics, Quota, Runner, Spawner, Supervisor, tokio};
use commonware_utils::ordered::{Map, Quorum as _, Set};
use commonware_utils::{N3f1, NZU16, NZU32, NZU64, NZUsize, NonZeroDuration};
use eigen_logging::log_level::LogLevel;
use gas_killer_common::ecdsa::{Ecdsa, PublicKey, get_signer};
use gas_killer_common::{
    EcdsaScheme, GasKillerValidator, OrchestratorConfig, SignatureScheme,
    SpeculativePrebuildConfig, StaticEpochMonitor, ValidatorMetrics, ack_messages_per_second,
    agg_activity_timeout, agg_window, get_operator_states, load_key_from_file,
    load_orchestrator_config, p2p_message_backlog, p2p_quota_period, rebroadcast_interval,
    round_timeout, schnorr_messages_per_second, signature_scheme, storage_directory,
};
use rand::TryRngCore as _;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::automaton::GasKillerAutomaton;
use crate::digest::DigestResolver;
use crate::reporter::NodeReporter;
use crate::task_book::TaskBook;

/// Unique namespace to avoid message replay attacks
const APPLICATION_NAMESPACE: &[u8] = b"_COMMONWARE_AGGREGATION_";

/// P2P channel carrying the aggregation engine's `TipAck` gossip.
const ENGINE_CHANNEL: u64 = 0;

/// P2P channel carrying the router's `TaskDirective` broadcasts (nodes only
/// receive; the sender half is registered but never used).
const TASK_DIRECTIVE_CHANNEL: u64 = 1;

/// P2P channel carrying the interactive Schnorr signing rounds
/// (`SIGNATURE_SCHEME=schnorr` mode only; never registered in ECDSA mode).
const SCHNORR_CHANNEL: u64 = 2;

#[derive(Clone)]
struct HealthState {
    ready: Arc<AtomicBool>,
    /// `tokio::Context` is not `Clone` in 2026.5.0; `Metrics::encode` works
    /// through a shared handle.
    context: Arc<tokio::Context>,
    validator_metrics: Arc<ValidatorMetrics>,
}

/// Liveness probe — always 200 if the process is running.
async fn healthz_handler() -> StatusCode {
    StatusCode::OK
}

/// Readiness probe — 503 until the engine is spawned and the network is starting.
async fn readyz_handler(State(s): State<HealthState>) -> StatusCode {
    if s.ready.load(Ordering::Relaxed) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Prometheus metrics endpoint — commonware runtime metrics + node validator timing.
async fn metrics_handler(State(s): State<HealthState>) -> impl IntoResponse {
    let mut output = s.context.encode();
    output.push_str(&s.validator_metrics.encode());
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        output,
    )
}

/// Resolve a hostname:port with retry logic for Docker DNS readiness
fn resolve_with_retry(
    address: &str,
    max_retries: u32,
    retry_delay: Duration,
) -> Option<SocketAddr> {
    for attempt in 1..=max_retries {
        match address.to_socket_addrs() {
            Ok(mut addrs) => {
                if let Some(addr) = addrs.next() {
                    tracing::info!(address, ?addr, attempt, "DNS resolution succeeded");
                    return Some(addr);
                }
            }
            Err(e) => {
                if attempt < max_retries {
                    tracing::warn!(
                        address,
                        attempt,
                        max_retries,
                        error = %e,
                        "DNS resolution failed, retrying..."
                    );
                    std::thread::sleep(retry_delay);
                } else {
                    tracing::error!(
                        address,
                        error = %e,
                        "DNS resolution failed after all retries"
                    );
                }
            }
        }
    }
    None
}

fn configure_identity(matches: &clap::ArgMatches) -> (Ecdsa, u16) {
    let key_file = matches
        .get_one::<String>("key-file")
        .expect("Please provide key file");
    let port = matches
        .get_one::<String>("port")
        .expect("Please provide port");
    let key = load_key_from_file(key_file);
    let signer = get_signer(&key);
    let port = port.parse::<u16>().expect("Port not well-formed");
    tracing::info!(port, "loaded identity");
    (signer, port)
}

fn configure_orchestrator(matches: &clap::ArgMatches) -> OrchestratorConfig {
    let orchestrator_file = matches
        .get_one::<String>("orchestrator")
        .expect("Please provide orchestrator config file");
    load_orchestrator_config(orchestrator_file)
}

fn main() {
    // Initialize runtime. The storage directory anchors the engine's journal:
    // without a stable path the runtime defaults to a random per-process temp
    // dir and journal replay after restart silently loses history.
    let storage_dir = storage_directory();
    let runtime_cfg = tokio::Config::default()
        // 2026.5.0 defaults to 2 worker threads; the node runs the engine, p2p,
        // EVMSketch validation, and the healthz server concurrently.
        .with_worker_threads(4)
        .with_storage_directory(storage_dir.clone());
    let runner = tokio::Runner::new(runtime_cfg);

    // Parse arguments
    let matches = Command::new("gas-killer-node")
        .about("Gas Killer AVS node - ECDSA signature aggregation participant")
        .arg(
            Arg::new("key-file")
                .long("key-file")
                .required(true)
                .help("Path to the JSON file containing the operator's ECDSA private key"),
        )
        .arg(
            Arg::new("port")
                .long("port")
                .required(true)
                .help("Port to run the P2P service on"),
        )
        .arg(
            Arg::new("orchestrator")
                .long("orchestrator")
                .required(true)
                .help("Path to orchestrator config file (JSON with the router's address and port)"),
        )
        .get_matches();

    // Configure my identity
    let (signer, port) = configure_identity(&matches);
    let orchestrator_config = configure_orchestrator(&matches);

    // In schnorr mode the SAME operator key doubles as the Schnorr signing key
    // (same secp256k1 scalar; the registry identity keccak256(x ‖ y)[12..] equals
    // the operator's Ethereum address). Loaded here so the async closure below
    // does not capture the CLI matches.
    let schnorr_key = match signature_scheme() {
        SignatureScheme::Schnorr | SignatureScheme::SchnorrPrecommit => Some(
            gas_killer_common::schnorr::private_key_from_hex(&load_key_from_file(
                matches
                    .get_one::<String>("key-file")
                    .expect("Please provide key file"),
            ))
            .expect("operator key is not a valid secp256k1 scalar"),
        ),
        SignatureScheme::Ecdsa => None,
    };

    // Start runtime
    runner.start(|context: tokio::Context| async move {
        let mut recipients: Vec<(PublicKey, SocketAddr)> = Vec::new();

        // Configure quorum number from environment (default: 0)
        let quorum_number: usize = std::env::var("QUORUM_NUMBER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        // Scoped to avoid configuring two loggers
        let orchestrator_pub_key;
        let quorum_infos;
        {
            eigen_logging::init_logger(LogLevel::Debug);
            quorum_infos = get_operator_states()
                .await
                .expect("Failed to get operator states");

            if quorum_number >= quorum_infos.len() {
                panic!(
                    "QUORUM_NUMBER {} is out of range (available quorums: 0..{})",
                    quorum_number,
                    quorum_infos.len()
                );
            }
            tracing::info!(
                quorum_number,
                total_quorums = quorum_infos.len(),
                "using quorum"
            );

            // Configure allowed peers from operator states
            let participants = &quorum_infos[quorum_number].operators;
            if participants.is_empty() {
                panic!("No operators found in quorum");
            }

            for participant in participants {
                let verifier = PublicKey::from(participant.address);
                tracing::info!(key = ?verifier, "registered authorized peer");
                if let Some(socket) = &participant.socket {
                    // Try to resolve hostname:port with retries (Docker DNS may need time)
                    if let Some(socket_addr) =
                        resolve_with_retry(socket, 30, Duration::from_secs(2))
                    {
                        recipients.push((verifier, socket_addr));
                    } else {
                        // Last resort: try parsing as direct IP:PORT
                        match SocketAddr::from_str(socket) {
                            Ok(socket_addr) => {
                                recipients.push((verifier, socket_addr));
                            }
                            Err(parse_err) => {
                                tracing::error!(
                                    socket,
                                    error = %parse_err,
                                    "Failed to resolve or parse socket address"
                                );
                                panic!("Socket address not well-formed: {socket}");
                            }
                        }
                    }
                }
            }

            // Parse the orchestrator (router) identity: its Ethereum address
            orchestrator_pub_key = PublicKey::from_address_str(&orchestrator_config.publicKey)
                .expect("Invalid orchestrator address");
            tracing::info!(key = ?orchestrator_pub_key, "registered orchestrator key");

            // Resolve orchestrator address (hostname:port or IP:port) with retries
            let orchestrator_socket = format!(
                "{}:{}",
                orchestrator_config
                    .address
                    .as_deref()
                    .unwrap_or("127.0.0.1"),
                orchestrator_config.port
            );
            tracing::info!(target = %orchestrator_socket, "resolving orchestrator address");

            // Retry DNS resolution with exponential backoff for Docker networking
            let mut orchestrator_addr = None;
            let max_retries = 10;
            for attempt in 0..max_retries {
                match orchestrator_socket.to_socket_addrs() {
                    Ok(mut addrs) => {
                        if let Some(addr) = addrs.next() {
                            tracing::info!(addr = %addr, "resolved orchestrator address");
                            orchestrator_addr = Some(addr);
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            attempt = attempt + 1,
                            max_retries = max_retries,
                            "DNS resolution failed, retrying..."
                        );
                    }
                }

                if attempt < max_retries - 1 {
                    let delay = std::time::Duration::from_millis(500 * (1 << attempt.min(4)));
                    std::thread::sleep(delay);
                }
            }

            let orchestrator_addr = orchestrator_addr.unwrap_or_else(|| {
                // Final fallback: try to parse as direct IP:PORT
                SocketAddr::from_str(&orchestrator_socket).unwrap_or_else(|_| {
                    panic!(
                        "Failed to resolve orchestrator address '{}' after {} retries",
                        orchestrator_socket, max_retries
                    )
                })
            });

            recipients.push((orchestrator_pub_key.clone(), orchestrator_addr));
        }

        // Configure tracing
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::stdout)
            .finish();
        _ = tracing::subscriber::set_default(subscriber);

        tracing::info!(storage_dir = %storage_dir.display(), "engine journal storage directory");

        // Configure P2P network
        const MAX_MESSAGE_SIZE: u32 = 1024 * 1024; // 1 MB
        let my_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        let mut p2p_cfg = lookup::Config::recommended(
            signer.clone(),
            APPLICATION_NAMESPACE,
            my_addr,
            MAX_MESSAGE_SIZE,
        );

        // recommended() sets this false, but in-cluster router<->node p2p on GKE resolves to private
        // pod IPs; leaving it false would drop every intra-cluster connection. Keep it true until the
        // topology uses public addresses.
        p2p_cfg.allow_private_ips = true;

        // Must stay true for K8s deployments (DNAT/SNAT means source IPs at the listener are
        // always pod IPs, never the registered ClusterIP addresses) and for mixed-network topologies
        // where external operators are behind NAT. IP-based pre-filtering cannot work in either
        // case; authentication relies entirely on the cryptographic handshake (peer public keys
        // checked against the registered operator set), which is secure for both topologies.
        // (2026.5.0 rename of `attempt_unregistered_handshakes`.)
        p2p_cfg.bypass_ip_check = true;

        // recommended() throttles peer discovery for large open gossip networks where aggressive
        // dialing is abusive. gas-killer instead runs a small, static, allowlisted operator set in a
        // full mesh, so keep discovery fast (500ms dial cadence) for quick (re)join while retaining
        // recommended's abuse-resistance (concurrent-handshake cap, subnet rate limit, ping cadence).
        p2p_cfg.dial_frequency = Duration::from_millis(500);
        // `peer_connection_cooldown` is the minimum time between dial reservations for a
        // single peer, so it also bounds how fast a FAILED initial dial retries. The
        // router starts after the nodes (compose staggers it), so a node's first dials
        // to the router fail; a long cooldown then delays reconnection past the round
        // timeout and the first task certifies as a skip. Keep it short so the static
        // mesh re-forms within a couple of seconds of the router coming up. (A larger
        // value was briefly used to damp a HandshakeError(DecryptionFailed) reconnect
        // flap, but that was specific to slow QEMU-emulated CI hosts, not native amd64.)
        p2p_cfg.peer_connection_cooldown = Duration::from_secs(3);
        p2p_cfg.allowed_handshake_rate_per_ip = Quota::per_second(NZU32!(16));

        let (mut network, mut oracle) = Network::new(context.child("network"), p2p_cfg);

        // Debug: Log all recipients before updating oracle
        tracing::info!(
            count = recipients.len(),
            "registering recipients with oracle"
        );
        for (key, addr) in &recipients {
            tracing::info!(key = ?key, addr = ?addr, "oracle recipient");
        }

        // Register authorized peers (operators + router) with the oracle. Peer
        // dialable addresses now come exclusively from this map; from_iter_dedup
        // keeps the first occurrence if the router key ever appears twice.
        let peers: Map<PublicKey, Address> = Map::from_iter_dedup(
            recipients
                .iter()
                .cloned()
                .map(|(pk, addr)| (pk, Address::Symmetric(addr))),
        );
        let _ = oracle.track(0, peers);

        // Build the ordered participant set (operator addresses, sorted by byte
        // order). Participant indices are positions in this sorted order — the
        // router builds the identical set from the same operator state, so
        // attestations attribute to the same indices everywhere.
        let operators = &quorum_infos[quorum_number].operators;
        if operators.is_empty() {
            panic!("No operators found");
        }
        let participants: Set<PublicKey> = Set::from_iter_dedup(
            operators
                .iter()
                .map(|operator| PublicKey::from(operator.address)),
        );
        for key in participants.iter() {
            tracing::info!(key = ?key, "registered participant");
        }

        let scheme_mode = signature_scheme();
        tracing::info!(?scheme_mode, "signature scheme");

        // Create network channels (must all be registered BEFORE network.start()):
        // channel 1 carries the router's task directives in both modes (nodes
        // receive; the sender is only used for rate-limited TipReport replies to
        // stale directives). The mode-specific signing channel — 0 for the ECDSA
        // engine's TipAck gossip, 2 for the interactive Schnorr rounds — is
        // registered in the mode branch below, still before network.start().
        let p2p_backlog = p2p_message_backlog();
        let p2p_quota = Quota::with_period(p2p_quota_period())
            .expect("p2p_quota_period always returns a non-zero duration");
        let (directive_sender, directive_receiver) =
            network.register(TASK_DIRECTIVE_CHANNEL, p2p_quota, p2p_backlog);

        // Create validator metrics and validator for the gas killer use case
        let validator_metrics = Arc::new(ValidatorMetrics::new());
        let validator = Arc::new(
            GasKillerValidator::new()
                .expect("HTTP_RPC environment variable must be set for gas analyzer")
                .with_validator_metrics(Arc::clone(&validator_metrics)),
        );

        // Warm the executor cache off the hot path: a background loop pre-builds the EVMSketch
        // executor for each chain's latest block so the first task validation is a cache hit.
        {
            let spec_validator = Arc::clone(&validator);
            let prebuild_cfg = SpeculativePrebuildConfig::from_env();
            context.child("prebuild").spawn(move |_| async move {
                spec_validator.run_speculative_prebuild(prebuild_cfg).await;
            });
        }

        // TaskBook actor: owns the router's per-height directives and parks the
        // automaton's subscriptions until the skip rules resolve them.
        let (task_book, task_book_mailbox) = TaskBook::new(context.child("task_book"));
        context
            .child("task_book_actor")
            .spawn(move |_| task_book.run());

        // Shared engine-tip mirror: written by the reporter, read by the directive
        // ingest loop to answer stale directives with a TipReport (see task_book).
        let engine_tip = Arc::new(AtomicU64::new(0));

        // Feed the TaskBook from channel 1 (router directives only; other peers
        // are authorized on the channel but must not assign heights).
        {
            let task_book_mailbox = task_book_mailbox.clone();
            let router_key = orchestrator_pub_key.clone();
            let engine_tip = Arc::clone(&engine_tip);
            let min_report_interval = rebroadcast_interval();
            context.child("directives").spawn(move |_| async move {
                task_book::ingest(
                    directive_receiver,
                    directive_sender,
                    router_key,
                    task_book_mailbox,
                    engine_tip,
                    min_report_interval,
                )
                .await;
            });
        }

        // Mode-specific signing path. ECDSA runs the commonware aggregation engine
        // (one-round: nodes sign TipAcks unilaterally, the engine assembles
        // certificates); Schnorr runs the interactive two-round MuSig2 participant
        // actor on channel 2 (see schnorr_participant.rs). The engine's epoch
        // monitor must outlive the engine, so the ECDSA arm hands its guard out to
        // the root future.
        let monitor_guard = match scheme_mode {
            SignatureScheme::Ecdsa => {
                // The engine's quorum is fixed at N3f1 (n - (n-1)/3); the
                // contract-derived threshold is informational only (the
                // authoritative operator-count check runs on-chain in
                // GasKillerSDK.verifyAndUpdate).
                tracing::info!(
                    participants = participants.len(),
                    quorum = participants.quorum::<N3f1>(),
                    contract_threshold = quorum_infos[quorum_number].threshold,
                    "aggregation quorum (engine-fixed N3f1)"
                );

                // Signing scheme over the participant set; our own participant
                // index is derived from our address's position in the sorted set.
                let scheme = EcdsaScheme::signer(participants, signer.private_key())
                    .unwrap_or_else(|| {
                        panic!(
                            "own operator address {:?} is not in the quorum-{quorum_number} operator set; \
                             register the operator on-chain before starting the node",
                            signer.public_key()
                        )
                    });

                // The engine channel needs its own, much larger quota: the engine
                // keeps rebroadcasting each signed height's TipAck until it falls
                // activity_timeout below the tip (even after certification), and
                // the p2p send-side limiter SILENTLY DROPS messages to
                // rate-limited peers — the legacy 1 msg/s default would starve
                // fresh acks and stall certification.
                let ack_rate = ack_messages_per_second();
                let ack_quota = Quota::per_second(ack_rate);
                tracing::info!(
                    ack_messages_per_second = ack_rate.get(),
                    "engine channel quota"
                );
                let (engine_sender, engine_receiver) =
                    network.register(ENGINE_CHANNEL, ack_quota, p2p_backlog);

                // Reporter actor: certificate/tip accounting + TaskBook pruning.
                let (node_reporter, reporter_mailbox) = NodeReporter::<EcdsaScheme>::new(
                    context.child("reporter"),
                    task_book_mailbox.clone(),
                    Arc::clone(&engine_tip),
                    None,
                );
                context
                    .child("reporter_actor")
                    .spawn(move |_| node_reporter.run());

                // Automaton: resolves each proposed height to the expected task
                // digest (validated via EVMSketch) or the skip digest, per the
                // TaskBook.
                let automaton = GasKillerAutomaton::new(
                    context.child("automaton"),
                    task_book_mailbox.clone(),
                    Arc::clone(&validator),
                    // Retry transient validation errors up to the router's round
                    // timeout: past that the router is broadcasting Skip{h} anyway.
                    round_timeout(),
                );

                // Static single-epoch supervision: ConstantProvider serves the
                // same scheme for every epoch and the monitor never fires. Keep a
                // clone of the monitor in the root future — it must outlive the
                // engine or the engine exits.
                let provider = ConstantProvider::<EcdsaScheme, Epoch>::new(scheme);
                let monitor = StaticEpochMonitor::new();
                let monitor_guard = monitor.clone();

                // Aggregation engine (journal knobs follow the upstream test defaults).
                tracing::info!(
                    window = agg_window().get(),
                    activity_timeout = agg_activity_timeout(),
                    rebroadcast_secs = rebroadcast_interval().as_secs_f64(),
                    round_timeout_secs = round_timeout().as_secs_f64(),
                    "aggregation engine tuning"
                );
                let engine = Engine::new(
                    context.child("engine"),
                    AggregationConfig {
                        monitor,
                        provider,
                        automaton,
                        reporter: reporter_mailbox,
                        // The oracle disconnects peers on blockable offenses (bad
                        // ack signatures / signer mismatches).
                        blocker: oracle.clone(),
                        priority_acks: false,
                        // Re-send our own ack until quorum; reuse the router's
                        // directive rebroadcast cadence.
                        rebroadcast_timeout: NonZeroDuration::new_panic(rebroadcast_interval()),
                        // Single static epoch: only epoch 0 acks are valid.
                        epoch_bounds: (EpochDelta::new(0), EpochDelta::new(0)),
                        window: agg_window(),
                        activity_timeout: HeightDelta::new(agg_activity_timeout()),
                        // Per-identity partition: two nodes sharing a storage
                        // directory (e.g. the local $TMPDIR fallback) must not
                        // share a journal.
                        journal_partition: format!("aggregation-node-{}", signer.public_key()),
                        journal_write_buffer: NZUsize!(4096),
                        journal_replay_buffer: NZUsize!(4096),
                        journal_heights_per_section: NZU64!(6),
                        journal_compression: Some(3),
                        journal_page_cache: CacheRef::from_pooler(
                            &context,
                            NZU16!(1024),
                            NZUsize!(10),
                        ),
                        strategy: Sequential,
                    },
                );
                engine.start((engine_sender, engine_receiver));
                Some(monitor_guard)
            }
            SignatureScheme::Schnorr => {
                let schnorr_key = schnorr_key.expect("schnorr key loaded before runtime start");
                // The Schnorr key IS the operator key, so its point address must
                // equal our p2p identity — and that identity must be a registered
                // operator (EcdsaScheme::signer performs this check in ECDSA mode).
                let own_address = schnorr_key.public_key().eth_address();
                assert_eq!(
                    own_address,
                    signer.public_key().address(),
                    "schnorr key/point address diverges from the operator identity"
                );
                if !operators.iter().any(|o| o.address == own_address) {
                    panic!(
                        "own operator address {own_address:?} is not in the \
                         quorum-{quorum_number} operator set; register the operator \
                         on-chain before starting the node"
                    );
                }

                // The Schnorr rounds are request/response (no steady-state
                // rebroadcast like TipAcks), but a dropped message costs a whole
                // retry attempt, so the quota is generous.
                let schnorr_quota = Quota::per_second(schnorr_messages_per_second());
                let (schnorr_sender, schnorr_receiver) =
                    network.register(SCHNORR_CHANNEL, schnorr_quota, p2p_backlog);

                // Same announce-vs-skip + validation logic the ECDSA automaton
                // uses — the two paths must vouch for identical digests.
                let resolver = DigestResolver::new(
                    task_book_mailbox.clone(),
                    Arc::clone(&validator),
                    round_timeout(),
                );
                let operator_addresses: HashSet<_> =
                    operators.iter().map(|o| o.address).collect();
                let router_key = orchestrator_pub_key.clone();
                let participant_tip = Arc::clone(&engine_tip);
                let participant_ctx = context.child("schnorr_participant");
                context
                    .child("schnorr_participant_actor")
                    .spawn(move |_| async move {
                        schnorr_participant::run(
                            participant_ctx,
                            schnorr_key,
                            router_key,
                            operator_addresses,
                            resolver,
                            participant_tip,
                            schnorr_receiver,
                            schnorr_sender,
                        )
                        .await;
                    });
                None
            }
            SignatureScheme::SchnorrPrecommit => {
                let schnorr_key = schnorr_key.expect("schnorr key loaded before runtime start");
                let own_address = schnorr_key.public_key().eth_address();
                assert_eq!(
                    own_address,
                    signer.public_key().address(),
                    "schnorr key/point address diverges from the operator identity"
                );

                // On-chain reads: operator key points from the stake registry and our
                // own registered batch metadata from the nonce registry (the scheme is
                // built from chain state; see docs/schnorr-nonce-registry.md §7.2).
                let http_rpc = std::env::var("HTTP_RPC")
                    .expect("HTTP_RPC environment variable must be set for schnorr-precommit");
                let stake_registry = gas_killer_common::resolve_deployed_address(
                    "SCHNORR_STAKE_REGISTRY_ADDRESS",
                    "schnorrStakeRegistry",
                )
                .expect("failed to resolve the SchnorrStakeRegistry address");
                let nonce_registry = gas_killer_common::resolve_deployed_address(
                    "SCHNORR_NONCE_REGISTRY_ADDRESS",
                    "schnorrNonceRegistry",
                )
                .expect("failed to resolve the SchnorrNonceRegistry address");
                let provider = alloy::providers::ProviderBuilder::new()
                    .connect_http(http_rpc.parse().expect("HTTP_RPC is not a valid URL"));
                let chain_id = alloy::providers::Provider::get_chain_id(&provider)
                    .await
                    .expect("failed to read chain id from HTTP_RPC");
                let operator_addresses: Vec<alloy::primitives::Address> =
                    operators.iter().map(|o| o.address).collect();
                let operator_points = gas_killer_common::schnorr::onchain::load_operator_keys(
                    provider.clone(),
                    stake_registry,
                    &operator_addresses,
                )
                .await
                .expect("failed to load operator keys from the SchnorrStakeRegistry");

                // Rebuild our own batches from the key + on-chain metadata (batch seeds
                // derive from the key; a root mismatch means the registered batches were
                // not produced by this key file — refuse to run rather than sign junk).
                let batch_metas = gas_killer_common::schnorr::onchain::load_batches(
                    provider.clone(),
                    nonce_registry,
                    own_address,
                )
                .await
                .expect("failed to load batch metadata from the SchnorrNonceRegistry");
                assert!(
                    !batch_metas.is_empty(),
                    "no nonce batches registered for {own_address}; run the deploy flow first"
                );
                let domain = gas_killer_common::schnorr::precommit::BatchDomain {
                    chain_id,
                    registry: nonce_registry,
                    operator: own_address,
                };
                let directory = Arc::new(
                    gas_killer_common::schnorr::scheme::MemoryNonceDirectory::default(),
                );
                let store = Arc::new(gas_killer_common::schnorr::batches::BatchStore::new(
                    chain_id,
                    nonce_registry,
                    Arc::clone(&directory),
                ));
                let mut fill = |b: &mut [u8]| {
                    rand::rngs::OsRng
                        .try_fill_bytes(b)
                        .expect("operating system RNG failed")
                };
                let mut secrets = Vec::with_capacity(batch_metas.len());
                let mut own_announces = Vec::new();
                for meta in &batch_metas {
                    let seed = gas_killer_common::schnorr::precommit::derive_batch_seed(
                        &schnorr_key,
                        meta.batch_index,
                    );
                    let batch = gas_killer_common::schnorr::precommit::NonceBatch::generate(
                        domain,
                        &seed,
                        meta.batch_index,
                        meta.start_slot,
                        meta.count,
                    )
                    .expect("on-chain batch metadata is out of local bounds");
                    assert_eq!(
                        batch.root(),
                        meta.root,
                        "locally derived batch {} root diverges from the on-chain                          registration — key file / registry mismatch",
                        meta.batch_index
                    );
                    let signature = batch
                        .sign_registration(&schnorr_key, &mut fill)
                        .expect("batch domain operator matches the key by construction");
                    assert!(
                        store.insert_local(&schnorr_key.public_key(), &batch, signature),
                        "locally generated batch failed its own registration check"
                    );
                    own_announces.extend(
                        gas_killer_common::schnorr::batches::BatchStore::chunk_batch(
                            &schnorr_key.public_key(),
                            &batch,
                            signature,
                        ),
                    );
                    secrets.push(gas_killer_common::schnorr::scheme::SeedSecrets::new(
                        domain,
                        seed,
                        meta.start_slot,
                        meta.count,
                    ));
                }

                // Invariant N1: the fsync write-ahead spend journal (survives restarts;
                // per-identity file so shared storage dirs cannot cross-contaminate).
                let journal = gas_killer_common::schnorr::journal::FileSpendJournal::open(
                    storage_dir.join(format!("schnorr-spend-{own_address}.journal")),
                )
                .expect("failed to open the nonce spend journal");

                let scheme = gas_killer_common::schnorr::scheme::SchnorrScheme::signer(
                    operator_points,
                    schnorr_key.clone(),
                    Arc::new(secrets),
                    Arc::new(journal),
                    directory,
                    gas_killer_common::schnorr::scheme::DEFAULT_ATTEMPTS_PER_HEIGHT,
                )
                .unwrap_or_else(|| {
                    panic!(
                        "own operator address {own_address:?} is not in the registered                          operator set; register the operator on-chain before starting"
                    )
                });

                // Aggregation engine — identical knobs to the ECDSA arm; only the
                // scheme differs (the ack payload is our MuSig2 partial).
                let ack_rate = ack_messages_per_second();
                let ack_quota = Quota::per_second(ack_rate);
                tracing::info!(
                    ack_messages_per_second = ack_rate.get(),
                    "engine channel quota"
                );
                let (engine_sender, engine_receiver) =
                    network.register(ENGINE_CHANNEL, ack_quota, p2p_backlog);

                // Reporter with a certificate tap: Attested certificates trigger the
                // precommit actor's completion partials.
                let (tap_sender, tap_receiver) = commonware_utils::channel::mpsc::channel(1024);
                let (node_reporter, reporter_mailbox) = NodeReporter::<
                    gas_killer_common::schnorr::scheme::SchnorrScheme,
                >::new(
                    context.child("reporter"),
                    task_book_mailbox.clone(),
                    Arc::clone(&engine_tip),
                    Some(tap_sender),
                );
                context
                    .child("reporter_actor")
                    .spawn(move |_| node_reporter.run());

                let automaton = GasKillerAutomaton::new(
                    context.child("automaton"),
                    task_book_mailbox.clone(),
                    Arc::clone(&validator),
                    round_timeout(),
                );

                let provider_epoch = ConstantProvider::<
                    gas_killer_common::schnorr::scheme::SchnorrScheme,
                    Epoch,
                >::new(scheme.clone());
                let monitor = StaticEpochMonitor::new();
                let monitor_guard = monitor.clone();

                tracing::info!(
                    window = agg_window().get(),
                    activity_timeout = agg_activity_timeout(),
                    rebroadcast_secs = rebroadcast_interval().as_secs_f64(),
                    round_timeout_secs = round_timeout().as_secs_f64(),
                    "aggregation engine tuning (schnorr-precommit)"
                );
                let engine = Engine::new(
                    context.child("engine"),
                    AggregationConfig {
                        monitor,
                        provider: provider_epoch,
                        automaton,
                        reporter: reporter_mailbox,
                        blocker: oracle.clone(),
                        priority_acks: false,
                        rebroadcast_timeout: NonZeroDuration::new_panic(rebroadcast_interval()),
                        epoch_bounds: (EpochDelta::new(0), EpochDelta::new(0)),
                        window: agg_window(),
                        activity_timeout: HeightDelta::new(agg_activity_timeout()),
                        journal_partition: format!("aggregation-node-{}", signer.public_key()),
                        journal_write_buffer: NZUsize!(4096),
                        journal_replay_buffer: NZUsize!(4096),
                        journal_heights_per_section: NZU64!(6),
                        journal_compression: Some(3),
                        journal_page_cache: CacheRef::from_pooler(
                            &context,
                            NZU16!(1024),
                            NZUsize!(10),
                        ),
                        strategy: Sequential,
                    },
                );
                engine.start((engine_sender, engine_receiver));

                // Precommit actor: batch gossip + completion partials on channel 2.
                let schnorr_quota = Quota::per_second(schnorr_messages_per_second());
                let (pc_sender, pc_receiver) =
                    network.register(SCHNORR_CHANNEL, schnorr_quota, p2p_backlog);
                let router_key = orchestrator_pub_key.clone();
                context
                    .child("precommit_actor")
                    .spawn(move |_| async move {
                        precommit::run(
                            scheme,
                            store,
                            own_announces,
                            operator_addresses,
                            router_key,
                            rebroadcast_interval(),
                            tap_receiver,
                            pc_receiver,
                            pc_sender,
                        )
                        .await;
                    });
                Some(monitor_guard)
            }
        };

        // Readiness flag: set to true after the engine is spawned and network is starting
        let ready = Arc::new(AtomicBool::new(false));

        // Spawn healthz/metrics HTTP server for Kubernetes probes and Prometheus scraping
        let healthz_port: u16 = std::env::var("HEALTHZ_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8081);
        let healthz_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), healthz_port);
        let health_state = HealthState {
            ready: Arc::clone(&ready),
            context: Arc::new(context.child("metrics_view")),
            validator_metrics,
        };
        context.child("healthz").spawn(move |_| async move {
            let app = Router::new()
                .route("/healthz", get(healthz_handler))
                .route("/readyz", get(readyz_handler))
                .route("/metrics", get(metrics_handler))
                .with_state(health_state);
            match TcpListener::bind(healthz_addr).await {
                Ok(listener) => {
                    tracing::info!(%healthz_addr, "healthz server running");
                    if let Err(e) = axum::serve(listener, app).await {
                        tracing::error!("healthz server error: {}", e);
                    }
                }
                Err(e) => {
                    tracing::error!(%healthz_addr, "failed to bind healthz server: {}", e);
                }
            }
        });

        // ECDSA key loaded and engine spawned — node is ready to participate
        ready.store(true, Ordering::Relaxed);

        // Start network; blocks the root future (and thus keeps every spawned
        // child alive) until the network shuts down.
        if let Err(e) = network.start().await {
            tracing::error!(error = %e, "p2p network terminated");
        }
        drop(monitor_guard);
    });
}

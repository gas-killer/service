//! Gas Killer router: verifier-only aggregation engine + task sequencer +
//! on-chain submitter.
//!
//! The router is NOT a signing participant. It runs the commonware aggregation
//! engine with a verifier-only [`EcdsaScheme`] (`me() == None`): the engine
//! validates the nodes' TipAcks on channel 0, assembles ECDSA certificates at
//! quorum, journals them, and reports them to the [`CertReporter`]. Task flow:
//! HTTP ingress → sequencer (assigns aggregation heights, broadcasts
//! [`gas_killer_common::TaskDirective`]s on channel 1) → nodes sign → engine
//! certifies → submitter calls `GasKillerSDK.verifyAndUpdate` on-chain.

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
use commonware_runtime::{
    Metrics, Quota, Runner, Spawner, Supervisor,
    tokio::{self},
};
use commonware_utils::ordered::{Map, Quorum as _, Set};
use commonware_utils::{N3f1, NZU16, NZU32, NZU64, NZUsize, NonZeroDuration};
use eigen_logging::log_level::LogLevel;
use gas_killer_common::ecdsa::{EcdsaScheme, PublicKey};
use gas_killer_common::get_operator_states;
use gas_killer_common::{
    GasKillerValidator, SignatureScheme, SpeculativePrebuildConfig, StaticEpochMonitor,
    ack_messages_per_second, agg_activity_timeout, agg_window, get_signer, load_key_from_file,
    p2p_message_backlog, p2p_quota_period, quorum_threshold_fraction, rebroadcast_interval,
    round_timeout, schnorr_messages_per_second, schnorr_stage_timeout, signature_scheme,
    storage_directory,
};
use gas_killer_router::automaton::RouterAutomaton;
use gas_killer_router::factories::{create_ingress, create_schnorr_submitter, create_submitter};
use gas_killer_router::metrics::MetricsCollector;
use gas_killer_router::precommit::SchnorrPrecommitRouter;
use gas_killer_router::reporter::{CertReporter, certified_channel};
use gas_killer_router::schnorr_coordinator::{SchnorrCoordinator, schnorr_certified_channel};
use gas_killer_router::sequencer::{
    DispatchTime, Sequencer, TipReports, ingest_tip_reports, resolution_channel, shared_assignments,
};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Unique namespace to avoid message replay attacks.
const APPLICATION_NAMESPACE: &[u8] = b"_COMMONWARE_AGGREGATION_";

/// Maximum p2p message size. `TipAck`s are tiny; `TaskDirective::Announce` is
/// bounded by the 128 KB combined calldata/storage-updates limit — 1 MB is
/// generous headroom (`Sender::send` panics above this).
const MAX_MESSAGE_SIZE: u32 = 1024 * 1024; // 1 MB

/// P2p channel carrying the aggregation engine's `TipAck`s (engine-internal).
const ACK_CHANNEL: u64 = 0;
/// P2p channel on which the router broadcasts `TaskDirective`s to the nodes.
const DIRECTIVE_CHANNEL: u64 = 1;

/// P2P channel carrying the interactive Schnorr signing rounds
/// (`SIGNATURE_SCHEME=schnorr` mode only; never registered in ECDSA mode).
const SCHNORR_CHANNEL: u64 = 2;

/// Journal partition for the router's verifier-only engine (a subdirectory of
/// the runtime storage directory).
const JOURNAL_PARTITION: &str = "aggregation-router";

#[derive(Clone)]
struct HealthState {
    ready: Arc<AtomicBool>,
    // tokio::Context is !Clone in 2026.5.0; encode() works through a shared handle.
    context: Arc<tokio::Context>,
    metrics: Arc<MetricsCollector>,
}

/// Liveness probe — always 200 if the process is running.
async fn healthz_handler() -> StatusCode {
    StatusCode::OK
}

/// Readiness probe — 503 until the engine/sequencer/submitter are spawned and
/// the network is starting.
async fn readyz_handler(State(s): State<HealthState>) -> StatusCode {
    if s.ready.load(Ordering::Relaxed) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Prometheus metrics endpoint — encodes commonware runtime metrics and gas-killer custom metrics.
async fn metrics_handler(State(s): State<HealthState>) -> impl IntoResponse {
    let mut output = s.context.encode();
    output.push_str(&s.metrics.encode());
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

fn main() {
    // Parse arguments (flags unchanged from the pre-migration router).
    let matches = Command::new("orchestrator")
        .about("collect and verify ECDSA multi-signatures")
        .arg(
            Arg::new("bootstrappers")
                .long("bootstrappers")
                .required(false)
                .value_delimiter(',')
                .value_parser(clap::value_parser!(String)),
        )
        .arg(
            Arg::new("key-file")
                .long("key-file")
                .required(true)
                .help("Path to the JSON file containing the router ECDSA private key"),
        )
        .arg(
            Arg::new("port")
                .long("port")
                .required(true)
                .help("Port to run the service on"),
        )
        .get_matches();

    // Configure my identity
    let key_file = matches
        .get_one::<String>("key-file")
        .expect("--key-file is required");
    let port = matches
        .get_one::<String>("port")
        .expect("--port is required");
    let key = load_key_from_file(key_file);
    let signer = get_signer(&key);
    let port = port.parse::<u16>().expect("Port not well-formed");
    tracing::info!(port, "loaded port");

    // Log the router's identity for config generation
    let my_pub_key = signer.public_key();
    println!("Router identity for public_orchestrator.json:");
    println!("  publicKey: {my_pub_key}");

    // Initialize runtime. A stable storage directory is REQUIRED: the engine's
    // certificate journal must survive restarts (the runtime default is a
    // random per-process temp dir, which would silently lose replay).
    let storage_dir = storage_directory().join("router");
    println!(
        "Engine journal storage directory: {}",
        storage_dir.display()
    );
    let runtime_cfg = tokio::Config::default()
        .with_worker_threads(4)
        .with_storage_directory(storage_dir);
    let runner = tokio::Runner::new(runtime_cfg);

    // Configure network
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
    // (Was `attempt_unregistered_handshakes` before 2026.5.0.)
    p2p_cfg.bypass_ip_check = true;

    // recommended() throttles peer discovery for large open gossip networks where aggressive
    // dialing is abusive. gas-killer instead runs a small, static, allowlisted operator set in a
    // full mesh, so keep discovery fast (500ms dial cadence) for quick (re)join while retaining
    // recommended's abuse-resistance (concurrent-handshake cap, subnet rate limit, ping cadence).
    p2p_cfg.dial_frequency = Duration::from_millis(500);
    // `peer_connection_cooldown` is the minimum time between dial reservations for a
    // single peer, so it also bounds how fast a FAILED initial dial retries. The
    // router starts after the nodes (compose staggers it), so the nodes' first dials
    // fail; a long cooldown then delays reconnection past the round timeout and the
    // first task certifies as a skip. Keep it short so the static mesh re-forms within
    // a couple of seconds of the router coming up. (A larger value was briefly used to
    // damp a HandshakeError(DecryptionFailed) reconnect flap, but that was specific to
    // slow QEMU-emulated CI hosts and does not occur on native amd64.)
    p2p_cfg.peer_connection_cooldown = Duration::from_secs(3);
    p2p_cfg.allowed_handshake_rate_per_ip = Quota::per_second(NZU32!(16));

    // Start runtime
    runner.start(|context| async move {
        let (mut network, mut oracle) = Network::new(context.child("network"), p2p_cfg);
        let mut recipients: Vec<(PublicKey, SocketAddr)>;
        let quorum_infos;
        // Configure quorum number from environment (default: 0)
        let quorum_number: usize = std::env::var("QUORUM_NUMBER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        {
            eigen_logging::init_logger(LogLevel::Debug);
            // Get operator states and configure allowed peers
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

            recipients = Vec::new();
            let participants = quorum_infos[quorum_number].operators.clone();
            if participants.is_empty() {
                panic!("Please provide at least one participant");
            }
            for participant in participants {
                let verifier = PublicKey::from(participant.address);
                if let Some(socket) = participant.socket {
                    // Try to resolve hostname:port with retries (Docker DNS may need time)
                    if let Some(socket_addr) =
                        resolve_with_retry(&socket, 30, Duration::from_secs(2))
                    {
                        recipients.push((verifier, socket_addr));
                    } else {
                        // Last resort: try parsing as direct IP:PORT
                        match SocketAddr::from_str(&socket) {
                            Ok(socket_addr) => {
                                recipients.push((verifier, socket_addr));
                            }
                            Err(parse_err) => {
                                tracing::error!(
                                    socket,
                                    error = %parse_err,
                                    "Failed to resolve or parse socket address"
                                );
                                panic!("Bootstrapper address not well-formed: {socket}");
                            }
                        }
                    }
                }
            }
            // Authorize ourselves too (nodes dial the router from
            // public_orchestrator.json; this entry is never dialed by us).
            let orchestrator_verifier = signer.public_key();
            recipients.push((orchestrator_verifier, my_addr));
        }
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::stdout)
            .finish();
        _ = tracing::subscriber::set_default(subscriber);

        // Provide authorized peers. `from_iter_dedup` keeps the first entry per
        // key (the operator list may already contain the router's key).
        let peers: Map<PublicKey, Address> = Map::from_iter_dedup(
            recipients
                .iter()
                .cloned()
                .map(|(pk, sa)| (pk, Address::Symmetric(sa))),
        );
        let _ = oracle.track(0, peers);

        // Build the participant set (sorted operator addresses — participant
        // indices derive from this order on every process), IDENTICAL to the
        // node's construction in gas-killer-node/src/main.rs.
        let operators = &quorum_infos[quorum_number].operators;
        if operators.is_empty() {
            panic!("Please provide at least one contributor");
        }
        let participants: Set<PublicKey> = Set::from_iter_dedup(operators.iter().map(|operator| {
            let key = PublicKey::from(operator.address);
            tracing::info!(key = ?key, "registered contributor");
            key
        }));

        let scheme_mode = signature_scheme();
        // The contract-derived threshold is informational only: the ECDSA
        // engine's quorum is fixed at N3f1 (n - (n-1)/3), the schnorr
        // coordinator uses the QUORUM_THRESHOLD fraction as its participation
        // floor, and the authoritative check runs on-chain either way.
        tracing::info!(
            ?scheme_mode,
            participants = participants.len(),
            engine_quorum = participants.quorum::<N3f1>(),
            contract_threshold = quorum_infos[quorum_number].threshold,
            "operator set loaded"
        );

        // Register the shared directive channel (must precede network.start()):
        // the router SENDS directives on channel 1 and receives the nodes'
        // rate-limited TipReport replies (journal-loss recovery) on the same
        // channel. The mode-specific signing channel — 0 for the ECDSA engine's
        // TipAck traffic, 2 for the interactive Schnorr rounds — is registered in
        // the mode branch below, still before network.start().
        let p2p_backlog = p2p_message_backlog();
        let p2p_quota = Quota::with_period(p2p_quota_period())
            .expect("p2p_quota_period always returns a non-zero duration");
        let (directive_sender, directive_receiver) =
            network.register(DIRECTIVE_CHANNEL, p2p_quota, p2p_backlog);

        // Custom Prometheus metrics — shared by ingress, sequencer, and submitter.
        let metrics = Arc::new(MetricsCollector::new());

        // Shared validator: the sequencer uses it for EVMSketch enrichment; its
        // speculative pre-build loop warms the executor cache off the hot path.
        let validator = Arc::new(
            GasKillerValidator::new()
                .expect("HTTP_RPC environment variable must be set for gas analyzer"),
        );
        {
            let spec_validator = Arc::clone(&validator);
            let prebuild_cfg = SpeculativePrebuildConfig::from_env();
            context.child("prebuild").spawn(move |_| async move {
                spec_validator.run_speculative_prebuild(prebuild_cfg).await;
            });
        }

        // State shared across sequencer / signing path / submitter.
        let assignments = shared_assignments();
        let dispatch_time: DispatchTime = Arc::new(Mutex::new(HashMap::new()));
        let (resolution_sender, resolution_receiver) = resolution_channel();

        // HTTP ingress (env-gated, unchanged endpoints). The returned sender is
        // kept alive below so the task channel never closes while running
        // without the HTTP server.
        let ingress = create_ingress(Arc::clone(&metrics))
            .await
            .expect("Failed to create ingress");
        let _task_sender = ingress.sender;

        // Node tip reports (channel 1, node → router): if this router lost its
        // journal and assigns heights the nodes are already past, their reports
        // fast-forward the sequencer instead of wedging on a dead height.
        let tip_reports = TipReports::new(participants.len());
        {
            let participant_keys: HashSet<PublicKey> = participants.iter().cloned().collect();
            let tip_reports = tip_reports.clone();
            context.child("tip_reports").spawn(move |_| async move {
                ingest_tip_reports(directive_receiver, participant_keys, tip_reports).await;
            });
        }

        // Directive recipients: the explicit operator keys (see Sequencer::broadcast).
        let directive_recipients: Vec<PublicKey> = participants.iter().cloned().collect();

        // Mode-specific signing path. ECDSA runs the verifier-only aggregation
        // engine + certificate reporter; Schnorr runs the interactive two-round
        // MuSig2 coordinator on channel 2 (see schnorr_coordinator.rs). The
        // sequencer is shared — only its certificate-observation source differs.
        match scheme_mode {
            SignatureScheme::Ecdsa => {
                // Verifier-only scheme: the router validates acks and assembles
                // certificates but never signs (its key is not in the participant
                // set).
                let scheme = EcdsaScheme::verifier(participants);

                // The ack channel needs its own, much larger quota: node engines
                // keep rebroadcasting each signed height's TipAck until it falls
                // activity_timeout below the tip (even after certification), and
                // the p2p limiter silently drops messages beyond the per-peer
                // rate — an undersized quota here starves the router of fresh
                // acks and stalls certification.
                let ack_rate = ack_messages_per_second();
                let ack_quota = Quota::per_second(ack_rate);
                tracing::info!(
                    ack_messages_per_second = ack_rate.get(),
                    "engine channel quota"
                );
                let (ack_sender, ack_receiver) =
                    network.register(ACK_CHANNEL, ack_quota, p2p_backlog);

                let (certified_sender, certified_receiver) = certified_channel();

                // Certificate reporter actor (the engine's Reporter).
                let (cert_reporter, reporter_mailbox) = CertReporter::new(
                    context.child("cert_reporter"),
                    scheme.clone(),
                    certified_sender,
                );
                context
                    .child("cert_reporter_actor")
                    .spawn(move |_| cert_reporter.run());

                // Verifier-only aggregation engine on channel 0.
                let engine = Engine::new(
                    context.child("engine"),
                    AggregationConfig {
                        monitor: StaticEpochMonitor::new(),
                        provider: ConstantProvider::<EcdsaScheme, Epoch>::new(scheme.clone()),
                        automaton: RouterAutomaton::new(assignments.clone()),
                        reporter: reporter_mailbox.clone(),
                        blocker: oracle.clone(),
                        priority_acks: false,
                        rebroadcast_timeout: NonZeroDuration::new_panic(rebroadcast_interval()),
                        // Single static epoch: nothing to keep or accept beyond it.
                        epoch_bounds: (EpochDelta::new(0), EpochDelta::new(0)),
                        window: agg_window(),
                        activity_timeout: HeightDelta::new(agg_activity_timeout()),
                        journal_partition: JOURNAL_PARTITION.to_string(),
                        journal_write_buffer: NZUsize!(4096),
                        journal_replay_buffer: NZUsize!(4096),
                        journal_heights_per_section: NZU64!(64),
                        journal_compression: None,
                        journal_page_cache: CacheRef::from_pooler(
                            &context,
                            NZU16!(4096),
                            NZUsize!(128),
                        ),
                        strategy: Sequential,
                    },
                );
                engine.start((ack_sender, ack_receiver));

                // On-chain submitter: consumes verified certificates, resolves heights.
                let submitter = create_submitter(
                    scheme.clone(),
                    assignments.clone(),
                    certified_receiver,
                    resolution_sender,
                    Arc::clone(&metrics),
                    Arc::clone(&dispatch_time),
                )
                .await
                .expect("Failed to create submitter");
                context.child("submitter").spawn(move |_| submitter.run());

                // Sequencer: dequeues tasks, assigns heights, broadcasts
                // directives to the operator set.
                let sequencer = Sequencer::new(
                    ingress.receiver,
                    ingress.queue_depth,
                    validator,
                    Some(Arc::clone(&metrics)),
                    dispatch_time,
                    assignments,
                    reporter_mailbox,
                    resolution_receiver,
                    directive_sender,
                    directive_recipients,
                    tip_reports,
                );
                context.child("sequencer").spawn(move |_| sequencer.run());
            }
            SignatureScheme::Schnorr => {
                // The Schnorr rounds are request/response (no steady-state
                // rebroadcast like TipAcks), but a dropped message costs a whole
                // retry attempt, so the quota is generous.
                let schnorr_quota = Quota::per_second(schnorr_messages_per_second());
                let (schnorr_sender, schnorr_receiver) =
                    network.register(SCHNORR_CHANNEL, schnorr_quota, p2p_backlog);

                let (certified_sender, certified_receiver) = schnorr_certified_channel();

                // Coordinator: drives the two-round signing sessions per assigned
                // height and doubles as the sequencer's certificate index.
                let operator_keys: Vec<PublicKey> = participants.iter().cloned().collect();
                let (coordinator, coordinator_mailbox) = SchnorrCoordinator::new(
                    assignments.clone(),
                    certified_sender,
                    schnorr_sender,
                    schnorr_receiver,
                    operator_keys,
                    quorum_threshold_fraction(),
                    schnorr_stage_timeout(),
                    round_timeout(),
                );
                context
                    .child("schnorr_coordinator")
                    .spawn(move |_| coordinator.run());

                // On-chain submitter: consumes aggregate signatures, resolves heights.
                let submitter = create_schnorr_submitter(
                    assignments.clone(),
                    certified_receiver,
                    resolution_sender,
                    Arc::clone(&metrics),
                    Arc::clone(&dispatch_time),
                )
                .await
                .expect("Failed to create schnorr submitter");
                context
                    .child("schnorr_submitter")
                    .spawn(move |_| submitter.run());

                // Sequencer: unchanged behavior; its certificate observations come
                // from the coordinator's mailbox instead of the engine reporter.
                let sequencer = Sequencer::new(
                    ingress.receiver,
                    ingress.queue_depth,
                    validator,
                    Some(Arc::clone(&metrics)),
                    dispatch_time,
                    assignments,
                    coordinator_mailbox,
                    resolution_receiver,
                    directive_sender,
                    directive_recipients,
                    tip_reports,
                );
                context.child("sequencer").spawn(move |_| sequencer.run());
            }
            SignatureScheme::SchnorrPrecommit => {
                // On-chain reads: operator key points (verifier scheme construction).
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
                let rpc_provider = alloy::providers::ProviderBuilder::new()
                    .connect_http(http_rpc.parse().expect("HTTP_RPC is not a valid URL"));
                let chain_id = alloy::providers::Provider::get_chain_id(&rpc_provider)
                    .await
                    .expect("failed to read chain id from HTTP_RPC");
                let operator_addresses: Vec<alloy::primitives::Address> =
                    participants.iter().map(|k| k.address()).collect();
                let operator_points = gas_killer_common::schnorr::onchain::load_operator_keys(
                    rpc_provider.clone(),
                    stake_registry,
                    &operator_addresses,
                )
                .await
                .expect("failed to load operator keys from the SchnorrStakeRegistry");

                // Gossip-fed nonce directory + verifier scheme (`me() == None`).
                let directory =
                    Arc::new(gas_killer_common::schnorr::scheme::MemoryNonceDirectory::default());
                let store = Arc::new(gas_killer_common::schnorr::batches::BatchStore::new(
                    chain_id,
                    nonce_registry,
                    Arc::clone(&directory),
                ));
                let scheme = gas_killer_common::schnorr::scheme::SchnorrScheme::verifier(
                    operator_points,
                    directory,
                    gas_killer_common::schnorr::scheme::DEFAULT_ATTEMPTS_PER_HEIGHT,
                )
                .expect("operator key set contains duplicates");

                // Engine ack channel (same quota rationale as the ECDSA arm) and
                // the channel-2 gossip/completion channel.
                let ack_rate = ack_messages_per_second();
                let ack_quota = Quota::per_second(ack_rate);
                tracing::info!(
                    ack_messages_per_second = ack_rate.get(),
                    "engine channel quota"
                );
                let (ack_sender, ack_receiver) =
                    network.register(ACK_CHANNEL, ack_quota, p2p_backlog);
                let schnorr_quota = Quota::per_second(schnorr_messages_per_second());
                let (pc_sender, pc_receiver) =
                    network.register(SCHNORR_CHANNEL, schnorr_quota, p2p_backlog);

                let (certified_sender, certified_receiver) = schnorr_certified_channel();

                // The combined reporter / batch-gossip / completion actor; its
                // mailbox is both the engine's Reporter and the sequencer's
                // certificate index.
                let (actor, actor_mailbox) = SchnorrPrecommitRouter::new(
                    context.child("schnorr_precommit"),
                    scheme.clone(),
                    store,
                    operator_addresses,
                    certified_sender,
                    pc_receiver,
                    pc_sender,
                    rebroadcast_interval(),
                );
                context
                    .child("schnorr_precommit_actor")
                    .spawn(move |_| actor.run());

                // Verifier-only aggregation engine on channel 0.
                let engine = Engine::new(
                    context.child("engine"),
                    AggregationConfig {
                        monitor: StaticEpochMonitor::new(),
                        provider: ConstantProvider::<
                            gas_killer_common::schnorr::scheme::SchnorrScheme,
                            Epoch,
                        >::new(scheme.clone()),
                        automaton: RouterAutomaton::new(assignments.clone()),
                        reporter: actor_mailbox.clone(),
                        blocker: oracle.clone(),
                        priority_acks: false,
                        rebroadcast_timeout: NonZeroDuration::new_panic(rebroadcast_interval()),
                        epoch_bounds: (EpochDelta::new(0), EpochDelta::new(0)),
                        window: agg_window(),
                        activity_timeout: HeightDelta::new(agg_activity_timeout()),
                        journal_partition: JOURNAL_PARTITION.to_string(),
                        journal_write_buffer: NZUsize!(4096),
                        journal_replay_buffer: NZUsize!(4096),
                        journal_heights_per_section: NZU64!(64),
                        journal_compression: None,
                        journal_page_cache: CacheRef::from_pooler(
                            &context,
                            NZU16!(4096),
                            NZUsize!(128),
                        ),
                        strategy: Sequential,
                    },
                );
                engine.start((ack_sender, ack_receiver));

                // On-chain submitter: consumes final aggregate observations.
                let submitter = create_schnorr_submitter(
                    assignments.clone(),
                    certified_receiver,
                    resolution_sender,
                    Arc::clone(&metrics),
                    Arc::clone(&dispatch_time),
                )
                .await
                .expect("Failed to create schnorr submitter");
                context
                    .child("schnorr_submitter")
                    .spawn(move |_| submitter.run());

                // Sequencer: certificate observations come from the precommit actor.
                let sequencer = Sequencer::new(
                    ingress.receiver,
                    ingress.queue_depth,
                    validator,
                    Some(Arc::clone(&metrics)),
                    dispatch_time,
                    assignments,
                    actor_mailbox,
                    resolution_receiver,
                    directive_sender,
                    directive_recipients,
                    tip_reports,
                );
                context.child("sequencer").spawn(move |_| sequencer.run());
            }
        }

        // Readiness flag: set to true after everything is spawned and the network is starting
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
            metrics: Arc::clone(&metrics),
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

        // ECDSA key loaded, engine + sequencer + submitter spawned — router is
        // ready to collect certificates.
        ready.store(true, Ordering::Relaxed);

        // Run the network; blocks the root future (and thus the process) until
        // shutdown. All tasks spawned above are children of this context and
        // abort when it returns.
        let _ = network.start().await;
    });
}

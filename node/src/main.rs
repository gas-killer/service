//! Gas Killer Node — aggregate-Schnorr signing participant for the Gas Killer AVS.
//!
//! The router announces a task on p2p channel 1 and the node validates it via EVMSketch. The
//! node then answers the router coordinator's two-round MuSig2 session on channel 2 (see
//! [`schnorr_participant`]), signing the expected task digest. The p2p transport identity is
//! BN254; the Schnorr signing key is a separate secp256k1 operator key loaded from
//! `--schnorr-key-file`.

mod digest;
mod schnorr_participant;

use ::tokio::net::TcpListener;
use axum::{
    Router, extract::State, http::StatusCode, http::header, response::IntoResponse, routing::get,
};
use clap::{Arg, Command};
use commonware_avs_core::bn254::{Bn254, PublicKey, get_signer};
use commonware_avs_node::task_book::{self, TaskBook};
use commonware_cryptography::Signer as _;
use commonware_p2p::authenticated::lookup::{self, Network};
use commonware_p2p::{Address, AddressableManager as _};
use commonware_runtime::{Metrics, Quota, Runner, Spawner, Supervisor, tokio};
use commonware_utils::NZU32;
use commonware_utils::ordered::{Map, Set};
use eigen_logging::log_level::LogLevel;
use gas_killer_common::{
    APPLICATION_NAMESPACE, ConfigMetrics, GasKillerTaskData, GasKillerValidator,
    OrchestratorConfig, SpeculativePrebuildConfig, ValidatorMetrics, agg_window,
    config_fingerprint, get_operator_states, load_key_from_file, load_orchestrator_config,
    p2p_message_backlog, p2p_quota_period, rebroadcast_interval, round_timeout,
    schnorr_messages_per_second, storage_directory,
};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::digest::DigestResolver;

/// P2P channel carrying the router's `TaskDirective` broadcasts (nodes only
/// receive; the sender half is registered but never used).
const TASK_DIRECTIVE_CHANNEL: u64 = 1;

/// P2P channel carrying the interactive Schnorr signing rounds.
const SCHNORR_CHANNEL: u64 = 2;

#[derive(Clone)]
struct HealthState {
    ready: Arc<AtomicBool>,
    /// `tokio::Context` is not `Clone` in 2026.5.0; `Metrics::encode` works
    /// through a shared handle.
    context: Arc<tokio::Context>,
    validator_metrics: Arc<ValidatorMetrics>,
    /// This process's configuration fingerprint, published identically by every operator and
    /// the router so a split fleet is one query across the deployment.
    config_metrics: Arc<ConfigMetrics>,
}

/// Liveness probe — always 200 if the process is running.
async fn healthz_handler() -> StatusCode {
    StatusCode::OK
}

/// Readiness probe — 503 until the signing path is spawned and the network is starting.
async fn readyz_handler(State(s): State<HealthState>) -> StatusCode {
    if s.ready.load(Ordering::Relaxed) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Prometheus metrics endpoint — commonware runtime metrics, node validator timing, and this
/// process's configuration fingerprint.
async fn metrics_handler(State(s): State<HealthState>) -> impl IntoResponse {
    let mut output = s.context.encode();
    output.push_str(&s.validator_metrics.encode());
    output.push_str(&s.config_metrics.encode());
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

fn configure_identity(matches: &clap::ArgMatches) -> (Bn254, u16) {
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
    // The runtime otherwise defaults to a random per-process temp dir.
    let storage_dir = storage_directory();
    let runtime_cfg = tokio::Config::default()
        // 2026.5.0 defaults to 2 worker threads; the node runs p2p, the Schnorr participant,
        // EVMSketch validation, and the healthz server concurrently.
        .with_worker_threads(4)
        .with_storage_directory(storage_dir.clone());
    let runner = tokio::Runner::new(runtime_cfg);

    // Parse arguments
    let matches = Command::new("gas-killer-node")
        .about("Gas Killer AVS node - aggregate-Schnorr signing participant")
        .arg(
            Arg::new("key-file")
                .long("key-file")
                .required(true)
                .help("Path to the JSON file containing the BN254 p2p identity key"),
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
                .help("Path to orchestrator config file (JSON with G2 coordinates and port)"),
        )
        .arg(
            Arg::new("schnorr-key-file")
                .long("schnorr-key-file")
                .required(true)
                .help(
                    "Path to the JSON file containing the operator's secp256k1 Schnorr signing key",
                ),
        )
        .get_matches();

    // Configure my identity
    let (signer, port) = configure_identity(&matches);
    let orchestrator_config = configure_orchestrator(&matches);

    // The node signs with a separate secp256k1 operator key; the BN254 --key-file identity is only
    // the p2p transport key. Loaded here so the async closure below does not capture the CLI
    // matches.
    let schnorr_key = gas_killer_common::schnorr::private_key_from_hex(&load_key_from_file(
        matches
            .get_one::<String>("schnorr-key-file")
            .expect("--schnorr-key-file is required"),
    ))
    .expect("schnorr key is not a valid secp256k1 scalar");

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
                let verifier = participant.pub_keys.as_ref().unwrap().g2_pub_key.clone();
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

            // Parse orchestrator (router) public key from G2 coordinates
            orchestrator_pub_key = PublicKey::create_from_g2_coordinates(
                &orchestrator_config.g2_x1,
                &orchestrator_config.g2_x2,
                &orchestrator_config.g2_y1,
                &orchestrator_config.g2_y2,
            )
            .expect("Invalid orchestrator G2 coordinates");
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

        tracing::info!(storage_dir = %storage_dir.display(), "runtime storage directory");

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

        // The participant set: the operators' BN254 p2p identities.
        let operators = &quorum_infos[quorum_number].operators;
        if operators.is_empty() {
            panic!("No operators found");
        }
        let participants: Set<PublicKey> = Set::from_iter_dedup(operators.iter().map(|operator| {
            let keys = operator.pub_keys.as_ref().expect("operator has BN254 keys");
            keys.g2_pub_key.clone()
        }));
        for key in participants.iter() {
            tracing::info!(key = ?key, "registered participant");
        }

        // Shared channel registration (all channels must be registered BEFORE
        // network.start()): channel 1 carries the router's task directives (nodes receive; the
        // sender is only used for rate-limited TipReport replies to stale directives), and
        // channel 2 the Schnorr rounds, registered below.
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
        // per-height subscriptions the digest resolver makes until the skip rules resolve them.
        let (task_book, task_book_mailbox) =
            TaskBook::<GasKillerTaskData>::new(context.child("task_book"));
        context
            .child("task_book_actor")
            .spawn(move |_| task_book.run());

        // The highest height the coordinator is working on, written by the Schnorr participant and
        // read by the directive ingest loop to answer stale directives with a TipReport (see
        // task_book).
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
                    agg_window().get(),
                    min_report_interval,
                )
                .await;
            });
        }

        // The p2p transport identity is BN254; the Schnorr signing key is a separate secp256k1
        // operator key. Fail fast if either identity is not a registered operator.
        let own_bn254 = signer.public_key();
        if !participants.iter().any(|k| *k == own_bn254) {
            panic!(
                "own BN254 G2 key {own_bn254:?} is not in the quorum-{quorum_number} \
                 operator set; register the operator on-chain before starting the node"
            );
        }
        let own_address = schnorr_key.public_key().eth_address();
        if !operators.iter().any(|o| o.address == own_address) {
            panic!(
                "own Schnorr key address {own_address:?} is not in the \
                 quorum-{quorum_number} operator set; register the operator \
                 on-chain before starting the node"
            );
        }

        // The Schnorr rounds are request/response, but a dropped message costs a whole retry
        // attempt, so the quota is generous.
        let schnorr_quota = Quota::per_second(schnorr_messages_per_second());
        let (schnorr_sender, schnorr_receiver) =
            network.register(SCHNORR_CHANNEL, schnorr_quota, p2p_backlog);

        // Announce-vs-skip resolution plus EVMSketch validation of the expected digest.
        let resolver = DigestResolver::new(
            task_book_mailbox.clone(),
            Arc::clone(&validator),
            APPLICATION_NAMESPACE.to_vec(),
            round_timeout(),
        );
        let operator_addresses: HashSet<_> = operators.iter().map(|o| o.address).collect();
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

        // Readiness flag: set to true after the signing path is spawned and network is starting
        let ready = Arc::new(AtomicBool::new(false));

        // Spawn healthz/metrics HTTP server for Kubernetes probes and Prometheus scraping
        let healthz_port: u16 = std::env::var("HEALTHZ_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8081);
        let healthz_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), healthz_port);
        let fingerprint = config_fingerprint();
        tracing::info!(%fingerprint, "configuration fingerprint");
        let health_state = HealthState {
            ready: Arc::clone(&ready),
            context: Arc::new(context.child("metrics_view")),
            validator_metrics,
            config_metrics: Arc::new(ConfigMetrics::new(&fingerprint)),
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

        // Key loaded and signing path spawned — node is ready to participate
        ready.store(true, Ordering::Relaxed);

        // Start network; blocks the root future (and thus keeps every spawned
        // child alive) until the network shuts down.
        if let Err(e) = network.start().await {
            tracing::error!(error = %e, "p2p network terminated");
        }
    });
}

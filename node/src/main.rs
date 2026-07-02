//! Gas Killer Node - Aggregation contributor for the Gas Killer AVS
//!
//! This node participates in BN254 signature aggregation for gas-efficient
//! state transitions on EigenLayer.

use ::tokio::net::TcpListener;
use axum::{
    Router, extract::State, http::StatusCode, http::header, response::IntoResponse, routing::get,
};
use clap::{Arg, Command};
use commonware_avs_core::bn254::{Bn254, PublicKey, get_signer};
use commonware_avs_node::contributor::{AggregationInput, Contribute, Contributor};
use commonware_p2p::authenticated::lookup::{self, Network};
use commonware_p2p::{Address, AddressableManager};
use commonware_runtime::{Metrics, Runner, Spawner, Supervisor, tokio};
use commonware_utils::NZU32;
use commonware_utils::ordered::Map;
use eigen_logging::log_level::LogLevel;
use gas_killer_common::{
    GasKillerTaskData, GasKillerValidator, OrchestratorConfig, SpeculativePrebuildConfig,
    ValidatorMetrics, get_operator_states, load_key_from_file, load_orchestrator_config,
    p2p_message_backlog, p2p_quota_period,
};
use governor::Quota;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Unique namespace to avoid message replay attacks
const APPLICATION_NAMESPACE: &[u8] = b"_COMMONWARE_AGGREGATION_";

#[derive(Clone)]
struct HealthState {
    ready: Arc<AtomicBool>,
    context: Arc<tokio::Context>,
    validator_metrics: Arc<ValidatorMetrics>,
}

/// Liveness probe — always 200 if the process is running.
async fn healthz_handler() -> StatusCode {
    StatusCode::OK
}

/// Readiness probe — 503 until the network is starting and the contributor is spawned.
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
    // Initialize runtime
    let runtime_cfg = tokio::Config::default();
    let runner = tokio::Runner::new(runtime_cfg);

    // Parse arguments
    let matches = Command::new("gas-killer-node")
        .about("Gas Killer AVS node - BN254 signature aggregation contributor")
        .arg(
            Arg::new("key-file")
                .long("key-file")
                .required(true)
                .help("Path to the JSON file containing the BLS private key"),
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
        .get_matches();

    // Configure my identity
    let (signer, port) = configure_identity(&matches);
    let orchestrator_config = configure_orchestrator(&matches);

    // Start runtime
    runner.start(|context: tokio::Context| async move {
        let mut recipients: Vec<(PublicKey, Address)> = Vec::new();

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
            let participants = quorum_infos[quorum_number].operators.clone();
            if participants.is_empty() {
                panic!("No operators found in quorum");
            }

            for participant in &participants {
                let verifier = participant.pub_keys.as_ref().unwrap().g2_pub_key.clone();
                tracing::info!(key = ?verifier, "registered authorized peer");
                if let Some(socket) = &participant.socket {
                    // Try to resolve hostname:port with retries (Docker DNS may need time)
                    if let Some(socket_addr) =
                        resolve_with_retry(socket, 30, Duration::from_secs(2))
                    {
                        recipients.push((verifier, Address::from(socket_addr)));
                    } else {
                        // Last resort: try parsing as direct IP:PORT
                        match SocketAddr::from_str(socket) {
                            Ok(socket_addr) => {
                                recipients.push((verifier, Address::from(socket_addr)));
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

            // Parse orchestrator public key from G2 coordinates
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

            use std::net::ToSocketAddrs;

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

            recipients.push((
                orchestrator_pub_key.clone(),
                Address::from(orchestrator_addr),
            ));
        }

        // Configure tracing
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::stdout)
            .finish();
        _ = tracing::subscriber::set_default(subscriber);

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
        p2p_cfg.bypass_ip_check = true;

        // recommended() throttles peer discovery for large open gossip networks where aggressive
        // dialing is abusive. gas-killer instead runs a small, static, allowlisted operator set in a
        // full mesh: every participant dials every other, so both ends frequently dial at once and
        // one connection loses the reservation race. The loser must re-dial quickly, and an operator
        // that restarts must rejoin the signing quorum in seconds rather than ~a minute. Restore fast
// (re)discovery while keeping recommended's abuse-resistance (concurrent-handshake cap, subnet
        // rate limit, ping cadence).
        p2p_cfg.dial_frequency = Duration::from_millis(500);
        p2p_cfg.peer_connection_cooldown = Duration::from_secs(1);
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

        // Register the authorized peer set (id 0) with the oracle.
        oracle.track(0, Map::from_iter_dedup(recipients));

        // Build contributor list and G1 map from operator states
        let mut contributors = Vec::new();
        let mut g1_map = HashMap::new();
        let operators = &quorum_infos[quorum_number].operators;

        if operators.is_empty() {
            panic!("No operators found");
        }

        for operator in operators {
            let g2_key = operator.pub_keys.as_ref().unwrap().g2_pub_key.clone();
            let g1_key = operator.pub_keys.as_ref().unwrap().g1_pub_key.clone();
            tracing::info!(key = ?g2_key, "registered contributor");

            contributors.push(g2_key.clone());
            g1_map.insert(g2_key, g1_key);
        }

        let threshold = quorum_infos[quorum_number].threshold;
        let aggregation_input = AggregationInput::new(threshold, g1_map);

        // Create network channel
        let p2p_backlog = p2p_message_backlog();
        let p2p_quota = Quota::with_period(p2p_quota_period())
            .expect("p2p_quota_period always returns a non-zero duration");
        let (sender, receiver) = network.register(0, p2p_quota, p2p_backlog);

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
            context
                .child("speculative_prebuild")
                .spawn(move |_| async move {
                    spec_validator.run_speculative_prebuild(prebuild_cfg).await;
                });
        }

        // Create contributor with GasKillerTaskData as the metadata type
        let contributor = Contributor::<GasKillerTaskData>::new(
            orchestrator_pub_key,
            signer,
            contributors,
            Some(aggregation_input),
        )
        .with_validator(validator);

        // Readiness flag: set to true after contributor is spawned and network is starting
        let ready = Arc::new(AtomicBool::new(false));

        // Spawn healthz/metrics HTTP server for Kubernetes probes and Prometheus scraping
        let healthz_port: u16 = std::env::var("HEALTHZ_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8081);
        let healthz_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), healthz_port);
        let health_state = HealthState {
            ready: Arc::clone(&ready),
            context: Arc::new(context.child("metrics")),
            validator_metrics,
        };
        context.child("healthz_server").spawn(move |_| async move {
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

        // Spawn contributor task
        context.spawn(|_| async move {
            if let Err(e) = contributor.run(sender, receiver).await {
                tracing::error!("Contributor error: {}", e);
            }
        });

        // BLS key loaded and contributor spawned — node is ready to participate
        ready.store(true, Ordering::Relaxed);

        // Start network
        _ = network.start().await;
    });
}

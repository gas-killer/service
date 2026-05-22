use ::tokio::net::TcpListener;
use ark_bn254::G2Affine;
use ark_ff::PrimeField;
use ark_serialize::CanonicalDeserialize;
use axum::{
    Router, extract::State, http::StatusCode, http::header, response::IntoResponse, routing::get,
};
use clap::{Arg, Command, value_parser};
use commonware_avs_core::bn254::{PublicKey, get_signer};
use commonware_avs_router::orchestrator::builder::OrchestratorBuilder;
use commonware_avs_router::orchestrator::traits::OrchestratorTrait;
use commonware_cryptography::Signer;
use commonware_p2p::Manager;
use commonware_p2p::authenticated::lookup::{self, Network};
use commonware_runtime::{
    Metrics, Runner, Spawner,
    tokio::{self},
};
use commonware_utils::NZU32;
use commonware_utils::set::OrderedAssociated;
use eigen_logging::log_level::LogLevel;
use gas_killer_common::{get_operator_states, load_key_from_file};
use gas_killer_router::GasKillerOrchestratorBuilder;
use gas_killer_router::metrics::MetricsCollector;
use governor::Quota;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

// Unique namespace to avoid message replay attacks.
const APPLICATION_NAMESPACE: &[u8] = b"_COMMONWARE_AGGREGATION_";

#[derive(Clone)]
struct HealthState {
    ready: Arc<AtomicBool>,
    context: tokio::Context,
    metrics: Arc<MetricsCollector>,
}

/// Liveness probe — always 200 if the process is running.
async fn healthz_handler() -> StatusCode {
    StatusCode::OK
}

/// Readiness probe — 503 until the network is starting and the orchestrator is spawned.
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

/// Generate a fresh BLS keypair and write router_orchestrator.json + public_orchestrator.json
/// to `output_dir`. Idempotent: skips if `.router_key_complete` already exists in that directory.
fn generate_key_files(output_dir: &str) {
    use rand::RngCore;
    use std::os::unix::fs::PermissionsExt;

    let dir = std::path::Path::new(output_dir);

    let marker = dir.join(".router_key_complete");
    if marker.exists() {
        println!(
            "Router keypair already exists ({} found). Skipping.",
            marker.display()
        );
        return;
    }

    // Generate 32 bytes of CSPRNG entropy, reduce into the BN254 scalar field.
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let sk = ark_bn254::Fr::from_be_bytes_mod_order(&bytes);
    // Display for prime field elements is the canonical decimal integer.
    let private_key_decimal = sk.to_string();

    // Derive G2 public key via the commonware signer (same path used at runtime).
    let signer = get_signer(&private_key_decimal);
    let pub_key_bytes = signer.public_key();
    let g2 = G2Affine::deserialize_compressed(pub_key_bytes.as_ref())
        .expect("failed to deserialize G2 point from freshly generated key");

    // Write private key file (mode 0600 — owner-readable only).
    let priv_path = dir.join("router_orchestrator.json");
    let priv_json =
        serde_json::to_string_pretty(&serde_json::json!({ "privateKey": private_key_decimal }))
            .expect("failed to serialize private key");
    std::fs::write(&priv_path, &priv_json)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", priv_path.display()));
    std::fs::set_permissions(&priv_path, std::fs::Permissions::from_mode(0o600))
        .unwrap_or_else(|e| panic!("failed to chmod {}: {e}", priv_path.display()));

    // Write public key file (mode 0644 — world-readable, not sensitive).
    let pub_path = dir.join("public_orchestrator.json");
    let pub_json = serde_json::to_string_pretty(&serde_json::json!({
        "g2_x1": g2.x.c0.to_string(),
        "g2_x2": g2.x.c1.to_string(),
        "g2_y1": g2.y.c0.to_string(),
        "g2_y2": g2.y.c1.to_string(),
        "port": "3000",
        "address": ""
    }))
    .expect("failed to serialize public key");
    std::fs::write(&pub_path, &pub_json)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", pub_path.display()));
    std::fs::set_permissions(&pub_path, std::fs::Permissions::from_mode(0o644))
        .unwrap_or_else(|e| panic!("failed to chmod {}: {e}", pub_path.display()));

    // Write completion marker so re-runs skip key generation.
    std::fs::write(&marker, b"")
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", marker.display()));
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644))
        .unwrap_or_else(|e| panic!("failed to chmod {}: {e}", marker.display()));

    println!("Generated router BLS keypair:");
    println!("  private key → {}", priv_path.display());
    println!("  public key  → {}", pub_path.display());
    println!("  g2_x1: {}", g2.x.c0);
    println!("  g2_x2: {}", g2.x.c1);
    println!("  g2_y1: {}", g2.y.c0);
    println!("  g2_y2: {}", g2.y.c1);
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
    // Initialize runtime
    let runtime_cfg = tokio::Config::default();
    let runner = tokio::Runner::new(runtime_cfg.clone());

    // Parse arguments
    let matches = Command::new("gas-killer-router")
        .about("Gas Killer BLS aggregation router")
        .subcommand(
            Command::new("generate-key")
                .about("Generate a BLS keypair for the router orchestrator")
                .arg(
                    Arg::new("output-dir")
                        .long("output-dir")
                        .required(true)
                        .help("Directory to write router_orchestrator.json and public_orchestrator.json"),
                ),
        )
        .arg(
            Arg::new("bootstrappers")
                .long("bootstrappers")
                .required(false)
                .value_delimiter(',')
                .value_parser(value_parser!(String)),
        )
        .arg(
            Arg::new("key-file")
                .long("key-file")
                .required(false)
                .help("Path to the JSON file containing the router BLS private key"),
        )
        .arg(
            Arg::new("port")
                .long("port")
                .required(false)
                .help("Port to run the service on"),
        )
        .get_matches();

    if let Some(keygen_matches) = matches.subcommand_matches("generate-key") {
        let output_dir = keygen_matches
            .get_one::<String>("output-dir")
            .expect("--output-dir is required");
        generate_key_files(output_dir);
        return;
    }

    // Configure my identity
    let key_file = matches
        .get_one::<String>("key-file")
        .expect("Please provide key file");
    let port = matches
        .get_one::<String>("port")
        .expect("Please provide port");
    let key = load_key_from_file(key_file);
    let me = format!("{key}@{port}");
    let parts = me.split('@').collect::<Vec<&str>>();
    if parts.len() != 2 {
        panic!("Identity not well-formed");
    }
    let key = parts[0];
    let signer = get_signer(key);
    let port = parts[1].parse::<u16>().expect("Port not well-formed");
    tracing::info!(port, "loaded port");

    // Log the router's public key G2 coordinates for config generation
    let my_pub_key = signer.public_key();
    let g2_point = G2Affine::deserialize_compressed(my_pub_key.as_ref()).unwrap();
    println!("Router G2 coordinates for public_orchestrator.json:");
    println!("  g2_x1: {}", g2_point.x.c0);
    println!("  g2_x2: {}", g2_point.x.c1);
    println!("  g2_y1: {}", g2_point.y.c0);
    println!("  g2_y2: {}", g2_point.y.c1);

    // Configure network
    const MAX_MESSAGE_SIZE: usize = 1024 * 1024; // 1 MB
    let my_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
    let my_local_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut p2p_cfg = lookup::Config::local(
        signer.clone(),
        APPLICATION_NAMESPACE,
        my_addr,
        my_local_addr,
        MAX_MESSAGE_SIZE,
    );

    // Must stay true for K8s deployments (DNAT/SNAT means source IPs at the listener are
    // always pod IPs, never the registered ClusterIP addresses) and for mixed-network topologies
    // where external operators are behind NAT. IP-based pre-filtering cannot work in either
    // case; authentication relies entirely on the cryptographic handshake (peer public keys
    // checked against the registered operator set), which is secure for both topologies.
    p2p_cfg.attempt_unregistered_handshakes = true;

    // Start runtime
    runner.start(|context| async move {
        let (mut network, mut oracle) = Network::new(context.with_label("network"), p2p_cfg);
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
                let verifier = participant.pub_keys.unwrap().g2_pub_key;
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
            let orchestrator_verifier = signer.public_key();
            recipients.push((orchestrator_verifier, my_addr));
        }
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::stdout)
            .finish();
        _ = tracing::subscriber::set_default(subscriber);

        // Provide authorized peers
        let authorized = OrderedAssociated::from_iter(recipients.clone());
        oracle.update(0, authorized).await;

        // Parse contributors from operator states
        let mut contributors = Vec::new();
        let mut contributors_map = HashMap::new();
        let operators = &quorum_infos[quorum_number].operators;
        if operators.is_empty() {
            panic!("Please provide at least one contributor");
        }
        for operator in operators {
            let verifier = operator.pub_keys.as_ref().unwrap().g2_pub_key.clone();
            let verifier_g1 = operator.pub_keys.as_ref().unwrap().g1_pub_key.clone();
            tracing::info!(key = ?verifier, "registered contributor",);
            contributors.push(verifier.clone());
            contributors_map.insert(verifier, verifier_g1);
        }

        let threshold = quorum_infos[quorum_number].threshold;

        // Run as the orchestrator using the builder pattern
        const DEFAULT_MESSAGE_BACKLOG: usize = 256;

        let (sender, receiver) =
            network.register(0, Quota::per_second(NZU32!(1)), DEFAULT_MESSAGE_BACKLOG);

        // Custom Prometheus metrics — shared with executor, creator, and ingress via builder
        let metrics = Arc::new(MetricsCollector::new());

        // Use the builder pattern to create the orchestrator
        let builder = OrchestratorBuilder::new(context.clone(), signer)
            .with_contributors(contributors)
            .with_g1_map(contributors_map)
            .with_threshold(threshold)
            .load_from_env(); // Read configuration from environment variables

        let orchestrator = GasKillerOrchestratorBuilder::build(builder, Arc::clone(&metrics))
            .await
            .expect("Failed to build orchestrator");

        context
            .clone()
            .spawn(|_| async move { orchestrator.run(sender, receiver).await });

        // Readiness flag: set to true after orchestrator is spawned and network is starting
        let ready = Arc::new(AtomicBool::new(false));

        // Spawn healthz/metrics HTTP server for Kubernetes probes and Prometheus scraping
        let healthz_port: u16 = std::env::var("HEALTHZ_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8081);
        let healthz_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), healthz_port);
        let health_state = HealthState {
            ready: Arc::clone(&ready),
            context: context.clone(),
            metrics: Arc::clone(&metrics),
        };
        context.clone().spawn(move |_| async move {
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

        // BLS key loaded and orchestrator spawned — router is ready to handle aggregation
        ready.store(true, Ordering::Relaxed);

        _ = network.start().await;
    });
}

//! Gas Killer Node - Aggregation contributor for the Gas Killer AVS
//!
//! This node participates in BN254 signature aggregation for gas-efficient
//! state transitions on EigenLayer.

use axum::{Router, http::StatusCode, routing::get};
use clap::{Arg, Command};
use commonware_avs_core::bn254::{Bn254, PublicKey, get_signer};
use commonware_avs_node::contributor::{AggregationInput, Contribute, Contributor};
use commonware_p2p::Manager;
use commonware_p2p::authenticated::lookup::{self, Network};
use commonware_runtime::{Metrics, Runner, Spawner, tokio};
use commonware_utils::{NZU32, set::OrderedAssociated};
use eigen_logging::log_level::LogLevel;
use gas_killer_common::{
    GasKillerTaskData, GasKillerValidator, OrchestratorConfig, get_operator_states,
    load_key_from_file, load_orchestrator_config,
};
use governor::Quota;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// Unique namespace to avoid message replay attacks
const APPLICATION_NAMESPACE: &[u8] = b"_COMMONWARE_AGGREGATION_";

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
        let mut recipients: Vec<(PublicKey, SocketAddr)> = Vec::new();

        // Configure quorum number from environment (default: 0)
        let quorum_number: usize = std::env::var("QUORUM_NUMBER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        // Scoped to avoid configuring two loggers
        let orchestrator_pub_key;
        {
            eigen_logging::init_logger(LogLevel::Debug);
            let quorum_infos = get_operator_states()
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

            recipients.push((orchestrator_pub_key.clone(), orchestrator_addr));
        }

        // Configure tracing
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::stdout)
            .finish();
        _ = tracing::subscriber::set_default(subscriber);

        // Configure P2P network
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

        // Allow handshakes from IPs that aren't yet in the registered peer set
        // (needed for Docker networking where resolved IPs may differ)
        p2p_cfg.attempt_unregistered_handshakes = true;

        let (mut network, mut oracle) = Network::new(context.with_label("network"), p2p_cfg);

        // Debug: Log all recipients before updating oracle
        tracing::info!(
            count = recipients.len(),
            "registering recipients with oracle"
        );
        for (key, addr) in &recipients {
            tracing::info!(key = ?key, addr = ?addr, "oracle recipient");
        }

        // Register authorized peers with the oracle
        oracle
            .update(0, OrderedAssociated::from_iter(recipients.clone()))
            .await;

        // Build contributor list and G1 map from operator states
        let mut contributors = Vec::new();
        let mut g1_map = HashMap::new();
        let quorum_infos = get_operator_states()
            .await
            .expect("Failed to get operator states");
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

        // Calculate threshold (e.g., 2/3 + 1)
        let threshold = (operators.len() * 2 / 3) + 1;
        let aggregation_input = AggregationInput::new(threshold, g1_map);

        // Create network channel
        const DEFAULT_MESSAGE_BACKLOG: usize = 256;
        let (sender, receiver) =
            network.register(0, Quota::per_second(NZU32!(1)), DEFAULT_MESSAGE_BACKLOG);

        // Create validator for the gas killer use case (uses full gas-analyzer validation)
        let validator = Arc::new(
            GasKillerValidator::new()
                .expect("HTTP_RPC environment variable must be set for gas analyzer"),
        );

        // Create contributor with GasKillerTaskData as the metadata type
        let contributor = Contributor::<GasKillerTaskData>::new(
            orchestrator_pub_key,
            signer,
            contributors,
            Some(aggregation_input),
        )
        .with_validator(validator);

        // Spawn healthz HTTP server for Kubernetes readiness/liveness probes
        let healthz_port: u16 = std::env::var("HEALTHZ_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8080);
        let healthz_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), healthz_port);
        context.clone().spawn(move |_| async move {
            let app = Router::new().route("/healthz", get(|| async { StatusCode::OK }));
            match ::tokio::net::TcpListener::bind(healthz_addr).await {
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

        // Start network
        _ = network.start().await;
    });
}

//! Gas Killer Node - Aggregation contributor for the Gas Killer AVS
//!
//! This node participates in BN254 signature aggregation for gas-efficient
//! state transitions on EigenLayer.

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
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;

/// Unique namespace to avoid message replay attacks
const APPLICATION_NAMESPACE: &[u8] = b"_COMMONWARE_AGGREGATION_";

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
            tracing::info!(quorum_number, total_quorums = quorum_infos.len(), "using quorum");

            // Configure allowed peers from operator states
            let participants = quorum_infos[quorum_number].operators.clone();
            if participants.is_empty() {
                panic!("No operators found in quorum");
            }

            for participant in &participants {
                let verifier = participant.pub_keys.as_ref().unwrap().g2_pub_key.clone();
                tracing::info!(key = ?verifier, "registered authorized peer");
                if let Some(socket) = &participant.socket {
                    // Try to resolve hostname:port to socket addresses
                    use std::net::ToSocketAddrs;
                    match socket.to_socket_addrs() {
                        Ok(mut addrs) => {
                            if let Some(socket_addr) = addrs.next() {
                                recipients.push((verifier, socket_addr));
                            } else {
                                panic!("No addresses found for socket: {socket}");
                            }
                        }
                        Err(e) => {
                            // If resolution fails, try parsing as direct IP:PORT
                            match SocketAddr::from_str(socket) {
                                Ok(socket_addr) => {
                                    recipients.push((verifier, socket_addr));
                                }
                                Err(parse_err) => {
                                    tracing::error!("Failed to resolve '{}': {:?}, and failed to parse as IP: {:?}",
                                                  socket, e, parse_err);
                                    panic!("Socket address not well-formed: {socket}");
                                }
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

            // Resolve orchestrator address (hostname:port or IP:port)
            let orchestrator_socket = format!(
                "{}:{}",
                orchestrator_config.address.as_deref().unwrap_or("127.0.0.1"),
                orchestrator_config.port
            );
            tracing::info!(target = %orchestrator_socket, "resolved orchestrator address");

            use std::net::ToSocketAddrs;
            let orchestrator_addr = match orchestrator_socket.to_socket_addrs() {
                Ok(mut addrs) => addrs.next().expect("No addresses found for orchestrator"),
                Err(_) => {
                    // Fallback: parse as direct IP:PORT
                    SocketAddr::from_str(&orchestrator_socket)
                        .expect("Invalid orchestrator socket address")
                }
            };
            recipients.push((orchestrator_pub_key.clone(), orchestrator_addr));
        }

        // Configure tracing
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::stdout)
            .finish();
        let _ = tracing::subscriber::set_default(subscriber);

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
        tracing::info!(count = recipients.len(), "registering recipients with oracle");
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
        let validator = Arc::new(GasKillerValidator::new()
            .expect("RPC_URL or HTTP_RPC environment variable must be set for gas analyzer"));

        // Create contributor with GasKillerTaskData as the metadata type
        let contributor = Contributor::<GasKillerTaskData>::new(
            orchestrator_pub_key,
            signer,
            contributors,
            Some(aggregation_input),
        )
        .with_validator(validator);

        // Spawn contributor task
        context.spawn(|_| async move {
            if let Err(e) = contributor.run(sender, receiver).await {
                tracing::error!("Contributor error: {}", e);
            }
        });

        // Start network
        let _ = network.start().await;
    });
}

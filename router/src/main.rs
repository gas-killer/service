use ark_bn254::G2Affine;
use ark_serialize::CanonicalDeserialize;
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
use governor::Quota;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::time::Duration;

// Unique namespace to avoid message replay attacks.
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

fn main() {
    // Initialize runtime
    let runtime_cfg = tokio::Config::default();
    let runner = tokio::Runner::new(runtime_cfg.clone());

    // Parse arguments
    let matches = Command::new("orchestrator")
        .about("generate and verify BN254 Multi-Signatures")
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
                .required(true)
                .help("Path to the YAML file containing the private key"),
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

    // Allow handshakes from IPs that aren't yet in the registered peer set
    // TODO: Remove this once we have a proper way to handle handshakes
    // https://github.com/BreadchainCoop/gas-killer-router/issues/82
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
        let _ = tracing::subscriber::set_default(subscriber);

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

        // TODO: Retrieve threshold from EigenLayer smart contracts after middleware refactor
        // https://github.com/BreadchainCoop/commonware-restaking/issues/135
        let threshold = 3; // hardcoded for now

        // Run as the orchestrator using the builder pattern
        const DEFAULT_MESSAGE_BACKLOG: usize = 256;

        let (sender, receiver) =
            network.register(0, Quota::per_second(NZU32!(1)), DEFAULT_MESSAGE_BACKLOG);

        // Use the builder pattern to create the orchestrator
        let builder = OrchestratorBuilder::new(context.clone(), signer)
            .with_contributors(contributors)
            .with_g1_map(contributors_map)
            .with_threshold(threshold)
            .load_from_env(); // Read configuration from environment variables

        let orchestrator = GasKillerOrchestratorBuilder::build(builder)
            .await
            .expect("Failed to build orchestrator");

        context.spawn(|_| async move { orchestrator.run(sender, receiver).await });

        let _ = network.start().await;
    });
}

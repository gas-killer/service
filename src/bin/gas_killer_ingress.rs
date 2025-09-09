use clap::{Arg, Command};
use commonware_avs_router::usecases::gas_killer::start_gas_killer_ingress;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logger
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // Parse command line arguments
    let matches = Command::new("gas-killer-ingress")
        .about("Gas Killer Ingress HTTP Server")
        .arg(
            Arg::new("port")
                .long("port")
                .short('p')
                .value_name("PORT")
                .help("Port to listen on")
                .default_value("8080")
                .value_parser(clap::value_parser!(u16)),
        )
        .arg(
            Arg::new("host")
                .long("host")
                .short('h')
                .value_name("HOST")
                .help("Host to bind to")
                .default_value("0.0.0.0"),
        )
        .arg(
            Arg::new("max-queue-size")
                .long("max-queue-size")
                .value_name("SIZE")
                .help("Maximum queue size")
                .default_value("1000")
                .value_parser(clap::value_parser!(usize)),
        )
        .get_matches();

    let port = *matches.get_one::<u16>("port").unwrap();
    let host = matches.get_one::<String>("host").unwrap();
    let max_queue_size = *matches.get_one::<usize>("max-queue-size").unwrap();

    let addr = format!("{host}:{port}");

    tracing::info!("Starting Gas Killer Ingress server on {addr}");

    start_gas_killer_ingress(addr, max_queue_size).await?;

    Ok(())
}

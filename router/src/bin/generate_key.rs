use clap::{Arg, Command, value_parser};
use commonware_cryptography::Signer;
use gas_killer_common::ecdsa::get_signer;
use rand::RngCore;
use std::os::unix::fs::PermissionsExt;

fn main() {
    let matches = Command::new("generate_key")
        .about("Generate an ECDSA keypair for the router orchestrator")
        .arg(
            Arg::new("output-dir")
                .long("output-dir")
                .required(true)
                .help("Directory to write router_orchestrator.json and public_orchestrator.json"),
        )
        .arg(
            Arg::new("router-address")
                .long("router-address")
                .required(true)
                .help("Hostname or DNS name nodes use to reach this router"),
        )
        .arg(
            Arg::new("router-port")
                .long("router-port")
                .required(true)
                .value_parser(value_parser!(u16))
                .help("Port nodes use to reach this router"),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .alias("rotate")
                .action(clap::ArgAction::SetTrue)
                .help("Overwrite an existing keypair (use only for intentional key rotation)"),
        )
        .get_matches();

    let output_dir = matches.get_one::<String>("output-dir").unwrap();
    let router_address = matches.get_one::<String>("router-address").unwrap();
    let router_port = *matches.get_one::<u16>("router-port").unwrap();
    let force = matches.get_flag("force");

    let dir = std::path::Path::new(output_dir);
    let marker = dir.join(".router_key_complete");

    if marker.exists() {
        if force {
            println!("--force set: removing existing marker to regenerate keypair.");
            std::fs::remove_file(&marker)
                .unwrap_or_else(|e| panic!("failed to remove {}: {e}", marker.display()));
        } else {
            println!(
                "Router keypair already exists ({} found). Skipping.",
                marker.display()
            );
            return;
        }
    }

    // Rejection-sample a valid secp256k1 scalar (a uniform 32-byte string is
    // outside the scalar field with probability ~2^-128, but loop anyway).
    let private_key_hex = loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let candidate = format!("0x{}", hex::encode(bytes));
        if std::panic::catch_unwind(|| get_signer(&candidate)).is_ok() {
            break candidate;
        }
    };

    let signer = get_signer(&private_key_hex);
    let public_key = signer.public_key();

    let priv_path = dir.join("router_orchestrator.json");
    let priv_json =
        serde_json::to_string_pretty(&serde_json::json!({ "privateKey": private_key_hex }))
            .expect("failed to serialize private key");
    std::fs::write(&priv_path, &priv_json)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", priv_path.display()));
    std::fs::set_permissions(&priv_path, std::fs::Permissions::from_mode(0o600))
        .unwrap_or_else(|e| panic!("failed to chmod {}: {e}", priv_path.display()));

    let pub_path = dir.join("public_orchestrator.json");
    let pub_json = serde_json::to_string_pretty(&serde_json::json!({
        "publicKey": public_key.to_string(),
        "port": router_port.to_string(),
        "address": router_address
    }))
    .expect("failed to serialize public key");
    std::fs::write(&pub_path, &pub_json)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", pub_path.display()));
    std::fs::set_permissions(&pub_path, std::fs::Permissions::from_mode(0o644))
        .unwrap_or_else(|e| panic!("failed to chmod {}: {e}", pub_path.display()));

    std::fs::write(&marker, b"")
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", marker.display()));
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644))
        .unwrap_or_else(|e| panic!("failed to chmod {}: {e}", marker.display()));

    println!("Generated router ECDSA keypair:");
    println!("  private key → {}", priv_path.display());
    println!("  public key  → {}", pub_path.display());
    println!("  publicKey: {public_key}");
}

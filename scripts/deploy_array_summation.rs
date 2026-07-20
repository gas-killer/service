use alloy::network::EthereumWallet;
use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use bindings::arraysummationfactory::ArraySummationFactory;
use bindings::schnorrarraysummationfactory::SchnorrArraySummationFactory;
use bindings::schnorrstakeregistry::SchnorrStakeRegistry;
use gas_killer_common::schnorr::{PrivateKey, private_key_from_hex};
use gas_killer_common::{SignatureScheme, quorum_threshold_fraction, signature_scheme};
use rand::RngCore;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

type DynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Deserialize)]
struct AvsDeploymentJson {
    addresses: AvsAddresses,
}

#[derive(Debug, Deserialize)]
struct AvsAddresses {
    #[serde(rename = "avsServiceManagerWrapper")]
    avs_service_manager_wrapper: String,
    #[serde(rename = "IncredibleSquaringTaskManager")]
    bls_sig_check: String,
}

/// Operator key files the eigenlayer setup container writes next to the deployment
/// JSON. The Schnorr signing key IS the operator's secp256k1 key.
const OPERATOR_KEY_FILE_SUFFIX: &str = ".private.ecdsa.key.json";

#[derive(Debug, Deserialize)]
struct OperatorKeyFile {
    #[serde(rename = "privateKey")]
    private_key: String,
}

#[tokio::main]
async fn main() -> Result<(), DynError> {
    dotenv::dotenv().ok();

    // `SIGNATURE_SCHEME` selects which quorum stack the target contract verifies
    // against (default: bls). The bls path deploys the ArraySummation target via the
    // pre-deployed factory; the schnorr path additionally deploys the
    // SchnorrStakeRegistry and registers each operator's Schnorr key (with a proof
    // of possession) before deploying the target.
    match signature_scheme() {
        SignatureScheme::Bls => deploy_bls().await,
        SignatureScheme::Schnorr => deploy_schnorr().await,
    }
}

fn env_address(name: &str) -> Result<Option<Address>, DynError> {
    match env::var(name).ok().filter(|s| !s.trim().is_empty()) {
        Some(raw) => {
            Ok(Some(raw.trim().parse().map_err(|_| {
                format!("Invalid address in {name}: {raw}")
            })?))
        }
        None => Ok(None),
    }
}

fn env_u64(name: &str) -> Result<u64, DynError> {
    let raw = env::var(name).map_err(|_| format!("{name} environment variable is required"))?;
    Ok(raw
        .parse::<u64>()
        .map_err(|_| format!("{name} must be a valid number"))?)
}

/// Reads and parses the AVS deployment JSON produced by the eigenlayer setup container.
fn read_avs_deployment(avs_deployment_path: &str) -> Result<AvsDeploymentJson, DynError> {
    println!("📖 Reading AVS deployment from: {}", avs_deployment_path);
    let avs_content = fs::read_to_string(avs_deployment_path)
        .map_err(|e| format!("Failed to read AVS deployment file: {}", e))?;

    Ok(serde_json::from_str(&avs_content)
        .map_err(|e| format!("Failed to parse AVS deployment JSON: {}", e))?)
}

/// Deploys the BLS `ArraySummation` target through the pre-deployed factory
/// (`ARRAY_SUMMATION_FACTORY_ADDRESS`), wired to the BLS signature checker.
async fn deploy_bls() -> Result<(), DynError> {
    println!("🚀 Deploying ArraySummation...");

    let http_rpc = env::var("HTTP_RPC").map_err(|_| "HTTP_RPC environment variable is required")?;
    let private_key =
        env::var("PRIVATE_KEY").map_err(|_| "PRIVATE_KEY environment variable is required")?;
    let array_summation_factory_address = env::var("ARRAY_SUMMATION_FACTORY_ADDRESS")
        .map_err(|_| "ARRAY_SUMMATION_FACTORY_ADDRESS environment variable is required")?;
    let array_size = env_u64("ARRAY_SUMMATION_ARRAY_SIZE")?;
    let max_value = env_u64("ARRAY_SUMMATION_MAX_VALUE")?;
    let seed = env_u64("ARRAY_SUMMATION_SEED")?;

    // Parse addresses
    let factory_address: Address = array_summation_factory_address
        .parse()
        .map_err(|_| "Invalid ARRAY_SUMMATION_FACTORY_ADDRESS format")?;

    // Get AVS address from deployment JSON
    let avs_deployment_path = env::var("AVS_DEPLOYMENT_PATH")
        .map_err(|_| "AVS_DEPLOYMENT_PATH environment variable is required")?;

    let avs_deployment = read_avs_deployment(&avs_deployment_path)?;

    let avs_address: Address = avs_deployment
        .addresses
        .avs_service_manager_wrapper
        .parse()
        .map_err(|_| "Invalid avsServiceManagerWrapper address format in deployment JSON")?;

    // Get BLS signature checker address from deployment JSON
    let bls_address: Address = avs_deployment
        .addresses
        .bls_sig_check
        .parse()
        .map_err(|_| "Invalid IncredibleSquaringTaskManager address format")?;
    println!("🔐 Using BLS Signature Checker: {}", bls_address);

    // Setup provider and signer
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|_| "Invalid private key format")?;
    let provider = ProviderBuilder::new()
        .wallet(signer)
        .connect_http(http_rpc.parse().map_err(|_| "Invalid RPC URL")?);

    // Sanity checks: ensure target addresses have code deployed
    println!("🔍 Checking deployed code of contracts...");
    let code_avs = provider
        .get_code_at(avs_address)
        .await
        .map_err(|e| format!("Failed to get code for AVS address {}: {}", avs_address, e))?;
    if code_avs.as_ref().is_empty() {
        return Err(format!(
            "AvsServiceManagerWrapper {} has no code deployed. Check AVS_DEPLOYMENT_PATH.",
            avs_address
        )
        .into());
    }

    let code_bls = provider.get_code_at(bls_address).await.map_err(|e| {
        format!(
            "Failed to get code for BLS Signature Checker {}: {}",
            bls_address, e
        )
    })?;
    if code_bls.as_ref().is_empty() {
        return Err(format!(
            "BLS Signature Checker {} has no code deployed. Ensure addresses.blsSigCheck is correct or set BLS_SIGNATURE_CHECKER_ADDRESS.",
            bls_address
        )
        .into());
    }

    // Optionally use pre-deployed contract if ARRAY_SUMMATION_ADDRESS is provided
    let maybe_existing = env::var("ARRAY_SUMMATION_ADDRESS").ok();
    let deployed_address: Address;
    let used_existing: bool;
    if let Some(addr) = maybe_existing {
        let addr: Address = addr
            .parse()
            .map_err(|_| "Invalid ARRAY_SUMMATION_ADDRESS format")?;
        // Ensure code exists at provided address
        let code = provider.get_code_at(addr).await.map_err(|e| {
            format!(
                "Failed to read code at ARRAY_SUMMATION_ADDRESS {}: {}",
                addr, e
            )
        })?;
        if code.as_ref().is_empty() {
            return Err(format!(
                "ARRAY_SUMMATION_ADDRESS {} has no code deployed; remove the env var or deploy first",
                addr
            )
            .into());
        }
        println!("✅ Using existing ArraySummation at: {}", addr);
        deployed_address = addr;
        used_existing = true;
    } else {
        // Create factory instance
        let factory = ArraySummationFactory::new(factory_address, provider);

        // Get contract count before deployment
        println!("📊 Getting deployed contract count before deployment...");
        let contract_count_before = factory
            .getDeployedContractCount()
            .call()
            .await
            .map_err(|e| format!("Failed to get deployed contract count: {}", e))?;

        println!(
            "📊 Contract count before deployment: {}",
            contract_count_before
        );

        // Deploy ArraySummation using the factory
        println!("🚀 Sending deployment transaction...");

        let deploy_call = factory.deployArraySummation(
            avs_address,
            bls_address,
            U256::from(array_size),
            U256::from(max_value),
            U256::from(seed),
        );

        let pending_tx = deploy_call
            .send()
            .await
            .map_err(|e| format!("Failed to send deployment transaction: {}", e))?;

        let tx_hash = *pending_tx.tx_hash();
        println!("📋 Transaction sent: {}", tx_hash);

        // Wait for transaction to be mined
        println!("⏳ Waiting for transaction to be mined...");
        let receipt = pending_tx
            .get_receipt()
            .await
            .map_err(|e| format!("Transaction failed or was not mined: {}", e))?;

        if !receipt.status() {
            return Err("Transaction reverted".into());
        }
        println!("✅ Transaction confirmed!");

        // Get the deployed contract address
        println!("🔍 Retrieving deployed contract address...");
        let addr = factory
            .deployedContracts(contract_count_before)
            .call()
            .await
            .map_err(|e| format!("Failed to get deployed contract address: {}", e))?;

        if addr == Address::ZERO {
            return Err("Deployed contract address is zero - deployment may have failed".into());
        }

        println!("✅ ArraySummation deployed successfully!");
        println!("🏠 Deployed contract address: {}", addr);
        deployed_address = addr;
        used_existing = false;
    }

    // Update deployment JSON if it exists
    update_deployment_json(
        &avs_deployment_path,
        &array_summation_factory_address,
        &format!("{:?}", deployed_address),
    )?;

    println!("🎉 Deployment completed successfully!");
    println!("📋 Summary:");
    println!(
        "  ArraySummation Factory: {}",
        array_summation_factory_address
    );
    if used_existing {
        println!("  ArraySummation Contract (existing): {}", deployed_address);
    } else {
        println!("  ArraySummation Contract: {}", deployed_address);
    }
    println!("  AVS Service Manager Wrapper: {}", avs_address);
    println!("  BLS Sig Check: {}", bls_address);

    Ok(())
}

/// Loads every operator's secp256k1 key from the `*.private.ecdsa.key.json` files
/// the eigenlayer setup container writes next to the deployment JSON (override the
/// directory with `OPERATOR_KEYS_DIR`). Sorted by filename for a deterministic
/// registration order.
fn load_operator_keys(avs_deployment_path: &str) -> Result<Vec<PrivateKey>, DynError> {
    let keys_dir: PathBuf = match env::var("OPERATOR_KEYS_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(avs_deployment_path)
            .parent()
            .map(|p| p.join("operator_keys"))
            .unwrap_or_else(|| PathBuf::from("operator_keys")),
    };
    println!("🔑 Loading operator keys from: {}", keys_dir.display());

    let mut key_files: Vec<PathBuf> = fs::read_dir(&keys_dir)
        .map_err(|e| {
            format!(
                "Failed to read operator keys directory {}: {}",
                keys_dir.display(),
                e
            )
        })?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(OPERATOR_KEY_FILE_SUFFIX))
        })
        .collect();
    key_files.sort();

    if key_files.is_empty() {
        return Err(format!(
            "no *{} files found in {} — the eigenlayer setup container writes them next to \
             the deployment JSON (or set OPERATOR_KEYS_DIR)",
            OPERATOR_KEY_FILE_SUFFIX,
            keys_dir.display()
        )
        .into());
    }

    let mut keys = Vec::with_capacity(key_files.len());
    for file in &key_files {
        let content = fs::read_to_string(file)
            .map_err(|e| format!("Failed to read operator key file {}: {}", file.display(), e))?;
        let parsed: OperatorKeyFile = serde_json::from_str(&content).map_err(|e| {
            format!(
                "Failed to parse operator key file {}: {}",
                file.display(),
                e
            )
        })?;
        let key = private_key_from_hex(&parsed.private_key)
            .ok_or_else(|| format!("Invalid privateKey in operator key file {}", file.display()))?;
        keys.push(key);
    }
    Ok(keys)
}

/// Deploys the Schnorr `ArraySummation` target: deploys (or reuses) the
/// `SchnorrStakeRegistry`, registers every operator's Schnorr key with a proof of
/// possession, then deploys a fresh factory and the target wired to that registry.
async fn deploy_schnorr() -> Result<(), DynError> {
    println!("🚀 Deploying SchnorrArraySummation...");

    let http_rpc = env::var("HTTP_RPC").map_err(|_| "HTTP_RPC environment variable is required")?;
    let private_key =
        env::var("PRIVATE_KEY").map_err(|_| "PRIVATE_KEY environment variable is required")?;
    let array_size = env_u64("ARRAY_SUMMATION_ARRAY_SIZE")?;
    let max_value = env_u64("ARRAY_SUMMATION_MAX_VALUE")?;
    let seed = env_u64("ARRAY_SUMMATION_SEED")?;

    // The on-chain stake threshold, fixed at registry deployment. Must match the
    // router coordinator's local participation floor (same env vars, see
    // `quorum_threshold_fraction`).
    let (threshold_num, threshold_den) = quorum_threshold_fraction();

    // The AVS reference comes from the eigenlayer deployment JSON — operators
    // register with EigenLayer through the same service manager regardless of the
    // quorum-signature scheme the target contract verifies.
    let avs_deployment_path = env::var("AVS_DEPLOYMENT_PATH")
        .map_err(|_| "AVS_DEPLOYMENT_PATH environment variable is required")?;

    let avs_deployment = read_avs_deployment(&avs_deployment_path)?;

    let avs_address: Address = avs_deployment
        .addresses
        .avs_service_manager_wrapper
        .parse()
        .map_err(|_| "Invalid avsServiceManagerWrapper address format in deployment JSON")?;

    // The operators' Schnorr keys are their existing secp256k1 keys, read from the
    // key files the eigenlayer setup container produced.
    let operator_keys = load_operator_keys(&avs_deployment_path)?;
    println!("🔑 Loaded {} operator key(s)", operator_keys.len());

    // Setup provider and signer; the deployer owns the registry (stand-in for the
    // EigenLayer registration lifecycle).
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|_| "Invalid private key format")?;
    let deployer = signer.address();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(signer))
        .connect_http(http_rpc.parse().map_err(|_| "Invalid RPC URL")?);

    // Sanity check: ensure the AVS address has code deployed
    println!("🔍 Checking deployed code of contracts...");
    let code_avs = provider
        .get_code_at(avs_address)
        .await
        .map_err(|e| format!("Failed to get code for AVS address {}: {}", avs_address, e))?;
    if code_avs.as_ref().is_empty() {
        return Err(format!(
            "AVS service manager {} has no code deployed. Check AVS_DEPLOYMENT_PATH.",
            avs_address
        )
        .into());
    }

    // Resolve the Schnorr stake registry: reuse SCHNORR_STAKE_REGISTRY_ADDRESS when
    // it points at deployed code (its operator set is assumed already registered),
    // otherwise deploy a fresh registry and register every operator with it.
    let registry_address: Address;
    let register_operators: bool;
    match env_address("SCHNORR_STAKE_REGISTRY_ADDRESS")? {
        Some(addr) => {
            let code = provider.get_code_at(addr).await.map_err(|e| {
                format!(
                    "Failed to get code for SchnorrStakeRegistry {}: {}",
                    addr, e
                )
            })?;
            if code.as_ref().is_empty() {
                return Err(format!(
                    "SCHNORR_STAKE_REGISTRY_ADDRESS {} has no code deployed; unset it to deploy a fresh registry",
                    addr
                )
                .into());
            }
            println!(
                "🏦 Using existing SchnorrStakeRegistry at: {} (skipping operator registrations)",
                addr
            );
            registry_address = addr;
            register_operators = false;
        }
        None => {
            println!(
                "🏦 Deploying SchnorrStakeRegistry (threshold {}/{}, owner {})...",
                threshold_num, threshold_den, deployer
            );
            let registry = SchnorrStakeRegistry::deploy(
                provider.clone(),
                U256::from(threshold_num),
                U256::from(threshold_den),
                deployer,
            )
            .await
            .map_err(|e| format!("Failed to deploy SchnorrStakeRegistry: {}", e))?;
            registry_address = *registry.address();
            println!("✅ SchnorrStakeRegistry deployed at: {}", registry_address);
            register_operators = true;
        }
    }

    // Register every operator's Schnorr key with a fresh proof of possession. This
    // MUST complete before the target deploy: the registry's `effectiveBlock`
    // watermark advances on every registration, and verification fail-closes for
    // reference blocks behind it.
    if register_operators {
        let registry = SchnorrStakeRegistry::new(registry_address, provider.clone());
        let mut rng = rand::rng();
        let mut fill = |b: &mut [u8]| rng.fill_bytes(b);
        for key in &operator_keys {
            let pubkey = key.public_key();
            let operator = pubkey.eth_address();
            let pop = key.prove_possession(&mut fill);
            // Cheap local check before spending gas: the registry verifies the same PoP.
            if !pubkey.verify_possession(&pop) {
                return Err(format!(
                    "locally generated proof of possession failed to verify for operator {}",
                    operator
                )
                .into());
            }
            let pop_bytes = pop.0.to_bytes();
            let pending_tx = registry
                .registerOperator(
                    U256::from_be_bytes(pubkey.x_bytes()),
                    U256::from_be_bytes(pubkey.y_bytes()),
                    // Uniform weight: the e2e stack has no stake differentiation.
                    U256::from(1),
                    U256::from_be_slice(&pop_bytes[..32]),
                    Address::from_slice(&pop_bytes[32..]),
                )
                .send()
                .await
                .map_err(|e| format!("Failed to send registerOperator for {}: {}", operator, e))?;
            let receipt = pending_tx.get_receipt().await.map_err(|e| {
                format!(
                    "registerOperator transaction for {} failed or was not mined: {}",
                    operator, e
                )
            })?;
            if !receipt.status() {
                return Err(format!("registerOperator reverted for operator {}", operator).into());
            }
            println!("✅ Registered operator {} (weight 1)", operator);
        }
    }

    // Deploy the factory and the target, strictly after the registrations above so
    // the first verification's `refBlock = head - 1` is at/after `effectiveBlock`.
    println!("🏭 Deploying a fresh SchnorrArraySummationFactory...");
    let factory = SchnorrArraySummationFactory::deploy(provider.clone())
        .await
        .map_err(|e| format!("Failed to deploy SchnorrArraySummationFactory: {}", e))?;
    let factory_address = *factory.address();
    println!(
        "✅ SchnorrArraySummationFactory deployed at: {}",
        factory_address
    );

    // Get contract count before deployment
    println!("📊 Getting deployed contract count before deployment...");
    let contract_count_before = factory
        .getDeployedContractCount()
        .call()
        .await
        .map_err(|e| format!("Failed to get deployed contract count: {}", e))?;

    println!(
        "📊 Contract count before deployment: {}",
        contract_count_before
    );

    // Deploy SchnorrArraySummation using the factory, wired to the Schnorr registry
    println!("🚀 Sending deployment transaction...");

    let deploy_call = factory.deploySchnorrArraySummation(
        avs_address,
        registry_address,
        U256::from(array_size),
        U256::from(max_value),
        U256::from(seed),
    );

    let pending_tx = deploy_call
        .send()
        .await
        .map_err(|e| format!("Failed to send deployment transaction: {}", e))?;

    let tx_hash = *pending_tx.tx_hash();
    println!("📋 Transaction sent: {}", tx_hash);

    // Wait for transaction to be mined
    println!("⏳ Waiting for transaction to be mined...");
    let receipt = pending_tx
        .get_receipt()
        .await
        .map_err(|e| format!("Transaction failed or was not mined: {}", e))?;

    if !receipt.status() {
        return Err("Transaction reverted".into());
    }
    println!("✅ Transaction confirmed!");

    // Get the deployed contract address
    println!("🔍 Retrieving deployed contract address...");
    let deployed_address = factory
        .deployedContracts(contract_count_before)
        .call()
        .await
        .map_err(|e| format!("Failed to get deployed contract address: {}", e))?;

    if deployed_address == Address::ZERO {
        return Err("Deployed contract address is zero - deployment may have failed".into());
    }

    println!("✅ SchnorrArraySummation deployed successfully!");
    println!("🏠 Deployed contract address: {}", deployed_address);

    // Update deployment JSON if it exists
    update_schnorr_deployment_json(
        &avs_deployment_path,
        &format!("{:?}", registry_address),
        &format!("{:?}", factory_address),
        &format!("{:?}", deployed_address),
    )?;

    println!("🎉 Deployment completed successfully!");
    println!("📋 Summary:");
    println!("  SchnorrStakeRegistry: {}", registry_address);
    println!("  SchnorrArraySummation Factory: {}", factory_address);
    println!("  SchnorrArraySummation Contract: {}", deployed_address);
    println!("  AVS Service Manager: {}", avs_address);

    Ok(())
}

fn update_deployment_json(
    avs_deployment_path: &str,
    factory_address: &str,
    deployed_address: &str,
) -> Result<(), DynError> {
    // Try to read existing deployment file
    let deployment_content = match fs::read_to_string(avs_deployment_path) {
        Ok(content) => content,
        Err(_) => {
            println!("⚠️  Could not read deployment file for updating, skipping JSON update");
            return Ok(());
        }
    };

    let mut deployment: serde_json::Value = serde_json::from_str(&deployment_content)
        .map_err(|e| format!("Failed to parse deployment JSON for updating: {}", e))?;

    // Ensure addresses object exists
    if !deployment["addresses"].is_object() {
        deployment["addresses"] = serde_json::json!({});
    }

    // Add addresses
    deployment["addresses"]["arraySummationFactory"] = serde_json::json!(factory_address);
    deployment["addresses"]["arraySummation"] = serde_json::json!(deployed_address);

    // Write back to file
    let updated_json = serde_json::to_string_pretty(&deployment)
        .map_err(|e| format!("Failed to serialize updated JSON: {}", e))?;

    fs::write(avs_deployment_path, updated_json)
        .map_err(|e| format!("Failed to write updated deployment JSON: {}", e))?;

    println!("📝 Updated deployment JSON with new addresses");
    Ok(())
}

fn update_schnorr_deployment_json(
    avs_deployment_path: &str,
    registry_address: &str,
    factory_address: &str,
    deployed_address: &str,
) -> Result<(), DynError> {
    // Try to read existing deployment file
    let deployment_content = match fs::read_to_string(avs_deployment_path) {
        Ok(content) => content,
        Err(_) => {
            println!("⚠️  Could not read deployment file for updating, skipping JSON update");
            return Ok(());
        }
    };

    let mut deployment: serde_json::Value = serde_json::from_str(&deployment_content)
        .map_err(|e| format!("Failed to parse deployment JSON for updating: {}", e))?;

    // Ensure addresses object exists
    if !deployment["addresses"].is_object() {
        deployment["addresses"] = serde_json::json!({});
    }

    // Add addresses. `arraySummation` also points at the schnorr target: send_request,
    // verify_message_hash_parity, and the CI assertions all read that key, and the
    // digest they compute against it is scheme-agnostic.
    deployment["addresses"]["schnorrStakeRegistry"] = serde_json::json!(registry_address);
    deployment["addresses"]["schnorrArraySummationFactory"] = serde_json::json!(factory_address);
    deployment["addresses"]["schnorrArraySummation"] = serde_json::json!(deployed_address);
    deployment["addresses"]["arraySummation"] = serde_json::json!(deployed_address);

    // Write back to file
    let updated_json = serde_json::to_string_pretty(&deployment)
        .map_err(|e| format!("Failed to serialize updated JSON: {}", e))?;

    fs::write(avs_deployment_path, updated_json)
        .map_err(|e| format!("Failed to write updated deployment JSON: {}", e))?;

    println!("📝 Updated deployment JSON with new addresses");
    Ok(())
}

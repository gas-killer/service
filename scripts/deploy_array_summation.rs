use alloy::network::EthereumWallet;
use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use bindings::arraysummationfactory::ArraySummationFactory;
use serde::Deserialize;
use std::env;
use std::fs;

#[derive(Debug, Deserialize)]
struct AvsDeploymentJson {
    addresses: AvsAddresses,
}

#[derive(Debug, Deserialize)]
struct AvsAddresses {
    /// Legacy BLS-middleware service-manager wrapper; retained as a fallback AVS
    /// reference. Optional so an ECDSA-only deployment JSON (only
    /// `gasKillerServiceManager`) still parses.
    #[serde(rename = "avsServiceManagerWrapper")]
    avs_service_manager_wrapper: Option<String>,
    /// The Gas Killer ECDSA service manager (the AVS operators registered to via
    /// the AVSDirectory), deployed by the eigenlayer setup container.
    #[serde(rename = "gasKillerServiceManager")]
    gas_killer_service_manager: Option<String>,
    /// The EigenLayer ECDSAStakeRegistry verifying operator quorums, deployed by
    /// the eigenlayer setup container.
    #[serde(rename = "ecdsaStakeRegistry")]
    ecdsa_stake_registry: Option<String>,
}

type DynError = Box<dyn std::error::Error + Send + Sync>;

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

#[tokio::main]
async fn main() -> Result<(), DynError> {
    println!("🚀 Deploying ArraySummation...");

    dotenv::dotenv().ok();

    let http_rpc = env::var("HTTP_RPC").map_err(|_| "HTTP_RPC environment variable is required")?;
    let private_key =
        env::var("PRIVATE_KEY").map_err(|_| "PRIVATE_KEY environment variable is required")?;
    let array_size = env::var("ARRAY_SUMMATION_ARRAY_SIZE")
        .map_err(|_| "ARRAY_SUMMATION_ARRAY_SIZE environment variable is required")?
        .parse::<u64>()
        .map_err(|_| "ARRAY_SUMMATION_ARRAY_SIZE must be a valid number")?;
    let max_value = env::var("ARRAY_SUMMATION_MAX_VALUE")
        .map_err(|_| "ARRAY_SUMMATION_MAX_VALUE environment variable is required")?
        .parse::<u64>()
        .map_err(|_| "ARRAY_SUMMATION_MAX_VALUE must be a valid number")?;
    let seed = env::var("ARRAY_SUMMATION_SEED")
        .map_err(|_| "ARRAY_SUMMATION_SEED environment variable is required")?
        .parse::<u64>()
        .map_err(|_| "ARRAY_SUMMATION_SEED must be a valid number")?;

    // Get AVS addresses from the deployment JSON (produced by the eigenlayer
    // setup container, which also deploys the ECDSA stack and registers the
    // operators with it).
    let avs_deployment_path = env::var("AVS_DEPLOYMENT_PATH")
        .map_err(|_| "AVS_DEPLOYMENT_PATH environment variable is required")?;

    println!("📖 Reading AVS deployment from: {}", avs_deployment_path);
    let avs_content = fs::read_to_string(&avs_deployment_path)
        .map_err(|e| format!("Failed to read AVS deployment file: {}", e))?;

    let avs_deployment: AvsDeploymentJson = serde_json::from_str(&avs_content)
        .map_err(|e| format!("Failed to parse AVS deployment JSON: {}", e))?;

    // The target contract's AVS reference: prefer the Gas Killer ECDSA service
    // manager (the AVS the operators actually registered to), fall back to the
    // legacy wrapper.
    let avs_address: Address = match (
        &avs_deployment.addresses.gas_killer_service_manager,
        &avs_deployment.addresses.avs_service_manager_wrapper,
    ) {
        (Some(addr), _) => addr
            .parse()
            .map_err(|_| "Invalid gasKillerServiceManager address in deployment JSON")?,
        (None, Some(addr)) => addr
            .parse()
            .map_err(|_| "Invalid avsServiceManagerWrapper address format in deployment JSON")?,
        (None, None) => {
            return Err(
                "neither gasKillerServiceManager nor avsServiceManagerWrapper found in \
                 the deployment JSON — the eigenlayer setup container must deploy the ECDSA stack"
                    .into(),
            );
        }
    };

    // The ECDSA stake registry verifying operator quorums: env override first,
    // else the address the setup container wrote into the deployment JSON.
    let registry_address: Address = match env_address("ECDSA_STAKE_REGISTRY_ADDRESS")? {
        Some(addr) => addr,
        None => avs_deployment
            .addresses
            .ecdsa_stake_registry
            .as_deref()
            .ok_or(
                "ecdsaStakeRegistry missing from the deployment JSON — the eigenlayer setup \
                 container must deploy the ECDSA stack (or set ECDSA_STAKE_REGISTRY_ADDRESS)",
            )?
            .parse()
            .map_err(|_| "Invalid ecdsaStakeRegistry address in deployment JSON")?,
    };
    println!("🏦 Using ECDSAStakeRegistry: {}", registry_address);

    // Setup provider and signer
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|_| "Invalid private key format")?;
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(signer))
        .connect_http(http_rpc.parse().map_err(|_| "Invalid RPC URL")?);

    // Sanity checks: ensure target addresses have code deployed
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

    let code_registry = provider.get_code_at(registry_address).await.map_err(|e| {
        format!(
            "Failed to get code for ECDSAStakeRegistry {}: {}",
            registry_address, e
        )
    })?;
    if code_registry.as_ref().is_empty() {
        return Err(format!(
            "ECDSAStakeRegistry {} has no code deployed. Ensure the eigenlayer setup deployed \
             the ECDSA stack or set ECDSA_STAKE_REGISTRY_ADDRESS.",
            registry_address
        )
        .into());
    }

    // Resolve the factory: reuse ARRAY_SUMMATION_FACTORY_ADDRESS when it points
    // at deployed code, otherwise deploy a fresh ECDSA factory (the pre-migration
    // BLS factory on public networks is ABI-incompatible with this deployment).
    let factory_address: Address = match env_address("ARRAY_SUMMATION_FACTORY_ADDRESS")? {
        Some(addr) => {
            let code = provider
                .get_code_at(addr)
                .await
                .map_err(|e| format!("Failed to get code for factory {}: {}", addr, e))?;
            if code.as_ref().is_empty() {
                return Err(format!(
                    "ARRAY_SUMMATION_FACTORY_ADDRESS {} has no code deployed; unset it to deploy a fresh ECDSA factory",
                    addr
                )
                .into());
            }
            println!("🏭 Using existing ArraySummationFactory at: {}", addr);
            addr
        }
        None => {
            println!("🏭 Deploying a fresh ArraySummationFactory...");
            let factory = ArraySummationFactory::deploy(provider.clone())
                .await
                .map_err(|e| format!("Failed to deploy ArraySummationFactory: {}", e))?;
            let addr = *factory.address();
            println!("✅ ArraySummationFactory deployed at: {}", addr);
            addr
        }
    };

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
        let factory = ArraySummationFactory::new(factory_address, provider.clone());

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

        // Deploy ArraySummation using the factory, wired to the ECDSA stake registry
        println!("🚀 Sending deployment transaction...");

        let deploy_call = factory.deployArraySummation(
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
        &format!("{:?}", factory_address),
        &format!("{:?}", deployed_address),
    )?;

    println!("🎉 Deployment completed successfully!");
    println!("📋 Summary:");
    println!("  ArraySummation Factory: {}", factory_address);
    if used_existing {
        println!("  ArraySummation Contract (existing): {}", deployed_address);
    } else {
        println!("  ArraySummation Contract: {}", deployed_address);
    }
    println!("  AVS Service Manager: {}", avs_address);
    println!("  ECDSAStakeRegistry: {}", registry_address);

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

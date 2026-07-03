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
    #[serde(rename = "avsServiceManagerWrapper")]
    avs_service_manager_wrapper: String,
}

/// One operator key file (`testaccN.private.ecdsa.key.json`) as produced by the
/// eigenlayer setup container: `{"privateKey": "0x...", "publicKey": "<address>"}`.
#[derive(Debug, Deserialize)]
struct OperatorEcdsaKeyFile {
    #[serde(rename = "publicKey")]
    public_key: String,
}

/// Resolves the initial operator set for the ArraySummation deployment.
///
/// `OPERATOR_ADDRESSES` (comma-separated `0x...` addresses) takes precedence;
/// otherwise the operator addresses are read from the
/// `operator_keys/*.private.ecdsa.key.json` files next to the AVS deployment JSON
/// (the layout the eigenlayer setup container produces under `config/.nodes`).
fn resolve_operator_addresses(
    avs_deployment_path: &str,
) -> Result<Vec<Address>, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(raw) = env::var("OPERATOR_ADDRESSES") {
        let addresses = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<Address>()
                    .map_err(|_| format!("Invalid address in OPERATOR_ADDRESSES: {s}").into())
            })
            .collect::<Result<Vec<Address>, Box<dyn std::error::Error + Send + Sync>>>()?;
        if !addresses.is_empty() {
            return Ok(addresses);
        }
    }

    let deployment_dir = std::path::Path::new(avs_deployment_path)
        .parent()
        .ok_or("AVS_DEPLOYMENT_PATH has no parent directory")?;
    let keys_dir = deployment_dir.join("operator_keys");
    let mut addresses = Vec::new();
    let entries = fs::read_dir(&keys_dir).map_err(|e| {
        format!(
            "Failed to read {} (set OPERATOR_ADDRESSES to pass the operator set explicitly): {e}",
            keys_dir.display()
        )
    })?;
    let mut paths: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".private.ecdsa.key.json"))
        })
        .collect();
    paths.sort();
    for path in paths {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let key_file: OperatorEcdsaKeyFile = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;
        let address: Address = key_file
            .public_key
            .parse()
            .map_err(|_| format!("Invalid publicKey in {}", path.display()))?;
        addresses.push(address);
    }
    if addresses.is_empty() {
        return Err(format!(
            "No operator ECDSA key files found in {} and OPERATOR_ADDRESSES is unset",
            keys_dir.display()
        )
        .into());
    }
    Ok(addresses)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    // Get AVS address from deployment JSON
    let avs_deployment_path = env::var("AVS_DEPLOYMENT_PATH")
        .map_err(|_| "AVS_DEPLOYMENT_PATH environment variable is required")?;

    println!("📖 Reading AVS deployment from: {}", avs_deployment_path);
    let avs_content = fs::read_to_string(&avs_deployment_path)
        .map_err(|e| format!("Failed to read AVS deployment file: {}", e))?;

    let avs_deployment: AvsDeploymentJson = serde_json::from_str(&avs_content)
        .map_err(|e| format!("Failed to parse AVS deployment JSON: {}", e))?;

    let avs_address: Address = avs_deployment
        .addresses
        .avs_service_manager_wrapper
        .parse()
        .map_err(|_| "Invalid avsServiceManagerWrapper address format in deployment JSON")?;

    // Resolve the operator ECDSA signing quorum for the new contract.
    let operators = resolve_operator_addresses(&avs_deployment_path)?;
    println!("🔐 Initial operator set ({}):", operators.len());
    for operator in &operators {
        println!("   {}", operator);
    }

    // Setup provider and signer. The deployer doubles as the contract's
    // operator-registry admin.
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|_| "Invalid private key format")?;
    let operator_admin = signer.address();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(signer))
        .connect_http(http_rpc.parse().map_err(|_| "Invalid RPC URL")?);

    // Sanity check: ensure the AVS wrapper has code deployed
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

    // Resolve the factory: reuse ARRAY_SUMMATION_FACTORY_ADDRESS when it points
    // at deployed code, otherwise deploy a fresh ECDSA factory (the pre-migration
    // BLS factory on public networks is ABI-incompatible with this deployment).
    let factory_address: Address = match env::var("ARRAY_SUMMATION_FACTORY_ADDRESS")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(raw) => {
            let addr: Address = raw
                .parse()
                .map_err(|_| "Invalid ARRAY_SUMMATION_FACTORY_ADDRESS format")?;
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
            operator_admin,
            operators.clone(),
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
    println!("  AVS Service Manager Wrapper: {}", avs_address);
    println!("  Operator Admin: {}", operator_admin);
    println!("  Operators: {}", operators.len());

    Ok(())
}

fn update_deployment_json(
    avs_deployment_path: &str,
    factory_address: &str,
    deployed_address: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

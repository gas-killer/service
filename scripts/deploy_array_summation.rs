use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use bindings::arraysummationfactory::ArraySummationFactory;
use serde::Deserialize;
use std::env;
use std::fs;

// Define minimal interface for IncredibleSquaringTaskManager
sol! {
    #[sol(rpc)]
    interface IIncredibleSquaringTaskManager {
        function blsSignatureChecker() external view returns (address);
    }
}

#[derive(Debug, Deserialize)]
struct AvsDeploymentJson {
    addresses: AvsAddresses,
}

#[derive(Debug, Deserialize)]
struct AvsAddresses {
    #[serde(rename = "IncredibleSquaringTaskManager")]
    incredible_squaring_task_manager: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("🚀 Deploying ArraySummation...");

    dotenv::dotenv().ok();

    let http_rpc = env::var("HTTP_RPC").map_err(|_| "HTTP_RPC environment variable is required")?;
    let private_key =
        env::var("PRIVATE_KEY").map_err(|_| "PRIVATE_KEY environment variable is required")?;
    let array_summation_factory_address = env::var("ARRAY_SUMMATION_FACTORY_ADDRESS")
        .map_err(|_| "ARRAY_SUMMATION_FACTORY_ADDRESS environment variable is required")?;
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

    // Parse addresses
    let factory_address: Address = array_summation_factory_address
        .parse()
        .map_err(|_| "Invalid ARRAY_SUMMATION_FACTORY_ADDRESS format")?;

    // Get AVS address from deployment JSON
    let avs_deployment_path = env::var("AVS_DEPLOYMENT_PATH")
        .map_err(|_| "AVS_DEPLOYMENT_PATH environment variable is required")?;

    println!("📖 Reading AVS deployment from: {}", avs_deployment_path);
    let avs_content = fs::read_to_string(&avs_deployment_path)
        .map_err(|e| format!("Failed to read AVS deployment file: {}", e))?;

    let avs_deployment: AvsDeploymentJson = serde_json::from_str(&avs_content)
        .map_err(|e| format!("Failed to parse AVS deployment JSON: {}", e))?;

    let task_manager_address: Address = avs_deployment
        .addresses
        .incredible_squaring_task_manager
        .parse()
        .map_err(|_| "Invalid IncredibleSquaringTaskManager address format in deployment JSON")?;

    println!("📋 IncredibleSquaringTaskManager: {}", task_manager_address);

    // Setup provider and signer (needed to query the contract)
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|_| "Invalid private key format")?;
    let provider = ProviderBuilder::new()
        .wallet(signer)
        .on_http(http_rpc.parse().map_err(|_| "Invalid RPC URL")?);

    // Query BLS signature checker address directly from TaskManager contract
    println!("📖 Querying BLS Signature Checker from IncredibleSquaringTaskManager...");
    let task_manager = IIncredibleSquaringTaskManager::new(task_manager_address, provider.clone());
    let bls_address = task_manager
        .blsSignatureChecker()
        .call()
        .await
        .map_err(|e| {
            format!(
                "Failed to call blsSignatureChecker() on TaskManager {}: {}",
                task_manager_address, e
            )
        })?
        ._0;

    println!("🔐 Using BLS Signature Checker: {}", bls_address);

    // Sanity checks: ensure target addresses have code deployed
    println!("🔍 Checking deployed code of contracts...");
    let code_task_manager = provider
        .get_code_at(task_manager_address)
        .await
        .map_err(|e| format!("Failed to get code for TaskManager {}: {}", task_manager_address, e))?;
    if code_task_manager.as_ref().is_empty() {
        return Err(format!(
            "TaskManager {} has no code deployed. Check AVS_DEPLOYMENT_PATH.",
            task_manager_address
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
            "BLS Signature Checker {} has no code deployed.",
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
            .map_err(|e| format!("Failed to get deployed contract count: {}", e))?
            .count;

        println!(
            "📊 Contract count before deployment: {}",
            contract_count_before
        );

        // Deploy ArraySummation using the factory
        println!("🚀 Sending deployment transaction...");

        let deploy_call = factory.deployArraySummation(
            task_manager_address,
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
            .map_err(|e| format!("Failed to get deployed contract address: {}", e))?
            ._0;

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
    println!("  IncredibleSquaringTaskManager: {}", task_manager_address);
    println!("  BLS Signature Checker: {}", bls_address);

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

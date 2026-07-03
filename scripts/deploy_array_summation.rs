use alloy::network::EthereumWallet;
use alloy::primitives::{Address, B256, U256, keccak256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use bindings::arraysummationfactory::ArraySummationFactory;
use bindings::ecdsastakeregistry::{
    ECDSAStakeRegistry, IECDSAStakeRegistryTypes, ISignatureUtilsMixinTypes,
};
use bindings::gaskillerservicemanager::GasKillerServiceManager;
use serde::Deserialize;
use std::env;
use std::fs;

// Minimal AVSDirectory surface: the EIP-712 digest operators sign to register
// with an AVS.
alloy::sol! {
    #[sol(rpc)]
    interface IAVSDirectory {
        function calculateOperatorAVSRegistrationDigestHash(
            address operator,
            address avs,
            bytes32 salt,
            uint256 expiry
        ) external view returns (bytes32);
    }
}

/// EigenLayer AVSDirectory on Sepolia (the chain the local anvil stack forks).
/// Override with `AVS_DIRECTORY_ADDRESS` on other networks.
const DEFAULT_SEPOLIA_AVS_DIRECTORY: &str = "0xa789c91ECDdae96865913130B786140Ee17aF545";

/// Stake-threshold percentage the registry enforces (matches the 66% quorum the
/// BLS deployment used).
const QUORUM_THRESHOLD_PERCENT: u64 = 66;

#[derive(Debug, Deserialize)]
struct AvsDeploymentJson {
    addresses: AvsAddresses,
}

#[derive(Debug, Deserialize)]
struct AvsAddresses {
    #[serde(rename = "avsServiceManagerWrapper")]
    avs_service_manager_wrapper: String,
    /// The LST strategy the operators deposited into during setup; weights the
    /// ECDSA quorum.
    strategy: Option<String>,
}

/// One operator key file (`testaccN.private.ecdsa.key.json`) as produced by the
/// eigenlayer setup container: `{"privateKey": "0x...", "publicKey": "<address>"}`.
#[derive(Debug, Deserialize)]
struct OperatorEcdsaKeyFile {
    #[serde(rename = "privateKey")]
    private_key: String,
}

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// Loads the operator ECDSA signers from the `operator_keys/*.private.ecdsa.key.json`
/// files next to the AVS deployment JSON (the layout the eigenlayer setup
/// container produces under `config/.nodes`). The signers are needed to register
/// each operator with the `ECDSAStakeRegistry` (registration is `msg.sender`-bound
/// and requires the operator's AVSDirectory signature).
fn load_operator_signers(avs_deployment_path: &str) -> Result<Vec<PrivateKeySigner>, DynError> {
    let deployment_dir = std::path::Path::new(avs_deployment_path)
        .parent()
        .ok_or("AVS_DEPLOYMENT_PATH has no parent directory")?;
    let keys_dir = deployment_dir.join("operator_keys");
    let entries = fs::read_dir(&keys_dir)
        .map_err(|e| format!("Failed to read {}: {e}", keys_dir.display()))?;
    let mut paths: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".private.ecdsa.key.json"))
        })
        .collect();
    paths.sort();

    let mut signers = Vec::new();
    for path in paths {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let key_file: OperatorEcdsaKeyFile = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;
        let signer: PrivateKeySigner = key_file
            .private_key
            .parse()
            .map_err(|_| format!("Invalid privateKey in {}", path.display()))?;
        signers.push(signer);
    }
    if signers.is_empty() {
        return Err(format!(
            "No operator ECDSA key files found in {}",
            keys_dir.display()
        )
        .into());
    }
    Ok(signers)
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

#[tokio::main]
async fn main() -> Result<(), DynError> {
    println!("🚀 Deploying Gas Killer ECDSA AVS stack + ArraySummation...");

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

    let wrapper_address: Address = avs_deployment
        .addresses
        .avs_service_manager_wrapper
        .parse()
        .map_err(|_| "Invalid avsServiceManagerWrapper address format in deployment JSON")?;

    // EigenLayer core addresses.
    let delegation_manager = env_address("DELEGATION_MANAGER_ADDRESS")?
        .ok_or("DELEGATION_MANAGER_ADDRESS environment variable is required")?;
    let avs_directory = env_address("AVS_DIRECTORY_ADDRESS")?
        .unwrap_or_else(|| DEFAULT_SEPOLIA_AVS_DIRECTORY.parse().expect("valid const"));
    let allocation_manager = env_address("ALLOCATION_MANAGER_ADDRESS")?.unwrap_or(Address::ZERO);
    let rewards_coordinator = env_address("REWARDS_COORDINATOR_ADDRESS")?.unwrap_or(Address::ZERO);
    // Quorum strategy: the LST strategy operators deposited into during setup.
    let strategy: Address = match env_address("LST_STRATEGY_ADDRESS")? {
        Some(addr) => addr,
        None => avs_deployment
            .addresses
            .strategy
            .as_deref()
            .ok_or("Set LST_STRATEGY_ADDRESS or provide addresses.strategy in the deployment JSON")?
            .parse()
            .map_err(|_| "Invalid strategy address in deployment JSON")?,
    };

    // Setup deployer provider/signer. The deployer owns the stake registry and
    // service manager.
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|_| "Invalid private key format")?;
    let deployer = signer.address();
    let rpc_url: url::Url = http_rpc.parse().map_err(|_| "Invalid RPC URL")?;
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(signer))
        .connect_http(rpc_url.clone());

    // Deploy (or reuse) the ECDSA stake registry + service manager and register
    // the operators.
    let registry_address: Address = match env_address("ECDSA_STAKE_REGISTRY_ADDRESS")? {
        Some(addr) => {
            let code = provider
                .get_code_at(addr)
                .await
                .map_err(|e| format!("Failed to get code for stake registry {}: {}", addr, e))?;
            if code.as_ref().is_empty() {
                return Err(format!(
                    "ECDSA_STAKE_REGISTRY_ADDRESS {} has no code deployed; unset it to deploy fresh",
                    addr
                )
                .into());
            }
            println!("🏦 Using existing ECDSAStakeRegistry at: {}", addr);
            addr
        }
        None => {
            let operator_signers = load_operator_signers(&avs_deployment_path)?;
            println!("🔐 Operator set ({}):", operator_signers.len());
            for operator in &operator_signers {
                println!("   {}", operator.address());
            }

            // 1. Deploy the stake registry (immutably bound to the DelegationManager).
            println!("🏦 Deploying ECDSAStakeRegistry...");
            let registry = ECDSAStakeRegistry::deploy(provider.clone(), delegation_manager)
                .await
                .map_err(|e| format!("Failed to deploy ECDSAStakeRegistry: {}", e))?;
            println!("✅ ECDSAStakeRegistry deployed at: {}", registry.address());

            // 2. Deploy the service manager wired to the registry and EigenLayer core.
            println!("🏛  Deploying GasKillerServiceManager...");
            let service_manager = GasKillerServiceManager::deploy(
                provider.clone(),
                avs_directory,
                *registry.address(),
                rewards_coordinator,
                delegation_manager,
                allocation_manager,
                deployer,
            )
            .await
            .map_err(|e| format!("Failed to deploy GasKillerServiceManager: {}", e))?;
            println!(
                "✅ GasKillerServiceManager deployed at: {}",
                service_manager.address()
            );

            // 3. Initialize the registry: single-strategy quorum (10_000 BPS) and a
            // placeholder threshold (raised after registration when total weight is
            // known).
            let quorum = IECDSAStakeRegistryTypes::Quorum {
                strategies: vec![IECDSAStakeRegistryTypes::StrategyParams {
                    strategy,
                    multiplier: alloy::primitives::Uint::<96, 2>::from(10_000u64),
                }],
            };
            registry
                .initialize(*service_manager.address(), U256::ZERO, quorum)
                .send()
                .await
                .map_err(|e| format!("Failed to send registry initialize: {}", e))?
                .get_receipt()
                .await
                .map_err(|e| format!("registry initialize not mined: {}", e))?;
            println!("✅ ECDSAStakeRegistry initialized");

            // 4. Register every operator: sign the AVSDirectory registration digest
            // with the operator key and call registerOperatorWithSignature as the
            // operator (signing key == operator address — what the nodes sign with).
            let directory = IAVSDirectory::new(avs_directory, provider.clone());
            let latest_block = provider
                .get_block(alloy::eips::BlockId::latest())
                .await
                .map_err(|e| format!("Failed to fetch latest block: {}", e))?
                .ok_or("Latest block unavailable")?;
            let expiry = U256::from(latest_block.header.timestamp + 24 * 60 * 60);

            for operator in &operator_signers {
                let operator_address = operator.address();
                let salt = keccak256(
                    [
                        operator_address.as_slice(),
                        b"gas-killer-ecdsa-registration",
                    ]
                    .concat(),
                );
                let digest: B256 = directory
                    .calculateOperatorAVSRegistrationDigestHash(
                        operator_address,
                        *service_manager.address(),
                        salt,
                        expiry,
                    )
                    .call()
                    .await
                    .map_err(|e| format!("Failed to compute registration digest: {}", e))?;
                let signature = operator
                    .sign_hash_sync(&digest)
                    .map_err(|e| format!("Failed to sign registration digest: {}", e))?;

                let operator_provider = ProviderBuilder::new()
                    .wallet(EthereumWallet::from(operator.clone()))
                    .connect_http(rpc_url.clone());
                let operator_registry =
                    ECDSAStakeRegistry::new(*registry.address(), operator_provider);
                operator_registry
                    .registerOperatorWithSignature(
                        ISignatureUtilsMixinTypes::SignatureWithSaltAndExpiry {
                            signature: signature.as_bytes().into(),
                            salt,
                            expiry,
                        },
                        operator_address,
                    )
                    .send()
                    .await
                    .map_err(|e| format!("Failed to register operator {operator_address}: {e}"))?
                    .get_receipt()
                    .await
                    .map_err(|e| format!("registration of {operator_address} not mined: {e}"))?;
                println!("✅ Registered operator {}", operator_address);
            }

            // 5. Set the stake threshold to 66% of the registered total weight.
            let total_weight = registry
                .getLastCheckpointTotalWeight()
                .call()
                .await
                .map_err(|e| format!("Failed to read total weight: {}", e))?;
            if total_weight.is_zero() {
                return Err(
                    "Registered operators have zero total weight; check the quorum \
                     strategy and operator deposits"
                        .into(),
                );
            }
            let threshold = (total_weight * U256::from(QUORUM_THRESHOLD_PERCENT) + U256::from(99))
                / U256::from(100);
            registry
                .updateStakeThreshold(threshold)
                .send()
                .await
                .map_err(|e| format!("Failed to send updateStakeThreshold: {}", e))?
                .get_receipt()
                .await
                .map_err(|e| format!("updateStakeThreshold not mined: {}", e))?;
            println!(
                "✅ Stake threshold set to {} ({}% of total weight {})",
                threshold, QUORUM_THRESHOLD_PERCENT, total_weight
            );

            *registry.address()
        }
    };

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
            wrapper_address,
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
        &format!("{:?}", registry_address),
    )?;

    println!("🎉 Deployment completed successfully!");
    println!("📋 Summary:");
    println!("  ECDSAStakeRegistry: {}", registry_address);
    println!("  ArraySummation Factory: {}", factory_address);
    if used_existing {
        println!("  ArraySummation Contract (existing): {}", deployed_address);
    } else {
        println!("  ArraySummation Contract: {}", deployed_address);
    }
    println!("  AVS Service Manager Wrapper: {}", wrapper_address);

    Ok(())
}

fn update_deployment_json(
    avs_deployment_path: &str,
    factory_address: &str,
    deployed_address: &str,
    registry_address: &str,
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
    deployment["addresses"]["ecdsaStakeRegistry"] = serde_json::json!(registry_address);

    // Write back to file
    let updated_json = serde_json::to_string_pretty(&deployment)
        .map_err(|e| format!("Failed to serialize updated JSON: {}", e))?;

    fs::write(avs_deployment_path, updated_json)
        .map_err(|e| format!("Failed to write updated deployment JSON: {}", e))?;

    println!("📝 Updated deployment JSON with new addresses");
    Ok(())
}

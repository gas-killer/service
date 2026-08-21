use alloy::network::EthereumWallet;
use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use bindings::arraysummationfactory::ArraySummationFactory;
use bindings::reentrantcheckpointfactory::ReentrantCheckpointFactory;
use bindings::schnorrarraysummationfactory::SchnorrArraySummationFactory;
use bindings::schnorrstakeregistry::SchnorrStakeRegistry;
use gas_killer_common::schnorr::{PrivateKey, private_key_from_hex};
use gas_killer_common::{
    SignatureScheme, StakeSource, quorum_threshold_fraction, schnorr_notice_window,
    signature_scheme, stake_source,
};
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
    // EigenLayer-mode stack (written by the eigenlayer setup container).
    #[serde(rename = "avsServiceManagerWrapper")]
    avs_service_manager_wrapper: Option<String>,
    #[serde(rename = "IncredibleSquaringTaskManager")]
    bls_sig_check: Option<String>,
    // Commitments-mode stack (written by the e2e orchestration from the forge deploy
    // legs: the commitments repo's own deploy scripts + solidity-sdk's
    // `CommitmentsGasKiller.s.sol`).
    #[serde(rename = "commitmentManager")]
    commitment_manager: Option<String>,
    #[serde(rename = "operatorRegistry")]
    operator_registry: Option<String>,
    #[serde(rename = "backingAdapter")]
    backing_adapter: Option<String>,
    #[serde(rename = "stakeToken")]
    stake_token: Option<String>,
    #[serde(rename = "gasKillerArbiter")]
    gas_killer_arbiter: Option<String>,
    #[serde(rename = "schnorrCommitmentsAdapter")]
    schnorr_commitments_adapter: Option<String>,
    #[serde(rename = "schnorrStakeRegistry")]
    schnorr_stake_registry: Option<String>,
}

impl AvsAddresses {
    /// Parses a required address field, with a mode-appropriate error when absent.
    fn require(field: Option<&String>, name: &str) -> Result<Address, DynError> {
        field
            .ok_or_else(|| format!("{name} missing from deployment JSON"))?
            .parse()
            .map_err(|_| format!("Invalid {name} address format in deployment JSON").into())
    }
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

    let avs_address = AvsAddresses::require(
        avs_deployment.addresses.avs_service_manager_wrapper.as_ref(),
        "avsServiceManagerWrapper",
    )?;

    // Get BLS signature checker address from deployment JSON
    let bls_address = AvsAddresses::require(
        avs_deployment.addresses.bls_sig_check.as_ref(),
        "IncredibleSquaringTaskManager",
    )?;
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

/// An operator's secp256k1 key: the parsed Schnorr signing key plus the raw hex it
/// came from (Commitments-mode onboarding builds an Ethereum wallet from it — the
/// Schnorr key IS the operator's on-chain identity and transaction signer).
struct OperatorKey {
    schnorr: PrivateKey,
    raw_hex: String,
}

/// Loads every operator's secp256k1 key from the `*.private.ecdsa.key.json` files
/// the eigenlayer setup container writes next to the deployment JSON (override the
/// directory with `OPERATOR_KEYS_DIR`). Sorted by filename for a deterministic
/// registration order.
fn load_operator_keys(avs_deployment_path: &str) -> Result<Vec<OperatorKey>, DynError> {
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
        keys.push(OperatorKey {
            schnorr: key,
            raw_hex: parsed.private_key,
        });
    }
    Ok(keys)
}

/// Suffix of the BN254 key files the setup tooling writes next to the ECDSA keys.
/// The BN254 key is the node's p2p/engine identity; Commitments-mode onboarding
/// publishes its public coordinates through the adapter sidecar.
const OPERATOR_BLS_KEY_FILE_SUFFIX: &str = ".private.bls.key.json";

/// Loads every operator's BN254 private scalar (decimal string) from the
/// `*.private.bls.key.json` files, sorted by filename — the same order
/// `load_operator_keys` uses, so index i of both lists is the same operator.
fn load_operator_bls_scalars(avs_deployment_path: &str) -> Result<Vec<ark_bn254::Fr>, DynError> {
    use std::str::FromStr;

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

    let mut key_files: Vec<PathBuf> = fs::read_dir(&keys_dir)
        .map_err(|e| format!("Failed to read operator keys directory {}: {}", keys_dir.display(), e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(OPERATOR_BLS_KEY_FILE_SUFFIX))
        })
        .collect();
    key_files.sort();

    let mut scalars = Vec::with_capacity(key_files.len());
    for file in &key_files {
        let content = fs::read_to_string(file)
            .map_err(|e| format!("Failed to read BLS key file {}: {}", file.display(), e))?;
        let parsed: OperatorKeyFile = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse BLS key file {}: {}", file.display(), e))?;
        let scalar = ark_bn254::Fr::from_str(parsed.private_key.trim())
            .map_err(|_| format!("Invalid decimal BN254 privateKey in {}", file.display()))?;
        scalars.push(scalar);
    }
    Ok(scalars)
}

/// Converts a BN254 base-field element into a U256 (big-endian).
fn fq_to_u256(fq: &ark_bn254::Fq) -> U256 {
    use ark_ff::{BigInteger, PrimeField};
    U256::from_be_slice(&fq.into_bigint().to_bytes_be())
}

/// Derives the affine BN254 G1/G2 public coordinates for a private scalar, in the
/// adapter's publishing convention: G1 = [x, y], G2 = [x_c0, x_c1, y_c0, y_c1] —
/// the (c0, c1) order `CommonwarePublicKeys::from_string_coordinates` consumes.
fn bls_public_coordinates(scalar: &ark_bn254::Fr) -> ([U256; 2], [U256; 4]) {
    use ark_ec::{CurveGroup, PrimeGroup};

    let g1 = (ark_bn254::G1Projective::generator() * scalar).into_affine();
    let g2 = (ark_bn254::G2Projective::generator() * scalar).into_affine();
    (
        [fq_to_u256(&g1.x), fq_to_u256(&g1.y)],
        [
            fq_to_u256(&g2.x.c0),
            fq_to_u256(&g2.x.c1),
            fq_to_u256(&g2.y.c0),
            fq_to_u256(&g2.y.c1),
        ],
    )
}

#[cfg(test)]
mod bls_coordinate_tests {
    use super::*;
    use ark_serialize::CanonicalSerialize;
    use std::str::FromStr;

    // The published-coordinate convention must round-trip into exactly the BN254 G2
    // public key the node derives from its own private scalar at startup
    // (`Bn254Scheme::signer` compares compressed bytes against the participant set).
    // A swapped Fq2 (c0, c1) order on either side would make every node fail its
    // own-membership assert against a Commitments-sourced operator set.
    #[test]
    fn published_coordinates_reconstruct_the_node_side_key() {
        use ark_ec::{CurveGroup, PrimeGroup};

        let sk = ark_bn254::Fr::from_str("123456789123456789123456789").unwrap();
        let (g1, g2) = bls_public_coordinates(&sk);

        let keys = commonware_avs_eigenlayer::CommonwarePublicKeys::from_string_coordinates(
            &g2[0].to_string(),
            &g2[1].to_string(),
            &g2[2].to_string(),
            &g2[3].to_string(),
            &g1[0].to_string(),
            &g1[1].to_string(),
        )
        .expect("coordinates decode");

        let expected = (ark_bn254::G2Projective::generator() * sk).into_affine();
        let mut expected_bytes = Vec::new();
        expected.serialize_compressed(&mut expected_bytes).unwrap();
        assert_eq!(
            keys.g2_pub_key.as_ref(),
            expected_bytes.as_slice(),
            "adapter-published G2 coordinates must reconstruct generator*sk"
        );
    }
}

/// Onboards every operator through the Commitments stack, mirroring each into the
/// Schnorr registry via the adapter:
///
/// 1. fund the operator address with gas ETH and mint it stake tokens (dev token),
/// 2. as the operator: approve + `manager.deposit`,
/// 3. `manager.createCommitment` naming the arbiter/registry per the acceptance
///    policy (amount `OPERATOR_STAKE_AMOUNT`, full `maxPenaltyBps`),
/// 4. `operatorRegistry.register(commitmentId)`,
/// 5. `adapter.join(schnorrKey, PoP, blsG1, blsG2, socket)` — the adapter registers
///    the key in the SchnorrStakeRegistry with weight = stake / weightScale.
///
/// Sockets come from `OPERATOR_SOCKETS` (comma-separated, index-aligned with the
/// sorted key files).
async fn onboard_operators_commitments<P>(
    deployer_provider: &P,
    avs_deployment: &AvsDeploymentJson,
    operator_keys: &[OperatorKey],
    avs_deployment_path: &str,
) -> Result<(), DynError>
where
    P: Provider + Clone,
{
    use alloy::network::TransactionBuilder;
    use alloy::rpc::types::TransactionRequest;
    use bindings::commitmentmanager::CommitmentManager;
    use bindings::mintableerc20::MintableERC20;
    use bindings::operatorregistry::OperatorRegistry;
    use bindings::schnorrcommitmentsadapter::SchnorrCommitmentsAdapter;

    let http_rpc = env::var("HTTP_RPC").map_err(|_| "HTTP_RPC environment variable is required")?;
    let addresses = &avs_deployment.addresses;
    let manager_address = AvsAddresses::require(addresses.commitment_manager.as_ref(), "commitmentManager")?;
    let registry_address = AvsAddresses::require(addresses.operator_registry.as_ref(), "operatorRegistry")?;
    let backing_adapter = AvsAddresses::require(addresses.backing_adapter.as_ref(), "backingAdapter")?;
    let stake_token = AvsAddresses::require(addresses.stake_token.as_ref(), "stakeToken")?;
    let arbiter = AvsAddresses::require(addresses.gas_killer_arbiter.as_ref(), "gasKillerArbiter")?;
    let schnorr_adapter =
        AvsAddresses::require(addresses.schnorr_commitments_adapter.as_ref(), "schnorrCommitmentsAdapter")?;

    let stake_amount: U256 = env::var("OPERATOR_STAKE_AMOUNT")
        .unwrap_or_else(|_| "100".to_string())
        .trim()
        .parse()
        .map_err(|_| "OPERATOR_STAKE_AMOUNT must be a decimal integer")?;

    let sockets_raw = env::var("OPERATOR_SOCKETS")
        .unwrap_or_else(|_| "node-1:3001,node-2:3002,node-3:3003".to_string());
    let sockets: Vec<&str> = sockets_raw.split(',').map(str::trim).collect();
    if sockets.len() < operator_keys.len() {
        return Err(format!(
            "OPERATOR_SOCKETS has {} entries but {} operators were loaded",
            sockets.len(),
            operator_keys.len()
        )
        .into());
    }

    let bls_scalars = load_operator_bls_scalars(avs_deployment_path)?;
    if bls_scalars.len() != operator_keys.len() {
        return Err(format!(
            "found {} *{} files but {} *{} files — every operator needs both keys",
            bls_scalars.len(),
            OPERATOR_BLS_KEY_FILE_SUFFIX,
            operator_keys.len(),
            OPERATOR_KEY_FILE_SUFFIX
        )
        .into());
    }

    // Gas budget per operator for its onboarding transactions (dev chains).
    let gas_ether = U256::from(10u128.pow(18));

    let token = MintableERC20::new(stake_token, deployer_provider.clone());
    let mut rng = rand::rng();
    let mut fill = |b: &mut [u8]| rng.fill_bytes(b);

    for (i, entry) in operator_keys.iter().enumerate() {
        let key = &entry.schnorr;
        let pubkey = key.public_key();
        let operator = pubkey.eth_address();
        println!("👷 Onboarding operator {} ({})", i + 1, operator);

        // 1. Gas + stake-token funding from the deployer.
        let fund_tx = TransactionRequest::default()
            .with_to(operator)
            .with_value(gas_ether);
        deployer_provider
            .send_transaction(fund_tx)
            .await
            .map_err(|e| format!("Failed to fund operator {}: {}", operator, e))?
            .get_receipt()
            .await
            .map_err(|e| format!("Funding transaction for {} not mined: {}", operator, e))?;
        token
            .mint(operator, stake_amount)
            .send()
            .await
            .map_err(|e| format!("Failed to mint stake for {}: {}", operator, e))?
            .get_receipt()
            .await
            .map_err(|e| format!("Mint transaction for {} not mined: {}", operator, e))?;

        // Operator wallet: the Schnorr key doubles as the transaction signer.
        let signer: PrivateKeySigner = entry
            .raw_hex
            .trim()
            .trim_start_matches("0x")
            .parse()
            .map_err(|_| format!("Operator key {} is not a valid Ethereum key", operator))?;
        let op_provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(signer))
            .connect_http(http_rpc.parse().map_err(|_| "Invalid RPC URL")?);

        let op_token = MintableERC20::new(stake_token, op_provider.clone());
        let manager = CommitmentManager::new(manager_address, op_provider.clone());
        let registry = OperatorRegistry::new(registry_address, op_provider.clone());
        let adapter = SchnorrCommitmentsAdapter::new(schnorr_adapter, op_provider.clone());

        // 2. Approve + deposit into the manager's free balance.
        op_token
            .approve(manager_address, stake_amount)
            .send()
            .await
            .map_err(|e| format!("approve failed for {}: {}", operator, e))?
            .get_receipt()
            .await
            .map_err(|e| format!("approve for {} not mined: {}", operator, e))?;
        manager
            .deposit(stake_token, stake_amount)
            .send()
            .await
            .map_err(|e| format!("deposit failed for {}: {}", operator, e))?
            .get_receipt()
            .await
            .map_err(|e| format!("deposit for {} not mined: {}", operator, e))?;

        // 3. Create the self-stake commitment per the registry's acceptance policy.
        let params = CommitmentManager::CommitmentParams {
            arbiter,
            counterparty: registry_address,
            token: stake_token,
            adapter: backing_adapter,
            amount: stake_amount,
            maxPenaltyBps: 10_000,
            challengeWindow: 86_400,
            expiresAt: 0,
            strategies: vec![],
            metadataURI: String::new(),
            metadataHash: [0u8; 32].into(),
        };
        let receipt = manager
            .createCommitment(params)
            .send()
            .await
            .map_err(|e| format!("createCommitment failed for {}: {}", operator, e))?
            .get_receipt()
            .await
            .map_err(|e| format!("createCommitment for {} not mined: {}", operator, e))?;
        if !receipt.status() {
            return Err(format!("createCommitment reverted for {}", operator).into());
        }
        let commitment_id = receipt
            .logs()
            .iter()
            .find_map(|log| {
                log.log_decode::<CommitmentManager::CommitmentCreated>()
                    .ok()
                    .map(|ev| ev.inner.commitmentId)
            })
            .ok_or_else(|| format!("no CommitmentCreated event for {}", operator))?;

        // 4. Register as a Commitments operator over that commitment.
        let receipt = registry
            .register(commitment_id)
            .send()
            .await
            .map_err(|e| format!("register failed for {}: {}", operator, e))?
            .get_receipt()
            .await
            .map_err(|e| format!("register for {} not mined: {}", operator, e))?;
        if !receipt.status() {
            return Err(format!("OperatorRegistry.register reverted for {}", operator).into());
        }

        // 5. Join the Schnorr signer set through the adapter (PoP checked locally
        //    first, exactly as the direct-registration path does).
        let pop = key.prove_possession(&mut fill);
        if !pubkey.verify_possession(&pop) {
            return Err(format!(
                "locally generated proof of possession failed to verify for operator {}",
                operator
            )
            .into());
        }
        let pop_bytes = pop.0.to_bytes();
        let (bls_g1, bls_g2) = bls_public_coordinates(&bls_scalars[i]);
        let receipt = adapter
            .join(
                U256::from_be_bytes(pubkey.x_bytes()),
                U256::from_be_bytes(pubkey.y_bytes()),
                U256::from_be_slice(&pop_bytes[..32]),
                Address::from_slice(&pop_bytes[32..]),
                bls_g1,
                bls_g2,
                sockets[i].to_string(),
            )
            .send()
            .await
            .map_err(|e| format!("adapter.join failed for {}: {}", operator, e))?
            .get_receipt()
            .await
            .map_err(|e| format!("adapter.join for {} not mined: {}", operator, e))?;
        if !receipt.status() {
            return Err(format!("adapter.join reverted for {}", operator).into());
        }
        println!("✅ Operator {} staked, registered, and joined the signer set", operator);
    }

    Ok(())
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

    // Blocks an operator-set change must be announced ahead of applying, fixed at registry
    // deployment. Zero for the e2e stack: the whole operator set is registered before the target
    // deploys, so no round is ever in flight for a mutation to invalidate.
    let notice_window = schnorr_notice_window();

    // The AVS reference comes from the deployment JSON. Its source depends on the
    // stake root: the EigenLayer service manager wrapper, or — in Commitments mode,
    // which has no EigenLayer contracts at all — the Commitments OperatorRegistry
    // proxy. Either way it only feeds the target's cosmetic `avsAddress`/`namespace`,
    // never the task digest.
    let avs_deployment_path = env::var("AVS_DEPLOYMENT_PATH")
        .map_err(|_| "AVS_DEPLOYMENT_PATH environment variable is required")?;

    let avs_deployment = read_avs_deployment(&avs_deployment_path)?;

    let source = stake_source();
    let avs_address: Address = match source {
        StakeSource::Eigenlayer => AvsAddresses::require(
            avs_deployment.addresses.avs_service_manager_wrapper.as_ref(),
            "avsServiceManagerWrapper",
        )?,
        StakeSource::Commitments => AvsAddresses::require(
            avs_deployment.addresses.operator_registry.as_ref(),
            "operatorRegistry",
        )?,
    };

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

    // Commitments mode: the registry was deployed by the forge legs with the
    // SchnorrCommitmentsAdapter as its immutable owner, so this binary never deploys
    // it and never calls `registerOperator` directly. Operators are onboarded through
    // the Commitments stack instead (stake commitment -> register -> adapter.join),
    // which mirrors them into the Schnorr registry. The ordering invariant is the
    // same as the direct path: every join must land before the target deploys, because
    // each one advances the registry's fail-closed `effectiveBlock` watermark.
    if source == StakeSource::Commitments {
        let registry_address = AvsAddresses::require(
            avs_deployment.addresses.schnorr_stake_registry.as_ref(),
            "schnorrStakeRegistry",
        )?;
        let code = provider.get_code_at(registry_address).await.map_err(|e| {
            format!(
                "Failed to get code for SchnorrStakeRegistry {}: {}",
                registry_address, e
            )
        })?;
        if code.as_ref().is_empty() {
            return Err(format!(
                "schnorrStakeRegistry {} from the deployment JSON has no code deployed — run the \
                 Commitments forge deploy legs first",
                registry_address
            )
            .into());
        }
        println!(
            "🏦 Using Commitments-owned SchnorrStakeRegistry at: {}",
            registry_address
        );

        onboard_operators_commitments(
            &provider,
            &avs_deployment,
            &operator_keys,
            &avs_deployment_path,
        )
        .await?;

        if e2e_example_is_reentrant() {
            return deploy_reentrant_checkpoint(
                provider.clone(),
                avs_address,
                registry_address,
                &avs_deployment_path,
            )
            .await;
        }
        return deploy_schnorr_target(
            provider,
            avs_address,
            registry_address,
            array_size,
            max_value,
            seed,
            &avs_deployment_path,
        )
        .await;
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
                "🏦 Deploying SchnorrStakeRegistry (threshold {}/{}, owner {}, notice window {} blocks)...",
                threshold_num, threshold_den, deployer, notice_window
            );
            let registry = SchnorrStakeRegistry::deploy(
                provider.clone(),
                U256::from(threshold_num),
                U256::from(threshold_den),
                deployer,
                U256::from(notice_window),
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
        for entry in &operator_keys {
            let key = &entry.schnorr;
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

    // Example selector: the re-entrancy demonstration deploys a different target (a
    // ReentrantCheckpoint + its Observer) but reuses the same registry/operators/JSON
    // wiring. Gated so the default array-summation e2e is byte-identical.
    if e2e_example_is_reentrant() {
        return deploy_reentrant_checkpoint(
            provider.clone(),
            avs_address,
            registry_address,
            &avs_deployment_path,
        )
        .await;
    }

    deploy_schnorr_target(
        provider,
        avs_address,
        registry_address,
        array_size,
        max_value,
        seed,
        &avs_deployment_path,
    )
    .await
}

/// Deploys the Schnorr factory + `SchnorrArraySummation` target wired to
/// `registry_address`, and records the addresses in the deployment JSON. Shared by
/// both stake roots; must run strictly after every operator registration/join so the
/// first verification's `refBlock = head - 1` is at/after the registry's
/// `effectiveBlock`.
#[allow(clippy::too_many_arguments)]
async fn deploy_schnorr_target<P>(
    provider: P,
    avs_address: Address,
    registry_address: Address,
    array_size: u64,
    max_value: u64,
    seed: u64,
    avs_deployment_path: &str,
) -> Result<(), DynError>
where
    P: Provider + Clone,
{
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
        avs_deployment_path,
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

/// True when `E2E_EXAMPLE=reentrant` (case-insensitive) selects the re-entrancy
/// demonstration target instead of the default array-summation one.
fn e2e_example_is_reentrant() -> bool {
    matches!(
        env::var("E2E_EXAMPLE").map(|v| v.trim().to_ascii_lowercase()),
        Ok(ref v) if v == "reentrant" || v == "reentrant-checkpoint"
    )
}

/// Deploy the re-entrancy demonstration target: a `ReentrantCheckpointFactory` that
/// produces a wired `{ReentrantObserver, ReentrantCheckpoint}` pair. The checkpoint's
/// `advance()` task re-enters itself through the observer mid-transition, so settling it
/// through the aggregate-Schnorr path proves re-entrancy is safe under the **canonical**
/// state encoding (`STATE_ENCODING=canonical`).
///
/// Writes the checkpoint address under the scheme-agnostic `arraySummation` alias (so
/// `send_request` / `verify_message_hash_parity` target it unchanged) plus explicit
/// `reentrantCheckpoint` / `reentrantObserver` keys.
async fn deploy_reentrant_checkpoint<P>(
    provider: P,
    avs_address: Address,
    registry_address: Address,
    avs_deployment_path: &str,
) -> Result<(), DynError>
where
    P: Provider + Clone,
{
    println!("🏭 Deploying a fresh ReentrantCheckpointFactory (re-entrancy e2e)...");
    let factory = ReentrantCheckpointFactory::deploy(provider.clone())
        .await
        .map_err(|e| format!("Failed to deploy ReentrantCheckpointFactory: {}", e))?;
    let factory_address = *factory.address();
    println!(
        "✅ ReentrantCheckpointFactory deployed at: {}",
        factory_address
    );

    let count_before = factory
        .getDeployedContractCount()
        .call()
        .await
        .map_err(|e| format!("Failed to get deployed contract count: {}", e))?;

    println!("🚀 Deploying the {{observer, checkpoint}} pair...");
    let pending_tx = factory
        .deployReentrantCheckpoint(avs_address, registry_address)
        .send()
        .await
        .map_err(|e| format!("Failed to send deployReentrantCheckpoint: {}", e))?;
    let receipt = pending_tx
        .get_receipt()
        .await
        .map_err(|e| format!("deployReentrantCheckpoint failed or was not mined: {}", e))?;
    if !receipt.status() {
        return Err("deployReentrantCheckpoint reverted".into());
    }

    let checkpoint_address = factory
        .deployedCheckpoints(count_before)
        .call()
        .await
        .map_err(|e| format!("Failed to read deployed checkpoint address: {}", e))?;
    if checkpoint_address == Address::ZERO {
        return Err("Deployed checkpoint address is zero".into());
    }
    let observer_address = factory
        .observerOf(checkpoint_address)
        .call()
        .await
        .map_err(|e| format!("Failed to read observer address: {}", e))?;

    println!("✅ ReentrantCheckpoint deployed at: {}", checkpoint_address);
    println!("✅ ReentrantObserver deployed at:   {}", observer_address);

    update_reentrant_deployment_json(
        avs_deployment_path,
        &format!("{:?}", registry_address),
        &format!("{:?}", factory_address),
        &format!("{:?}", checkpoint_address),
        &format!("{:?}", observer_address),
    )?;

    println!("🎉 Re-entrancy target deployment completed successfully!");
    Ok(())
}

fn update_reentrant_deployment_json(
    avs_deployment_path: &str,
    registry_address: &str,
    factory_address: &str,
    checkpoint_address: &str,
    observer_address: &str,
) -> Result<(), DynError> {
    let deployment_content = match fs::read_to_string(avs_deployment_path) {
        Ok(content) => content,
        Err(_) => {
            println!("⚠️  Could not read deployment file for updating, skipping JSON update");
            return Ok(());
        }
    };

    let mut deployment: serde_json::Value = serde_json::from_str(&deployment_content)
        .map_err(|e| format!("Failed to parse deployment JSON for updating: {}", e))?;

    if !deployment["addresses"].is_object() {
        deployment["addresses"] = serde_json::json!({});
    }

    deployment["addresses"]["schnorrStakeRegistry"] = serde_json::json!(registry_address);
    deployment["addresses"]["reentrantCheckpointFactory"] = serde_json::json!(factory_address);
    deployment["addresses"]["reentrantCheckpoint"] = serde_json::json!(checkpoint_address);
    deployment["addresses"]["reentrantObserver"] = serde_json::json!(observer_address);
    // Scheme-agnostic alias the trigger/parity readers use as the task target.
    deployment["addresses"]["arraySummation"] = serde_json::json!(checkpoint_address);

    let updated_json = serde_json::to_string_pretty(&deployment)
        .map_err(|e| format!("Failed to serialize updated JSON: {}", e))?;
    fs::write(avs_deployment_path, updated_json)
        .map_err(|e| format!("Failed to write updated deployment JSON: {}", e))?;

    println!("📝 Updated deployment JSON with re-entrancy target addresses");
    Ok(())
}

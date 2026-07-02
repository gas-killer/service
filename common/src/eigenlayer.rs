//! EigenLayer operator-set discovery, vendored from `commonware-restaking`.
//!
//! [`EigenStakingClient`] reads the quorum-0 operator set (addresses, stakes, BN254
//! public keys, sockets) from the deployed EigenLayer middleware contracts. The
//! router and nodes use this at startup to build the (static) participant set for
//! the aggregation engine, so every process MUST observe the same operator list —
//! participant indices are positions in the sorted G2-key order.
//!
//! Behavior is unchanged from the old `commonware_avs_eigenlayer` crate:
//! `QUORUM_THRESHOLD()`/`THRESHOLD_DENOMINATOR()` reads, quorum-0 operator state at
//! the current block, and socket strings.

use alloy_primitives::{Address, U256};
use alloy_provider::Provider;
use eigen_client_avsregistry::reader::AvsRegistryChainReader;
use eigen_common::get_provider;
use eigen_crypto_bls::{BlsG1Point, BlsG2Point};
use eigen_services_operatorsinfo::operator_info::OperatorInfoService;
use eigen_services_operatorsinfo::operatorsinfo_inmemory::OperatorInfoServiceInMemory;
use eigen_utils::rewardsv2::middleware::operator_state_retriever::OperatorStateRetriever;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::{env, fs};

use crate::bn254::{G1PublicKey, PublicKey};

// Minimal interface for the threshold reads on the AVS service manager (wrapper).
// Vendored in place of the old `AvsServiceManagerWrapper` binding: only these two
// view functions are called, so a full ABI binding is unnecessary.
alloy::sol! {
    #[sol(rpc)]
    interface IAvsThresholds {
        function QUORUM_THRESHOLD() external view returns (uint256);
        function THRESHOLD_DENOMINATOR() external view returns (uint256);
    }
}

/// An operator's BN254 public keys in the vendored `crate::bn254` representation.
///
/// `g2_pub_key` is the registered identity/signing key; `g1_pub_key` is the paired
/// G1 point needed for on-chain `NonSignerStakesAndSignature` assembly.
#[derive(Clone)]
pub struct CommonwarePublicKeys {
    pub g1_pub_key: G1PublicKey,
    pub g2_pub_key: PublicKey,
}

impl CommonwarePublicKeys {
    /// Builds the key pair from decimal-string coordinates (operator config files).
    ///
    /// Returns `None` if any coordinate fails to parse.
    pub fn from_string_coordinates(
        g2x1: &str,
        g2x2: &str,
        g2y1: &str,
        g2y2: &str,
        g1x: &str,
        g1y: &str,
    ) -> Option<Self> {
        let g2_pub_key = PublicKey::create_from_g2_coordinates(g2x1, g2x2, g2y1, g2y2)?;
        let g1_pub_key = G1PublicKey::create_from_g1_coordinates(g1x, g1y)?;
        Some(Self {
            g1_pub_key,
            g2_pub_key,
        })
    }

    /// Converts from the `eigen-crypto-bls` point types returned by the operator
    /// info service.
    pub fn from_bls_keys(g1_pub_key: BlsG1Point, g2_pub_key: BlsG2Point) -> Self {
        let g1_pub_key = G1PublicKey::from(g1_pub_key.g1());
        let g2_pub_key = PublicKey::from(g2_pub_key.g2());
        Self {
            g1_pub_key,
            g2_pub_key,
        }
    }
}

impl std::fmt::Debug for CommonwarePublicKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommonwarePublicKeys")
            .field("g1_pub_key", &format!("{:?}", self.g1_pub_key))
            .field("g2_pub_key", &format!("{:?}", self.g2_pub_key))
            .finish()
    }
}

/// One operator's registration state in a quorum.
#[derive(Debug, Clone)]
pub struct OperatorInfo {
    pub address: Address,
    pub stake: U256,
    pub pub_keys: Option<CommonwarePublicKeys>,
    pub socket: Option<String>,
    pub quorum_number: u8,
}

/// The full operator state of one quorum at the queried block.
#[derive(Debug)]
pub struct QuorumInfo {
    pub quorum_number: u8,
    pub operator_count: usize,
    /// Contract-derived signer count: `ceil(operator_count * QUORUM_THRESHOLD /
    /// THRESHOLD_DENOMINATOR)`.
    ///
    /// Informational only (startup logs): the aggregation engine's quorum is fixed
    /// at N3f1 (`n - (n-1)/3`), and the authoritative stake check runs on-chain in
    /// `BLSSignatureChecker` during `verifyAndUpdate`.
    pub threshold: usize,
    pub total_stake: U256,
    pub operators: Vec<OperatorInfo>,
}

/// Client for reading the AVS operator set from EigenLayer middleware contracts.
pub struct EigenStakingClient {
    http_endpoint: String,
    registry_coordinator_address: Address,
    registry_coordinator_deploy_block: u64,
    operator_info_service: Arc<OperatorInfoServiceInMemory>,
    operator_state_retriever_address: Address,
    service_manager_address: Address,
}

/// Contract addresses parsed from the AVS deployment JSON (internal to
/// [`EigenStakingClient::new`]).
#[derive(Debug)]
pub struct AvsDeploymentConfig {
    pub registry_coordinator_address: Address,
    pub deploy_block: u64,
    pub operator_state_retriever_address: Address,
    pub service_manager_address: Address,
    pub service_manager_is_wrapper: bool,
}

impl EigenStakingClient {
    fn read_avs_deployment_config(
        path: &str,
    ) -> Result<AvsDeploymentConfig, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        let json: Value = serde_json::from_str(&contents)?;

        let addresses = json["addresses"]
            .as_object()
            .ok_or("Missing addresses in deployment config")?;

        let registry_coordinator = addresses["registryCoordinator"]
            .as_str()
            .ok_or("Missing registryCoordinator address")?;

        // Read operator state retriever address from blsSigCheck field
        // This is the BLSSigCheckOperatorStateRetriever which implements
        // both signature checking and operator state retrieval
        let operator_state_retriever = addresses["blsSigCheck"]
            .as_str()
            .ok_or("Missing blsSigCheck address")?;

        let last_update = json["lastUpdate"]
            .as_object()
            .ok_or("Missing lastUpdate in deployment config")?;

        let deploy_block = last_update["block_number"]
            .as_str()
            .ok_or("Missing block_number in lastUpdate")?
            .parse::<u64>()?;

        let (service_manager_address, service_manager_is_wrapper) =
            if let Some(addr) = addresses.get("avsServiceManager").and_then(|v| v.as_str()) {
                (
                    addr.parse::<Address>()
                        .map_err(|_| "Failed to parse service manager address")?,
                    false,
                )
            } else if let Some(addr) = addresses
                .get("avsServiceManagerWrapper")
                .and_then(|v| v.as_str())
            {
                (
                    addr.parse::<Address>()
                        .map_err(|_| "Failed to parse service manager address")?,
                    true,
                )
            } else {
                return Err("Missing avsServiceManager or avsServiceManagerWrapper address".into());
            };

        let registry_coordinator_address = registry_coordinator
            .parse::<Address>()
            .map_err(|_| "Failed to parse registry coordinator address")?;

        let operator_state_retriever_address = operator_state_retriever
            .parse::<Address>()
            .map_err(|_| "Failed to parse operator state retriever address")?;

        Ok(AvsDeploymentConfig {
            registry_coordinator_address,
            deploy_block,
            operator_state_retriever_address,
            service_manager_address,
            service_manager_is_wrapper,
        })
    }

    /// Connects to the middleware contracts named in the deployment JSON at
    /// `avs_deployment_path` and starts an in-memory operator info service over
    /// the websocket endpoint.
    pub async fn new(
        http_endpoint: String,
        ws_endpoint: String,
        avs_deployment_path: String,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let config = Self::read_avs_deployment_config(&avs_deployment_path)?;
        let avs_registry_reader = AvsRegistryChainReader::new(
            config.registry_coordinator_address,
            config.operator_state_retriever_address,
            http_endpoint.clone(),
        )
        .await?;
        let (operator_info_service, _rx) =
            OperatorInfoServiceInMemory::new(avs_registry_reader.clone(), ws_endpoint)
                .await
                .expect("Failed to create OperatorInfoServiceInMemory");

        Ok(Self {
            http_endpoint,
            registry_coordinator_address: config.registry_coordinator_address,
            registry_coordinator_deploy_block: config.deploy_block,
            operator_info_service: Arc::new(operator_info_service),
            operator_state_retriever_address: config.operator_state_retriever_address,
            service_manager_address: config.service_manager_address,
        })
    }

    /// Reads the quorum-0 operator state (stakes, keys, sockets) at the current
    /// block, backfilling operator registration events from the deploy block.
    pub async fn get_operator_states(&self) -> Result<Vec<QuorumInfo>, Box<dyn std::error::Error>> {
        // Query current block and backfill operator events
        let provider = get_provider(&self.http_endpoint);
        let current_block_number = provider.get_block_number().await?;

        // Retrieve threshold from the AVS service manager contract.
        let avs = IAvsThresholds::new(self.service_manager_address, provider.clone());
        let quorum_threshold = avs.QUORUM_THRESHOLD().call().await?.to::<u64>();
        let threshold_denominator = avs.THRESHOLD_DENOMINATOR().call().await?.to::<u64>();
        self.operator_info_service
            .query_past_registered_operator_events_and_fill_db(
                self.registry_coordinator_deploy_block,
                current_block_number,
            )
            .await?;

        // Query operator states using the dynamic address from config
        let operator_state_retriever =
            OperatorStateRetriever::new(self.operator_state_retriever_address, provider);
        let quorum_numbers: Vec<u8> = vec![0];
        let operators_state = operator_state_retriever
            .getOperatorState_0(
                self.registry_coordinator_address,
                quorum_numbers.into(),
                current_block_number.try_into().unwrap(),
            )
            .call()
            .await?;

        let mut quorum_infos = Vec::new();

        for (quorum_number, operators) in operators_state.iter().enumerate() {
            let mut quorum_operators = Vec::new();
            let mut total_stake = U256::ZERO;

            for op in operators {
                let stake = U256::from(op.stake);
                total_stake += stake;

                let pub_keys = if let Ok(info) = self
                    .operator_info_service
                    .get_operator_info(op.operator)
                    .await
                {
                    info.map(|keys| {
                        CommonwarePublicKeys::from_bls_keys(keys.g1_pub_key, keys.g2_pub_key)
                    })
                } else {
                    None
                };

                let socket = self
                    .operator_info_service
                    .get_operator_socket(op.operator)
                    .await
                    .ok()
                    .flatten();

                quorum_operators.push(OperatorInfo {
                    address: op.operator,
                    stake,
                    pub_keys,
                    socket,
                    quorum_number: quorum_number as u8,
                });
            }

            let operator_count = operators.len() as u64;
            let threshold =
                (operator_count * quorum_threshold).div_ceil(threshold_denominator) as usize;

            quorum_infos.push(QuorumInfo {
                quorum_number: quorum_number as u8,
                operator_count: operators.len(),
                threshold,
                total_stake,
                operators: quorum_operators,
            });
        }

        Ok(quorum_infos)
    }
}

/// The AVS deployment JSON (`AVS_DEPLOYMENT_PATH`) as consumed by the router's
/// on-chain submission path.
#[derive(Debug, Deserialize)]
pub struct AvsDeployment {
    pub addresses: ContractAddresses,
}

/// Contract addresses from the deployment JSON. Unknown keys are collected in
/// `extra` and resolvable via [`AvsDeployment::custom_address`].
#[derive(Debug, Deserialize)]
pub struct ContractAddresses {
    #[serde(rename = "registryCoordinator")]
    pub registry_coordinator: String,
    #[serde(rename = "blsapkRegistry")]
    pub bls_apk_registry: String,
    #[serde(rename = "blsSigCheck")]
    pub bls_sig_check_operator_state_retriever: String,
    #[serde(flatten)]
    pub extra: HashMap<String, String>,
}

impl AvsDeployment {
    /// Loads the deployment JSON from the path in `AVS_DEPLOYMENT_PATH`.
    pub fn load() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let deployment_path =
            env::var("AVS_DEPLOYMENT_PATH").map_err(|_| "AVS_DEPLOYMENT_PATH must be set")?;
        let content = fs::read_to_string(deployment_path)?;
        let deployment: AvsDeployment = serde_json::from_str(&content)?;
        Ok(deployment)
    }

    pub fn registry_coordinator_address(
        &self,
    ) -> Result<Address, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Address::from_str(&self.addresses.registry_coordinator)?)
    }

    pub fn bls_apk_registry_address(
        &self,
    ) -> Result<Address, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Address::from_str(&self.addresses.bls_apk_registry)?)
    }

    pub fn bls_sig_check_operator_state_retriever_address(
        &self,
    ) -> Result<Address, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Address::from_str(
            &self.addresses.bls_sig_check_operator_state_retriever,
        )?)
    }

    /// Resolves an address by its raw JSON key (for contracts not modeled above).
    pub fn custom_address(
        &self,
        name: &str,
    ) -> Result<Address, Box<dyn std::error::Error + Send + Sync>> {
        let addr = self
            .addresses
            .extra
            .get(name)
            .ok_or_else(|| format!("Address '{}' not found in deployment config", name))?;
        Ok(Address::from_str(addr)?)
    }
}

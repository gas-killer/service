//! EigenLayer operator-set discovery, vendored from `commonware-restaking`.
//!
//! [`EigenStakingClient`] reads the quorum-0 operator set (addresses, stakes,
//! sockets) from the deployed EigenLayer middleware contracts. The router and
//! nodes use this at startup to build the (static) participant set for the
//! aggregation engine, so every process MUST observe the same operator list —
//! participant indices are positions in the sorted operator-address order.
//!
//! With the ECDSA scheme the operator's registered Ethereum address IS the
//! signing identity, so no on-chain public-key material needs to be fetched —
//! only the operator addresses (from the operator state retriever) and the
//! sockets (from the registration events indexed by the operator info service).

use alloy_primitives::{Address, U256};
use alloy_provider::Provider;
use eigen_client_avsregistry::reader::AvsRegistryChainReader;
use eigen_common::get_provider;
use eigen_services_operatorsinfo::operator_info::OperatorInfoService;
use eigen_services_operatorsinfo::operatorsinfo_inmemory::OperatorInfoServiceInMemory;
use eigen_utils::rewardsv2::middleware::operator_state_retriever::OperatorStateRetriever;
use serde_json::Value;
use std::fs;
use std::sync::Arc;

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

/// One operator's registration state in a quorum.
#[derive(Debug, Clone)]
pub struct OperatorInfo {
    /// The operator's registered Ethereum address — also its ECDSA signing
    /// identity in the aggregation scheme.
    pub address: Address,
    pub stake: U256,
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
    /// at N3f1 (`n - (n-1)/3`), and the authoritative operator-count check runs
    /// on-chain in `GasKillerSDK.verifyAndUpdate`.
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

    /// Reads the quorum-0 operator state (addresses, stakes, sockets) at the
    /// current block, backfilling operator registration events from the deploy
    /// block.
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

                let socket = self
                    .operator_info_service
                    .get_operator_socket(op.operator)
                    .await
                    .ok()
                    .flatten();

                quorum_operators.push(OperatorInfo {
                    address: op.operator,
                    stake,
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

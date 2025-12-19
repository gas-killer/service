use crate::GasKillerHandler;
use crate::creator::{
    GasKillerConfig, GasKillerCreator, GasKillerCreatorType, ListeningGasKillerCreator,
    SimpleTaskQueue,
};
use crate::ingress::start_gas_killer_http_server;
use alloy::primitives::Address;
use alloy_provider::ProviderBuilder;
use alloy_signer_local::PrivateKeySigner;
use anyhow::Result;
use commonware_avs_router::bindings::blsapkregistry::BLSApkRegistry;
use commonware_avs_router::bindings::blssigcheckoperatorstateretriever::BLSSigCheckOperatorStateRetriever;
use commonware_avs_router::executor::bls::BlsEigenlayerExecutor;
use commonware_avs_usecases::AvsDeployment;
use gas_killer_common::GasKillerValidator;
use serde::Deserialize;
use std::{env, fs, str::FromStr, sync::Arc};

/// Struct to parse avs_deploy.json and extract the BLSSigCheckOperatorStateRetriever address
/// The IncredibleSquaringTaskManager field contains the BLSSigCheckOperatorStateRetriever
#[derive(Debug, Deserialize)]
struct AvsDeploymentJson {
    addresses: AvsAddresses,
}

#[derive(Debug, Deserialize)]
struct AvsAddresses {
    /// The BLSSigCheckOperatorStateRetriever address (used for both signature checking and operator state retrieval)
    #[serde(rename = "IncredibleSquaringTaskManager")]
    bls_sig_check_operator_state_retriever: String,
}

/// Factory function to create a default creator
pub async fn create_creator() -> anyhow::Result<GasKillerCreatorType> {
    let creator = GasKillerCreator::new();
    Ok(GasKillerCreatorType::Basic(creator))
}

/// Factory function to create a listening creator with HTTP server
pub async fn create_listening_creator_with_server(
    addr: String,
    validator: Arc<GasKillerValidator>,
) -> anyhow::Result<GasKillerCreatorType> {
    let queue = SimpleTaskQueue::new();
    let config = GasKillerConfig::default();
    let creator = ListeningGasKillerCreator::new(queue.clone(), config, validator);
    // Wrap the queue in Arc for the HTTP server
    let queue_for_server = Arc::new(queue);
    tokio::spawn(async move {
        start_gas_killer_http_server(queue_for_server, &addr).await;
    });
    Ok(GasKillerCreatorType::Listening(creator))
}

/// Creates a new BlsEigenlayerExecutor configured for Gas Killer operations
pub async fn create_gas_killer_executor() -> Result<BlsEigenlayerExecutor<GasKillerHandler>> {
    let http_rpc = env::var("HTTP_RPC").expect("HTTP_RPC must be set");
    let view_only_provider =
        ProviderBuilder::new().connect_http(url::Url::parse(&http_rpc).unwrap());

    let deployment =
        AvsDeployment::load().map_err(|e| anyhow::anyhow!("Failed to load deployment: {}", e))?;

    let bls_apk_registry_address = deployment
        .bls_apk_registry_address()
        .map_err(|e| anyhow::anyhow!("Failed to get BLS APK registry address: {}", e))?;
    let registry_coordinator_address = deployment
        .registry_coordinator_address()
        .map_err(|e| anyhow::anyhow!("Failed to get registry coordinator address: {}", e))?;

    let ecdsa_signer =
        PrivateKeySigner::from_str(&env::var("PRIVATE_KEY").expect("PRIVATE_KEY must be set"))
            .map_err(|e| anyhow::anyhow!("Failed to parse private key: {}", e))?;

    // Read the BLSSigCheckOperatorStateRetriever address directly from avs_deploy.json
    // The IncredibleSquaringTaskManager field contains the BLSSigCheckOperatorStateRetriever
    // which implements both signature checking AND operator state retrieval
    let avs_deployment_path =
        env::var("AVS_DEPLOYMENT_PATH").expect("AVS_DEPLOYMENT_PATH must be set");
    let avs_content = fs::read_to_string(&avs_deployment_path)
        .map_err(|e| anyhow::anyhow!("Failed to read AVS deployment file: {}", e))?;
    let avs_deployment_json: AvsDeploymentJson = serde_json::from_str(&avs_content)
        .map_err(|e| anyhow::anyhow!("Failed to parse AVS deployment JSON: {}", e))?;
    let bls_operator_state_retriever_address: Address = avs_deployment_json
        .addresses
        .bls_sig_check_operator_state_retriever
        .parse()
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse BLS operator state retriever address: {}",
                e
            )
        })?;

    let write_provider = ProviderBuilder::new()
        .wallet(ecdsa_signer)
        .connect(&http_rpc)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect write provider: {}", e))?;

    let bls_apk_registry =
        BLSApkRegistry::new(bls_apk_registry_address, view_only_provider.clone());
    let bls_operator_state_retriever = BLSSigCheckOperatorStateRetriever::new(
        bls_operator_state_retriever_address,
        view_only_provider.clone(),
    );

    let gas_killer_handler = GasKillerHandler::new(write_provider);

    Ok(BlsEigenlayerExecutor::new(
        view_only_provider,
        bls_apk_registry,
        bls_operator_state_retriever,
        registry_coordinator_address,
        gas_killer_handler,
    ))
}

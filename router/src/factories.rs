use crate::GasKillerHandler;
use crate::creator::{
    GasKillerConfig, GasKillerCreator, GasKillerCreatorType, ListeningGasKillerCreator,
    SimpleTaskQueue,
};
use crate::ingress::start_gas_killer_http_server;
use alloy_provider::ProviderBuilder;
use alloy_signer_local::PrivateKeySigner;
use anyhow::Result;
use commonware_avs_router::bindings::blsapkregistry::BLSApkRegistry;
use commonware_avs_router::bindings::blssigcheckoperatorstateretriever::BLSSigCheckOperatorStateRetriever;
use commonware_avs_router::executor::bls::BlsEigenlayerExecutor;
use commonware_avs_usecases::AvsDeployment;
use gas_killer_common::{ChainId, GasKillerValidator, WalletProvider};
use std::collections::HashMap;
use std::{env, str::FromStr, sync::Arc};
use tracing::info;

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

/// Creates a wallet provider for a specific chain
async fn create_wallet_provider_for_chain(
    chain_id: ChainId,
    private_key: &str,
) -> Result<WalletProvider> {
    let http_rpc = match chain_id {
        ChainId::Sepolia => {
            env::var("HTTP_RPC").map_err(|_| anyhow::anyhow!("HTTP_RPC must be set for Sepolia"))?
        }
        ChainId::Gnosis => env::var("GNOSIS_HTTP_RPC")
            .map_err(|_| anyhow::anyhow!("GNOSIS_HTTP_RPC must be set for Gnosis"))?,
    };

    let ecdsa_signer = PrivateKeySigner::from_str(private_key)
        .map_err(|e| anyhow::anyhow!("Failed to parse private key: {}", e))?;

    let provider = ProviderBuilder::new()
        .wallet(ecdsa_signer)
        .connect(&http_rpc)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect write provider for {}: {}", chain_id, e))?;

    Ok(provider)
}

/// Creates a new BlsEigenlayerExecutor configured for Gas Killer operations with multi-chain support
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

    let private_key = env::var("PRIVATE_KEY").expect("PRIVATE_KEY must be set");

    let bls_operator_state_retriever_address = deployment
        .bls_sig_check_operator_state_retriever_address()
        .map_err(|e| {
            anyhow::anyhow!("Failed to get BLS operator state retriever address: {}", e)
        })?;

    // Create wallet providers for each supported chain
    let mut providers: HashMap<ChainId, WalletProvider> = HashMap::new();

    // Sepolia provider (required)
    let sepolia_provider = create_wallet_provider_for_chain(ChainId::Sepolia, &private_key).await?;
    providers.insert(ChainId::Sepolia, sepolia_provider);
    info!(chain = %ChainId::Sepolia, "Created wallet provider");

    // Gnosis provider (optional - only if GNOSIS_HTTP_RPC is set)
    if env::var("GNOSIS_HTTP_RPC").is_ok() {
        match create_wallet_provider_for_chain(ChainId::Gnosis, &private_key).await {
            Ok(gnosis_provider) => {
                providers.insert(ChainId::Gnosis, gnosis_provider);
                info!(chain = %ChainId::Gnosis, "Created wallet provider");
            }
            Err(e) => {
                tracing::warn!(
                    chain = %ChainId::Gnosis,
                    error = %e,
                    "Failed to create Gnosis wallet provider, Gnosis chain will be unavailable"
                );
            }
        }
    } else {
        info!("GNOSIS_HTTP_RPC not set, Gnosis chain support disabled");
    }

    let bls_apk_registry =
        BLSApkRegistry::new(bls_apk_registry_address, view_only_provider.clone());
    let bls_operator_state_retriever = BLSSigCheckOperatorStateRetriever::new(
        bls_operator_state_retriever_address,
        view_only_provider.clone(),
    );

    // Create handler with multi-chain providers
    let gas_killer_handler = GasKillerHandler::with_providers(providers);

    Ok(BlsEigenlayerExecutor::new(
        view_only_provider,
        bls_apk_registry,
        bls_operator_state_retriever,
        registry_coordinator_address,
        gas_killer_handler,
    ))
}

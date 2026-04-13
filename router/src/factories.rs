use crate::GasKillerHandler;
use crate::creator::{
    GasKillerConfig, GasKillerCreator, GasKillerCreatorType, ListeningGasKillerCreator,
    SimpleTaskQueue,
};
use crate::ingress::start_gas_killer_http_server;
use alloy::network::{Ethereum, EthereumWallet};
use alloy_provider::{
    Identity, ProviderBuilder, RootProvider,
    fillers::{
        BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
        SimpleNonceManager, WalletFiller,
    },
};
use alloy_signer_local::PrivateKeySigner;
use anyhow::Result;
use commonware_avs_eigenlayer::AvsDeployment;
use commonware_avs_router::bindings::bls_apk_registry::BLSApkRegistry;
use commonware_avs_router::bindings::bls_sig_check_operator_state_retriever::BLSSigCheckOperatorStateRetriever;
use commonware_avs_router::executor::bls::BlsEigenlayerExecutor;
use gas_killer_common::{ChainId, GasKillerValidator};
use std::collections::HashMap;
use std::{env, str::FromStr, sync::Arc};
use tracing::info;

/// Wallet provider that uses SimpleNonceManager to always fetch the pending nonce from the
/// chain rather than caching it locally. This prevents nonce corruption when a transaction
/// fails during gas estimation (e.g., due to a stale transition_index from double-execution),
/// because the cached counter is never pre-incremented before the tx is actually broadcast.
pub type SimpleWalletProvider = FillProvider<
    JoinFill<
        JoinFill<
            Identity,
            JoinFill<
                GasFiller,
                JoinFill<BlobGasFiller, JoinFill<NonceFiller<SimpleNonceManager>, ChainIdFiller>>,
            >,
        >,
        WalletFiller<EthereumWallet>,
    >,
    RootProvider,
    Ethereum,
>;

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

/// Creates a wallet provider for a specific chain using SimpleNonceManager.
///
/// SimpleNonceManager always fetches the pending nonce from the node on every transaction
/// rather than caching it locally. This ensures that if a transaction fails during gas
/// estimation (e.g., double-execution with a stale transition_index), the local nonce counter
/// is never corrupted, keeping subsequent rounds healthy.
async fn create_wallet_provider_for_chain(
    chain_id: ChainId,
    private_key: &str,
) -> Result<SimpleWalletProvider> {
    let http_rpc = match chain_id {
        ChainId::L1 => env::var("HTTP_RPC")
            .map_err(|_| anyhow::anyhow!("HTTP_RPC must be set for L1 chain"))?,
        ChainId::L2 => env::var("L2_HTTP_RPC")
            .map_err(|_| anyhow::anyhow!("L2_HTTP_RPC must be set for L2 chain"))?,
    };

    let ecdsa_signer = PrivateKeySigner::from_str(private_key)
        .map_err(|e| anyhow::anyhow!("Failed to parse private key: {}", e))?;

    let provider = ProviderBuilder::default()
        .filler(JoinFill::new(
            GasFiller,
            JoinFill::new(
                BlobGasFiller::default(),
                JoinFill::new(NonceFiller::<SimpleNonceManager>::default(), ChainIdFiller::default()),
            ),
        ))
        .wallet(ecdsa_signer)
        .connect(&http_rpc)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect write provider for {}: {}", chain_id, e))?;

    Ok(provider)
}

/// Creates a new BlsEigenlayerExecutor configured for Gas Killer operations with multi-chain support.
///
/// The executor's read side (view_only_provider, BLS contracts) always points at L1 via
/// `HTTP_RPC` and `AVS_DEPLOYMENT_PATH`. Operator state lives on L1 and is not
/// available on the L2 mimic contract.
///
/// `L2_HTTP_RPC` is used exclusively for the write side: submitting `verifyAndUpdate`
/// transactions on L2 when the target contract lives there.
pub async fn create_gas_killer_executor() -> Result<BlsEigenlayerExecutor<GasKillerHandler<SimpleWalletProvider>>> {
    let http_rpc = env::var("HTTP_RPC").expect("HTTP_RPC must be set");
    let private_key = env::var("PRIVATE_KEY").expect("PRIVATE_KEY must be set");

    let l2_http_rpc = env::var("L2_HTTP_RPC").ok();

    let deployment =
        AvsDeployment::load().map_err(|e| anyhow::anyhow!("Failed to load deployment: {}", e))?;
    info!("Executor reads operator state from L1 (HTTP_RPC)");

    let view_only_provider = ProviderBuilder::new().connect_http(
        url::Url::parse(&http_rpc)
            .map_err(|e| anyhow::anyhow!("Failed to parse RPC URL '{}': {}", http_rpc, e))?,
    );

    let bls_apk_registry_address = deployment
        .bls_apk_registry_address()
        .map_err(|e| anyhow::anyhow!("Failed to get BLS APK registry address: {}", e))?;
    let registry_coordinator_address = deployment
        .registry_coordinator_address()
        .map_err(|e| anyhow::anyhow!("Failed to get registry coordinator address: {}", e))?;
    let bls_operator_state_retriever_address = deployment
        .bls_sig_check_operator_state_retriever_address()
        .map_err(|e| {
            anyhow::anyhow!("Failed to get BLS operator state retriever address: {}", e)
        })?;

    // Create wallet providers for each supported chain
    let mut providers: HashMap<ChainId, SimpleWalletProvider> = HashMap::new();

    // L1 provider (required)
    let l1_provider = create_wallet_provider_for_chain(ChainId::L1, &private_key).await?;
    providers.insert(ChainId::L1, l1_provider);
    info!(chain = %ChainId::L1, "Created L1 wallet provider");

    // L2 provider — optional, only used for write-side tx execution on L2
    if l2_http_rpc.is_some() {
        match create_wallet_provider_for_chain(ChainId::L2, &private_key).await {
            Ok(l2_provider) => {
                providers.insert(ChainId::L2, l2_provider);
                info!(chain = %ChainId::L2, "Created L2 wallet provider");
            }
            Err(e) => {
                tracing::warn!(
                    chain = %ChainId::L2,
                    error = %e,
                    "Failed to create L2 wallet provider, L2 chain will be unavailable"
                );
            }
        }
    } else {
        info!("L2_HTTP_RPC not set, L2 chain support disabled");
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

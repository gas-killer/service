use crate::GasKillerHandler;
use crate::creator::{
    DispatchTime, GasKillerConfig, GasKillerCreator, GasKillerCreatorType,
    ListeningGasKillerCreator, task_channel, task_queue_depth,
};
use crate::ingress::{
    AvsMetadata, AvsOperatorSetMetadata, AvsOperatorSetSoftware, IngressState,
    start_gas_killer_http_server,
};
use crate::metrics::MetricsCollector;
use crate::store::SqliteStore;
use alloy::network::{Ethereum, EthereumWallet};
use alloy_provider::{
    Identity, Provider, ProviderBuilder, RootProvider,
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
use commonware_runtime::Metrics;
use gas_killer_common::{ChainRole, GasKillerValidator};
use std::collections::HashMap;
use std::time::Duration;
use std::{env, str::FromStr, sync::Arc};
use tracing::info;

/// How often the background loop re-checks SQLite store liveness for the `gas_killer_db_up`
/// metric. Aligned with a typical Prometheus scrape interval so the gauge stays fresh.
const DB_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(15);

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
    metrics: Arc<MetricsCollector>,
    dispatch_time: DispatchTime,
) -> anyhow::Result<GasKillerCreatorType> {
    let (sender, receiver) = task_channel();
    let queue_depth = task_queue_depth();
    let config = GasKillerConfig::default();
    let creator = ListeningGasKillerCreator::new(
        receiver,
        queue_depth.clone(),
        config,
        validator,
        dispatch_time,
    )
    .with_metrics(Arc::clone(&metrics));
    let providers = build_ingress_providers()?;
    let ingress_password = env::var("INGRESS_PASSWORD").ok().filter(|p| !p.is_empty());
    if ingress_password.is_none() {
        tracing::warn!(
            "INGRESS_PASSWORD is not set — /trigger endpoint is unauthenticated; set INGRESS_PASSWORD in production"
        );
    }
    let operator_sets = {
        let opset_name = env::var("AVS_OPSET_NAME").unwrap_or_default();
        if opset_name.is_empty() {
            None
        } else {
            let slashing_conditions = env::var("AVS_OPSET_SLASHING_CONDITIONS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            Some(vec![AvsOperatorSetMetadata {
                name: opset_name,
                id: env::var("AVS_OPSET_ID").unwrap_or_else(|_| "0".to_string()),
                description: env::var("AVS_OPSET_DESCRIPTION").unwrap_or_default(),
                software: vec![AvsOperatorSetSoftware {
                    name: env::var("AVS_OPSET_SOFTWARE_NAME")
                        .unwrap_or_else(|_| "gas-killer-node".to_string()),
                    description: env::var("AVS_OPSET_SOFTWARE_DESCRIPTION").unwrap_or_default(),
                    url: env::var("AVS_OPSET_SOFTWARE_URL").unwrap_or_default(),
                }],
                slashing_conditions,
            }])
        }
    };
    let avs_metadata = AvsMetadata {
        name: env::var("AVS_METADATA_NAME").unwrap_or_else(|_| "Gas Killer".to_string()),
        website: env::var("AVS_METADATA_WEBSITE")
            .unwrap_or_else(|_| "https://gaskiller.xyz".to_string()),
        description: env::var("AVS_METADATA_DESCRIPTION").unwrap_or_else(|_| {
            "Verifiable off-chain compute service for EVM smart contracts via EigenLayer"
                .to_string()
        }),
        logo: env::var("AVS_METADATA_LOGO").ok().filter(|s| !s.is_empty()),
        twitter: env::var("AVS_METADATA_TWITTER")
            .ok()
            .filter(|s| !s.is_empty()),
        operator_sets,
    };
    // Open the durable store and apply migrations before serving traffic. A failure here
    // aborts router startup rather than running against an unmigrated or unwritable store.
    let store = SqliteStore::connect().await?;

    // Publish store liveness as `gas_killer_db_up`. connect() already proved the store
    // answers, so seed the gauge to 1; a background loop then re-checks so a later volume
    // loss (detached PVC, full or read-only disk) surfaces as db_up=0 on the dashboard.
    metrics.db_up.set(1);
    {
        let store = store.clone();
        let metrics = Arc::clone(&metrics);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(DB_HEALTH_CHECK_INTERVAL);
            loop {
                ticker.tick().await;
                metrics.db_up.set(store.health_check().await.is_ok() as i64);
            }
        });
    }

    let ingress_state = IngressState::new(
        sender,
        queue_depth,
        gas_killer_common::p2p_message_backlog(),
        metrics,
        providers,
        ingress_password,
        avs_metadata,
    )
    .with_store(store);
    tokio::spawn(async move {
        start_gas_killer_http_server(ingress_state, &addr).await;
    });
    Ok(GasKillerCreatorType::Listening(Box::new(creator)))
}

fn build_ingress_providers()
-> anyhow::Result<HashMap<ChainRole, gas_killer_common::ReadOnlyProvider>> {
    let chain_rpc_urls = gas_killer_common::chain_rpc_urls_from_env()?;
    let providers = gas_killer_common::build_read_providers(&chain_rpc_urls);

    if providers.is_empty() {
        anyhow::bail!("no ingress providers could be created: set HTTP_RPC and/or L2_HTTP_RPC");
    }

    Ok(providers)
}

/// Creates a wallet provider for a specific chain using SimpleNonceManager.
///
/// SimpleNonceManager always fetches the pending nonce from the node on every transaction
/// rather than caching it locally. This ensures that if a transaction fails during gas
/// estimation (e.g., double-execution with a stale transition_index), the local nonce counter
/// is never corrupted, keeping subsequent rounds healthy.
async fn create_wallet_provider_for_chain(
    chain_role: ChainRole,
    private_key: &str,
) -> Result<SimpleWalletProvider> {
    let http_rpc = chain_role.rpc_url()?;

    let ecdsa_signer = PrivateKeySigner::from_str(private_key)
        .map_err(|e| anyhow::anyhow!("Failed to parse private key: {}", e))?;

    let provider = ProviderBuilder::default()
        .filler(JoinFill::new(
            GasFiller,
            JoinFill::new(
                BlobGasFiller::default(),
                JoinFill::new(
                    NonceFiller::<SimpleNonceManager>::default(),
                    ChainIdFiller::default(),
                ),
            ),
        ))
        .wallet(ecdsa_signer)
        .connect(&http_rpc)
        .await
        .map_err(|e| {
            anyhow::anyhow!("Failed to connect write provider for {}: {}", chain_role, e)
        })?;

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
pub async fn create_gas_killer_executor(
    metrics: Arc<MetricsCollector>,
    dispatch_time: DispatchTime,
    context: &impl Metrics,
) -> Result<BlsEigenlayerExecutor<GasKillerHandler<SimpleWalletProvider>>> {
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

    // Create wallet providers for each supported chain, keyed by actual EVM chain ID.
    // `chain_roles` records the role behind each numeric ID so the executor can pick
    // the per-role receipt timeout from the chain ID carried in task data.
    let mut providers: HashMap<u64, SimpleWalletProvider> = HashMap::new();
    let mut chain_roles: HashMap<u64, ChainRole> = HashMap::new();

    // L1 provider (required)
    let l1_provider = create_wallet_provider_for_chain(ChainRole::L1, &private_key).await?;
    let l1_chain_id = l1_provider
        .get_chain_id()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get L1 chain ID: {}", e))?;
    providers.insert(l1_chain_id, l1_provider);
    chain_roles.insert(l1_chain_id, ChainRole::L1);
    info!(
        chain_id = l1_chain_id,
        chain = "l1",
        "Created L1 wallet provider"
    );

    // L2 provider — optional, only used for write-side tx execution on L2
    if l2_http_rpc.is_some() {
        match create_wallet_provider_for_chain(ChainRole::L2, &private_key).await {
            Ok(l2_provider) => match l2_provider.get_chain_id().await {
                Ok(l2_chain_id) if l2_chain_id == l1_chain_id => {
                    tracing::warn!(
                        chain_id = l2_chain_id,
                        "L2_HTTP_RPC resolves to the same EVM chain ID as HTTP_RPC (L1); \
                         skipping L2 provider to avoid overwriting L1. Check that HTTP_RPC \
                         and L2_HTTP_RPC point at different chains"
                    );
                }
                Ok(l2_chain_id) => {
                    providers.insert(l2_chain_id, l2_provider);
                    chain_roles.insert(l2_chain_id, ChainRole::L2);
                    info!(
                        chain_id = l2_chain_id,
                        chain = "l2",
                        "Created L2 wallet provider"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        chain = "l2",
                        error = %e,
                        "Failed to get L2 chain ID, L2 chain will be unavailable"
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    chain = "l2",
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

    // Optional override (seconds) for the verifyAndUpdate receipt-wait timeout.
    // Unset falls back to the executor's per-chain defaults.
    let receipt_timeout_override = env::var("EXECUTOR_RECEIPT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());

    // Create handler with multi-chain providers
    let gas_killer_handler = GasKillerHandler::with_providers(providers)
        .with_chain_roles(chain_roles)
        .with_metrics(metrics)
        .with_dispatch_time(dispatch_time)
        .with_receipt_timeout(receipt_timeout_override);

    Ok(BlsEigenlayerExecutor::new(
        view_only_provider,
        bls_apk_registry,
        bls_operator_state_retriever,
        registry_coordinator_address,
        gas_killer_handler,
    )
    .with_metrics(context))
}

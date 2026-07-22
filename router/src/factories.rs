//! Construction of the router's environment-configured components: alloy
//! providers, the HTTP ingress, and the on-chain submitter.

use crate::GasKillerHandler;
use crate::ingress::{
    AvsMetadata, AvsOperatorSetMetadata, AvsOperatorSetSoftware, GasKillerTaskRequest,
    IngressState, start_gas_killer_http_server,
};
use crate::metrics::MetricsCollector;
use crate::schnorr_coordinator::SchnorrCertifiedReceiver;
use crate::schnorr_submitter::SchnorrSubmitter;
use crate::sequencer::{
    InFlightTask, QueuedTask, TaskQueueDepth, TaskReceiver, TaskSender, task_channel,
    task_queue_depth,
};
use crate::store::SqliteStore;
use alloy::network::{Ethereum, EthereumWallet};
use alloy_primitives::Bytes;
use alloy_provider::{
    Identity, Provider, ProviderBuilder, RootProvider,
    fillers::{
        BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
        SimpleNonceManager, WalletFiller,
    },
};
use alloy_signer_local::PrivateKeySigner;
use anyhow::Result;
use commonware_avs_core::bn254::Bn254Scheme;
use commonware_avs_eigenlayer::AvsDeployment;
use commonware_avs_router::reporter::CertifiedReceiver;
use commonware_avs_router::sequencer::{DispatchTime, ResolutionSender, SharedAssignments};
use commonware_avs_router::submitter::Submitter;
use gas_killer_common::ChainRole;
use gas_killer_common::bindings::bls_apk_registry::BLSApkRegistry;
use gas_killer_common::bindings::bls_sig_check_operator_state_retriever::BLSSigCheckOperatorStateRetriever;
use gas_killer_common::task_data::GasKillerTaskData;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::{env, str::FromStr, sync::Arc};
use tracing::{error, info};

/// Quorum 0 — the only quorum this deployment operates on.
const QUORUM_NUMBERS: &[u8] = &[0x00];

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

/// The ingress side of the task pipeline: the sequencer consumes `receiver`;
/// `sender` is retained by the caller so the channel stays open even when the
/// HTTP server is disabled (`INGRESS != true`) — a closed channel would shut the
/// sequencer down.
pub struct IngressHandles {
    pub sender: TaskSender,
    pub receiver: TaskReceiver,
    pub queue_depth: TaskQueueDepth,
    /// The durable store opened for the HTTP ingress, shared with the task source
    /// and executor so they can drive task status transitions. `None` when
    /// `INGRESS != true` (no store is opened without the HTTP server).
    pub store: Option<SqliteStore>,
}

/// Creates the ingress task channel and, when `INGRESS=true`, spawns the HTTP
/// server (`/trigger`, `/healthz`, `/avs-metadata`) on `INGRESS_ADDRESS`
/// (default `0.0.0.0:8080`). Behavior, env knobs, and endpoint shapes are
/// unchanged from the pre-migration router.
pub async fn create_ingress(metrics: Arc<MetricsCollector>) -> Result<IngressHandles> {
    let (sender, receiver) = task_channel();
    let queue_depth = task_queue_depth();

    let use_ingress = env::var("INGRESS").unwrap_or_default().to_lowercase() == "true";
    if !use_ingress {
        info!("INGRESS is not 'true'; HTTP ingress disabled (task channel stays idle)");
        return Ok(IngressHandles {
            sender,
            receiver,
            queue_depth,
            store: None,
        });
    }

    let addr = env::var("INGRESS_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    info!(address = %addr, "starting GasKiller HTTP ingress");

    let providers = build_ingress_providers()?;
    let admin_key = env::var("ADMIN_KEY").ok().filter(|k| !k.is_empty());
    if admin_key.is_none() {
        tracing::warn!(
            "ADMIN_KEY is not set — /admin/keys endpoints are disabled; set ADMIN_KEY to manage API keys"
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

    let store_for_return = store.clone();
    let ingress_state = IngressState::new(
        sender.clone(),
        queue_depth.clone(),
        gas_killer_common::p2p_message_backlog(),
        metrics,
        providers,
        avs_metadata,
    )
    .with_store(store)
    .with_admin_key(admin_key);
    // Plain tokio::spawn works: the commonware tokio runtime shares the ambient
    // reactor with axum.
    tokio::spawn(async move {
        start_gas_killer_http_server(ingress_state, &addr).await;
    });

    Ok(IngressHandles {
        sender,
        receiver,
        queue_depth,
        store: Some(store_for_return),
    })
}

/// Re-enqueues every task left `queued` or `processing` by a previous router life,
/// so a restart resumes work already acknowledged to a client rather than losing
/// it. Called once at startup, before the sequencer starts pulling from `sender`.
///
/// Each task is rebuilt from its persisted request and pushed through the same
/// channel a fresh `POST /tasks` submission uses, so it flows through the normal
/// dequeue → processing → ready/failed pipeline indistinguishably from new work.
pub async fn requeue_incomplete_tasks(
    store: &SqliteStore,
    sender: &TaskSender,
    queue_depth: &TaskQueueDepth,
) -> Result<()> {
    let tasks = store.incomplete_tasks().await?;
    if tasks.is_empty() {
        return Ok(());
    }

    info!(
        count = tasks.len(),
        "re-enqueuing incomplete tasks from a previous router life"
    );
    for task in tasks {
        let task_id = task.id;
        let queued = QueuedTask {
            task_id: task_id.clone(),
            request: GasKillerTaskRequest { body: task.request },
        };
        if sender.send(queued).is_err() {
            error!(
                task_id,
                "failed to re-enqueue incomplete task: channel closed"
            );
            continue;
        }
        queue_depth.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
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

/// Builds the pieces every submitter shares: the L1 read-side provider (for
/// block-number / operator-state reads) and the multi-chain [`GasKillerHandler`]
/// that executes `verifyAndUpdate` on the write side.
///
/// The read side always points at L1 via `HTTP_RPC`. `L2_HTTP_RPC` is used
/// exclusively for the write side: submitting `verifyAndUpdate` transactions on L2
/// when the target contract lives there.
async fn create_handler_parts(
    metrics: Arc<MetricsCollector>,
    dispatch_time: DispatchTime,
    store: Option<SqliteStore>,
    in_flight: InFlightTask,
) -> Result<(
    gas_killer_common::bindings::ReadOnlyProvider,
    GasKillerHandler<SimpleWalletProvider>,
)> {
    let http_rpc = env::var("HTTP_RPC").expect("HTTP_RPC must be set");
    let private_key = env::var("PRIVATE_KEY").expect("PRIVATE_KEY must be set");

    let l2_http_rpc = env::var("L2_HTTP_RPC").ok();

    let view_only_provider = ProviderBuilder::new().connect_http(
        url::Url::parse(&http_rpc)
            .map_err(|e| anyhow::anyhow!("Failed to parse RPC URL '{}': {}", http_rpc, e))?,
    );

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

    // Optional override (seconds) for the verifyAndUpdate receipt-wait timeout.
    // Unset falls back to the executor's per-chain defaults.
    let receipt_timeout_override = env::var("EXECUTOR_RECEIPT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());

    // Create handler with multi-chain providers. The store and in-flight task slot
    // let a settled height advance its task's terminal status; the slot is shared
    // with the task source (see `GasKillerTaskSource`).
    let mut gas_killer_handler = GasKillerHandler::with_providers(providers)
        .with_chain_roles(chain_roles)
        .with_metrics(metrics)
        .with_dispatch_time(dispatch_time)
        .with_receipt_timeout(receipt_timeout_override)
        .with_in_flight_task(in_flight)
        .with_payload_block_buffer(gas_killer_common::payload_block_buffer());
    if let Some(store) = store {
        gas_killer_handler = gas_killer_handler.with_store(store);
    }

    Ok((view_only_provider, gas_killer_handler))
}

/// Creates the BLS [`Submitter`] with multi-chain support (`bls` mode).
///
/// The read side (view_only_provider, BLS contracts) always points at L1 via
/// `HTTP_RPC` and `AVS_DEPLOYMENT_PATH`. Operator state lives on L1 and is not
/// available on the L2 mimic contract.
#[allow(clippy::too_many_arguments)]
pub async fn create_submitter(
    scheme: Bn254Scheme,
    assignments: SharedAssignments<GasKillerTaskData>,
    certified: CertifiedReceiver<Bn254Scheme>,
    resolutions: ResolutionSender,
    metrics: Arc<MetricsCollector>,
    dispatch_time: DispatchTime,
    namespace: Vec<u8>,
    store: Option<SqliteStore>,
    in_flight: InFlightTask,
) -> Result<Submitter<GasKillerTaskData, GasKillerHandler<SimpleWalletProvider>>> {
    let (view_only_provider, gas_killer_handler) =
        create_handler_parts(metrics, dispatch_time, store, in_flight).await?;

    let deployment =
        AvsDeployment::load().map_err(|e| anyhow::anyhow!("Failed to load deployment: {}", e))?;
    info!("Submitter reads operator state from L1 (HTTP_RPC)");

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

    let bls_apk_registry =
        BLSApkRegistry::new(bls_apk_registry_address, view_only_provider.clone());
    let bls_operator_state_retriever = BLSSigCheckOperatorStateRetriever::new(
        bls_operator_state_retriever_address,
        view_only_provider.clone(),
    );

    Ok(Submitter::new(
        scheme,
        view_only_provider,
        bls_apk_registry,
        bls_operator_state_retriever,
        registry_coordinator_address,
        gas_killer_handler,
        assignments,
        certified,
        resolutions,
        namespace,
        Bytes::from_static(QUORUM_NUMBERS),
    ))
}

/// Creates the [`SchnorrSubmitter`] (`schnorr` mode). Same environment surface as
/// [`create_submitter`]; only the certified-observation source and the on-chain
/// calling convention differ (no BLS registry — the Schnorr registry is read
/// on-chain by the target contract itself).
#[allow(clippy::too_many_arguments)]
pub async fn create_schnorr_submitter(
    assignments: SharedAssignments<GasKillerTaskData>,
    certified: SchnorrCertifiedReceiver,
    resolutions: ResolutionSender,
    metrics: Arc<MetricsCollector>,
    dispatch_time: DispatchTime,
    namespace: Vec<u8>,
    store: Option<SqliteStore>,
    in_flight: InFlightTask,
) -> Result<SchnorrSubmitter> {
    let (view_only_provider, gas_killer_handler) =
        create_handler_parts(metrics, dispatch_time, store, in_flight).await?;
    Ok(SchnorrSubmitter::new(
        view_only_provider,
        gas_killer_handler,
        assignments,
        certified,
        resolutions,
        namespace,
    ))
}

pub mod creator;
pub mod executor;
pub mod gas_killer_creator;
pub mod gas_killer_orchestrator;
pub mod listening_creator;
pub mod orchestrator;

pub use gas_killer_creator::create_gas_killer_creator_with_server;
pub use gas_killer_orchestrator::GasKillerOrchestrator;
pub use orchestrator::Orchestrator;

use crate::handlers::{creator::Creator, gas_killer_creator::GasKillerCreator, listening_creator::ListeningCreator};
use std::sync::Arc;

use alloy::{network::EthereumWallet, providers::fillers::FillProvider};
use alloy_provider::{
    RootProvider,
    fillers::{BlobGasFiller, ChainIdFiller, GasFiller, JoinFill, NonceFiller, WalletFiller},
};

// Type alias for the complex provider type used across handlers
pub type CounterProvider = FillProvider<
    JoinFill<
        JoinFill<
            alloy_provider::Identity,
            JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
        >,
        WalletFiller<EthereumWallet>,
    >,
    RootProvider,
>;

// Type alias for view-only provider (without wallet)
pub type ViewOnlyProvider = FillProvider<
    JoinFill<
        alloy_provider::Identity,
        JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
    >,
    RootProvider,
>;

/// Shared trait for creators that can generate payloads and round numbers
pub trait TaskCreator: Send + Sync {
    /// Get the current payload and round number
    async fn get_payload_and_round(&self) -> anyhow::Result<(Vec<u8>, u64)>;
}
enum TaskCreatorEnum {
    Creator(Creator),
    ListeningCreator(Arc<ListeningCreator>),
    GasKillerCreator(Arc<GasKillerCreator>),
}

impl TaskCreator for TaskCreatorEnum {
    async fn get_payload_and_round(&self) -> anyhow::Result<(Vec<u8>, u64)> {
        match self {
            TaskCreatorEnum::Creator(creator) => creator
                .get_payload_and_round()
                .await
                .map_err(|e| anyhow::anyhow!("Creator error: {}", e)),
            TaskCreatorEnum::ListeningCreator(listening_creator) => listening_creator
                .get_payload_and_round()
                .await
                .map_err(|e| anyhow::anyhow!("ListeningCreator error: {}", e)),
            TaskCreatorEnum::GasKillerCreator(gas_killer_creator) => gas_killer_creator
                .get_payload_and_round()
                .await
                .map_err(|e| anyhow::anyhow!("GasKillerCreator error: {}", e)),
        }
    }
}

pub mod creator;
pub mod executor;
pub mod listening_creator;
pub mod orchestrator;

pub use orchestrator::Orchestrator;

use crate::handlers::{creator::Creator, listening_creator::ListeningCreator};
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

/// Task details including target contract and function information
pub struct TaskDetails {
    pub payload: Vec<u8>,
    pub round: u64,
    pub target_contract: String,
    pub target_function: String,
    pub function_params: Vec<u8>,
}

/// Shared trait for creators that can generate payloads and round numbers
pub trait TaskCreator: Send + Sync {
    /// Get the current task details including payload, round, and target information
    async fn get_task_details(&self) -> anyhow::Result<TaskDetails>;
}
enum TaskCreatorEnum {
    Creator(Creator),
    ListeningCreator(Arc<ListeningCreator>),
}

impl TaskCreator for TaskCreatorEnum {
    async fn get_task_details(&self) -> anyhow::Result<TaskDetails> {
        match self {
            TaskCreatorEnum::Creator(creator) => creator
                .get_task_details()
                .await
                .map_err(|e| anyhow::anyhow!("Creator error: {}", e)),
            TaskCreatorEnum::ListeningCreator(listening_creator) => listening_creator
                .get_task_details()
                .await
                .map_err(|e| anyhow::anyhow!("ListeningCreator error: {}", e)),
        }
    }
}

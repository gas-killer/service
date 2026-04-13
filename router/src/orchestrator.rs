use crate::executor::GasKillerHandler;
use crate::factories::SimpleWalletProvider;
use crate::{GasKillerCreatorType, GasKillerValidator};
use commonware_avs_router::executor::bls::BlsEigenlayerExecutor;
use commonware_avs_router::orchestrator::types::Orchestrator;

/// Type alias for counter orchestrator using the generic framework
pub type GasKillerOrchestrator<C> = Orchestrator<
    GasKillerCreatorType,
    BlsEigenlayerExecutor<GasKillerHandler<SimpleWalletProvider>>,
    GasKillerValidator,
    C,
>;

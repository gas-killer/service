use crate::{GasKillerCreatorType, GasKillerHandler, GasKillerValidator};
use commonware_avs_router::executor::bls::BlsEigenlayerExecutor;
use commonware_avs_router::orchestrator::types::Orchestrator;

/// Type alias for counter orchestrator using the generic framework
pub type GasKillerOrchestrator<C> = Orchestrator<
    GasKillerCreatorType,
    BlsEigenlayerExecutor<GasKillerHandler>,
    GasKillerValidator,
    C,
>;

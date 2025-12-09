use crate::executor::BlsEigenlayerExecutor;
use crate::orchestrator::generic::Orchestrator;
use crate::usecases::gas_killer::{GasKillerCreatorType, GasKillerHandler, GasKillerValidator};

/// Type alias for counter orchestrator using the generic framework
#[allow(dead_code)]
pub type GasKillerOrchestrator<C> = Orchestrator<
    GasKillerCreatorType,
    BlsEigenlayerExecutor<GasKillerHandler>,
    GasKillerValidator,
    C,
>;

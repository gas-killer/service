use super::creator::GasKillerCreator;
use crate::orchestrator::generic::Orchestrator;
use crate::usecases::gas_killer::executor::GasKillerExecutor;
use crate::usecases::gas_killer::validator::GasKillerValidator;

/// Type alias for the Gas Killer Orchestrator
pub type GasKillerOrchestrator<C> =
    Orchestrator<GasKillerCreator, GasKillerExecutor, GasKillerValidator, C>;

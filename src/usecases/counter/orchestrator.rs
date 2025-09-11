use crate::executor::BlsEigenlayerExecutor;
use crate::orchestrator::generic::Orchestrator;
use crate::usecases::counter::{CounterCreatorType, CounterHandler, CounterValidator};

use super::creator::CounterTaskData;

/// Type alias for counter orchestrator using the generic framework
pub type CounterOrchestrator<C> = Orchestrator<
    CounterCreatorType,
    BlsEigenlayerExecutor<CounterHandler, CounterTaskData>,
    CounterValidator,
    C,
>;

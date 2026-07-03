// Gas killer router: verifier-only certificate collector, task sequencer, and
// on-chain submitter around the commonware aggregation engine.
pub mod automaton;
pub mod error;
pub mod executor;
pub mod factories;
pub mod ingress;
pub mod metrics;
pub mod reporter;
pub mod sequencer;
pub mod submitter;

// Re-export task_data from common crate
pub mod task_data {
    pub use gas_killer_common::task_data::GasKillerTaskData;
}

// Re-export validator from common crate
pub mod validator {
    pub use gas_killer_common::validator::*;
}

// Re-export main types for easy access
pub use error::{ApiError, ApiErrorBody, ApiErrorEnvelope, ApiJson, ErrorCode};
pub use executor::{ExecutionResult, GasKillerHandler};
pub use gas_killer_common::GasKillerTaskData;
pub use gas_killer_common::GasKillerValidator;
pub use ingress::{GasKillerTaskRequest, GasKillerTaskRequestBody, ValidationError};

//! Gas Killer executor module for optimized transaction execution
//!
//! This module implements the Gas Killer execution logic for processing
//! validated execution packages with state updates and aggregated signatures.

pub mod executor;
pub mod handlers;
pub mod traits;
pub mod types;

#[cfg(test)]
mod tests;

// Re-export main types
pub use executor::GasKillerExecutor;
pub use handlers::{DefaultGasKillerHandler, DefaultGasPriceOracle};
pub use traits::{GasKillerContractHandler, GasKillerExecutorTrait, GasPriceOracle};
pub use types::{
    ExecutionPackage, ExecutionStatus, GasKillerConfig, GasKillerExecutionResult, GasPriceConfig,
    StateUpdate,
};

//! Shared services module
//!
//! This module contains services that are shared across multiple components
//! of the gas-killer system, such as the creator and validator.

pub mod gas_analyzer;

pub use gas_analyzer::GasAnalyzer;

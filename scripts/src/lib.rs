//! Shared support code for the `scripts` crate's binaries.
//!
//! The binaries themselves live at the crate root (`scripts/*.rs`) and are declared as
//! `[[bin]]` targets; anything two of them need lives here instead of being duplicated.

/// `alloy::sol!` contract bindings generated from the vendored artifacts under
/// `src/bindings/abis/`.
pub mod bindings;

/// Resolving the deployed Gas Killer target out of the AVS deployment JSON.
pub mod deployment;

/// Polling and submission of router-rendered `verifyAndUpdate` payloads.
pub mod task_payload;

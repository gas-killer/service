//! Shared support code for the `scripts` crate's binaries: `alloy::sol!` contract
//! bindings generated from the vendored artifacts under `bindings/abis/`, plus
//! hand-written helpers the binaries have in common.

/// Polling and submission of router-rendered `verifyAndUpdate` payloads.
pub mod task_payload;

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    missing_docs,
    dead_code
)]
pub mod arraysummation;

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    missing_docs,
    dead_code
)]
pub mod arraysummationfactory;

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    missing_docs,
    dead_code
)]
pub mod schnorrstakeregistry;

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    missing_docs,
    dead_code
)]
pub mod schnorrarraysummationfactory;

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    missing_docs,
    dead_code
)]
pub mod reentrantcheckpointfactory;

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    missing_docs,
    dead_code
)]
pub mod reentrantcheckpoint;

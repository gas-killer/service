//! `alloy::sol!` contract bindings, generated at compile time from the Foundry artifacts
//! vendored under `abis/`.
//!
//! Every module here is generated code, hence the blanket `allow`s: the macro emits Solidity
//! naming (`sumCall`, `_0`) that Rust lint conventions would otherwise reject, and each
//! binding exposes the contract's whole ABI whether or not a binary calls all of it.

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
pub mod reentrantcheckpoint;

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    missing_docs,
    dead_code
)]
pub mod onchainlife;

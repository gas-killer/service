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

#[cfg(test)]
mod tests {
    use alloy::sol_types::SolCall;

    /// The router declares `nextPossibleMutationBlock` by hand
    /// (`gas_killer_common::bindings::schnorrstakeregistry`) instead of vendoring the registry's ABI
    /// artifact a second time. This crate does carry that artifact, because it deploys the registry,
    /// so comparing the two selectors pins the hand-written declaration against the real ABI.
    ///
    /// Without this, an upstream rename would leave the router's call reverting at runtime and
    /// silently falling back to unclamped payload validity — a warning in the logs rather than a
    /// failure.
    #[test]
    fn router_registry_declaration_matches_the_deploy_artifact() {
        assert_eq!(
            gas_killer_common::bindings::schnorrstakeregistry::ISchnorrStakeRegistry::nextPossibleMutationBlockCall::SELECTOR,
            crate::bindings::schnorrstakeregistry::SchnorrStakeRegistry::nextPossibleMutationBlockCall::SELECTOR,
        );
    }
}

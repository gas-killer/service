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
            crate::schnorrstakeregistry::SchnorrStakeRegistry::nextPossibleMutationBlockCall::SELECTOR,
        );
    }
}

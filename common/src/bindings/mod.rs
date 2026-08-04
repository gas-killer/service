use alloy_primitives::FixedBytes;

pub use commonware_avs_bindings::{ReadOnlyProvider, WalletProvider};
pub use commonware_avs_bindings::{bls_apk_registry, bls_sig_check_operator_state_retriever};

/// ERC-165 interface ID for the GasKiller interface. A target contract must
/// report support for this ID before the router submits `verifyAndUpdate`.
///
/// `IGasKillerSDK` declares exactly one function, so its interface ID is the
/// `verifyAndUpdate` selector — see the test below, which ties this constant to the
/// vendored ABI so refreshing the artifact without updating it cannot pass silently.
pub const GAS_KILLER_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0x2a, 0x79, 0xd7, 0xba]);

/// ERC-165 interface ID for the SchnorrGasKiller interface. A target contract
/// must report support for this ID before the router submits `verifyAndUpdate`.
///
/// Same single-function derivation as [`GAS_KILLER_INTERFACE_ID`].
pub const SCHNORR_GAS_KILLER_INTERFACE_ID: FixedBytes<4> =
    FixedBytes::new([0xb9, 0x5d, 0x5f, 0x32]);

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    missing_docs,
    dead_code
)]
pub mod gaskillersdk;

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    missing_docs,
    dead_code
)]
pub mod schnorrgaskillersdk;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;

    /// The router's ERC-165 preflight hard-fails every submission when the probed ID does not
    /// match what the target reports, so these constants must track the vendored ABI. Both
    /// interfaces declare exactly one function, making the ID the `verifyAndUpdate` selector —
    /// which the generated bindings expose, so a refreshed artifact that moves the selector
    /// fails here instead of in a live e2e run.
    #[test]
    fn interface_ids_match_the_vendored_verify_and_update_selectors() {
        assert_eq!(
            GAS_KILLER_INTERFACE_ID,
            gaskillersdk::GasKillerSDK::verifyAndUpdateCall::SELECTOR
        );
        assert_eq!(
            SCHNORR_GAS_KILLER_INTERFACE_ID,
            schnorrgaskillersdk::SchnorrGasKillerSDK::verifyAndUpdateCall::SELECTOR
        );
    }
}

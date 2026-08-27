use alloy_primitives::FixedBytes;

pub use commonware_avs_bindings::{ReadOnlyProvider, WalletProvider};
pub use commonware_avs_bindings::{bls_apk_registry, bls_sig_check_operator_state_retriever};

alloy::sol! {
    /// The one getter the router needs from an `IBLSSignatureChecker`: the registry coordinator it
    /// verifies operator state against.
    ///
    /// A target's checker and the router's own operator set must be the same deployment, or the
    /// quorum APK the router computes will not match the one the contract checks and every
    /// submission reverts `InvalidQuorumApkHash`. Reading this back is how that pairing is
    /// confirmed rather than assumed.
    #[sol(rpc)]
    interface IBLSSignatureCheckerRegistry {
        function registryCoordinator() external view returns (address);
    }
}

/// ERC-165 interface ID for the GasKiller interface. A target contract must
/// report support for this ID before the router submits `verifyAndUpdate`.
pub const GAS_KILLER_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0x93, 0xde, 0x45, 0x31]);

/// ERC-165 interface ID for the SchnorrGasKiller interface. A target contract
/// must report support for this ID before the router submits `verifyAndUpdate`.
pub const SCHNORR_GAS_KILLER_INTERFACE_ID: FixedBytes<4> =
    FixedBytes::new([0x82, 0xb3, 0x5a, 0x01]);

/// The compiled ABI the [`gaskillersdk`] bindings are generated from, exposed so callers can
/// enumerate what the SDK declares rather than restating it. Consumers that map SDK errors to
/// their own data check themselves against this, so an error added upstream cannot go unnoticed.
pub const GAS_KILLER_SDK_ABI: &str = include_str!("abis/GasKillerSDK.json");

/// Schnorr twin of [`GAS_KILLER_SDK_ABI`], for the [`schnorrgaskillersdk`] bindings.
pub const SCHNORR_GAS_KILLER_SDK_ABI: &str = include_str!("abis/SchnorrGasKillerSDK.json");

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

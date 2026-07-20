use alloy_primitives::FixedBytes;

pub use commonware_avs_bindings::{ReadOnlyProvider, WalletProvider};
pub use commonware_avs_bindings::{bls_apk_registry, bls_sig_check_operator_state_retriever};

/// ERC-165 interface ID for the GasKiller interface. A target contract must
/// report support for this ID before the router submits `verifyAndUpdate`.
pub const GAS_KILLER_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0x93, 0xde, 0x45, 0x31]);

/// ERC-165 interface ID for the SchnorrGasKiller interface. A target contract
/// must report support for this ID before the router submits `verifyAndUpdate`.
pub const SCHNORR_GAS_KILLER_INTERFACE_ID: FixedBytes<4> =
    FixedBytes::new([0x82, 0xb3, 0x5a, 0x01]);

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

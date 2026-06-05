use alloy_primitives::FixedBytes;

// Re-export common bindings from commonware-avs-router
pub use commonware_avs_router::bindings::{
    ReadOnlyProvider, WalletProvider, bls_apk_registry, bls_sig_check_operator_state_retriever,
};

/// ERC-165 interface ID for the GasKiller interface. A target contract must
/// report support for this ID before the router submits `verifyAndUpdate`.
pub const GAS_KILLER_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0x93, 0xde, 0x45, 0x31]);

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

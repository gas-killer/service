use alloy_primitives::FixedBytes;

pub use commonware_avs_bindings::{ReadOnlyProvider, WalletProvider};

/// ERC-165 interface ID for the Gas Killer interface. A target contract must report support for
/// this ID before the router submits `verifyAndUpdate`.
pub const GAS_KILLER_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0x82, 0xb3, 0x5a, 0x01]);

/// ERC-165 interface ID for `IGasKillerNested`. A root reporting it can settle a nested tree
/// through `verifyAndUpdateTree`, and a callee reporting it can apply its own frame.
pub const GAS_KILLER_NESTED_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0x78, 0x48, 0x18, 0x74]);

/// The compiled ABI the [`gaskillersdk`] bindings are generated from, exposed so callers can
/// enumerate what the SDK declares rather than restating it. Consumers that map SDK errors to
/// their own data check themselves against this, so an error added upstream cannot go unnoticed.
pub const GAS_KILLER_SDK_ABI: &str = include_str!("abis/GasKillerSDK.json");

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
pub mod schnorrstakeregistry;

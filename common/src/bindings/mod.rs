use alloy_primitives::FixedBytes;

pub use commonware_avs_bindings::{ReadOnlyProvider, WalletProvider};

/// ERC-165 interface ID for `IGasKillerSDK` v0.2.0: the selector of `verifyAndUpdate`, which no
/// longer takes the digest. A target must report it before the router submits `verifyAndUpdate`.
/// It also means the target's digest binds the chain id and the value. SDK v0.1.0 targets report
/// `0x82b35a01` and aren't routed.
pub const GAS_KILLER_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0x57, 0xa9, 0xb0, 0x02]);

/// ERC-165 interface ID for `IGasKillerNested`. A root reporting it can settle a nested tree
/// through `verifyAndUpdateTree`, and a callee reporting it can apply its own frame.
pub const GAS_KILLER_NESTED_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0x78, 0x48, 0x18, 0x74]);

/// The compiled ABI the [`gaskillersdk`] bindings are generated from, exposed so callers can
/// enumerate what the SDK declares rather than restating it. Consumers that map SDK errors to
/// their own data check themselves against this, so an error added upstream cannot go unnoticed.
/// Refresh it from solidity-sdk's `GasKillerNestedSDK` artifact, a superset of `GasKillerSDK`'s:
/// the base artifact lacks the tree entrypoints and errors the router calls and decodes.
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

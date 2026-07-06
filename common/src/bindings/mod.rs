use alloy::{network::Ethereum, network::EthereumWallet, providers::fillers::FillProvider};
use alloy_primitives::FixedBytes;
use alloy_provider::{
    RootProvider,
    fillers::{BlobGasFiller, ChainIdFiller, GasFiller, JoinFill, NonceFiller, WalletFiller},
};

/// ERC-165 interface ID for the GasKiller interface. A target contract must
/// report support for this ID before the router submits `verifyAndUpdate`.
///
/// `type(IGasKillerSDK).interfaceId` — the selector of
/// `verifyAndUpdate(bytes32,uint32,bytes,uint256,bytes4,address[],bytes[])`
/// (the ECDSAStakeRegistry variant), the interface's only function.
pub const GAS_KILLER_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0xeb, 0x9e, 0xcb, 0x2e]);

/// ERC-165 interface ID for the aggregate-Schnorr GasKiller interface. A target
/// contract must report support for this ID before the router submits the schnorr
/// `verifyAndUpdate`.
///
/// `type(ISchnorrGasKillerSDK).interfaceId` — the selector of
/// `verifyAndUpdate(bytes32,uint32,bytes,uint256,bytes4,uint256,address,address[])`
/// (the SchnorrStakeRegistry variant), the interface's only function.
pub const SCHNORR_GAS_KILLER_INTERFACE_ID: FixedBytes<4> =
    FixedBytes::new([0x82, 0xb3, 0x5a, 0x01]);

/// Provider with wallet capabilities (for transactions).
pub type WalletProvider = FillProvider<
    JoinFill<
        JoinFill<
            alloy_provider::Identity,
            JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
        >,
        WalletFiller<EthereumWallet>,
    >,
    RootProvider,
    Ethereum,
>;

/// Read-only provider (without wallet, for queries).
pub type ReadOnlyProvider = FillProvider<
    JoinFill<
        alloy_provider::Identity,
        JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
    >,
    RootProvider,
    Ethereum,
>;

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

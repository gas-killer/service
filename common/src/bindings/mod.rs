use alloy::{network::Ethereum, network::EthereumWallet, providers::fillers::FillProvider};
use alloy_primitives::FixedBytes;
use alloy_provider::{
    RootProvider,
    fillers::{BlobGasFiller, ChainIdFiller, GasFiller, JoinFill, NonceFiller, WalletFiller},
};

/// ERC-165 interface ID for the GasKiller interface. A target contract must
/// report support for this ID before the router submits `verifyAndUpdate`.
pub const GAS_KILLER_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0x93, 0xde, 0x45, 0x31]);

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
    clippy::too_many_arguments,
    clippy::type_complexity,
    missing_docs,
    dead_code
)]
pub mod bls_apk_registry;

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    clippy::too_many_arguments,
    clippy::type_complexity,
    missing_docs,
    dead_code
)]
pub mod bls_sig_check_operator_state_retriever;

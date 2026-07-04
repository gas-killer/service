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
/// `verifyAndUpdate(bytes32,uint32,bytes,uint256,bytes32,address,bytes,address[],bytes[])`
/// (the slashable ECDSAStakeRegistry variant), the interface's only function. Verified on-chain.
pub const GAS_KILLER_INTERFACE_ID: FixedBytes<4> = FixedBytes::new([0x2e, 0x04, 0xb7, 0xc5]);

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

#[cfg(test)]
mod tests {
    use super::GAS_KILLER_INTERFACE_ID;
    use super::gaskillersdk::GasKillerSDK;
    use alloy::sol_types::SolCall;

    /// `type(IGasKillerSDK).interfaceId` equals the `verifyAndUpdate` selector (its only
    /// function). Tie the constant to the ABI binding so a signature change can't silently
    /// leave the router checking a stale interface id (which fails the ERC-165 preflight and
    /// blocks every submission).
    #[test]
    fn interface_id_matches_verify_and_update_selector() {
        assert_eq!(
            GAS_KILLER_INTERFACE_ID.0,
            GasKillerSDK::verifyAndUpdateCall::SELECTOR,
            "GAS_KILLER_INTERFACE_ID is stale; regenerate it as type(IGasKillerSDK).interfaceId"
        );
    }
}

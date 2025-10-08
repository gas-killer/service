#![allow(dead_code)]
use alloy::{primitives::U256, sol_types::SolValue};
use anyhow::Result;

use crate::bindings::{WalletProvider as AlloyProvider, gaskillersdk::GasKillerSDK};

/// Concrete provider for the gas killer usecase
pub struct GasKillerProvider {
    gaskiller: GasKillerSDK::GasKillerSDKInstance<(), AlloyProvider>,
}

impl GasKillerProvider {
    pub fn new(gas_killer_address: alloy::primitives::Address, provider: AlloyProvider) -> Self {
        let gaskiller = GasKillerSDK::new(gas_killer_address, provider);
        Self { gaskiller }
    }

    /// Reads the current state transition count on-chain
    pub async fn get_state_transition_count(&self) -> Result<u64> {
        let current = self.gaskiller.stateTransitionCount().call().await?;
        Ok(current.count.to::<u64>())
    }

    /// Encodes the state transition count into ABI-encoded bytes for hashing/signing
    pub fn encode_state_transition_count(&self, count: U256) -> Vec<u8> {
        count.abi_encode()
    }
}

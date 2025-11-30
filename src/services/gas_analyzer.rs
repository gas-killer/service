//! Gas Analyzer Service
//!
//! This service wraps gas-analyzer-rs and provides a clean interface for
//! computing storage updates from transaction parameters. It is designed
//! to be shared between the creator (for computing storage_updates) and
//! the validator (for verifying storage_updates).

use alloy::primitives::{Address, Bytes, U256};
use alloy::rpc::types::eth::TransactionRequest as AlloyTransactionRequest;
use anyhow::Result;
use std::env;
use tracing::debug;
use url::Url;

use gas_analyzer_rs::{call_to_encoded_state_updates_with_gas_estimate, gk::GasKillerDefault};

/// Result of gas analysis containing storage updates and gas information
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    /// The storage updates extracted from the transaction
    pub storage_updates: Vec<u8>,
    /// The gas estimate from gas-analyzer-rs
    #[allow(dead_code)]
    pub gas_estimate: u64,
}

/// Service for analyzing transactions and computing storage updates
///
/// This service wraps gas-analyzer-rs and provides a clean interface
/// for computing storage updates from transaction parameters. It spawns
/// an Anvil fork process to simulate transaction execution.
#[derive(Clone)]
pub struct GasAnalyzer {
    /// RPC URL for forking blockchain state
    rpc_url: String,
}

impl GasAnalyzer {
    /// Creates a new GasAnalyzer with the given RPC URL
    pub fn new(rpc_url: String) -> Self {
        Self { rpc_url }
    }

    /// Creates a GasAnalyzer from environment variables
    ///
    /// Checks RPC_URL first, then falls back to a default Holesky endpoint
    pub fn from_env() -> Self {
        let rpc_url = env::var("RPC_URL")
            .unwrap_or_else(|_| "https://ethereum-holesky.publicnode.com".to_string());
        Self::new(rpc_url)
    }

    /// Returns the configured RPC URL
    #[allow(dead_code)]
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// Performs the core gas analysis using gas-analyzer-rs
    ///
    /// This method:
    /// 1. Forks the blockchain state at the configured RPC
    /// 2. Executes the transaction in a simulated environment (Anvil)
    /// 3. Extracts storage changes and gas information
    ///
    /// # Arguments
    /// * `contract_address` - The target contract address
    /// * `call_data` - The transaction call data (function selector + parameters)
    /// * `from_address` - Optional sender address (uses default if None)
    /// * `value` - Optional ETH value to send (uses 0 if None)
    ///
    /// # Returns
    /// * `Result<AnalysisResult>` - Storage updates and gas estimate on success
    pub async fn analyze_transaction(
        &self,
        contract_address: Address,
        call_data: &[u8],
        from_address: Option<Address>,
        value: Option<U256>,
    ) -> Result<AnalysisResult> {
        let rpc_url =
            Url::parse(&self.rpc_url).map_err(|e| anyhow::anyhow!("Invalid RPC URL: {}", e))?;

        debug!(
            "Analyzing transaction: contract={:?}, call_data_len={}, from={:?}, value={:?}",
            contract_address,
            call_data.len(),
            from_address,
            value
        );

        // Create transaction request for gas-analyzer-rs
        let tx_request = AlloyTransactionRequest {
            from: from_address,
            to: Some(contract_address.into()),
            input: alloy::rpc::types::TransactionInput::new(Bytes::from(call_data.to_vec())),
            value,
            gas: None,
            ..Default::default()
        };

        // Initialize GasKiller instance (spawns new Anvil process)
        let gk = GasKillerDefault::new(rpc_url.clone(), None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize GasKiller: {}", e))?;

        // Get actual storage updates from gas-analyzer-rs
        let (encoded_updates, gas_estimate, _) =
            call_to_encoded_state_updates_with_gas_estimate(rpc_url, tx_request, gk)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to compute state updates: {}", e))?;

        debug!(
            "Analysis complete: storage_updates_len={}, gas_estimate={}",
            encoded_updates.len(),
            gas_estimate
        );

        Ok(AnalysisResult {
            storage_updates: encoded_updates.to_vec(),
            gas_estimate,
        })
    }
}

impl Default for GasAnalyzer {
    fn default() -> Self {
        Self::from_env()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_with_rpc_url() {
        let analyzer = GasAnalyzer::new("http://localhost:8545".to_string());
        assert_eq!(analyzer.rpc_url(), "http://localhost:8545");
    }

    #[test]
    fn test_analyzer_is_clone() {
        let analyzer = GasAnalyzer::new("http://localhost:8545".to_string());
        let cloned = analyzer.clone();
        assert_eq!(analyzer.rpc_url(), cloned.rpc_url());
    }

    #[test]
    fn test_default_uses_from_env() {
        let analyzer = GasAnalyzer::default();
        // Should have some RPC URL (either from env or default)
        assert!(!analyzer.rpc_url().is_empty());
    }
}

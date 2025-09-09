use anyhow::Result;
use async_trait::async_trait;
use alloy_primitives::{Address, Bytes, FixedBytes, U256};

use super::types::{ExecutionPackage, GasKillerExecutionResult, GasPriceConfig};

/// Main trait for Gas Killer executor implementation
#[async_trait]
pub trait GasKillerExecutorTrait: Send + Sync {
    /// Execute a Gas Killer optimized transaction
    async fn execute_gas_killer_transaction(
        &mut self,
        package: ExecutionPackage,
    ) -> Result<GasKillerExecutionResult>;
    
    /// Verify that a contract implements the Gas Killer SDK interface
    async fn verify_gas_killer_interface(
        &self,
        contract_address: Address,
    ) -> Result<bool>;
    
    /// Prepare optimized calldata with state updates
    fn prepare_calldata(
        &self,
        target_method: &str,
        params: &Bytes,
        package: &ExecutionPackage,
    ) -> Result<Bytes>;
    
    /// Estimate gas for the transaction with state preloading
    async fn estimate_gas_with_state(
        &self,
        contract_address: Address,
        calldata: &Bytes,
    ) -> Result<U256>;
    
    /// Calculate optimal gas price based on network conditions
    async fn calculate_optimal_gas_price(
        &self,
        priority: u8,
    ) -> Result<GasPriceConfig>;
    
    /// Submit transaction to the network
    async fn submit_transaction(
        &self,
        contract_address: Address,
        calldata: Bytes,
        gas_limit: U256,
        gas_price_config: GasPriceConfig,
    ) -> Result<FixedBytes<32>>;
    
    /// Monitor transaction until confirmation or timeout
    async fn monitor_transaction(
        &self,
        tx_hash: FixedBytes<32>,
    ) -> Result<(u64, U256)>; // Returns (block_number, gas_used)
    
    /// Handle transaction replacement for stuck transactions
    async fn replace_transaction(
        &self,
        original_tx_hash: FixedBytes<32>,
        gas_price_increase_percent: u8,
    ) -> Result<FixedBytes<32>>;
}

/// Trait for handling Gas Killer SDK contract interactions
#[async_trait]
pub trait GasKillerContractHandler: Send + Sync {
    /// Execute the optimized transaction with state updates
    async fn execute_with_state_updates(
        &self,
        contract_address: Address,
        calldata: Bytes,
    ) -> Result<FixedBytes<32>>;
    
    /// Verify aggregated signatures on-chain
    async fn verify_aggregated_signature(
        &self,
        contract_address: Address,
        message_hash: FixedBytes<32>,
        aggregated_signature: Bytes,
        operator_addresses: Vec<Address>,
    ) -> Result<bool>;
    
    /// Check if contract supports Gas Killer interface
    async fn supports_interface(
        &self,
        contract_address: Address,
        interface_id: FixedBytes<4>,
    ) -> Result<bool>;
}

/// Trait for gas price optimization strategies
#[async_trait]
pub trait GasPriceOracle: Send + Sync {
    /// Get current base fee from the network
    async fn get_base_fee(&self) -> Result<U256>;
    
    /// Get suggested priority fee based on network congestion
    async fn get_priority_fee(&self, priority: u8) -> Result<U256>;
    
    /// Get maximum fee per gas cap
    async fn get_max_fee_per_gas(&self) -> Result<U256>;
    
    /// Predict gas price for future blocks
    async fn predict_gas_price(&self, blocks_ahead: u64) -> Result<U256>;
}
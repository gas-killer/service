use anyhow::{anyhow, Result};
use async_trait::async_trait;
use alloy::providers::Provider;
use alloy::rpc::types::{BlockNumberOrTag, BlockId};
use alloy_primitives::{Address, Bytes, FixedBytes, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use tracing::{debug, info};

use super::traits::{GasKillerContractHandler, GasPriceOracle};

// Re-use the interface definition
sol! {
    interface IGasKiller {
        function supportsInterface(bytes4 interfaceId) external view returns (bool);
        
        function executeWithStateUpdates(
            bytes calldata targetCalldata,
            bytes32[] calldata stateKeys,
            bytes32[] calldata stateValues,
            bytes calldata aggregatedSignature,
            address[] calldata operators
        ) external returns (bool success, bytes memory returnData);
        
        function verifyAggregatedSignature(
            bytes32 messageHash,
            bytes calldata aggregatedSignature,
            address[] calldata operators
        ) external view returns (bool valid);
    }
}

/// Default implementation of Gas Killer contract handler
pub struct DefaultGasKillerHandler {
    provider: Box<dyn Provider>,
}

impl DefaultGasKillerHandler {
    pub fn new(provider: Box<dyn Provider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl GasKillerContractHandler for DefaultGasKillerHandler {
    async fn execute_with_state_updates(
        &self,
        contract_address: Address,
        calldata: Bytes,
    ) -> Result<FixedBytes<32>> {
        // In a real implementation, this would submit the transaction
        // For now, return a placeholder
        debug!(
            contract = ?contract_address,
            calldata_len = calldata.len(),
            "Would execute state updates on contract"
        );
        
        Ok(FixedBytes::default())
    }
    
    async fn verify_aggregated_signature(
        &self,
        contract_address: Address,
        message_hash: FixedBytes<32>,
        aggregated_signature: Bytes,
        operator_addresses: Vec<Address>,
    ) -> Result<bool> {
        let call = IGasKiller::verifyAggregatedSignatureCall {
            messageHash: message_hash,
            aggregatedSignature: aggregated_signature,
            operators: operator_addresses,
        };
        
        // This would make the actual call to the contract
        // For now, return true as placeholder
        debug!(
            contract = ?contract_address,
            message_hash = ?message_hash,
            "Would verify aggregated signature"
        );
        
        Ok(true)
    }
    
    async fn supports_interface(
        &self,
        contract_address: Address,
        interface_id: FixedBytes<4>,
    ) -> Result<bool> {
        let call = IGasKiller::supportsInterfaceCall {
            interfaceId: interface_id,
        };
        
        // This would make the actual call to check interface support
        // For now, return true as placeholder
        debug!(
            contract = ?contract_address,
            interface_id = ?interface_id,
            "Would check interface support"
        );
        
        Ok(true)
    }
}

/// Default implementation of gas price oracle
pub struct DefaultGasPriceOracle {
    provider: Box<dyn Provider>,
}

impl DefaultGasPriceOracle {
    pub fn new(provider: Box<dyn Provider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl GasPriceOracle for DefaultGasPriceOracle {
    async fn get_base_fee(&self) -> Result<U256> {
        let block = self.provider
            .get_block(BlockId::from(BlockNumberOrTag::Latest))
            .await
            .map_err(|e| anyhow!("Failed to get latest block: {}", e))?
            .ok_or_else(|| anyhow!("Latest block not found"))?;
        
        let base_fee = block.header.base_fee_per_gas
            .ok_or_else(|| anyhow!("No base fee in block (pre-EIP-1559?)"))?;
        
        Ok(U256::from(base_fee))
    }
    
    async fn get_priority_fee(&self, priority: u8) -> Result<U256> {
        // Base priority fee in gwei
        let base_priority = U256::from(2_000_000_000u64); // 2 gwei
        
        // Apply priority multiplier
        let multiplier = match priority {
            0 => 100,  // Normal: 2 gwei
            1 => 150,  // Medium: 3 gwei
            2 => 250,  // High: 5 gwei
            _ => 100,
        };
        
        Ok(base_priority * U256::from(multiplier) / U256::from(100))
    }
    
    async fn get_max_fee_per_gas(&self) -> Result<U256> {
        // Get base fee and add a generous cap
        let base_fee = self.get_base_fee().await?;
        
        // Cap at 3x base fee + 10 gwei priority
        let max_priority = U256::from(10_000_000_000u64); // 10 gwei
        Ok(base_fee * U256::from(3) + max_priority)
    }
    
    async fn predict_gas_price(&self, blocks_ahead: u64) -> Result<U256> {
        // Simple prediction: current base fee with small increase per block
        let current_base = self.get_base_fee().await?;
        
        // Assume 12.5% max increase per block (EIP-1559)
        let increase_factor = U256::from(1125); // 112.5%
        let base_factor = U256::from(1000);    // 100%
        
        let mut predicted = current_base;
        for _ in 0..blocks_ahead.min(10) {
            predicted = predicted * increase_factor / base_factor;
        }
        
        Ok(predicted)
    }
}

/// Mock implementation for testing
#[cfg(test)]
pub struct MockGasKillerHandler;

#[cfg(test)]
#[async_trait]
impl GasKillerContractHandler for MockGasKillerHandler {
    async fn execute_with_state_updates(
        &self,
        _contract_address: Address,
        _calldata: Bytes,
    ) -> Result<FixedBytes<32>> {
        Ok(FixedBytes::from([1u8; 32]))
    }
    
    async fn verify_aggregated_signature(
        &self,
        _contract_address: Address,
        _message_hash: FixedBytes<32>,
        _aggregated_signature: Bytes,
        _operator_addresses: Vec<Address>,
    ) -> Result<bool> {
        Ok(true)
    }
    
    async fn supports_interface(
        &self,
        _contract_address: Address,
        _interface_id: FixedBytes<4>,
    ) -> Result<bool> {
        Ok(true)
    }
}

#[cfg(test)]
pub struct MockGasPriceOracle;

#[cfg(test)]
#[async_trait]
impl GasPriceOracle for MockGasPriceOracle {
    async fn get_base_fee(&self) -> Result<U256> {
        Ok(U256::from(30_000_000_000u64)) // 30 gwei
    }
    
    async fn get_priority_fee(&self, priority: u8) -> Result<U256> {
        Ok(U256::from(priority as u64 + 1) * U256::from(1_000_000_000u64))
    }
    
    async fn get_max_fee_per_gas(&self) -> Result<U256> {
        Ok(U256::from(100_000_000_000u64)) // 100 gwei
    }
    
    async fn predict_gas_price(&self, _blocks_ahead: u64) -> Result<U256> {
        Ok(U256::from(35_000_000_000u64)) // 35 gwei
    }
}
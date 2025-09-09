use alloy_primitives::{Address, Bytes, FixedBytes, U256};
use bn254::Signature;
use serde::{Deserialize, Serialize};

/// Execution package containing all data needed for Gas Killer transaction execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPackage {
    /// Unique identifier for this task
    pub task_id: String,
    /// Target contract address that implements Gas Killer SDK
    pub target_contract: Address,
    /// Method name to call on the target contract
    pub target_method: String,
    /// Encoded parameters for the method call
    pub params: Bytes,
    /// Pre-computed state updates for gas optimization
    pub state_updates: Vec<StateUpdate>,
    /// Aggregated BLS signature from validators (serialized as bytes)
    #[serde(with = "signature_serde")]
    pub aggregated_signature: Signature,
    /// List of operator addresses that signed
    pub operator_set: Vec<Address>,
    /// Timestamp when validation occurred
    pub validation_timestamp: u64,
    /// Chain ID for execution
    pub chain_id: u64,
}

/// Custom serialization for bn254::Signature
mod signature_serde {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(_sig: &Signature, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Since bn254::Signature is opaque, we'll serialize it as empty bytes for now
        // This will need to be updated once we know the actual signature structure
        // BLS signatures on BN254 are typically 96 bytes (2 * 48 bytes for G1 point)
        let bytes = Bytes::from(vec![0u8; 96]);
        bytes.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Signature, D::Error>
    where
        D: Deserializer<'de>,
    {
        let _bytes = Bytes::deserialize(deserializer)?;
        // For now, we cannot deserialize without knowing the bn254 API
        // This would need proper implementation based on the actual bn254 crate API
        Err(serde::de::Error::custom(
            "Signature deserialization not yet implemented",
        ))
    }
}

/// Represents a single state update for gas optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateUpdate {
    /// Contract address where state is updated
    pub contract: Address,
    /// Storage slot to update
    pub slot: U256,
    /// New value for the slot
    pub value: U256,
}

/// Result of Gas Killer execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasKillerExecutionResult {
    /// Task identifier
    pub task_id: String,
    /// Transaction hash on chain
    pub tx_hash: FixedBytes<32>,
    /// Block number where transaction was included
    pub block_number: u64,
    /// Actual gas used
    pub gas_used: U256,
    /// Gas saved compared to original estimate
    pub gas_saved: U256,
    /// Execution status
    pub status: ExecutionStatus,
    /// Optional error message if failed
    pub error_message: Option<String>,
}

/// Execution status enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExecutionStatus {
    Pending,
    Submitted,
    Confirmed,
    Failed,
    Reverted,
}

/// Gas price configuration
#[derive(Debug, Clone)]
pub struct GasPriceConfig {
    /// Base fee from the network
    pub base_fee: U256,
    /// Maximum priority fee per gas
    pub max_priority_fee: U256,
    /// Maximum fee per gas
    pub max_fee_per_gas: U256,
    /// Priority level (0-2, where 2 is highest)
    pub priority: u8,
}

impl GasPriceConfig {
    /// Calculate optimal gas price based on network conditions
    pub fn calculate_optimal_price(&self) -> U256 {
        // EIP-1559 calculation
        let target_fee = self.base_fee + self.max_priority_fee;

        // Apply priority multiplier
        let multiplier = match self.priority {
            0 => 100, // Normal priority
            1 => 110, // Medium priority (10% higher)
            2 => 125, // High priority (25% higher)
            _ => 100,
        };

        let optimal = target_fee * U256::from(multiplier) / U256::from(100);

        // Cap at max_fee_per_gas
        if optimal > self.max_fee_per_gas {
            self.max_fee_per_gas
        } else {
            optimal
        }
    }
}

/// Configuration for the Gas Killer executor
#[derive(Debug, Clone)]
pub struct GasKillerConfig {
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Private key for transaction signing
    pub private_key: String,
    /// Gas Killer SDK interface ID for verification
    pub gk_interface_id: FixedBytes<4>,
    /// Gas estimation buffer percentage (e.g., 110 for 10% buffer)
    pub gas_buffer_percent: u8,
    /// Maximum retries for stuck transactions
    pub max_retries: u8,
    /// Timeout for transaction confirmation (in seconds)
    pub confirmation_timeout: u64,
}

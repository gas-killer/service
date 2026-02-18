//! Task data types for the Gas Killer AVS

use alloy::primitives::FixedBytes;
use alloy_primitives::{Address, U256};
use anyhow::Result;
use bytes::{Buf, BufMut};
use commonware_codec::{EncodeSize, Read, ReadExt, Write};
use serde::{Deserialize, Serialize};

/// Task data specific to the gas killer use case
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GasKillerTaskData {
    /// Encoded storage updates to be applied
    pub storage_updates: Vec<u8>,
    /// Index of the state transition
    pub transition_index: u64,
    /// Target contract address for function call
    pub target_address: Address,
    /// Call data for the transaction (includes function selector + parameters)
    pub call_data: Vec<u8>,
    /// Sender address for the transaction
    pub from_address: Address,
    /// ETH value to send with the transaction
    pub value: U256,
    /// Block height at which storage_updates were computed (for deterministic validation)
    pub block_height: u64,
}

/// Maximum calldata size for a single EVM transaction (128 KB)
/// This is the limit enforced by Geth's txpool (txMaxSize = 4 * txSlotSize).
/// See: https://github.com/ethereum/go-ethereum/blob/master/core/txpool/legacypool/legacypool.go
pub const MAX_EVM_TX_CALLDATA_SIZE: usize = 128 * 1024;

impl GasKillerTaskData {
    /// Extracts the function selector (first 4 bytes) from call_data
    pub fn function_selector(&self) -> FixedBytes<4> {
        if self.call_data.len() >= 4 {
            FixedBytes::from_slice(&self.call_data[0..4])
        } else {
            FixedBytes::ZERO
        }
    }

    /// Validates that the task data is within EVM transaction limits.
    ///
    /// Call this before encoding to get a proper error instead of a panic.
    ///
    /// # Errors
    /// Returns an error if combined call_data + storage_updates exceeds
    /// the EVM transaction calldata limit (128 KB).
    pub fn validate(&self) -> Result<()> {
        let combined_size = self
            .call_data
            .len()
            .saturating_add(self.storage_updates.len());
        if combined_size > MAX_EVM_TX_CALLDATA_SIZE {
            return Err(anyhow::anyhow!(
                "combined call_data ({} bytes) + storage_updates ({} bytes) = {} bytes exceeds EVM transaction calldata limit ({} bytes / 128 KB)",
                self.call_data.len(),
                self.storage_updates.len(),
                combined_size,
                MAX_EVM_TX_CALLDATA_SIZE
            ));
        }
        Ok(())
    }
}

impl Default for GasKillerTaskData {
    fn default() -> Self {
        Self {
            storage_updates: vec![],
            transition_index: 0,
            target_address: Address::ZERO,
            call_data: vec![],
            from_address: Address::ZERO,
            value: U256::ZERO,
            block_height: 0,
        }
    }
}

impl Write for GasKillerTaskData {
    fn write(&self, buf: &mut impl BufMut) {
        // Note: The Write trait doesn't return Result, so we assert on invalid data.
        // Call validate() before encoding to get a proper error instead of a panic.
        let combined = self.storage_updates.len() + self.call_data.len();
        assert!(
            combined <= MAX_EVM_TX_CALLDATA_SIZE,
            "combined data size ({combined} bytes) exceeds EVM tx limit ({MAX_EVM_TX_CALLDATA_SIZE} bytes). \
             Call validate() before encoding to handle this gracefully."
        );

        // Write storage updates as length-prefixed bytes
        (self.storage_updates.len() as u32).write(buf);
        buf.put_slice(&self.storage_updates);

        // Write transition index as u64
        self.transition_index.write(buf);

        // Write target address as 20 bytes
        buf.put_slice(self.target_address.as_slice());

        // Write from address as 20 bytes
        buf.put_slice(self.from_address.as_slice());

        // Write value as 32 bytes (U256)
        buf.put_slice(&self.value.to_le_bytes::<32>());

        // Write call data as length-prefixed bytes
        (self.call_data.len() as u32).write(buf);
        buf.put_slice(&self.call_data);

        // Write block height as u64
        self.block_height.write(buf);
    }
}

impl Read for GasKillerTaskData {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _: &()) -> Result<Self, commonware_codec::Error> {
        // Read storage updates (u32 length prefix + bytes)
        let storage_updates_len = u32::read(buf)? as usize;
        if buf.remaining() < storage_updates_len {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let mut storage_updates = vec![0u8; storage_updates_len];
        buf.copy_to_slice(&mut storage_updates);

        // Read transition index (u64)
        let transition_index = u64::read(buf)?;

        // Read target address (20 bytes)
        if buf.remaining() < 20 {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let mut address_bytes = [0u8; 20];
        buf.copy_to_slice(&mut address_bytes);
        let target_address = Address::from_slice(&address_bytes);

        // Read from_address (20 bytes)
        if buf.remaining() < 20 {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let mut from_address_bytes = [0u8; 20];
        buf.copy_to_slice(&mut from_address_bytes);
        let from_address = Address::from_slice(&from_address_bytes);

        // Read value (32 bytes - U256 little-endian)
        if buf.remaining() < 32 {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let mut value_bytes = [0u8; 32];
        buf.copy_to_slice(&mut value_bytes);
        let value = U256::from_le_bytes(value_bytes);

        // Read call data (u32 length prefix + bytes)
        let call_data_len = u32::read(buf)? as usize;
        if buf.remaining() < call_data_len {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let mut call_data = vec![0u8; call_data_len];
        buf.copy_to_slice(&mut call_data);

        // Read block height (u64)
        let block_height = u64::read(buf)?;

        Ok(Self {
            storage_updates,
            transition_index,
            target_address,
            call_data,
            from_address,
            value,
            block_height,
        })
    }
}

impl EncodeSize for GasKillerTaskData {
    fn encode_size(&self) -> usize {
        // Calculate serialized size matching the Write implementation exactly
        // storage_updates: u32 length prefix (4 bytes) + raw bytes
        const U32_SIZE: usize = std::mem::size_of::<u32>(); // Length prefix for storage_updates and call_data
        const U64_SIZE: usize = std::mem::size_of::<u64>(); // transition_index and block_height
        const ADDRESS_SIZE: usize = 20; // target_address and from_address (Ethereum addresses)
        const U256_SIZE: usize = 32; // value (U256)

        U32_SIZE
            + self.storage_updates.len()
            + U64_SIZE // transition_index
            + ADDRESS_SIZE
            + ADDRESS_SIZE
            + U256_SIZE
            + U32_SIZE
            + self.call_data.len()
            + U64_SIZE // block_height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_success() {
        let task_data = GasKillerTaskData::default();
        assert!(task_data.validate().is_ok());
    }

    #[test]
    fn test_validate_with_normal_data() {
        let task_data = GasKillerTaskData {
            storage_updates: vec![0u8; 1024],
            call_data: vec![0u8; 256],
            ..Default::default()
        };
        assert!(task_data.validate().is_ok());
    }

    #[test]
    fn test_function_selector() {
        let task_data = GasKillerTaskData {
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00],
            ..Default::default()
        };
        assert_eq!(
            task_data.function_selector(),
            FixedBytes::from([0x12, 0x34, 0x56, 0x78])
        );
    }

    #[test]
    fn test_function_selector_empty() {
        let task_data = GasKillerTaskData::default();
        assert_eq!(task_data.function_selector(), FixedBytes::ZERO);
    }

    #[test]
    fn test_validate_exceeds_evm_limit() {
        let task_data = GasKillerTaskData {
            call_data: vec![0u8; MAX_EVM_TX_CALLDATA_SIZE + 1],
            ..Default::default()
        };
        let result = task_data.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds EVM transaction calldata limit")
        );
    }

    #[test]
    fn test_validate_combined_exceeds_evm_limit() {
        // Each field is under the limit individually, but combined they exceed it
        let half_limit = MAX_EVM_TX_CALLDATA_SIZE / 2 + 1;
        let task_data = GasKillerTaskData {
            storage_updates: vec![0u8; half_limit],
            call_data: vec![0u8; half_limit],
            ..Default::default()
        };
        let result = task_data.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds EVM transaction calldata limit")
        );
    }

    #[test]
    fn test_validate_at_evm_limit() {
        // Exactly at the limit should pass
        let task_data = GasKillerTaskData {
            call_data: vec![0u8; MAX_EVM_TX_CALLDATA_SIZE],
            ..Default::default()
        };
        assert!(task_data.validate().is_ok());
    }

    #[test]
    fn test_validate_combined_at_evm_limit() {
        // Combined exactly at the limit should pass
        let half_limit = MAX_EVM_TX_CALLDATA_SIZE / 2;
        let task_data = GasKillerTaskData {
            storage_updates: vec![0u8; half_limit],
            call_data: vec![0u8; half_limit],
            ..Default::default()
        };
        assert!(task_data.validate().is_ok());
    }
}

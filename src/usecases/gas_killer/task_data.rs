use alloy::primitives::FixedBytes;
use alloy_primitives::{Address, U256};
use anyhow::Result;
use bytes::{Buf, BufMut};
use commonware_codec::{EncodeSize, Read, ReadExt, Write};
use serde::{Deserialize, Serialize};

/// Task data specific to the gas killer use case
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[allow(dead_code)]
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
}

impl GasKillerTaskData {
    /// Extracts the function selector (first 4 bytes) from call_data
    pub fn function_selector(&self) -> FixedBytes<4> {
        if self.call_data.len() >= 4 {
            FixedBytes::from_slice(&self.call_data[0..4])
        } else {
            FixedBytes::ZERO
        }
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
        }
    }
}

impl Write for GasKillerTaskData {
    fn write(&self, buf: &mut impl BufMut) {
        // Write storage updates as length-prefixed bytes
        // Note: Using u32 for length prefix limits storage_updates to ~4.3GB
        // This is sufficient for gas killer use cases but could be extended to u64 if needed
        let len = self.storage_updates.len();
        if len > u32::MAX as usize {
            panic!(
                "storage_updates length ({}) exceeds u32::MAX ({})",
                len,
                u32::MAX
            );
        }
        (len as u32).write(buf);
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
        let call_data_len = self.call_data.len();
        if call_data_len > u32::MAX as usize {
            panic!(
                "call_data length ({}) exceeds u32::MAX ({})",
                call_data_len,
                u32::MAX
            );
        }
        (call_data_len as u32).write(buf);
        buf.put_slice(&self.call_data);
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

        Ok(Self {
            storage_updates,
            transition_index,
            target_address,
            call_data,
            from_address,
            value,
        })
    }
}

impl EncodeSize for GasKillerTaskData {
    fn encode_size(&self) -> usize {
        // Calculate serialized size matching the Write implementation exactly
        // storage_updates: u32 length prefix (4 bytes) + raw bytes
        const U32_SIZE: usize = std::mem::size_of::<u32>(); // Length prefix for storage_updates and call_data
        const U64_SIZE: usize = std::mem::size_of::<u64>(); // transition_index
        const ADDRESS_SIZE: usize = 20; // target_address and from_address (Ethereum addresses)
        const U256_SIZE: usize = 32; // value (U256)

        U32_SIZE
            + self.storage_updates.len()
            + U64_SIZE
            + ADDRESS_SIZE
            + ADDRESS_SIZE
            + U256_SIZE
            + U32_SIZE
            + self.call_data.len()
    }
}

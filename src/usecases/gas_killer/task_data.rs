use alloy::primitives::{Address, FixedBytes};
use anyhow::Result;
use bytes::{Buf, BufMut};
use commonware_codec::{EncodeSize, Read, ReadExt, Write};

/// Task data specific to the gas killer use case
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct GasKillerTaskData {
    /// Encoded storage updates to be applied
    pub storage_updates: Vec<u8>,
    /// Index of the state transition
    pub transition_index: u64,
    /// Target contract address for function call
    pub target_address: Address,
    /// Function selector for target call (4 bytes)
    pub target_function: FixedBytes<4>,
    /// Estimated gas savings from the analysis
    pub gas_savings: u64,
    /// Gas limit for the transaction
    pub gas_limit: u64,
    /// Call data for the transaction (includes function selector + parameters)
    pub call_data: Vec<u8>,
}

impl Default for GasKillerTaskData {
    fn default() -> Self {
        Self {
            storage_updates: vec![],
            transition_index: 0,
            target_address: Address::ZERO,
            target_function: FixedBytes::ZERO,
            gas_savings: 0,
            gas_limit: 1000000, // Default gas limit
            call_data: vec![],
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

        // Write target function selector as 4 bytes
        buf.put_slice(self.target_function.as_slice());

        // Write gas savings as u64
        self.gas_savings.write(buf);

        // Write gas limit as u64
        self.gas_limit.write(buf);

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
        // Read storage updates
        // Note: Reading u32 length prefix, matching the Write implementation
        // This limits deserialized storage_updates to ~4.3GB
        let storage_updates_len = u32::read(buf)? as usize;
        if buf.remaining() < storage_updates_len {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let mut storage_updates = vec![0u8; storage_updates_len];
        buf.copy_to_slice(&mut storage_updates);

        // Read transition index
        let transition_index = u64::read(buf)?;

        // Read target address (20 bytes)
        if buf.remaining() < 20 {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let mut address_bytes = [0u8; 20];
        buf.copy_to_slice(&mut address_bytes);
        let target_address = Address::from_slice(&address_bytes);

        // Read target function selector (4 bytes)
        if buf.remaining() < 4 {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let mut function_bytes = [0u8; 4];
        buf.copy_to_slice(&mut function_bytes);
        let target_function = FixedBytes::from_slice(&function_bytes);

        // Read gas savings
        let gas_savings = u64::read(buf)?;

        // Read gas limit
        let gas_limit = u64::read(buf)?;

        // Read call data
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
            target_function,
            gas_savings,
            gas_limit,
            call_data,
        })
    }
}

impl EncodeSize for GasKillerTaskData {
    fn encode_size(&self) -> usize {
        // Calculate serialized size matching the Write implementation exactly
        // storage_updates: u32 length prefix (4 bytes) + raw bytes
        const U32_SIZE: usize = std::mem::size_of::<u32>(); // Length prefix for storage_updates and call_data
        const U64_SIZE: usize = std::mem::size_of::<u64>(); // transition_index, gas_savings, and gas_limit
        const ADDRESS_SIZE: usize = 20; // target_address (Ethereum address)
        const FUNCTION_SELECTOR_SIZE: usize = 4; // target_function (4-byte selector)

        U32_SIZE
            + self.storage_updates.len()
            + U64_SIZE
            + ADDRESS_SIZE
            + FUNCTION_SELECTOR_SIZE
            + U64_SIZE
            + U64_SIZE
            + U32_SIZE
            + self.call_data.len()
    }
}

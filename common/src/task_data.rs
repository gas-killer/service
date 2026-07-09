//! Task data types for the Gas Killer AVS

use alloy::primitives::FixedBytes;
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes, U256};
use anyhow::Result;
use bytes::{Buf, BufMut};
use commonware_codec::{EncodeSize, Read, ReadExt, Write};
use commonware_cryptography::sha256::Digest;
use commonware_cryptography::{Hasher, Sha256};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Authenticating fields of a user's EIP-1559 transaction, carried when a task originates from
/// the JSON-RPC ingress (`eth_sendRawTransaction`) rather than the permissionless `/trigger` path.
///
/// When present, the task settles on-chain through `verifyAndUpdateWithAuth`: the contract
/// reconstructs the transaction's signing hash from these fields (plus `from_address` as the
/// signer, `value`, and `call_data` from the enclosing [`GasKillerTaskData`]) and recovers the
/// sender, so `msg.sender` attribution is bound to the signature cryptographically instead of
/// trusted. `nonce` is also the on-chain replay key.
///
/// `r`/`s` are the ECDSA signature components and `y_parity` its recovery bit; the gas fields and
/// `nonce` are part of the EIP-1559 signing preimage the contract rebuilds.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TxAuth {
    pub nonce: u64,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub gas_limit: u64,
    pub y_parity: bool,
    pub r: U256,
    pub s: U256,
}

/// Task data specific to the gas killer use case
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GasKillerTaskData {
    /// Encoded storage updates to be applied.
    ///
    /// `Bytes` is `Arc`-backed, so cloning the task data through the router hot path
    /// (creator → `current_task` → executor) is a reference-count bump.
    pub storage_updates: Bytes,
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
    /// Actual EVM chain ID (e.g. 1 = Ethereum mainnet, 100 = Gnosis, 31337 = Anvil local)
    pub chain_id: u64,
    /// Present when the task came from a user's signed transaction via the JSON-RPC ingress.
    /// Drives sender-authenticated, replay-protected settlement (`verifyAndUpdateWithAuth`) and
    /// selects the signed-message-hash preimage in [`Self::build_payload_hash`]. `None` for the
    /// permissionless `/trigger` path, which behaves exactly as before.
    pub auth: Option<TxAuth>,
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

    /// Builds the payload hash for this task from the given storage updates.
    ///
    /// Matches the on-chain `expectedHash` from `GasKillerSDK.verifyAndUpdate` (returned by
    /// `getMessageHash`):
    ///
    /// ```solidity
    /// sha256(abi.encode(transitionIndex, address(this), targetFunction, storageUpdates))
    /// ```
    ///
    /// `storage_updates` is a separate argument because the validator hashes the storage updates
    /// it recomputes via EVMSketch.
    pub fn build_payload_hash(&self, storage_updates: &[u8]) -> Digest {
        // Sender-authenticated tasks commit to the full call and signer via the expanded
        // getSignedMessageHash preimage; permissionless tasks keep the original 4-field hash.
        if self.auth.is_some() {
            return self.build_signed_payload_hash(storage_updates);
        }

        let selector = self.function_selector();

        if tracing::enabled!(tracing::Level::DEBUG) {
            // Debug: hash the full storage_updates so divergent inputs are detectable from logs.
            let mut storage_hasher = Sha256::new();
            storage_hasher.update(storage_updates);
            let storage_hash = storage_hasher.finalize();
            let storage_hash_hex: String = storage_hash
                .iter()
                .take(8)
                .map(|b| format!("{b:02x}"))
                .collect();

            debug!(
                transition_index = self.transition_index,
                target_address = %self.target_address,
                target_function = %selector,
                storage_updates_len = storage_updates.len(),
                storage_updates_hash = %storage_hash_hex,
                "build_payload_hash inputs"
            );
        }

        // Build flattened ABI encoding matching
        // abi.encode(transitionIndex, address(this), selector, storageUpdates).
        // Heads (32 bytes each)
        let head_transition = U256::from(self.transition_index).abi_encode();
        let head_address = self.target_address.abi_encode();
        let head_selector = selector.abi_encode();
        // Offset to the dynamic bytes tail: 4 words (3 static + 1 offset) = 0x80
        let head_offset = U256::from(32u64 * 4u64).abi_encode();

        // Tail for dynamic bytes: length (u256) + data + padding
        let mut tail = Vec::with_capacity(32 + storage_updates.len() + 31);
        tail.extend_from_slice(&U256::from(storage_updates.len()).abi_encode());
        tail.extend_from_slice(storage_updates);
        let pad_len = (32 - (storage_updates.len() % 32)) % 32;
        if pad_len > 0 {
            tail.extend(std::iter::repeat_n(0u8, pad_len));
        }

        // Concatenate head and tail into final payload
        let mut payload = Vec::with_capacity(32 * 4 + tail.len());
        payload.extend_from_slice(&head_transition);
        payload.extend_from_slice(&head_address);
        payload.extend_from_slice(&head_selector);
        payload.extend_from_slice(&head_offset);
        payload.extend_from_slice(&tail);

        let mut hasher = Sha256::new();
        hasher.update(&payload);
        let payload_hash = hasher.finalize();

        debug!("Built payload hash: {:?}", payload_hash);
        payload_hash
    }

    /// Builds the payload hash for a sender-authenticated task, matching the on-chain
    /// `getSignedMessageHash` (returned by `GasKillerSDK.verifyAndUpdateWithAuth`):
    ///
    /// ```solidity
    /// sha256(abi.encode(transitionIndex, address(this), signer, value, nonce, callData, storageUpdates))
    /// ```
    ///
    /// `signer` is `from_address` (the ecrecovered sender) and `nonce` comes from [`TxAuth`]. The
    /// hash commits to the full `call_data` (not just the selector) and to the sender, value, and
    /// nonce, so the operators' attestation cannot be detached from the exact authorized call. The
    /// tuple encodes identically to Solidity `abi.encode`, so `abi_encode_params` (no outer offset)
    /// is the correct alloy call.
    ///
    /// # Panics
    /// Only via [`Self::build_payload_hash`], which calls this exclusively when `auth` is `Some`.
    fn build_signed_payload_hash(&self, storage_updates: &[u8]) -> Digest {
        let nonce = self
            .auth
            .as_ref()
            .expect("build_signed_payload_hash requires auth")
            .nonce;

        let encoded = (
            U256::from(self.transition_index),
            self.target_address,
            self.from_address,
            self.value,
            U256::from(nonce),
            Bytes::from(self.call_data.clone()),
            Bytes::copy_from_slice(storage_updates),
        )
            .abi_encode_params();

        let mut hasher = Sha256::new();
        hasher.update(&encoded);
        let payload_hash = hasher.finalize();
        debug!("Built signed payload hash: {:?}", payload_hash);
        payload_hash
    }
}

impl Default for GasKillerTaskData {
    fn default() -> Self {
        Self {
            storage_updates: Bytes::new(),
            transition_index: 0,
            target_address: Address::ZERO,
            call_data: vec![],
            from_address: Address::ZERO,
            value: U256::ZERO,
            block_height: 0,
            chain_id: 0,
            auth: None,
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

        // Write chain_id as u64 (actual EVM chain ID, e.g. 1, 100, 31337)
        self.chain_id.write(buf);

        // Optional auth blob: 1-byte discriminant (0 = None, 1 = Some) then, if present, the
        // authenticating fields. All components are fixed-width, so no length prefixes are needed.
        match &self.auth {
            None => buf.put_u8(0),
            Some(a) => {
                buf.put_u8(1);
                a.nonce.write(buf);
                buf.put_slice(&a.max_priority_fee_per_gas.to_be_bytes()); // 16 bytes
                buf.put_slice(&a.max_fee_per_gas.to_be_bytes()); // 16 bytes
                a.gas_limit.write(buf);
                buf.put_u8(u8::from(a.y_parity));
                buf.put_slice(&a.r.to_be_bytes::<32>());
                buf.put_slice(&a.s.to_be_bytes::<32>());
            }
        }
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

        // Read chain_id as u64 (actual EVM chain ID)
        let chain_id = u64::read(buf)?;

        // Optional auth blob (mirrors the Write layout).
        if buf.remaining() < 1 {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let auth = match buf.get_u8() {
            0 => None,
            1 => {
                let nonce = u64::read(buf)?;
                // 16 (max_priority) + 16 (max_fee) + gas_limit(8, read below) + 1 (y_parity)
                // + 32 (r) + 32 (s); check the two u128s + y_parity + r + s up front, gas_limit
                // is validated by u64::read.
                if buf.remaining() < 16 + 16 {
                    return Err(commonware_codec::Error::EndOfBuffer);
                }
                let mut u128_buf = [0u8; 16];
                buf.copy_to_slice(&mut u128_buf);
                let max_priority_fee_per_gas = u128::from_be_bytes(u128_buf);
                buf.copy_to_slice(&mut u128_buf);
                let max_fee_per_gas = u128::from_be_bytes(u128_buf);
                let gas_limit = u64::read(buf)?;
                if buf.remaining() < 1 + 32 + 32 {
                    return Err(commonware_codec::Error::EndOfBuffer);
                }
                let y_parity = match buf.get_u8() {
                    0 => false,
                    1 => true,
                    _ => return Err(commonware_codec::Error::Invalid("TxAuth", "y_parity")),
                };
                let mut word = [0u8; 32];
                buf.copy_to_slice(&mut word);
                let r = U256::from_be_bytes(word);
                buf.copy_to_slice(&mut word);
                let s = U256::from_be_bytes(word);
                Some(TxAuth {
                    nonce,
                    max_priority_fee_per_gas,
                    max_fee_per_gas,
                    gas_limit,
                    y_parity,
                    r,
                    s,
                })
            }
            _ => return Err(commonware_codec::Error::Invalid("TxAuth", "discriminant")),
        };

        Ok(Self {
            storage_updates: storage_updates.into(),
            transition_index,
            target_address,
            call_data,
            from_address,
            value,
            block_height,
            chain_id,
            auth,
        })
    }
}

impl EncodeSize for GasKillerTaskData {
    fn encode_size(&self) -> usize {
        // Calculate serialized size matching the Write implementation exactly
        const U32_SIZE: usize = std::mem::size_of::<u32>(); // Length prefix for storage_updates and call_data
        const U64_SIZE: usize = std::mem::size_of::<u64>(); // transition_index, block_height, chain_id
        const ADDRESS_SIZE: usize = 20; // target_address and from_address (Ethereum addresses)
        const U256_SIZE: usize = 32; // value (U256)

        // Optional auth blob: 1-byte discriminant, plus fixed-width fields when present.
        // nonce(8) + max_priority(16) + max_fee(16) + gas_limit(8) + y_parity(1) + r(32) + s(32).
        const AUTH_SIZE: usize = 8 + 16 + 16 + 8 + 1 + 32 + 32;
        let auth_size = 1 + self.auth.as_ref().map_or(0, |_| AUTH_SIZE);

        U32_SIZE
            + self.storage_updates.len()
            + U64_SIZE // transition_index
            + ADDRESS_SIZE
            + ADDRESS_SIZE
            + U256_SIZE
            + U32_SIZE
            + self.call_data.len()
            + U64_SIZE // block_height
            + U64_SIZE // chain_id
            + auth_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_codec::{DecodeExt, Encode};

    #[test]
    fn test_validate_success() {
        let task_data = GasKillerTaskData::default();
        assert!(task_data.validate().is_ok());
    }

    #[test]
    fn test_validate_with_normal_data() {
        let task_data = GasKillerTaskData {
            storage_updates: vec![0u8; 1024].into(),
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
            storage_updates: vec![0u8; half_limit].into(),
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
            storage_updates: vec![0u8; half_limit].into(),
            call_data: vec![0u8; half_limit],
            ..Default::default()
        };
        assert!(task_data.validate().is_ok());
    }

    #[test]
    fn test_chain_id_roundtrip_mainnet() {
        let original = GasKillerTaskData {
            chain_id: 1, // Ethereum mainnet
            ..Default::default()
        };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        let decoded = GasKillerTaskData::decode(encoded).expect("decode failed");
        assert_eq!(decoded.chain_id, 1);
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_chain_id_roundtrip_gnosis() {
        let original = GasKillerTaskData {
            chain_id: 100, // Gnosis chain
            ..Default::default()
        };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        let decoded = GasKillerTaskData::decode(encoded).expect("decode failed");
        assert_eq!(decoded.chain_id, 100);
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_chain_id_roundtrip_anvil() {
        let original = GasKillerTaskData {
            chain_id: 31337, // Anvil local fork
            ..Default::default()
        };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        let decoded = GasKillerTaskData::decode(encoded).expect("decode failed");
        assert_eq!(decoded.chain_id, 31337);
        assert_eq!(decoded, original);
    }

    // -- auth blob: codec + signed hash --

    fn sample_auth() -> TxAuth {
        TxAuth {
            nonce: 7,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 2_000_000_000,
            gas_limit: 1_000_000,
            y_parity: true,
            r: U256::from(0x1234u64),
            s: U256::MAX,
        }
    }

    #[test]
    fn test_auth_none_roundtrip() {
        let original = GasKillerTaskData {
            chain_id: 1,
            auth: None,
            ..Default::default()
        };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        let decoded = GasKillerTaskData::decode(encoded).expect("decode failed");
        assert_eq!(decoded, original);
        assert!(decoded.auth.is_none());
    }

    #[test]
    fn test_auth_some_roundtrip() {
        let original = GasKillerTaskData {
            storage_updates: vec![0xAA; 40].into(),
            transition_index: 3,
            target_address: "0x00000000000000000000000000000000000000A1"
                .parse()
                .unwrap(),
            call_data: vec![0xAB, 0xCD, 0xEF, 0x01],
            from_address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
                .parse()
                .unwrap(),
            value: U256::from(12345u64),
            block_height: 100,
            chain_id: 11155111,
            auth: Some(sample_auth()),
        };
        let encoded = original.encode();
        assert_eq!(
            encoded.len(),
            original.encode_size(),
            "encode_size must match the written length"
        );
        let decoded = GasKillerTaskData::decode(encoded).expect("decode failed");
        assert_eq!(decoded, original);
        assert_eq!(decoded.auth, Some(sample_auth()));
    }

    #[test]
    fn test_auth_discriminant_is_first_appended_byte() {
        // The None encoding is exactly the old encoding plus a single 0 discriminant byte, so the
        // wire format extends the previous one rather than reshuffling it.
        let none = GasKillerTaskData {
            chain_id: 1,
            auth: None,
            ..Default::default()
        };
        let some = GasKillerTaskData {
            auth: Some(sample_auth()),
            ..none.clone()
        };
        assert_eq!(
            some.encode_size(),
            none.encode_size() + (8 + 16 + 16 + 8 + 1 + 32 + 32)
        );
    }

    /// Pins the authenticated payload hash to the value produced by the on-chain
    /// `GasKillerSDK.getSignedMessageHash` for the same inputs (verified in the solidity-sdk repo
    /// via forge). If the ABI encoding or field order drifts from Solidity, this fails and the
    /// operator signatures would never satisfy `verifyAndUpdateWithAuth` on-chain.
    #[test]
    fn test_signed_payload_hash_matches_onchain_vector() {
        let task = GasKillerTaskData {
            transition_index: 3,
            target_address: "0x00000000000000000000000000000000000000A1"
                .parse()
                .unwrap(),
            from_address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
                .parse()
                .unwrap(),
            value: U256::from(12345u64),
            call_data: vec![0xAB, 0xCD, 0xEF, 0x01],
            auth: Some(TxAuth {
                nonce: 7,
                ..sample_auth()
            }),
            ..Default::default()
        };
        let storage_updates = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let digest = task.build_payload_hash(&storage_updates);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex, "277f15508518421c6369f69da7b1129ace7f5e4fc3f783df91ad59340a08dbc0",
            "signed payload hash diverged from on-chain getSignedMessageHash"
        );
    }

    /// The presence of `auth` alone flips `build_payload_hash` to the signed preimage, so an
    /// authenticated task and an otherwise-identical permissionless one produce different digests.
    #[test]
    fn test_auth_selects_signed_hash_variant() {
        let base = GasKillerTaskData {
            transition_index: 1,
            target_address: "0x00000000000000000000000000000000000000A1"
                .parse()
                .unwrap(),
            from_address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
                .parse()
                .unwrap(),
            call_data: vec![0xAB, 0xCD, 0xEF, 0x01],
            ..Default::default()
        };
        let storage_updates = [1u8, 2, 3, 4];
        let permissionless = base.build_payload_hash(&storage_updates);
        let authenticated = GasKillerTaskData {
            auth: Some(sample_auth()),
            ..base
        }
        .build_payload_hash(&storage_updates);
        assert_ne!(
            permissionless, authenticated,
            "auth must select a different (signed) hash preimage"
        );
    }

    #[test]
    fn test_auth_bad_discriminant_rejected() {
        let mut encoded = GasKillerTaskData {
            auth: None,
            ..Default::default()
        }
        .encode()
        .to_vec();
        *encoded.last_mut().unwrap() = 2; // neither 0 nor 1
        assert!(GasKillerTaskData::decode(bytes::Bytes::from(encoded)).is_err());
    }
}

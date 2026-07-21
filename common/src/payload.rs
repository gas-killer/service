//! Wire types for the user-executable payload flow.
//!
//! When an aggregation round completes, the router persists the round as a [`TaskBundle`] — the
//! structured `verifyAndUpdate` argument components — and renders a [`PayloadView`], a
//! ready-to-sign transaction request the user submits themselves. The bundle is the durable
//! unit: the payload is one rendering of it, so the retained on-chain broadcast path (future
//! auto-execute / AA tier) can consume the same bundle to submit the transaction directly.

use alloy_primitives::{Address, B256, Bytes, FixedBytes, U256};
use serde::{Deserialize, Serialize};

/// A ready-to-sign transaction request returned by `GET /tasks/{id}` once a task is ready.
///
/// The caller signs and submits it as-is. `to` and `value` are server-controlled rather than
/// bare `verifyAndUpdate` calldata, so introducing an on-chain protocol fee (a payable target or
/// a routing entrypoint) changes the server output, never the integrator's client code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadView {
    /// Contract the transaction is sent to (the gas-killer target in beta).
    pub to: Address,
    /// Full ABI-encoded `verifyAndUpdate` calldata (selector + arguments).
    pub data: Bytes,
    /// Wei sent with the transaction. Zero in beta; `verifyAndUpdate` is not payable.
    pub value: U256,
    /// Numeric EVM chain id the transaction must be submitted to.
    pub chain_id: u64,
    /// `eth_estimateGas` result for the rendered call, provided as a submission hint.
    pub estimated_gas: u64,
    /// Last block for which the payload is accepted on-chain; past it the caller re-requests.
    pub valid_until_block: u64,
}

/// Scheme-specific quorum proof carried by a [`TaskBundle`].
///
/// The outer transaction request is scheme-agnostic; only the encoded proof — and therefore the
/// `verifyAndUpdate` calldata — differs between BLS and Schnorr.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scheme", rename_all = "snake_case")]
pub enum BundleProof {
    /// BLS aggregate proof.
    Bls {
        /// Quorum numbers the certificate covers.
        quorum_numbers: Bytes,
        /// ABI-encoded `GasKillerSDK` `NonSignerStakesAndSignature` struct.
        non_signer_stakes_and_signature: Bytes,
    },
    /// Aggregate-Schnorr proof.
    Schnorr {
        /// Aggregate signature scalar.
        s: U256,
        /// Aggregate nonce commitment address.
        r_addr: Address,
        /// Non-signing operator addresses, strictly ascending.
        non_signers: Vec<Address>,
    },
}

/// A completed aggregation round, persisted keyed by task id.
///
/// Holds every `verifyAndUpdate` argument component plus the chain, transition index, and
/// validity bound needed to render a [`PayloadView`] and to check freshness on read without
/// re-deriving anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBundle {
    /// Quorum-signed payload hash.
    pub msg_hash: B256,
    /// Operator-set reference block the certificate is anchored to.
    pub reference_block_number: u32,
    /// State transition index this round advances.
    pub transition_index: u64,
    /// Gas-killer target contract.
    pub target_address: Address,
    /// Selector of the target function being optimized.
    pub target_function: FixedBytes<4>,
    /// EVMSketch storage updates to apply.
    pub storage_updates: Bytes,
    /// Numeric EVM chain id.
    pub chain_id: u64,
    /// Wei to send with the rendered transaction (zero in beta).
    pub value: U256,
    /// Last block for which the rendered payload stays valid.
    pub valid_until_block: u64,
    /// Scheme-specific proof material.
    pub proof: BundleProof,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> PayloadView {
        PayloadView {
            to: Address::from([0x11; 20]),
            data: Bytes::from(vec![0x93, 0xde, 0x45, 0x31, 0x01, 0x02]),
            value: U256::ZERO,
            chain_id: 1,
            estimated_gas: 234_000,
            valid_until_block: 22_345_678,
        }
    }

    #[test]
    fn payload_view_serde_round_trip_and_wire_shape() {
        let payload = sample_payload();
        let json = serde_json::to_value(&payload).unwrap();

        // `to`/`data`/`value` are 0x-hex strings; the scalars are plain JSON numbers. This is the
        // wire contract integrators depend on (matches the issue's payload example).
        assert_eq!(
            json["to"].as_str().unwrap(),
            "0x1111111111111111111111111111111111111111"
        );
        assert_eq!(json["data"].as_str().unwrap(), "0x93de45310102");
        assert_eq!(json["value"].as_str().unwrap(), "0x0");
        assert_eq!(json["chain_id"], 1);
        assert_eq!(json["estimated_gas"], 234_000);
        assert_eq!(json["valid_until_block"], 22_345_678u64);

        let decoded: PayloadView = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn bundle_bls_round_trips_and_tags_scheme() {
        let bundle = TaskBundle {
            msg_hash: B256::from([0xab; 32]),
            reference_block_number: 100,
            transition_index: 7,
            target_address: Address::from([0x22; 20]),
            target_function: FixedBytes::<4>::from([0xde, 0xad, 0xbe, 0xef]),
            storage_updates: Bytes::from(vec![0x01, 0x02, 0x03]),
            chain_id: 31337,
            value: U256::ZERO,
            valid_until_block: 150,
            proof: BundleProof::Bls {
                quorum_numbers: Bytes::from(vec![0x00]),
                non_signer_stakes_and_signature: Bytes::from(vec![0xff, 0xee]),
            },
        };
        let json = serde_json::to_value(&bundle).unwrap();
        assert_eq!(json["proof"]["scheme"], "bls");

        let decoded: TaskBundle = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, bundle);
    }

    #[test]
    fn bundle_schnorr_round_trips_and_tags_scheme() {
        let bundle = TaskBundle {
            msg_hash: B256::from([0xcd; 32]),
            reference_block_number: 200,
            transition_index: 3,
            target_address: Address::from([0x33; 20]),
            target_function: FixedBytes::<4>::from([0x01, 0x02, 0x03, 0x04]),
            storage_updates: Bytes::from(vec![0x09]),
            chain_id: 1,
            value: U256::ZERO,
            valid_until_block: 250,
            proof: BundleProof::Schnorr {
                s: U256::from(42u64),
                r_addr: Address::from([0x44; 20]),
                non_signers: vec![Address::from([0x55; 20]), Address::from([0x66; 20])],
            },
        };
        let json = serde_json::to_value(&bundle).unwrap();
        assert_eq!(json["proof"]["scheme"], "schnorr");

        let decoded: TaskBundle = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, bundle);
    }
}

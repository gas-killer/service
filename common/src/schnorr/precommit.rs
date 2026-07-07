//! Pre-committed nonce batches for **non-interactive** aggregate Schnorr signing.
//!
//! An operator derives a batch of MuSig2 two-point nonces from a single 32-byte seed,
//! commits the batch's Merkle root on-chain in the `SchnorrNonceRegistry`, and gossips the
//! full public points p2p. Signing then needs no online nonce round: every party derives
//! the same absolute **slot** for a session from its `(height, attempt)` and reconstructs
//! the signing context from the committed points (see `docs/schnorr-nonce-registry.md`).
//!
//! # Invariant N1 — one partial per slot, ever
//!
//! Nonce derivation is deterministic, so a restart can recompute every secret nonce. That
//! kills the interactive mode's safe-by-amnesia property: **every** signing path built on
//! this module must gate slot use behind a durable spend journal (see
//! [`super::scheme::SpendJournal`]) — signing two different contexts with one slot's
//! nonces leaks the private key.
//!
//! # Replay domain
//!
//! Every derivation and the registration message bind `(chain id, registry address,
//! operator identity)` via [`BatchDomain`], so batches and registrations cannot be
//! replayed across deployments or chains, and leaves bind `(operator, absolute slot)`, so
//! points cannot be replayed across positions.

use super::musig::{PubNonce, SecNonce};
use super::{AggregateSignature, Entropy, MESSAGE_LEN, PrivateKey, PublicKey, verify_aggregate};
use alloy_primitives::{Address, U256 as AlloyU256, keccak256};
use k256::Scalar;
use k256::elliptic_curve::bigint::U256;
use k256::elliptic_curve::ops::Reduce;

/// Domain tag for secret-nonce derivation (never leaves the operator).
pub const NONCE_SEED_TAG: &[u8] = b"gas-killer/schnorr/nonce-seed/v1";
/// Domain tag for Merkle leaves. Padding leaves use [`empty_leaf`]'s suffixed tag.
pub const NONCE_LEAF_TAG: &[u8] = b"gas-killer/schnorr/nonce-leaf/v1";
/// Domain tag for the on-chain batch registration message — must equal the Solidity
/// `SchnorrNonceRegistry.BATCH_TAG` byte-for-byte.
pub const NONCE_BATCH_TAG: &[u8] = b"gas-killer/schnorr/nonce-batch/v1";

/// Hard cap on slots per batch (mirrors the contract's `MAX_BATCH_SLOTS`).
pub const MAX_BATCH_SLOTS: u64 = 1 << 20;

/// The absolute slot for a signing session, `height · attempts_per_height + attempt`.
///
/// Injective across sessions (given a fixed `attempts_per_height`), which is what makes
/// the request→slot mapping collision-free — see the plan's §4 for why a hash-based
/// mapping was rejected. Returns `None` on overflow or `attempt ≥ attempts_per_height`.
pub fn slot_index(height: u64, attempt: u32, attempts_per_height: u32) -> Option<u64> {
    if attempts_per_height == 0 || attempt >= attempts_per_height {
        return None;
    }
    height
        .checked_mul(u64::from(attempts_per_height))?
        .checked_add(u64::from(attempt))
}

/// Deployment-scoping parameters mixed into every nonce derivation, Merkle leaf, and the
/// registration message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchDomain {
    /// EVM chain id (encoded as a 32-byte word, matching Solidity's `block.chainid`).
    pub chain_id: u64,
    /// The `SchnorrNonceRegistry` contract address.
    pub registry: Address,
    /// The operator identity: the Schnorr public key's Ethereum address
    /// (`keccak256(x ‖ y)[12..]`), i.e. `SchnorrStakeRegistry.pointAddress`.
    pub operator: Address,
}

/// Derives one secret scalar of a slot's nonce pair:
/// `keccak256(NONCE_SEED_TAG ‖ chainid ‖ registry ‖ operator ‖ seed ‖ slot ‖ j ‖ retry)`
/// reduced mod `n`. `retry` starts at 0 and bumps only on the ~2⁻²⁵⁶ zero reduction.
fn derive_scalar(domain: &BatchDomain, batch_seed: &[u8; 32], slot: u64, j: u8) -> Scalar {
    for retry in 0u8..=u8::MAX {
        let mut pre = Vec::with_capacity(NONCE_SEED_TAG.len() + 32 + 20 + 20 + 32 + 8 + 2);
        pre.extend_from_slice(NONCE_SEED_TAG);
        pre.extend_from_slice(&AlloyU256::from(domain.chain_id).to_be_bytes::<32>());
        pre.extend_from_slice(domain.registry.as_slice());
        pre.extend_from_slice(domain.operator.as_slice());
        pre.extend_from_slice(batch_seed);
        pre.extend_from_slice(&slot.to_be_bytes());
        pre.push(j);
        pre.push(retry);
        let s = Scalar::reduce(U256::from_be_slice(keccak256(pre).as_slice()));
        if !bool::from(s.is_zero()) {
            return s;
        }
    }
    unreachable!("256 consecutive zero scalar reductions")
}

/// Derives the secret nonce pair for an absolute `slot`.
///
/// Seed secrecy ≡ key secrecy: a leaked seed plus one later-published partial reveals the
/// private key, so store `batch_seed` exactly like the signing key.
pub fn derive_secnonce(domain: &BatchDomain, batch_seed: &[u8; 32], slot: u64) -> SecNonce {
    let k1 = derive_scalar(domain, batch_seed, slot, 0);
    let k2 = derive_scalar(domain, batch_seed, slot, 1);
    SecNonce::from_scalars(k1, k2).expect("derive_scalar never returns zero")
}

/// Merkle leaf binding `(operator, absolute slot, R1, R2)`.
pub fn leaf_hash(operator: Address, slot: u64, nonce: &PubNonce) -> [u8; 32] {
    let points = nonce.to_bytes();
    let mut pre = Vec::with_capacity(NONCE_LEAF_TAG.len() + 20 + 8 + 66);
    pre.extend_from_slice(NONCE_LEAF_TAG);
    pre.extend_from_slice(operator.as_slice());
    pre.extend_from_slice(&slot.to_be_bytes());
    pre.extend_from_slice(&points);
    keccak256(pre).0
}

/// The padding leaf for non-power-of-two batches (domain-separated from real leaves).
fn empty_leaf() -> [u8; 32] {
    keccak256([NONCE_LEAF_TAG, b"/empty"].concat()).0
}

fn merkle_parent(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut pre = [0u8; 64];
    pre[..32].copy_from_slice(left);
    pre[32..].copy_from_slice(right);
    keccak256(pre).0
}

/// Root of a positional keccak Merkle tree over `leaves`, padded with [`empty_leaf`] to
/// the next power of two.
pub fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    assert!(!leaves.is_empty(), "merkle_root of zero leaves");
    let width = leaves.len().next_power_of_two();
    let mut level: Vec<[u8; 32]> = Vec::with_capacity(width);
    level.extend_from_slice(leaves);
    level.resize(width, empty_leaf());
    while level.len() > 1 {
        level = level
            .chunks_exact(2)
            .map(|pair| merkle_parent(&pair[0], &pair[1]))
            .collect();
    }
    level[0]
}

/// Sibling path for `index` in the padded positional tree (bottom-up).
pub fn merkle_prove(leaves: &[[u8; 32]], index: usize) -> Option<Vec<[u8; 32]>> {
    if index >= leaves.len() {
        return None;
    }
    let width = leaves.len().next_power_of_two();
    let mut level: Vec<[u8; 32]> = Vec::with_capacity(width);
    level.extend_from_slice(leaves);
    level.resize(width, empty_leaf());
    let mut path = Vec::new();
    let mut idx = index;
    while level.len() > 1 {
        path.push(level[idx ^ 1]);
        level = level
            .chunks_exact(2)
            .map(|pair| merkle_parent(&pair[0], &pair[1]))
            .collect();
        idx >>= 1;
    }
    Some(path)
}

/// Verifies a bottom-up sibling `path` for `leaf` at `index` against `root`.
///
/// `count` is the batch's leaf count; the path length must equal the padded tree depth
/// (rejecting truncated/extended proofs, which would shift positions).
pub fn merkle_verify(
    root: &[u8; 32],
    leaf: &[u8; 32],
    index: u64,
    count: u64,
    path: &[[u8; 32]],
) -> bool {
    if index >= count || count == 0 {
        return false;
    }
    let width = count.next_power_of_two();
    if path.len() != width.trailing_zeros() as usize {
        return false;
    }
    let mut node = *leaf;
    let mut idx = index;
    for sibling in path {
        node = if idx & 1 == 0 {
            merkle_parent(&node, sibling)
        } else {
            merkle_parent(sibling, &node)
        };
        idx >>= 1;
    }
    node == *root
}

/// A batch of pre-committed public nonces covering the absolute slots
/// `[start_slot, start_slot + nonces.len())` for one operator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonceBatch {
    pub domain: BatchDomain,
    /// Position in the operator's on-chain batch list (0-based, append-only).
    pub batch_index: u64,
    /// First absolute slot this batch covers (the contract enforces contiguity).
    pub start_slot: u64,
    /// Public nonce pairs, index `i` covering slot `start_slot + i`.
    pub nonces: Vec<PubNonce>,
}

impl NonceBatch {
    /// Derives a batch from a seed. Returns `None` for a zero/oversized count or a slot
    /// range that overflows.
    pub fn generate(
        domain: BatchDomain,
        batch_seed: &[u8; 32],
        batch_index: u64,
        start_slot: u64,
        count: u64,
    ) -> Option<Self> {
        if count == 0 || count > MAX_BATCH_SLOTS {
            return None;
        }
        start_slot.checked_add(count)?;
        let nonces = (0..count)
            .map(|i| derive_secnonce(&domain, batch_seed, start_slot + i).pub_nonce())
            .collect();
        Some(Self {
            domain,
            batch_index,
            start_slot,
            nonces,
        })
    }

    pub fn count(&self) -> u64 {
        self.nonces.len() as u64
    }

    /// One past the last covered slot.
    pub fn end_slot(&self) -> u64 {
        self.start_slot + self.count()
    }

    pub fn covers(&self, slot: u64) -> bool {
        slot >= self.start_slot && slot < self.end_slot()
    }

    /// The committed nonce pair for an absolute `slot`, if covered.
    pub fn pub_nonce(&self, slot: u64) -> Option<&PubNonce> {
        self.covers(slot)
            .then(|| &self.nonces[(slot - self.start_slot) as usize])
    }

    fn leaves(&self) -> Vec<[u8; 32]> {
        self.nonces
            .iter()
            .enumerate()
            .map(|(i, nonce)| leaf_hash(self.domain.operator, self.start_slot + i as u64, nonce))
            .collect()
    }

    /// The Merkle root registered on-chain.
    pub fn root(&self) -> [u8; 32] {
        merkle_root(&self.leaves())
    }

    /// The committed nonce pair plus its Merkle path for an absolute `slot`.
    pub fn prove(&self, slot: u64) -> Option<(PubNonce, Vec<[u8; 32]>)> {
        let nonce = *self.pub_nonce(slot)?;
        let path = merkle_prove(&self.leaves(), (slot - self.start_slot) as usize)?;
        Some((nonce, path))
    }

    /// The registration message this batch signs (see [`batch_message`]).
    pub fn registration_message(&self) -> [u8; MESSAGE_LEN] {
        batch_message(
            &self.domain,
            self.batch_index,
            self.start_slot,
            self.count(),
            &self.root(),
        )
    }

    /// Signs the registration message with the operator key. Returns `None` if the key's
    /// identity address does not match the domain's operator.
    pub fn sign_registration(
        &self,
        key: &PrivateKey,
        fill: &mut impl Entropy,
    ) -> Option<AggregateSignature> {
        if key.public_key().eth_address() != self.domain.operator {
            return None;
        }
        Some(key.sign_single(&self.registration_message(), fill))
    }

    /// Verifies a batch (root + registration signature) against the operator key —
    /// exactly the check `SchnorrNonceRegistry.registerBatch` performs on-chain, for
    /// peers validating a gossiped batch against its on-chain registration.
    pub fn verify_registration(&self, key: &PublicKey, signature: &AggregateSignature) -> bool {
        key.eth_address() == self.domain.operator
            && verify_aggregate(key, &self.registration_message(), signature)
    }
}

/// The on-chain batch registration message:
/// `keccak256(NONCE_BATCH_TAG ‖ chainid₃₂ ‖ registry ‖ operator ‖ batchIndex₈ ‖ startSlot₈ ‖ count₈ ‖ root)`
/// — must match `SchnorrNonceRegistry.batchMessage` byte-for-byte (`uint64` fields are
/// 8-byte big-endian under `abi.encodePacked`).
pub fn batch_message(
    domain: &BatchDomain,
    batch_index: u64,
    start_slot: u64,
    count: u64,
    root: &[u8; 32],
) -> [u8; MESSAGE_LEN] {
    let mut pre = Vec::with_capacity(NONCE_BATCH_TAG.len() + 32 + 20 + 20 + 8 + 8 + 8 + 32);
    pre.extend_from_slice(NONCE_BATCH_TAG);
    pre.extend_from_slice(&AlloyU256::from(domain.chain_id).to_be_bytes::<32>());
    pre.extend_from_slice(domain.registry.as_slice());
    pre.extend_from_slice(domain.operator.as_slice());
    pre.extend_from_slice(&batch_index.to_be_bytes());
    pre.extend_from_slice(&start_slot.to_be_bytes());
    pre.extend_from_slice(&count.to_be_bytes());
    pre.extend_from_slice(root);
    keccak256(pre).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schnorr::Entropy;
    use rand::rngs::StdRng;
    use rand::{RngCore, SeedableRng};

    fn seeded(seed: u64) -> impl Entropy {
        let mut rng = StdRng::seed_from_u64(seed);
        move |b: &mut [u8]| rng.fill_bytes(b)
    }

    fn test_domain(key: &PrivateKey) -> BatchDomain {
        BatchDomain {
            chain_id: 31337,
            registry: Address::repeat_byte(0x42),
            operator: key.public_key().eth_address(),
        }
    }

    #[test]
    fn slot_index_is_injective_and_bounded() {
        assert_eq!(slot_index(0, 0, 4), Some(0));
        assert_eq!(slot_index(0, 3, 4), Some(3));
        assert_eq!(slot_index(1, 0, 4), Some(4));
        assert_eq!(slot_index(7, 2, 4), Some(30));
        // attempt out of range / zero attempts rejected.
        assert_eq!(slot_index(1, 4, 4), None);
        assert_eq!(slot_index(1, 0, 0), None);
        // overflow rejected, not wrapped.
        assert_eq!(slot_index(u64::MAX, 0, 2), None);
        // distinct sessions → distinct slots across a dense range.
        let mut seen = std::collections::HashSet::new();
        for h in 0..64u64 {
            for a in 0..4u32 {
                assert!(seen.insert(slot_index(h, a, 4).unwrap()));
            }
        }
    }

    #[test]
    fn derivation_is_deterministic_and_domain_separated() {
        let key = PrivateKey::from_seed(1);
        let domain = test_domain(&key);
        let seed = [7u8; 32];

        let a = derive_secnonce(&domain, &seed, 42).pub_nonce();
        let b = derive_secnonce(&domain, &seed, 42).pub_nonce();
        assert_eq!(a, b, "same inputs must derive the same nonce");

        // Different slot, seed, chain, registry, or operator → different nonce.
        assert_ne!(a, derive_secnonce(&domain, &seed, 43).pub_nonce());
        assert_ne!(a, derive_secnonce(&domain, &[8u8; 32], 42).pub_nonce());
        let other_chain = BatchDomain {
            chain_id: 1,
            ..domain
        };
        assert_ne!(a, derive_secnonce(&other_chain, &seed, 42).pub_nonce());
        let other_registry = BatchDomain {
            registry: Address::repeat_byte(0x43),
            ..domain
        };
        assert_ne!(a, derive_secnonce(&other_registry, &seed, 42).pub_nonce());
        let other_operator = BatchDomain {
            operator: PrivateKey::from_seed(2).public_key().eth_address(),
            ..domain
        };
        assert_ne!(a, derive_secnonce(&other_operator, &seed, 42).pub_nonce());
    }

    #[test]
    fn merkle_roundtrip_all_positions() {
        // Cover padded (non-power-of-two) and exact-power widths.
        for count in [1usize, 2, 3, 5, 8] {
            let leaves: Vec<[u8; 32]> = (0..count).map(|i| keccak256([i as u8; 7]).0).collect();
            let root = merkle_root(&leaves);
            for (i, leaf) in leaves.iter().enumerate() {
                let path = merkle_prove(&leaves, i).unwrap();
                assert!(
                    merkle_verify(&root, leaf, i as u64, count as u64, &path),
                    "count={count} index={i}"
                );
                // Wrong index / wrong leaf / truncated path rejected.
                assert!(
                    !merkle_verify(
                        &root,
                        leaf,
                        (i as u64 + 1) % count as u64,
                        count as u64,
                        &path
                    ) || count == 1
                );
                let mut bad = *leaf;
                bad[0] ^= 1;
                assert!(!merkle_verify(&root, &bad, i as u64, count as u64, &path));
                if !path.is_empty() {
                    assert!(!merkle_verify(
                        &root,
                        leaf,
                        i as u64,
                        count as u64,
                        &path[..path.len() - 1]
                    ));
                }
            }
            // Out-of-range index rejected.
            assert!(!merkle_verify(
                &root,
                &leaves[0],
                count as u64,
                count as u64,
                &merkle_prove(&leaves, 0).unwrap()
            ));
        }
    }

    #[test]
    fn batch_generate_prove_verify() {
        let key = PrivateKey::from_seed(3);
        let domain = test_domain(&key);
        let seed = [9u8; 32];
        let batch = NonceBatch::generate(domain, &seed, 0, 0, 12).unwrap();
        assert_eq!(batch.count(), 12);
        assert_eq!(batch.end_slot(), 12);

        let root = batch.root();
        for slot in 0..12u64 {
            let (nonce, path) = batch.prove(slot).unwrap();
            assert_eq!(Some(&nonce), batch.pub_nonce(slot));
            let leaf = leaf_hash(domain.operator, slot, &nonce);
            assert!(merkle_verify(
                &root,
                &leaf,
                slot - batch.start_slot,
                batch.count(),
                &path
            ));
            // The committed point matches the secret derivation.
            assert_eq!(nonce, derive_secnonce(&domain, &seed, slot).pub_nonce());
        }
        assert!(batch.prove(12).is_none());
        assert!(NonceBatch::generate(domain, &seed, 0, 0, 0).is_none());
        assert!(NonceBatch::generate(domain, &seed, 0, 0, MAX_BATCH_SLOTS + 1).is_none());
        assert!(NonceBatch::generate(domain, &seed, 0, u64::MAX, 2).is_none());

        // A second batch continues coverage with different nonces.
        let next = NonceBatch::generate(domain, &[10u8; 32], 1, batch.end_slot(), 12).unwrap();
        assert_eq!(next.start_slot, 12);
        assert_ne!(next.nonces[0], batch.nonces[0]);
    }

    #[test]
    fn registration_sign_verify_roundtrip() {
        let mut fill = seeded(5);
        let key = PrivateKey::from_seed(4);
        let domain = test_domain(&key);
        let batch = NonceBatch::generate(domain, &[1u8; 32], 0, 0, 4).unwrap();

        let sig = batch.sign_registration(&key, &mut fill).unwrap();
        assert!(batch.verify_registration(&key.public_key(), &sig));

        // Wrong key (identity mismatch) can neither sign nor verify.
        let other = PrivateKey::from_seed(5);
        assert!(batch.sign_registration(&other, &mut fill).is_none());
        assert!(!batch.verify_registration(&other.public_key(), &sig));

        // Any field change breaks the signature.
        let mut tampered = batch.clone();
        tampered.batch_index = 1;
        assert!(!tampered.verify_registration(&key.public_key(), &sig));
        let mut tampered = batch.clone();
        tampered.start_slot = 1;
        assert!(!tampered.verify_registration(&key.public_key(), &sig));
        let mut tampered = batch.clone();
        tampered.nonces.pop();
        assert!(!tampered.verify_registration(&key.public_key(), &sig));
    }

    /// PARITY ANCHOR — the Solidity `SchnorrNonceRegistry.t.sol` asserts this exact
    /// constant for the same inputs. If the preimage layout changes, regenerate BOTH.
    #[test]
    fn batch_message_parity_vector() {
        let domain = BatchDomain {
            chain_id: 31337,
            registry: Address::repeat_byte(0x42),
            operator: Address::repeat_byte(0xaa),
        };
        let msg = batch_message(&domain, 1, 1024, 2048, &[0x11u8; 32]);
        assert_eq!(
            alloy_primitives::hex::encode(msg),
            "40d92eb65c7bf4b8a1af08673c34e217aaef9979016364b87cc9b078c26569e7",
            "batch_message parity vector changed — update the Solidity test too"
        );
    }
}

//! Batch distribution + completion-round collection for the pre-committed-nonce mode —
//! the pure logic behind the node/router precommit actors (plan §6.2, §7.3).
//!
//! [`BatchStore`] ingests [`PrecommitMsg::BatchAnnounce`] chunks, reassembles and
//! verifies whole batches, feeds the shared [`MemoryNonceDirectory`] the scheme reads,
//! and re-serves chunks to peers. Verification of a completed batch is
//! **self-authenticating**: the sender's p2p identity must equal the announced key's
//! address, and the recomputed Merkle root must carry a valid registration signature by
//! that key (the exact statement `SchnorrNonceRegistry.registerBatch` verified on-chain).
//! Pinning the root against the on-chain registration additionally (an RPC read) closes
//! a per-operator equivocation-liveness nuisance and can be layered on the actor.
//!
//! [`CompletionCollector`] is the router-side state for one `(height, attempt)`
//! completion round: it validates [`PrecommitMsg::CompletionPartial`]s from exactly the
//! engine-certified signer set and assembles the final constant-size signature.

use super::musig::PubNonce;
use super::precommit::{BatchDomain, MAX_BATCH_SLOTS, NonceBatch};
use super::scheme::{CompletionContext, MemoryNonceDirectory, SchnorrScheme};
use super::wire::{MAX_CHUNK_NONCES, PrecommitMsg};
use super::{AggregateSignature, MESSAGE_LEN, PublicKey as SchnorrPublicKey, verify_aggregate};
use crate::ecdsa::PublicKey as OperatorKey;
use alloy_primitives::Address;
use commonware_cryptography::certificate::{Scheme as _, Signers};
use commonware_utils::Participant;
use commonware_utils::ordered::Quorum as _;
use k256::Scalar;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// Why an announce chunk (or the batch it completed) was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// `eth_address(pubkey) != sender` — the p2p identity binding failed.
    SenderMismatch,
    /// The operator announced a different key than previously seen.
    KeyMismatch,
    /// total = 0 or above `MAX_BATCH_SLOTS`, or metadata conflicts across chunks.
    BadMetadata,
    /// The same chunk offset arrived with different points.
    ConflictingChunk,
    /// The assembled batch's registration signature failed to verify.
    BadRegistration,
}

/// Outcome of ingesting one announce chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingest {
    /// Chunk stored; the batch is still incomplete.
    Pending,
    /// The chunk completed a batch that verified and entered the directory.
    Completed,
    /// The chunk (or its completed batch) was rejected.
    Rejected(Rejection),
}

/// A fully verified, re-servable batch.
struct StoredBatch {
    start_slot: u64,
    signature: AggregateSignature,
    nonces: Vec<PubNonce>,
}

/// An in-flight reassembly.
struct PendingBatch {
    start_slot: u64,
    total: u64,
    signature: AggregateSignature,
    /// chunk offset → points (validated non-overlapping on insert).
    chunks: BTreeMap<u64, Vec<PubNonce>>,
}

impl PendingBatch {
    fn received(&self) -> u64 {
        self.chunks.values().map(|c| c.len() as u64).sum()
    }

    /// Whether `[0, total)` is contiguously covered.
    fn complete(&self) -> bool {
        let mut next = 0u64;
        for (offset, chunk) in &self.chunks {
            if *offset != next {
                return false;
            }
            next += chunk.len() as u64;
        }
        next == self.total
    }
}

#[derive(Default)]
struct OperatorBatches {
    pubkey: Option<SchnorrPublicKey>,
    pending: HashMap<u64, PendingBatch>,
    complete: BTreeMap<u64, StoredBatch>,
}

/// Assembles gossiped nonce batches and feeds the scheme's directory. Shared between
/// the p2p actor (ingest/serve) and scheme construction (directory).
pub struct BatchStore {
    chain_id: u64,
    registry: Address,
    directory: Arc<MemoryNonceDirectory>,
    inner: Mutex<HashMap<Address, OperatorBatches>>,
}

impl BatchStore {
    pub fn new(chain_id: u64, registry: Address, directory: Arc<MemoryNonceDirectory>) -> Self {
        Self {
            chain_id,
            registry,
            directory,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// The directory this store feeds (hand to [`SchnorrScheme`] construction).
    pub fn directory(&self) -> Arc<MemoryNonceDirectory> {
        Arc::clone(&self.directory)
    }

    /// Inserts a locally generated (already trusted) batch — the node's own — making it
    /// visible to the scheme and re-servable to peers. Returns `false` if the
    /// registration signature does not verify (a configuration bug worth failing loud).
    pub fn insert_local(
        &self,
        pubkey: &SchnorrPublicKey,
        batch: &NonceBatch,
        signature: AggregateSignature,
    ) -> bool {
        if !batch.verify_registration(pubkey, &signature) {
            return false;
        }
        let mut inner = self.inner.lock().expect("batch store lock");
        let entry = inner.entry(batch.domain.operator).or_default();
        entry.pubkey = Some(*pubkey);
        entry.complete.insert(
            batch.batch_index,
            StoredBatch {
                start_slot: batch.start_slot,
                signature,
                nonces: batch.nonces.clone(),
            },
        );
        self.directory.insert_batch(batch);
        true
    }

    /// Ingests one gossiped announce chunk from an authenticated p2p `sender`.
    #[allow(clippy::too_many_arguments)]
    pub fn ingest(
        &self,
        sender: Address,
        pubkey: SchnorrPublicKey,
        batch_index: u64,
        start_slot: u64,
        total: u64,
        signature: AggregateSignature,
        chunk_offset: u64,
        nonces: Vec<PubNonce>,
    ) -> Ingest {
        if pubkey.eth_address() != sender {
            return Ingest::Rejected(Rejection::SenderMismatch);
        }
        if total == 0 || total > MAX_BATCH_SLOTS {
            return Ingest::Rejected(Rejection::BadMetadata);
        }

        let mut inner = self.inner.lock().expect("batch store lock");
        let entry = inner.entry(sender).or_default();
        match entry.pubkey {
            Some(known) if known != pubkey => return Ingest::Rejected(Rejection::KeyMismatch),
            _ => entry.pubkey = Some(pubkey),
        }
        if entry.complete.contains_key(&batch_index) {
            return Ingest::Completed; // idempotent re-announce of a verified batch
        }

        let pending = entry
            .pending
            .entry(batch_index)
            .or_insert_with(|| PendingBatch {
                start_slot,
                total,
                signature,
                chunks: BTreeMap::new(),
            });
        if pending.start_slot != start_slot
            || pending.total != total
            || pending.signature != signature
        {
            return Ingest::Rejected(Rejection::BadMetadata);
        }
        match pending.chunks.get(&chunk_offset) {
            Some(existing) if *existing != nonces => {
                return Ingest::Rejected(Rejection::ConflictingChunk);
            }
            Some(_) => return Ingest::Pending, // duplicate chunk
            None => {
                pending.chunks.insert(chunk_offset, nonces);
            }
        }
        if pending.received() > pending.total || !pending.complete() {
            return Ingest::Pending;
        }

        // Assemble and verify the whole batch against the registration signature.
        let pending = entry
            .pending
            .remove(&batch_index)
            .expect("pending batch just inserted");
        let batch = NonceBatch {
            domain: BatchDomain {
                chain_id: self.chain_id,
                registry: self.registry,
                operator: sender,
            },
            batch_index,
            start_slot: pending.start_slot,
            nonces: pending.chunks.into_values().flatten().collect(),
        };
        if !batch.verify_registration(&pubkey, &pending.signature) {
            // Poisoned reassembly (a bad chunk slipped in, or a forged announce):
            // drop it entirely so an honest re-announce can start clean.
            return Ingest::Rejected(Rejection::BadRegistration);
        }
        entry.complete.insert(
            batch_index,
            StoredBatch {
                start_slot: batch.start_slot,
                signature: pending.signature,
                nonces: batch.nonces.clone(),
            },
        );
        self.directory.insert_batch(&batch);
        Ingest::Completed
    }

    /// Whether a verified batch is held for `(operator, batch_index)`.
    pub fn has(&self, operator: Address, batch_index: u64) -> bool {
        self.inner
            .lock()
            .expect("batch store lock")
            .get(&operator)
            .is_some_and(|entry| entry.complete.contains_key(&batch_index))
    }

    /// Serves one chunk of a held batch (answering [`PrecommitMsg::BatchRequest`]).
    pub fn serve(
        &self,
        operator: Address,
        batch_index: u64,
        chunk_offset: u64,
    ) -> Option<PrecommitMsg> {
        let inner = self.inner.lock().expect("batch store lock");
        let entry = inner.get(&operator)?;
        let pubkey = entry.pubkey?;
        let stored = entry.complete.get(&batch_index)?;
        let total = stored.nonces.len() as u64;
        if chunk_offset >= total {
            return None;
        }
        let end = (chunk_offset + MAX_CHUNK_NONCES as u64).min(total);
        Some(PrecommitMsg::BatchAnnounce {
            pubkey,
            batch_index,
            start_slot: stored.start_slot,
            total,
            signature: stored.signature,
            chunk_offset,
            nonces: stored.nonces[chunk_offset as usize..end as usize].to_vec(),
        })
    }

    /// Splits a batch into broadcastable announce chunks.
    pub fn chunk_batch(
        pubkey: &SchnorrPublicKey,
        batch: &NonceBatch,
        signature: AggregateSignature,
    ) -> Vec<PrecommitMsg> {
        batch
            .nonces
            .chunks(MAX_CHUNK_NONCES)
            .enumerate()
            .map(|(i, chunk)| PrecommitMsg::BatchAnnounce {
                pubkey: *pubkey,
                batch_index: batch.batch_index,
                start_slot: batch.start_slot,
                total: batch.count(),
                signature,
                chunk_offset: (i * MAX_CHUNK_NONCES) as u64,
                nonces: chunk.to_vec(),
            })
            .collect()
    }
}

/// Outcome of adding one completion partial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Added {
    /// Valid; still waiting on other members.
    Accepted,
    /// Valid and it completed the set: the final, self-verified signature.
    Complete(AggregateSignature),
    /// Idempotent duplicate of an already-accepted partial.
    Duplicate,
    /// Invalid (unknown/non-member sender, context mismatch, or bad partial).
    Rejected,
}

/// Router-side collection state for one `(height, attempt)` completion round over an
/// engine-certified signer set (plan §7.3).
pub struct CompletionCollector {
    scheme: SchnorrScheme,
    cc: CompletionContext,
    x_agg: SchnorrPublicKey,
    digest: [u8; MESSAGE_LEN],
    collected: BTreeMap<Participant, Scalar>,
}

impl CompletionCollector {
    /// Builds the collector for the certified `signers` bitmap. `None` when the context
    /// is underivable (missing coverage at the attempt slot, bad attempt, …) — the
    /// caller should shrink/retry or fall to the deadline path.
    pub fn new(
        scheme: SchnorrScheme,
        height: u64,
        attempt: u32,
        digest: [u8; MESSAGE_LEN],
        signers: &Signers,
    ) -> Option<Self> {
        let cc = scheme.completion_context(height, attempt, &digest, signers)?;
        let x_agg = scheme.aggregate_key(signers)?;
        Some(Self {
            scheme,
            cc,
            x_agg,
            digest,
            collected: BTreeMap::new(),
        })
    }

    /// The context's effective nonce address (what members must echo).
    pub fn r_addr(&self) -> Address {
        self.cc.ctx.r_addr
    }

    /// Members still missing (for targeted re-requests / logging).
    pub fn missing(&self) -> Vec<Participant> {
        self.cc
            .members
            .iter()
            .filter(|m| !self.collected.contains_key(m))
            .copied()
            .collect()
    }

    /// Validates and adds one member's partial (sender = authenticated p2p identity).
    pub fn add(&mut self, sender: Address, r_addr: Address, partial: Scalar) -> Added {
        let Some(participant) = self.scheme.participants().index(&OperatorKey::from(sender)) else {
            return Added::Rejected;
        };
        if self.cc.members.binary_search(&participant).is_err() {
            return Added::Rejected;
        }
        if r_addr != self.cc.ctx.r_addr {
            return Added::Rejected;
        }
        if let Some(existing) = self.collected.get(&participant) {
            return if *existing == partial {
                Added::Duplicate
            } else {
                Added::Rejected
            };
        }
        if !self
            .scheme
            .verify_completion_partial(&self.cc, participant, &partial)
        {
            return Added::Rejected;
        }
        self.collected.insert(participant, partial);
        if self.collected.len() < self.cc.members.len() {
            return Added::Accepted;
        }

        // All members in: sum and self-verify with the exact on-chain identity.
        let sum = self.collected.values().fold(Scalar::ZERO, |acc, s| acc + s);
        if bool::from(sum.is_zero()) {
            return Added::Rejected;
        }
        let signature = AggregateSignature {
            s: sum,
            r_addr: self.cc.ctx.r_addr,
        };
        if !verify_aggregate(&self.x_agg, &self.digest, &signature) {
            return Added::Rejected;
        }
        Added::Complete(signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schnorr::precommit::derive_batch_seed;
    use crate::schnorr::scheme::{MemorySpendJournal, NonceDirectory as _, SeedSecrets};
    use crate::schnorr::{Entropy, PrivateKey};
    use commonware_codec::{DecodeExt, Encode};
    use rand::rngs::StdRng;
    use rand::{RngCore, SeedableRng};

    const CHAIN_ID: u64 = 31337;
    const ATTEMPTS: u32 = 4;
    const SLOTS: u64 = 64;

    fn seeded(seed: u64) -> impl Entropy {
        let mut rng = StdRng::seed_from_u64(seed);
        move |b: &mut [u8]| rng.fill_bytes(b)
    }

    fn registry() -> Address {
        Address::repeat_byte(0x42)
    }

    fn signed_batch(key: &PrivateKey, count: u64) -> (NonceBatch, AggregateSignature) {
        let domain = BatchDomain {
            chain_id: CHAIN_ID,
            registry: registry(),
            operator: key.public_key().eth_address(),
        };
        let batch = NonceBatch::generate(domain, &derive_batch_seed(key, 0), 0, 0, count).unwrap();
        let signature = batch.sign_registration(key, &mut seeded(9)).unwrap();
        (batch, signature)
    }

    #[test]
    fn chunked_ingest_out_of_order_completes_and_serves() {
        let key = PrivateKey::from_seed(1);
        let operator = key.public_key().eth_address();
        let (batch, signature) = signed_batch(&key, SLOTS);

        let store = BatchStore::new(
            CHAIN_ID,
            registry(),
            Arc::new(MemoryNonceDirectory::default()),
        );

        // Split into 4 hand-made chunks and deliver out of order (16 each).
        let mut chunks: Vec<(u64, Vec<PubNonce>)> = batch
            .nonces
            .chunks(16)
            .enumerate()
            .map(|(i, c)| ((i * 16) as u64, c.to_vec()))
            .collect();
        chunks.swap(0, 3);
        chunks.swap(1, 2);

        for (i, (offset, nonces)) in chunks.iter().enumerate() {
            let outcome = store.ingest(
                operator,
                key.public_key(),
                0,
                0,
                SLOTS,
                signature,
                *offset,
                nonces.clone(),
            );
            if i + 1 < chunks.len() {
                assert_eq!(outcome, Ingest::Pending, "chunk {i}");
            } else {
                assert_eq!(outcome, Ingest::Completed);
            }
        }

        // Directory now answers every slot with the committed points.
        for slot in 0..SLOTS {
            assert_eq!(
                store.directory().pub_nonce(operator, slot),
                batch.pub_nonce(slot).copied()
            );
        }
        assert!(store.has(operator, 0));

        // Serving returns a decodable announce whose points match.
        let served = store.serve(operator, 0, 0).unwrap();
        let bytes = served.encode();
        let decoded = PrecommitMsg::decode(bytes).unwrap();
        let PrecommitMsg::BatchAnnounce { nonces, total, .. } = decoded else {
            panic!("expected announce");
        };
        assert_eq!(total, SLOTS);
        assert_eq!(nonces, batch.nonces[..SLOTS as usize].to_vec());
        assert!(store.serve(operator, 0, SLOTS).is_none());

        // Re-announce of a verified batch is idempotent.
        let again = store.ingest(
            operator,
            key.public_key(),
            0,
            0,
            SLOTS,
            signature,
            0,
            batch.nonces[..16].to_vec(),
        );
        assert_eq!(again, Ingest::Completed);
    }

    #[test]
    fn ingest_rejections() {
        let key = PrivateKey::from_seed(2);
        let operator = key.public_key().eth_address();
        let (batch, signature) = signed_batch(&key, 8);
        let store = BatchStore::new(
            CHAIN_ID,
            registry(),
            Arc::new(MemoryNonceDirectory::default()),
        );

        // Sender/key binding.
        assert_eq!(
            store.ingest(
                Address::repeat_byte(0x99),
                key.public_key(),
                0,
                0,
                8,
                signature,
                0,
                batch.nonces.clone(),
            ),
            Ingest::Rejected(Rejection::SenderMismatch)
        );

        // Oversized / zero totals.
        assert_eq!(
            store.ingest(
                operator,
                key.public_key(),
                0,
                0,
                MAX_BATCH_SLOTS + 1,
                signature,
                0,
                batch.nonces.clone(),
            ),
            Ingest::Rejected(Rejection::BadMetadata)
        );

        // A tampered point set fails registration verification at completion, and the
        // poisoned reassembly is dropped so an honest re-announce succeeds.
        let mut tampered = batch.nonces.clone();
        tampered.swap(0, 1);
        assert_eq!(
            store.ingest(operator, key.public_key(), 0, 0, 8, signature, 0, tampered),
            Ingest::Rejected(Rejection::BadRegistration)
        );
        assert_eq!(
            store.ingest(
                operator,
                key.public_key(),
                0,
                0,
                8,
                signature,
                0,
                batch.nonces.clone(),
            ),
            Ingest::Completed
        );

        // Conflicting duplicate chunk (second batch, partial delivery).
        let key2 = PrivateKey::from_seed(3);
        let (batch2, sig2) = signed_batch(&key2, 32);
        let op2 = key2.public_key().eth_address();
        assert_eq!(
            store.ingest(
                op2,
                key2.public_key(),
                0,
                0,
                32,
                sig2,
                0,
                batch2.nonces[..16].to_vec(),
            ),
            Ingest::Pending
        );
        assert_eq!(
            store.ingest(
                op2,
                key2.public_key(),
                0,
                0,
                32,
                sig2,
                0,
                batch2.nonces[16..].to_vec(),
            ),
            Ingest::Rejected(Rejection::ConflictingChunk)
        );
    }

    /// Full production-API completion flow: 4 operators, one offline, quorum bitmap →
    /// members sign via `sign_completion`, the collector assembles + self-verifies.
    #[test]
    fn completion_round_end_to_end() {
        let mut keys: Vec<PrivateKey> = (0..4).map(|i| PrivateKey::from_seed(200 + i)).collect();
        keys.sort_by_key(|k| k.public_key().eth_address());
        let pubkeys: Vec<SchnorrPublicKey> = keys.iter().map(|k| k.public_key()).collect();
        let directory = Arc::new(MemoryNonceDirectory::default());

        let mut schemes = Vec::new();
        for (i, key) in keys.iter().enumerate() {
            let domain = BatchDomain {
                chain_id: CHAIN_ID,
                registry: registry(),
                operator: key.public_key().eth_address(),
            };
            let seed = derive_batch_seed(key, 0);
            let batch = NonceBatch::generate(domain, &seed, 0, 0, SLOTS).unwrap();
            directory.insert_batch(&batch);
            schemes.push(
                SchnorrScheme::signer(
                    pubkeys.clone(),
                    key.clone(),
                    Arc::new(SeedSecrets::new(domain, seed, 0, SLOTS)),
                    Arc::new(MemorySpendJournal::default()),
                    Arc::clone(&directory) as _,
                    ATTEMPTS,
                )
                .unwrap(),
            );
            assert_eq!(schemes[i].me(), Some(Participant::new(i as u32)));
        }
        let verifier =
            SchnorrScheme::verifier(pubkeys, Arc::clone(&directory) as _, ATTEMPTS).unwrap();

        // Certified bitmap S₁ = {0, 1, 3} (participant 2 was offline for attempt 0).
        let height = 5u64;
        let digest = alloy_primitives::keccak256(b"completion e2e").0;
        let members = [0u32, 1, 3];
        let signers = Signers::from(4, members.iter().map(|i| Participant::new(*i)));

        let mut collector =
            CompletionCollector::new(verifier.clone(), height, 1, digest, &signers).unwrap();
        assert_eq!(collector.missing().len(), 3);

        let mut final_sig = None;
        for (n, i) in members.iter().enumerate() {
            let scheme = &schemes[*i as usize];
            let (r_addr, partial) = scheme
                .sign_completion(height, 1, &digest, &signers)
                .expect("member signs completion");
            assert_eq!(r_addr, collector.r_addr());

            let sender = keys[*i as usize].public_key().eth_address();
            match collector.add(sender, r_addr, partial) {
                Added::Accepted => assert!(n + 1 < members.len()),
                Added::Complete(sig) => {
                    assert_eq!(n + 1, members.len());
                    final_sig = Some(sig);
                }
                other => panic!("unexpected outcome: {other:?}"),
            }
            // Idempotent duplicate.
            if final_sig.is_none() {
                assert_eq!(collector.add(sender, r_addr, partial), Added::Duplicate);
            }
        }

        // The final signature verifies against S₁'s aggregate key — on-chain parity.
        let signature = final_sig.expect("completed");
        let x_s1 = verifier.aggregate_key(&signers).unwrap();
        assert!(verify_aggregate(&x_s1, &digest, &signature));

        // The non-member cannot contribute; wrong r_addr rejected; and the offline
        // member's OWN sign_completion refuses (not in the certified set).
        let outsider = keys[2].public_key().eth_address();
        let (_, stray) = schemes[0]
            .sign_completion(height, 1, &digest, &signers)
            .expect("idempotent re-sign");
        assert_eq!(
            collector.add(outsider, collector.r_addr(), stray),
            Added::Rejected
        );
        assert!(
            schemes[2]
                .sign_completion(height, 1, &digest, &signers)
                .is_none()
        );

        // INVARIANT N1 across attempts: a member asked to complete the SAME attempt
        // under a different digest refuses.
        let other_digest = alloy_primitives::keccak256(b"different task").0;
        assert!(
            schemes[0]
                .sign_completion(height, 1, &other_digest, &signers)
                .is_none()
        );
    }
}

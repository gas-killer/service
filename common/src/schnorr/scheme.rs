//! Aggregate-Schnorr [`certificate::Scheme`] for the aggregation engine, built on
//! **pre-committed nonce batches** ([`super::precommit`]).
//!
//! [`SchnorrScheme`] mirrors [`crate::ecdsa::scheme::EcdsaScheme`] structurally, but the
//! attestation is a MuSig2 **partial signature** for the deterministic full committed set
//! `S₀(h)` (every participant with verified nonce coverage at the height's attempt-0
//! slot), not an independently useful signature. That set-boundness is why the
//! certificate has two forms (see the plan doc §7):
//!
//! * [`SchnorrCertificate::Aggregate`] — the attestation set covered `S₀` exactly, so the
//!   partials sum to a final constant-size signature, on-chain-submittable as-is.
//! * [`SchnorrCertificate::Attested`] — an engine quorum short of `S₀`: an attributable
//!   bundle of verified partials. It certifies *which set* attested (the set agreement a
//!   coordinator used to provide); a completion round re-signs for exactly that set at
//!   the next attempt slot.
//!
//! # Determinism contract
//!
//! All parties must build the scheme from the same operator key set and a
//! [`NonceDirectory`] holding the same registered batches (epoch-snapshot semantics —
//! see the plan's §7.2). A briefly divergent directory only degrades an `Aggregate` into
//! an `Attested` (or fails a partial's verification); it cannot forge.
//!
//! # Invariant N1 — one partial per slot, ever
//!
//! `sign` consults a durable [`SpendJournal`] **before** producing a partial
//! (write-ahead): a slot is bound to the exact signing context's fingerprint on first
//! use, re-signing the *same* context is idempotent (partials are deterministic), and any
//! *different* context for a bound slot is refused. Restart safety therefore lives in the
//! journal implementation, not in process memory.

use super::musig::{Coordinator, PubNonce, SecNonce, SigningContext, partial_sign};
use super::precommit::{BatchDomain, NonceBatch, derive_secnonce, slot_index};
use super::{
    AggregateSignature, MESSAGE_LEN, PrivateKey, PublicKey as SchnorrPublicKey, verify_aggregate,
};
use crate::ecdsa::PublicKey as OperatorKey;
use alloy_primitives::{Address, keccak256};
use bytes::{Buf, BufMut};
use commonware_codec::{EncodeSize, Error, FixedSize, Read, ReadExt, Write};
use commonware_consensus::aggregation::types::Item;
use commonware_cryptography::Digest;
use commonware_cryptography::certificate::{Attestation, Scheme, Signers};
use commonware_parallel::Strategy;
use commonware_utils::ordered::{Quorum, Set};
use commonware_utils::{Faults, Participant, TryCollect};
use k256::Scalar;
use k256::elliptic_curve::PrimeField;
use rand_core::CryptoRngCore;
use std::collections::HashMap;
use std::fmt::{self, Debug};
use std::sync::{Arc, Mutex, RwLock};

/// Default `MAX_ATTEMPTS` in the `idx = height · attempts + attempt` slot mapping.
/// Attempt 0 is the engine ack; attempts ≥ 1 belong to the completion round.
pub const DEFAULT_ATTEMPTS_PER_HEIGHT: u32 = 4;

/// Read access to every operator's **verified** committed public nonces (fed by batch
/// gossip checked against the on-chain `SchnorrNonceRegistry` roots).
pub trait NonceDirectory: Debug + Send + Sync {
    /// The committed nonce pair for `(operator identity address, absolute slot)`, or
    /// `None` if the directory holds no verified coverage there.
    fn pub_nonce(&self, operator: Address, slot: u64) -> Option<PubNonce>;
}

/// This operator's own derivable secret nonces (seed-scoped; see
/// [`super::precommit::derive_secnonce`]).
pub trait SecretNonces: Debug + Send + Sync {
    fn sec_nonce(&self, slot: u64) -> Option<SecNonce>;
}

/// Durable enforcement of invariant N1.
///
/// `bind` MUST persist before returning `true` in production implementations (fsync
/// write-ahead): a partial may be emitted the instant it returns.
pub trait SpendJournal: Debug + Send + Sync {
    /// Binds `slot` to a signing-context `fingerprint`. Returns `true` iff the slot is
    /// fresh or already bound to this exact fingerprint (idempotent re-sign); `false`
    /// means the slot was consumed under a DIFFERENT context — the caller MUST refuse.
    fn bind(&self, slot: u64, fingerprint: &[u8; 32]) -> bool;
}

/// In-memory [`NonceDirectory`] (tests and single-process wiring; the live node feeds a
/// persisted, gossip-backed implementation).
#[derive(Debug, Default)]
pub struct MemoryNonceDirectory {
    nonces: RwLock<HashMap<(Address, u64), PubNonce>>,
}

impl MemoryNonceDirectory {
    pub fn insert(&self, operator: Address, slot: u64, nonce: PubNonce) {
        self.nonces
            .write()
            .expect("nonce directory lock")
            .insert((operator, slot), nonce);
    }

    /// Ingests a whole verified batch (keyed by the batch domain's operator).
    pub fn insert_batch(&self, batch: &NonceBatch) {
        let mut nonces = self.nonces.write().expect("nonce directory lock");
        for (i, nonce) in batch.nonces.iter().enumerate() {
            nonces.insert((batch.domain.operator, batch.start_slot + i as u64), *nonce);
        }
    }
}

impl NonceDirectory for MemoryNonceDirectory {
    fn pub_nonce(&self, operator: Address, slot: u64) -> Option<PubNonce> {
        self.nonces
            .read()
            .expect("nonce directory lock")
            .get(&(operator, slot))
            .copied()
    }
}

/// Seed-backed [`SecretNonces`] over one batch's slot range.
pub struct SeedSecrets {
    domain: BatchDomain,
    seed: [u8; 32],
    start_slot: u64,
    end_slot: u64,
}

impl SeedSecrets {
    pub fn new(domain: BatchDomain, seed: [u8; 32], start_slot: u64, count: u64) -> Self {
        Self {
            domain,
            seed,
            start_slot,
            end_slot: start_slot.saturating_add(count),
        }
    }
}

impl Debug for SeedSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the seed — it is key-equivalent material.
        f.debug_struct("SeedSecrets")
            .field("operator", &self.domain.operator)
            .field("start_slot", &self.start_slot)
            .field("end_slot", &self.end_slot)
            .finish_non_exhaustive()
    }
}

impl SecretNonces for SeedSecrets {
    fn sec_nonce(&self, slot: u64) -> Option<SecNonce> {
        (slot >= self.start_slot && slot < self.end_slot)
            .then(|| derive_secnonce(&self.domain, &self.seed, slot))
    }
}

/// In-memory [`SpendJournal`] — NOT restart-safe; tests and harnesses only.
#[derive(Debug, Default)]
pub struct MemorySpendJournal {
    bound: Mutex<HashMap<u64, [u8; 32]>>,
}

impl SpendJournal for MemorySpendJournal {
    fn bind(&self, slot: u64, fingerprint: &[u8; 32]) -> bool {
        match self.bound.lock().expect("spend journal lock").entry(slot) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(*fingerprint);
                true
            }
            std::collections::hash_map::Entry::Occupied(entry) => entry.get() == fingerprint,
        }
    }
}

/// Extracts the raw 32 digest bytes from an [`Item`]'s digest (32-byte digests only,
/// mirroring `EcdsaScheme`).
fn item_digest<D: Digest>(item: &Item<D>) -> Option<[u8; MESSAGE_LEN]> {
    item.digest.as_ref().try_into().ok()
}

/// Collision-resistant fingerprint of a slot's full signing context — what the
/// [`SpendJournal`] binds a slot to. Any change to the message, signer set (via the
/// aggregates), or effective nonce changes the fingerprint.
fn context_fingerprint(slot: u64, ctx: &SigningContext) -> [u8; 32] {
    let mut pre = Vec::with_capacity(8 + MESSAGE_LEN + 20 + 66 + 33);
    pre.extend_from_slice(&slot.to_be_bytes());
    pre.extend_from_slice(&ctx.message);
    pre.extend_from_slice(ctx.r_addr.as_slice());
    pre.extend_from_slice(&ctx.agg_nonces().to_bytes());
    pre.extend_from_slice(&ctx.x_agg.to_compressed());
    keccak256(pre).0
}

/// Signer half of the scheme (index + key + secret-nonce access + spend journal).
#[derive(Clone)]
struct SchnorrSigner {
    index: Participant,
    key: Arc<PrivateKey>,
    secrets: Arc<dyn SecretNonces>,
    journal: Arc<dyn SpendJournal>,
}

impl Debug for SchnorrSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchnorrSigner")
            .field("index", &self.index)
            .field("secrets", &self.secrets)
            .finish_non_exhaustive()
    }
}

/// The deterministic attempt-0 signing context for one height.
struct HeightContext {
    /// Absolute slot `idx(height, 0)`.
    slot: u64,
    /// `S₀`: participants with committed nonces at `slot`, ascending by index.
    signers: Vec<Participant>,
    /// Committed nonce pairs, index-aligned with `signers`.
    nonces: Vec<PubNonce>,
    ctx: SigningContext,
}

/// Aggregate-Schnorr certificate scheme over a fixed participant set.
///
/// Participant identity keys are the operators' 20-byte Ethereum addresses (the same
/// p2p/EigenLayer identity `EcdsaScheme` uses — the Schnorr key is the same secp256k1
/// point, so the address is identical); indices are positions in the ordered address
/// set and MUST match on every process.
#[derive(Clone)]
pub struct SchnorrScheme {
    /// Ordered operator identity addresses; participant indices are positions here.
    participants: Set<OperatorKey>,
    /// Operator Schnorr public-key points, index-aligned with `participants`.
    keys: Arc<[SchnorrPublicKey]>,
    directory: Arc<dyn NonceDirectory>,
    attempts_per_height: u32,
    signer: Option<SchnorrSigner>,
}

impl Debug for SchnorrScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchnorrScheme")
            .field("participants", &self.participants)
            .field("attempts_per_height", &self.attempts_per_height)
            .field("signer", &self.signer)
            .finish_non_exhaustive()
    }
}

impl SchnorrScheme {
    /// Orders `keys` into the participant set and index-aligned point vector.
    ///
    /// Returns `None` on duplicate keys. Panics on an empty set (construction-time
    /// configuration error, mirroring `EcdsaScheme`).
    fn build_participants(
        keys: Vec<SchnorrPublicKey>,
    ) -> Option<(Set<OperatorKey>, Arc<[SchnorrPublicKey]>)> {
        assert!(
            !keys.is_empty(),
            "participant set must not be empty (quorum math panics on n == 0)"
        );
        let participants: Set<OperatorKey> = keys
            .iter()
            .map(|k| OperatorKey::from(k.eth_address()))
            .try_collect()
            .ok()?;
        let mut aligned: Vec<Option<SchnorrPublicKey>> = vec![None; keys.len()];
        for key in keys {
            let index = participants.index(&OperatorKey::from(key.eth_address()))?;
            aligned[usize::from(index)] = Some(key);
        }
        let keys: Option<Vec<SchnorrPublicKey>> = aligned.into_iter().collect();
        Some((participants, keys?.into()))
    }

    /// Creates a signing scheme instance for the operator holding `private_key`.
    ///
    /// Returns `None` if the key set has duplicates or `private_key`'s identity address
    /// is not in it.
    pub fn signer(
        keys: Vec<SchnorrPublicKey>,
        private_key: PrivateKey,
        secrets: Arc<dyn SecretNonces>,
        journal: Arc<dyn SpendJournal>,
        directory: Arc<dyn NonceDirectory>,
        attempts_per_height: u32,
    ) -> Option<Self> {
        let (participants, aligned) = Self::build_participants(keys)?;
        let identity = OperatorKey::from(private_key.public_key().eth_address());
        let index = participants.index(&identity)?;
        Some(Self {
            participants,
            keys: aligned,
            directory,
            attempts_per_height,
            signer: Some(SchnorrSigner {
                index,
                key: Arc::new(private_key),
                secrets,
                journal,
            }),
        })
    }

    /// Creates a verifier-only instance (`me() == None`) — what the router runs.
    pub fn verifier(
        keys: Vec<SchnorrPublicKey>,
        directory: Arc<dyn NonceDirectory>,
        attempts_per_height: u32,
    ) -> Option<Self> {
        let (participants, aligned) = Self::build_participants(keys)?;
        Some(Self {
            participants,
            keys: aligned,
            directory,
            attempts_per_height,
            signer: None,
        })
    }

    pub fn attempts_per_height(&self) -> u32 {
        self.attempts_per_height
    }

    /// The Schnorr public-key point of a participant.
    pub fn schnorr_key(&self, participant: Participant) -> Option<&SchnorrPublicKey> {
        self.keys.get(usize::from(participant))
    }

    /// Maps a certificate bitmap to the signers' operator identity addresses
    /// (ascending — participant order is address-byte order).
    pub fn signer_identity_addresses(&self, signers: &Signers) -> Vec<Address> {
        signers
            .iter()
            .filter_map(|p| self.schnorr_key(p).map(|k| k.eth_address()))
            .collect()
    }

    /// The complement of a certificate bitmap as operator identity addresses, strictly
    /// ascending — exactly the `nonSigners` list `SchnorrStakeRegistry.isValidSignature`
    /// subtracts on-chain.
    pub fn non_signer_identity_addresses(&self, signers: &Signers) -> Vec<Address> {
        let included: std::collections::HashSet<u32> = signers.iter().map(|p| p.get()).collect();
        (0..self.participants.len() as u32)
            .filter(|i| !included.contains(i))
            .filter_map(|i| {
                self.schnorr_key(Participant::new(i))
                    .map(|k| k.eth_address())
            })
            .collect()
    }

    /// The plain aggregate key `Σ X_i` over a certificate bitmap.
    pub fn aggregate_key(&self, signers: &Signers) -> Option<SchnorrPublicKey> {
        let keys: Vec<&SchnorrPublicKey> = signers
            .iter()
            .map(|p| self.schnorr_key(p))
            .collect::<Option<_>>()?;
        SchnorrPublicKey::aggregate(keys)
    }

    /// `S₀` membership at `height`: every participant with committed coverage at the
    /// attempt-0 slot, in ascending index order, with their committed nonces.
    fn coverage_at(&self, height: u64) -> Option<(u64, Vec<Participant>, Vec<PubNonce>)> {
        let slot = slot_index(height, 0, self.attempts_per_height)?;
        let mut signers = Vec::new();
        let mut nonces = Vec::new();
        for (i, key) in self.keys.iter().enumerate() {
            if let Some(nonce) = self.directory.pub_nonce(key.eth_address(), slot) {
                signers.push(Participant::new(i as u32));
                nonces.push(nonce);
            }
        }
        if signers.is_empty() {
            return None;
        }
        Some((slot, signers, nonces))
    }

    /// Builds the full attempt-0 context for an item (`S₀`, aggregates, `b`/`e` inputs).
    fn context_for<D: Digest>(&self, item: &Item<D>) -> Option<HeightContext> {
        let digest = item_digest(item)?;
        let (slot, signers, nonces) = self.coverage_at(item.height.get())?;
        let keys: Vec<&SchnorrPublicKey> = signers
            .iter()
            .map(|p| self.schnorr_key(*p))
            .collect::<Option<_>>()?;
        let x_agg = SchnorrPublicKey::aggregate(keys)?;
        let ctx = SigningContext::derive(x_agg, nonces.iter(), &digest)?;
        Some(HeightContext {
            slot,
            signers,
            nonces,
            ctx,
        })
    }

    /// Builds the signing context for a **completion attempt** (`attempt ≥ 1`): the
    /// signer set is the engine-certified bitmap (not coverage-derived), evaluated at
    /// that attempt's slot. Returns `None` if any bitmap member lacks committed
    /// coverage at the slot, the bitmap is empty/oversized, or the attempt is out of
    /// range.
    pub fn completion_context(
        &self,
        height: u64,
        attempt: u32,
        digest: &[u8; MESSAGE_LEN],
        signers: &Signers,
    ) -> Option<CompletionContext> {
        if attempt == 0 || signers.len() != self.participants.len() {
            return None;
        }
        let slot = slot_index(height, attempt, self.attempts_per_height)?;
        let members: Vec<Participant> = signers.iter().collect();
        if members.is_empty() {
            return None;
        }
        let mut nonces = Vec::with_capacity(members.len());
        for participant in &members {
            let key = self.schnorr_key(*participant)?;
            nonces.push(self.directory.pub_nonce(key.eth_address(), slot)?);
        }
        let x_agg = self.aggregate_key(signers)?;
        let ctx = SigningContext::derive(x_agg, nonces.iter(), digest)?;
        Some(CompletionContext {
            slot,
            members,
            nonces,
            ctx,
        })
    }

    /// Produces this operator's completion-round partial for a certified signer set.
    ///
    /// Same rules as [`Scheme::sign`]: `None` when verifier-only, not a member of the
    /// set, missing coverage/secrets — or the invariant-N1 refusal when the attempt's
    /// slot is already bound to a different context. Idempotent for the same context.
    pub fn sign_completion(
        &self,
        height: u64,
        attempt: u32,
        digest: &[u8; MESSAGE_LEN],
        signers: &Signers,
    ) -> Option<(Address, Scalar)> {
        let signer = self.signer.as_ref()?;
        let cc = self.completion_context(height, attempt, digest, signers)?;
        cc.members.binary_search(&signer.index).ok()?;
        let fingerprint = context_fingerprint(cc.slot, &cc.ctx);
        if !signer.journal.bind(cc.slot, &fingerprint) {
            return None;
        }
        let sec = signer.secrets.sec_nonce(cc.slot)?;
        let partial = partial_sign(sec, &signer.key, &cc.ctx)?;
        Some((cc.ctx.r_addr, partial))
    }

    /// Verifies one completion partial against a member's committed slot nonce.
    pub fn verify_completion_partial(
        &self,
        cc: &CompletionContext,
        participant: Participant,
        partial: &Scalar,
    ) -> bool {
        let Some(key) = self.schnorr_key(participant) else {
            return false;
        };
        let Ok(position) = cc.members.binary_search(&participant) else {
            return false;
        };
        Coordinator::verify_partial(&cc.ctx, key, &cc.nonces[position], partial)
    }
}

/// A completion attempt's derived signing context (see
/// [`SchnorrScheme::completion_context`]).
pub struct CompletionContext {
    /// Absolute slot `idx(height, attempt)`.
    pub slot: u64,
    /// The certified signer set, ascending by participant index.
    pub members: Vec<Participant>,
    /// Committed nonce pairs, index-aligned with `members`.
    nonces: Vec<PubNonce>,
    /// The derived MuSig2 context (exposes `r_addr`, aggregates, message).
    pub ctx: SigningContext,
}

impl Scheme for SchnorrScheme {
    type Subject<'a, D: Digest> = &'a Item<D>;
    type PublicKey = OperatorKey;
    type Signature = SchnorrPartial;
    type Certificate = SchnorrCertificate;

    fn me(&self) -> Option<Participant> {
        self.signer.as_ref().map(|signer| signer.index)
    }

    fn participants(&self) -> &Set<OperatorKey> {
        &self.participants
    }

    /// Produces this operator's MuSig2 partial for the item's attempt-0 context.
    ///
    /// Returns `None` when: verifier-only; the digest is not 32 bytes; we hold no
    /// coverage at the slot (we are not in `S₀`); the secret nonce is underivable; or —
    /// the invariant-N1 refusal — the spend journal has the slot bound to a different
    /// context. Re-signing the same context returns the identical attestation
    /// (deterministic nonces ⇒ deterministic partial).
    fn sign<D: Digest>(&self, subject: Self::Subject<'_, D>) -> Option<Attestation<Self>> {
        let signer = self.signer.as_ref()?;
        let hc = self.context_for(subject)?;
        if hc.signers.binary_search(&signer.index).is_err() {
            return None;
        }
        // Write-ahead: bind the slot to this exact context BEFORE producing the partial.
        let fingerprint = context_fingerprint(hc.slot, &hc.ctx);
        if !signer.journal.bind(hc.slot, &fingerprint) {
            return None;
        }
        let sec = signer.secrets.sec_nonce(hc.slot)?;
        // `partial_sign` re-derives b/e and rejects a degenerate/inconsistent R.
        let partial = partial_sign(sec, &signer.key, &hc.ctx)?;
        Some(Attestation {
            signer: signer.index,
            signature: SchnorrPartial::new(subject.height.get(), hc.ctx.r_addr, partial).into(),
        })
    }

    /// Verifies a partial against the sender's **committed** slot nonce and the
    /// deterministic attempt-0 context: `s_i·G == R1_i + b·R2_i − e·X_i`.
    fn verify_attestation<R, D>(
        &self,
        _rng: &mut R,
        subject: Self::Subject<'_, D>,
        attestation: &Attestation<Self>,
        _strategy: &impl Strategy,
    ) -> bool
    where
        R: CryptoRngCore,
        D: Digest,
    {
        // Lazy decode of untrusted bytes: `None` means malformed — reject, never panic.
        let Some(signature) = attestation.signature.get() else {
            return false;
        };
        if signature.height != subject.height.get() {
            return false;
        }
        let Some(key) = self.schnorr_key(attestation.signer) else {
            return false;
        };
        let Some(hc) = self.context_for(subject) else {
            return false;
        };
        // The claimed effective nonce must match the derived context (binds the partial
        // to S₀ as this process knows it).
        if signature.r_addr != hc.ctx.r_addr {
            return false;
        }
        let Ok(position) = hc.signers.binary_search(&attestation.signer) else {
            return false; // signer has no committed coverage at this slot
        };
        let Some(partial) = signature.scalar() else {
            return false;
        };
        Coordinator::verify_partial(&hc.ctx, key, &hc.nonces[position], &partial)
    }

    /// Assembles verified partials into a certificate:
    ///
    /// * attestation set == `S₀` → [`SchnorrCertificate::Aggregate`] (partials summed);
    /// * engine quorum but ⊊ `S₀` → [`SchnorrCertificate::Attested`] (partial bundle).
    ///
    /// All attestations must share one height and one claimed `address(R)` — mixed
    /// contexts cannot combine and yield `None`.
    fn assemble<I, M>(
        &self,
        attestations: I,
        _strategy: &impl Strategy,
    ) -> Option<Self::Certificate>
    where
        I: IntoIterator<Item = Attestation<Self>>,
        I::IntoIter: Send,
        M: Faults,
    {
        let mut entries: Vec<(Participant, SchnorrPartial)> = Vec::new();
        for Attestation { signer, signature } in attestations {
            if usize::from(signer) >= self.participants.len() {
                return None;
            }
            entries.push((signer, signature.get().cloned()?));
        }
        let (height, r_addr) = {
            let first = entries.first()?;
            (first.1.height, first.1.r_addr)
        };
        if entries
            .iter()
            .any(|(_, partial)| partial.height != height || partial.r_addr != r_addr)
        {
            return None;
        }
        if entries.len() < self.participants.quorum::<M>() as usize {
            return None;
        }
        entries.sort_by_key(|(signer, _)| *signer);

        // Exactly-S₀ detection (no digest needed — coverage is height-only).
        let full_set = self.coverage_at(height).is_some_and(|(_, signers, _)| {
            signers.len() == entries.len()
                && signers
                    .iter()
                    .zip(entries.iter())
                    .all(|(covered, (signer, _))| covered == signer)
        });

        let signers = Signers::from(
            self.participants.len(),
            entries.iter().map(|(signer, _)| *signer),
        );

        if full_set {
            let mut sum = Scalar::ZERO;
            for (_, partial) in &entries {
                sum += partial.scalar()?;
            }
            if bool::from(sum.is_zero()) {
                return None;
            }
            Some(SchnorrCertificate::Aggregate {
                height,
                r_addr,
                signers,
                s: sum.to_bytes().into(),
            })
        } else {
            Some(SchnorrCertificate::Attested {
                height,
                r_addr,
                signers,
                partials: entries.into_iter().map(|(_, partial)| partial.s).collect(),
            })
        }
    }

    fn verify_certificate<R, D, M>(
        &self,
        _rng: &mut R,
        subject: Self::Subject<'_, D>,
        certificate: &Self::Certificate,
        _strategy: &impl Strategy,
    ) -> bool
    where
        R: CryptoRngCore,
        D: Digest,
        M: Faults,
    {
        let signers = certificate.signers();
        // Exact participant-set sizing (a resized bitmap would shift signer indices).
        if signers.len() != self.participants.len() {
            return false;
        }
        if signers.count() < self.participants.quorum::<M>() as usize {
            return false;
        }
        if certificate.height() != subject.height.get() {
            return false;
        }
        let Some(digest) = item_digest(subject) else {
            return false;
        };

        match certificate {
            SchnorrCertificate::Aggregate {
                r_addr, signers, s, ..
            } => {
                // Verified with the exact on-chain `ecrecover` identity against the
                // bitmap subset's aggregate key — no nonce knowledge needed.
                let Some(x_agg) = self.aggregate_key(signers) else {
                    return false;
                };
                let Some(s) = canonical_scalar(s) else {
                    return false;
                };
                if bool::from(s.is_zero()) {
                    return false;
                }
                verify_aggregate(&x_agg, &digest, &AggregateSignature { s, r_addr: *r_addr })
            }
            SchnorrCertificate::Attested {
                r_addr,
                signers,
                partials,
                ..
            } => {
                if signers.count() != partials.len() {
                    return false;
                }
                let Some(hc) = self.context_for(subject) else {
                    return false;
                };
                if hc.ctx.r_addr != *r_addr {
                    return false;
                }
                for (signer, partial_bytes) in signers.iter().zip(partials) {
                    let Some(key) = self.schnorr_key(signer) else {
                        return false;
                    };
                    let Ok(position) = hc.signers.binary_search(&signer) else {
                        return false; // bitmap signer outside S₀
                    };
                    let Some(partial) = canonical_scalar(partial_bytes) else {
                        return false;
                    };
                    if !Coordinator::verify_partial(&hc.ctx, key, &hc.nonces[position], &partial) {
                        return false;
                    }
                }
                true
            }
        }
    }

    fn is_attributable() -> bool {
        // Both certificate forms carry the signer bitmap; `Attested` partials are
        // individually verifiable, and an `Aggregate` names exactly its subset.
        true
    }

    fn is_batchable() -> bool {
        false
    }

    fn certificate_codec_config(&self) -> <Self::Certificate as Read>::Cfg {
        self.participants.len()
    }

    fn certificate_codec_config_unbounded() -> <Self::Certificate as Read>::Cfg {
        u32::MAX as usize
    }
}

/// Decodes a canonical (`< n`) scalar from 32 big-endian bytes.
fn canonical_scalar(bytes: &[u8; 32]) -> Option<Scalar> {
    Option::<Scalar>::from(Scalar::from_repr((*bytes).into()))
}

/// A MuSig2 partial signature bound to its session: the height (⇒ slot), the claimed
/// effective nonce address (binds the context), and the partial scalar `s_i`.
///
/// Fixed 60-byte codec: `height₈ ‖ Raddr₂₀ ‖ s₃₂`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SchnorrPartial {
    /// The aggregation height this partial belongs to (attempt 0 of it).
    pub height: u64,
    /// The signer's view of `address(R)` for the attempt-0 context.
    pub r_addr: Address,
    /// Big-endian partial scalar (canonical form enforced at decode).
    s: [u8; 32],
}

impl SchnorrPartial {
    pub fn new(height: u64, r_addr: Address, partial: Scalar) -> Self {
        Self {
            height,
            r_addr,
            s: partial.to_bytes().into(),
        }
    }

    /// The partial scalar (`None` only for a non-canonical encoding, which decode
    /// already rejects).
    pub fn scalar(&self) -> Option<Scalar> {
        canonical_scalar(&self.s)
    }
}

impl FixedSize for SchnorrPartial {
    const SIZE: usize = 8 + 20 + 32;
}

impl Write for SchnorrPartial {
    fn write(&self, buf: &mut impl BufMut) {
        self.height.write(buf);
        buf.put_slice(self.r_addr.as_slice());
        buf.put_slice(&self.s);
    }
}

impl Read for SchnorrPartial {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _: &()) -> Result<Self, Error> {
        let height = u64::read(buf)?;
        let r_addr = Address::from(read_array::<20>(buf)?);
        if r_addr == Address::ZERO {
            return Err(Error::Invalid("schnorr::SchnorrPartial", "zero R address"));
        }
        let s = read_array::<32>(buf)?;
        if canonical_scalar(&s).is_none() {
            return Err(Error::Invalid(
                "schnorr::SchnorrPartial",
                "partial scalar out of range",
            ));
        }
        Ok(Self { height, r_addr, s })
    }
}

/// Certificate for one aggregation item under the pre-committed-nonce Schnorr scheme.
///
/// Codec (`Read::Cfg` = max participant count):
/// `tag₁ ‖ height₈ ‖ Raddr₂₀ ‖ signers ‖ (s₃₂ | partials: count·32)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SchnorrCertificate {
    /// The attestation set covered `S₀` exactly: a final aggregate signature
    /// `(s, address(R))`, verifiable on-chain in constant gas against the bitmap
    /// subset's aggregate key.
    Aggregate {
        height: u64,
        r_addr: Address,
        signers: Signers,
        /// Big-endian aggregate scalar `s = Σ s_i`.
        s: [u8; 32],
    },
    /// An engine quorum short of `S₀`: attributable partials (ascending participant
    /// order, bitmap-aligned) pending the completion round for exactly this signer set.
    Attested {
        height: u64,
        r_addr: Address,
        signers: Signers,
        /// Big-endian partial scalars, index-aligned with the bitmap's iteration.
        partials: Vec<[u8; 32]>,
    },
}

const TAG_AGGREGATE: u8 = 0;
const TAG_ATTESTED: u8 = 1;

impl SchnorrCertificate {
    pub fn height(&self) -> u64 {
        match self {
            Self::Aggregate { height, .. } | Self::Attested { height, .. } => *height,
        }
    }

    pub fn r_addr(&self) -> Address {
        match self {
            Self::Aggregate { r_addr, .. } | Self::Attested { r_addr, .. } => *r_addr,
        }
    }

    pub fn signers(&self) -> &Signers {
        match self {
            Self::Aggregate { signers, .. } | Self::Attested { signers, .. } => signers,
        }
    }

    /// The submittable aggregate signature, if this certificate is final.
    pub fn aggregate_signature(&self) -> Option<AggregateSignature> {
        match self {
            Self::Aggregate { r_addr, s, .. } => {
                let s = canonical_scalar(s)?;
                if bool::from(s.is_zero()) {
                    return None;
                }
                Some(AggregateSignature { s, r_addr: *r_addr })
            }
            Self::Attested { .. } => None,
        }
    }
}

impl Write for SchnorrCertificate {
    fn write(&self, buf: &mut impl BufMut) {
        match self {
            Self::Aggregate {
                height,
                r_addr,
                signers,
                s,
            } => {
                TAG_AGGREGATE.write(buf);
                height.write(buf);
                buf.put_slice(r_addr.as_slice());
                signers.write(buf);
                buf.put_slice(s);
            }
            Self::Attested {
                height,
                r_addr,
                signers,
                partials,
            } => {
                TAG_ATTESTED.write(buf);
                height.write(buf);
                buf.put_slice(r_addr.as_slice());
                signers.write(buf);
                for partial in partials {
                    buf.put_slice(partial);
                }
            }
        }
    }
}

impl EncodeSize for SchnorrCertificate {
    fn encode_size(&self) -> usize {
        let header = 1 + 8 + 20;
        match self {
            Self::Aggregate { signers, .. } => header + signers.encode_size() + 32,
            Self::Attested {
                signers, partials, ..
            } => header + signers.encode_size() + partials.len() * 32,
        }
    }
}

impl Read for SchnorrCertificate {
    type Cfg = usize;

    fn read_cfg(reader: &mut impl Buf, max_participants: &usize) -> Result<Self, Error> {
        let tag = u8::read(reader)?;
        let height = u64::read(reader)?;
        let r_addr = Address::from(read_array::<20>(reader)?);
        if r_addr == Address::ZERO {
            return Err(Error::Invalid(
                "schnorr::SchnorrCertificate",
                "zero R address",
            ));
        }
        let signers = Signers::read_cfg(reader, max_participants)?;
        if signers.count() == 0 {
            return Err(Error::Invalid(
                "schnorr::SchnorrCertificate",
                "certificate contains no signers",
            ));
        }
        match tag {
            TAG_AGGREGATE => {
                let s = read_array::<32>(reader)?;
                match canonical_scalar(&s) {
                    Some(scalar) if !bool::from(scalar.is_zero()) => {}
                    _ => {
                        return Err(Error::Invalid(
                            "schnorr::SchnorrCertificate",
                            "aggregate scalar out of range",
                        ));
                    }
                }
                Ok(Self::Aggregate {
                    height,
                    r_addr,
                    signers,
                    s,
                })
            }
            TAG_ATTESTED => {
                let mut partials = Vec::with_capacity(signers.count());
                for _ in 0..signers.count() {
                    let partial = read_array::<32>(reader)?;
                    if canonical_scalar(&partial).is_none() {
                        return Err(Error::Invalid(
                            "schnorr::SchnorrCertificate",
                            "partial scalar out of range",
                        ));
                    }
                    partials.push(partial);
                }
                Ok(Self::Attested {
                    height,
                    r_addr,
                    signers,
                    partials,
                })
            }
            other => Err(Error::InvalidEnum(other)),
        }
    }
}

/// Reads exactly `N` bytes off the buffer.
fn read_array<const N: usize>(buf: &mut impl Buf) -> Result<[u8; N], Error> {
    if buf.remaining() < N {
        return Err(Error::EndOfBuffer);
    }
    let mut out = [0u8; N];
    buf.copy_to_slice(&mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_codec::{Decode, DecodeExt, Encode};
    use commonware_consensus::aggregation::types::{Ack, Certificate as AggCertificate};
    use commonware_consensus::types::{Epoch, Height};
    use commonware_cryptography::sha256::Digest as Sha256Digest;
    use commonware_cryptography::{Hasher as _, Sha256};
    use commonware_parallel::Sequential;
    use commonware_utils::{N3f1, test_rng};

    const ATTEMPTS: u32 = 4;
    const SLOTS: u64 = 256; // covers heights 0..64 at 4 attempts/height

    struct Fixture {
        schemes: Vec<SchnorrScheme>,
        verifier: SchnorrScheme,
        keys: Vec<PrivateKey>,
        directory: Arc<MemoryNonceDirectory>,
        domains: Vec<BatchDomain>,
        seeds: Vec<[u8; 32]>,
    }

    /// Builds `n` operators with registered coverage for slots `[0, SLOTS)`; operators
    /// listed in `uncovered` get NO directory coverage (batch never registered).
    ///
    /// All fixture vectors (`schemes`, `keys`, `domains`, `seeds`) are in PARTICIPANT
    /// order — keys are pre-sorted by identity address, which is exactly the ordered
    /// set's participant ordering — so index `i` everywhere means participant `i`.
    fn setup_with(n: usize, uncovered: &[usize]) -> Fixture {
        let mut keys: Vec<PrivateKey> = (0..n)
            .map(|i| PrivateKey::from_seed(100 + i as u64))
            .collect();
        keys.sort_by_key(|k| k.public_key().eth_address());
        let pubkeys: Vec<SchnorrPublicKey> = keys.iter().map(|k| k.public_key()).collect();
        let directory = Arc::new(MemoryNonceDirectory::default());
        let registry = Address::repeat_byte(0x42);

        let mut domains = Vec::new();
        let mut seeds = Vec::new();
        for (i, key) in keys.iter().enumerate() {
            let domain = BatchDomain {
                chain_id: 31337,
                registry,
                operator: key.public_key().eth_address(),
            };
            let seed = [i as u8 + 1; 32];
            if !uncovered.contains(&i) {
                let batch = NonceBatch::generate(domain, &seed, 0, 0, SLOTS).unwrap();
                directory.insert_batch(&batch);
            }
            domains.push(domain);
            seeds.push(seed);
        }

        let schemes: Vec<SchnorrScheme> = keys
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let scheme = SchnorrScheme::signer(
                    pubkeys.clone(),
                    key.clone(),
                    Arc::new(SeedSecrets::new(domains[i], seeds[i], 0, SLOTS)),
                    Arc::new(MemorySpendJournal::default()),
                    directory.clone(),
                    ATTEMPTS,
                )
                .expect("key is in participant set");
                // Address-sorted keys ⇒ scheme i IS participant i.
                assert_eq!(scheme.me(), Some(Participant::new(i as u32)));
                scheme
            })
            .collect();
        let verifier =
            SchnorrScheme::verifier(pubkeys, directory.clone(), ATTEMPTS).expect("verifier");
        Fixture {
            schemes,
            verifier,
            keys,
            directory,
            domains,
            seeds,
        }
    }

    fn setup(n: usize) -> Fixture {
        setup_with(n, &[])
    }

    fn item(height: u64, payload: &[u8]) -> Item<Sha256Digest> {
        let mut hasher = Sha256::new();
        hasher.update(payload);
        Item {
            height: Height::new(height),
            digest: hasher.finalize(),
        }
    }

    fn sign_all(
        schemes: &[SchnorrScheme],
        item: &Item<Sha256Digest>,
        count: usize,
    ) -> Vec<Attestation<SchnorrScheme>> {
        schemes
            .iter()
            .take(count)
            .map(|s| s.sign::<Sha256Digest>(item).expect("signer can sign"))
            .collect()
    }

    #[test]
    fn attestation_roundtrip() {
        let mut rng = test_rng();
        let fixture = setup(4);
        let subject = item(1, b"task payload");

        for scheme in &fixture.schemes {
            let attestation = scheme.sign::<Sha256Digest>(&subject).unwrap();
            assert_eq!(Some(attestation.signer), scheme.me());
            assert!(fixture.verifier.verify_attestation::<_, Sha256Digest>(
                &mut rng,
                &subject,
                &attestation,
                &Sequential,
            ));

            // Wrong digest rejected (context/e mismatch).
            let wrong = item(1, b"different payload");
            assert!(!fixture.verifier.verify_attestation::<_, Sha256Digest>(
                &mut rng,
                &wrong,
                &attestation,
                &Sequential,
            ));

            // Attributing the partial to another participant rejected.
            let mut reattributed = attestation.clone();
            reattributed.signer =
                Participant::new((attestation.signer.get() + 1) % fixture.schemes.len() as u32);
            assert!(!fixture.verifier.verify_attestation::<_, Sha256Digest>(
                &mut rng,
                &subject,
                &reattributed,
                &Sequential,
            ));

            // Out-of-range signer index rejected (not panicking).
            let mut out_of_range = attestation.clone();
            out_of_range.signer = Participant::new(999);
            assert!(!fixture.verifier.verify_attestation::<_, Sha256Digest>(
                &mut rng,
                &subject,
                &out_of_range,
                &Sequential,
            ));

            // Wrong height rejected even with the same digest.
            let other_height = item(2, b"task payload");
            assert!(!fixture.verifier.verify_attestation::<_, Sha256Digest>(
                &mut rng,
                &other_height,
                &attestation,
                &Sequential,
            ));
        }
    }

    #[test]
    fn verifier_cannot_sign() {
        let fixture = setup(4);
        assert!(fixture.verifier.me().is_none());
        assert!(
            fixture
                .verifier
                .sign::<Sha256Digest>(&item(1, b"x"))
                .is_none()
        );
    }

    // Unlike ECDSA mode, the partial is height-BOUND: the same digest at two heights
    // consumes two different slots and produces different partials.
    #[test]
    fn partial_is_height_bound() {
        let fixture = setup(4);
        let a = fixture.schemes[0]
            .sign::<Sha256Digest>(&item(1, b"same payload"))
            .unwrap();
        let b = fixture.schemes[0]
            .sign::<Sha256Digest>(&item(2, b"same payload"))
            .unwrap();
        assert_ne!(a.signature, b.signature);
    }

    // INVARIANT N1: one partial per slot, ever. A different digest for an
    // already-signed height is refused; the identical context re-signs idempotently.
    #[test]
    fn spend_journal_refuses_second_context_and_allows_idempotent_resign() {
        let fixture = setup(4);
        let scheme = &fixture.schemes[0];
        let original = item(5, b"first context");

        let first = scheme.sign::<Sha256Digest>(&original).unwrap();

        // Same height, different digest → different context → REFUSED.
        assert!(
            scheme
                .sign::<Sha256Digest>(&item(5, b"second context"))
                .is_none(),
            "re-signing a consumed slot under a different context must be refused"
        );

        // The exact same context re-signs idempotently (identical bytes).
        let again = scheme.sign::<Sha256Digest>(&original).unwrap();
        assert_eq!(first.signature, again.signature);
        assert_eq!(first.signer, again.signer);
    }

    // Full S₀ participation assembles directly into a FINAL aggregate certificate,
    // and the signature passes the exact on-chain ecrecover identity.
    #[test]
    fn full_set_assembles_final_aggregate() {
        let mut rng = test_rng();
        let fixture = setup(4);
        let subject = item(3, b"full participation");

        let attestations = sign_all(&fixture.schemes, &subject, 4);
        let certificate = fixture
            .verifier
            .assemble::<_, N3f1>(attestations, &Sequential)
            .unwrap();

        let SchnorrCertificate::Aggregate { signers, .. } = &certificate else {
            panic!("full-S₀ attestation set must assemble to Aggregate");
        };
        assert_eq!(signers.count(), 4);
        assert!(
            fixture
                .verifier
                .verify_certificate::<_, Sha256Digest, N3f1>(
                    &mut rng,
                    &subject,
                    &certificate,
                    &Sequential,
                )
        );
        assert!(
            !fixture
                .verifier
                .verify_certificate::<_, Sha256Digest, N3f1>(
                    &mut rng,
                    &item(3, b"other payload"),
                    &certificate,
                    &Sequential,
                )
        );

        // ON-CHAIN PARITY ANCHOR: the certificate's signature verifies through
        // `verify_aggregate`, which mirrors `SchnorrVerify.verify` byte-for-byte,
        // against the plain sum of all operator keys.
        let signature = certificate.aggregate_signature().unwrap();
        let x_all = SchnorrPublicKey::aggregate(
            fixture
                .keys
                .iter()
                .map(|k| k.public_key())
                .collect::<Vec<_>>()
                .iter(),
        )
        .unwrap();
        let digest: [u8; 32] = subject.digest.as_ref().try_into().unwrap();
        assert!(verify_aggregate(&x_all, &digest, &signature));

        // No non-signers to subtract on-chain.
        assert!(
            fixture
                .verifier
                .non_signer_identity_addresses(certificate.signers())
                .is_empty()
        );
    }

    // An engine quorum short of S₀ assembles into an Attested certificate; the
    // completion round (attempt 1, fresh slots, explicit set) then produces the final
    // aggregate for exactly the certified set — the full degraded-path cryptography.
    #[test]
    fn subset_assembles_attested_and_completion_round_finalizes() {
        let mut rng = test_rng();
        let fixture = setup(4);
        let subject = item(7, b"one operator down");
        let digest: [u8; 32] = subject.digest.as_ref().try_into().unwrap();

        // Operator 3 is offline: quorum (3 of 4) attests.
        let attestations = sign_all(&fixture.schemes, &subject, 3);
        let certificate = fixture
            .verifier
            .assemble::<_, N3f1>(attestations, &Sequential)
            .unwrap();
        let SchnorrCertificate::Attested { signers, .. } = &certificate else {
            panic!("subset must assemble to Attested");
        };
        assert_eq!(signers.count(), 3);
        assert!(
            fixture
                .verifier
                .verify_certificate::<_, Sha256Digest, N3f1>(
                    &mut rng,
                    &subject,
                    &certificate,
                    &Sequential,
                )
        );

        // The bitmap complement is the ascending non-signer list for the chain.
        let non_signers = fixture.verifier.non_signer_identity_addresses(signers);
        assert_eq!(non_signers.len(), 1);
        let mut sorted = non_signers.clone();
        sorted.sort();
        assert_eq!(non_signers, sorted);

        // ---- completion round: S₁ = certified bitmap, attempt 1, fresh slots ----
        let s1: Vec<Participant> = signers.iter().collect();
        let slot1 = slot_index(subject.height.get(), 1, ATTEMPTS).unwrap();

        // Every S₁ member derives the same context from committed attempt-1 nonces.
        let nonces: Vec<PubNonce> = s1
            .iter()
            .map(|p| {
                let addr = fixture.verifier.schnorr_key(*p).unwrap().eth_address();
                fixture.directory.pub_nonce(addr, slot1).unwrap()
            })
            .collect();
        let x_s1 = fixture.verifier.aggregate_key(signers).unwrap();
        let ctx = SigningContext::derive(x_s1, nonces.iter(), &digest).unwrap();

        // Members re-sign with their attempt-1 secret nonces (slot1 ≠ slot0: no reuse).
        let partials: Vec<Scalar> = s1
            .iter()
            .map(|p| {
                let i = usize::from(*p);
                let sec = derive_secnonce(&fixture.domains[i], &fixture.seeds[i], slot1);
                partial_sign(sec, &fixture.keys[i], &ctx).unwrap()
            })
            .collect();

        // Anyone assembles + the final signature passes the on-chain identity for S₁.
        let signature = Coordinator::assemble(&ctx, partials).expect("completion assembles");
        assert!(verify_aggregate(&x_s1, &digest, &signature));
    }

    // S₀ is DYNAMIC: an operator with no registered coverage is simply not in the set —
    // the remaining operators' full participation still assembles a final Aggregate.
    #[test]
    fn uncovered_operator_shrinks_s0() {
        let mut rng = test_rng();
        let fixture = setup_with(4, &[2]);
        let subject = item(9, b"operator 2 never registered a batch");

        // The uncovered operator cannot sign (it is outside S₀).
        assert!(fixture.schemes[2].sign::<Sha256Digest>(&subject).is_none());

        // The other three ARE the full S₀ → final Aggregate with 3 signers.
        let attestations: Vec<_> = [0usize, 1, 3]
            .iter()
            .map(|&i| fixture.schemes[i].sign::<Sha256Digest>(&subject).unwrap())
            .collect();
        let certificate = fixture
            .verifier
            .assemble::<_, N3f1>(attestations, &Sequential)
            .unwrap();
        let SchnorrCertificate::Aggregate { signers, .. } = &certificate else {
            panic!("coverage-complete set must assemble to Aggregate");
        };
        assert_eq!(signers.count(), 3);
        assert!(
            fixture
                .verifier
                .verify_certificate::<_, Sha256Digest, N3f1>(
                    &mut rng,
                    &subject,
                    &certificate,
                    &Sequential,
                )
        );
    }

    #[test]
    fn assemble_rejects_below_quorum_and_malformed_sets() {
        let fixture = setup(4);
        let subject = item(11, b"boundaries");
        let quorum = fixture.verifier.participants().quorum::<N3f1>() as usize;
        assert_eq!(quorum, 3);

        // Below quorum: None.
        let below = sign_all(&fixture.schemes, &subject, quorum - 1);
        assert!(
            fixture
                .verifier
                .assemble::<_, N3f1>(below, &Sequential)
                .is_none()
        );

        // Out-of-range signer: None (not a panic).
        let mut attestations = sign_all(&fixture.schemes, &subject, quorum);
        attestations[0].signer = Participant::new(999);
        assert!(
            fixture
                .verifier
                .assemble::<_, N3f1>(attestations, &Sequential)
                .is_none()
        );

        // Mixed heights: None.
        let mut mixed = sign_all(&fixture.schemes, &subject, quorum);
        mixed[0] = fixture.schemes[0]
            .sign::<Sha256Digest>(&item(12, b"boundaries"))
            .unwrap();
        assert!(
            fixture
                .verifier
                .assemble::<_, N3f1>(mixed, &Sequential)
                .is_none()
        );

        // Mixed claimed R (tampered): None.
        let mut mixed_r = sign_all(&fixture.schemes, &subject, quorum);
        let original = mixed_r[0].signature.get().unwrap().clone();
        mixed_r[0].signature = SchnorrPartial::new(
            original.height,
            Address::repeat_byte(0x99),
            original.scalar().unwrap(),
        )
        .into();
        assert!(
            fixture
                .verifier
                .assemble::<_, N3f1>(mixed_r, &Sequential)
                .is_none()
        );
    }

    #[test]
    fn tampered_certificates_rejected() {
        let mut rng = test_rng();
        let fixture = setup(4);
        let subject = item(13, b"tamper");

        let attestations = sign_all(&fixture.schemes, &subject, 3);
        let certificate = fixture
            .verifier
            .assemble::<_, N3f1>(attestations, &Sequential)
            .unwrap();
        let SchnorrCertificate::Attested {
            height,
            r_addr,
            signers,
            partials,
        } = certificate.clone()
        else {
            panic!("expected Attested");
        };

        // A tampered partial fails its per-signer verification.
        let mut bad_partials = partials.clone();
        bad_partials[0][31] ^= 1;
        let tampered = SchnorrCertificate::Attested {
            height,
            r_addr,
            signers: signers.clone(),
            partials: bad_partials,
        };
        assert!(
            !fixture
                .verifier
                .verify_certificate::<_, Sha256Digest, N3f1>(
                    &mut rng,
                    &subject,
                    &tampered,
                    &Sequential,
                )
        );

        // Claiming a different signer set for the same partials fails.
        let swapped: Vec<Participant> = signers
            .iter()
            .map(|p| Participant::new((p.get() + 1) % 4))
            .collect();
        let reattributed = SchnorrCertificate::Attested {
            height,
            r_addr,
            signers: Signers::from(4, swapped),
            partials: partials.clone(),
        };
        assert!(
            !fixture
                .verifier
                .verify_certificate::<_, Sha256Digest, N3f1>(
                    &mut rng,
                    &subject,
                    &reattributed,
                    &Sequential,
                )
        );

        // Count mismatch (extra bitmap signer, same partials) fails.
        let mut extra: Vec<Participant> = signers.iter().collect();
        extra.push(Participant::new(3));
        let padded = SchnorrCertificate::Attested {
            height,
            r_addr,
            signers: Signers::from(4, extra),
            partials: partials.clone(),
        };
        assert!(
            !fixture
                .verifier
                .verify_certificate::<_, Sha256Digest, N3f1>(
                    &mut rng,
                    &subject,
                    &padded,
                    &Sequential,
                )
        );

        // A bitmap sized for a different participant count is rejected outright.
        let resized = SchnorrCertificate::Attested {
            height,
            r_addr,
            signers: Signers::from(5, signers.iter().collect::<Vec<_>>()),
            partials,
        };
        assert!(
            !fixture
                .verifier
                .verify_certificate::<_, Sha256Digest, N3f1>(
                    &mut rng,
                    &subject,
                    &resized,
                    &Sequential,
                )
        );

        // Aggregate with a flipped scalar bit fails the ecrecover identity.
        let full = sign_all(&fixture.schemes, &subject, 4);
        let SchnorrCertificate::Aggregate {
            height,
            r_addr,
            signers,
            mut s,
        } = fixture
            .verifier
            .assemble::<_, N3f1>(full, &Sequential)
            .unwrap()
        else {
            panic!("expected Aggregate");
        };
        s[31] ^= 1;
        let flipped = SchnorrCertificate::Aggregate {
            height,
            r_addr,
            signers,
            s,
        };
        assert!(
            !fixture
                .verifier
                .verify_certificate::<_, Sha256Digest, N3f1>(
                    &mut rng,
                    &subject,
                    &flipped,
                    &Sequential,
                )
        );
    }

    #[test]
    fn certificate_codec_roundtrip() {
        let fixture = setup(4);
        let subject = item(15, b"codec");

        // Attested form.
        let attested = fixture
            .verifier
            .assemble::<_, N3f1>(sign_all(&fixture.schemes, &subject, 3), &Sequential)
            .unwrap();
        let encoded = attested.encode();
        assert_eq!(encoded.len(), attested.encode_size());
        let decoded = SchnorrCertificate::decode_cfg(
            encoded.clone(),
            &fixture.verifier.certificate_codec_config(),
        )
        .unwrap();
        assert_eq!(decoded, attested);
        let unbounded = SchnorrCertificate::decode_cfg(
            encoded.clone(),
            &SchnorrScheme::certificate_codec_config_unbounded(),
        )
        .unwrap();
        assert_eq!(unbounded, attested);
        // A tighter bound than the actual bitmap is rejected.
        assert!(SchnorrCertificate::decode_cfg(encoded, &2).is_err());

        // Aggregate form.
        let subject2 = item(16, b"codec agg");
        let aggregate = fixture
            .verifier
            .assemble::<_, N3f1>(sign_all(&fixture.schemes, &subject2, 4), &Sequential)
            .unwrap();
        let encoded = aggregate.encode();
        assert_eq!(encoded.len(), aggregate.encode_size());
        assert_eq!(
            SchnorrCertificate::decode_cfg(encoded.clone(), &4).unwrap(),
            aggregate
        );

        // Non-canonical scalar rejected at decode (order ≤ value): 0xff-fill.
        let mut corrupt = aggregate.encode_mut();
        let len = corrupt.len();
        corrupt[len - 32..].fill(0xff);
        assert!(SchnorrCertificate::decode_cfg(corrupt.freeze(), &4).is_err());

        // Truncated partial list fails decode.
        let mut truncated = attested.encode_mut();
        let len = truncated.len();
        truncated.truncate(len - 1);
        assert!(SchnorrCertificate::decode_cfg(truncated.freeze(), &4).is_err());

        // Empty-signer certificates are rejected at decode time.
        let empty = SchnorrCertificate::Aggregate {
            height: 1,
            r_addr: Address::repeat_byte(1),
            signers: Signers::from(4, std::iter::empty::<Participant>()),
            s: [1u8; 32],
        };
        assert!(SchnorrCertificate::decode_cfg(empty.encode(), &4).is_err());

        // Partial codec: roundtrip + non-canonical scalar rejected.
        let partial = fixture.schemes[0]
            .sign::<Sha256Digest>(&item(17, b"partial codec"))
            .unwrap()
            .signature
            .get()
            .unwrap()
            .clone();
        let encoded = partial.encode();
        assert_eq!(encoded.len(), SchnorrPartial::SIZE);
        assert_eq!(SchnorrPartial::decode(encoded).unwrap(), partial);
        let mut bad = partial.encode_mut();
        let len = bad.len();
        bad[len - 32..].fill(0xff);
        assert!(SchnorrPartial::decode(bad.freeze()).is_err());
    }

    // Engine-integration smoke test through the aggregation types (the same entry
    // points the engine uses). Proves the blanket `aggregation::scheme::Scheme`
    // marker holds for SchnorrScheme.
    #[test]
    fn aggregation_engine_smoke() {
        let mut rng = test_rng();
        let fixture = setup(4);
        let subject = item(19, b"engine smoke");

        let acks: Vec<Ack<SchnorrScheme, Sha256Digest>> = fixture
            .schemes
            .iter()
            .map(|s| Ack::sign(s, Epoch::zero(), subject.clone()).expect("participant signs"))
            .collect();
        for ack in &acks {
            assert!(ack.verify(&mut rng, &fixture.verifier, &Sequential));
        }

        // Verifier-only schemes cannot produce acks.
        assert!(Ack::sign(&fixture.verifier, Epoch::zero(), subject.clone()).is_none());

        // A quorum of acks (3 of 4) certifies as Attested…
        let partial_cert = AggCertificate::from_acks(&fixture.verifier, &acks[..3], &Sequential)
            .expect("quorum of acks");
        assert!(matches!(
            partial_cert.certificate,
            SchnorrCertificate::Attested { .. }
        ));
        assert!(partial_cert.verify(&mut rng, &fixture.verifier, &Sequential));

        // …and the full ack set certifies as a FINAL Aggregate.
        let full_cert =
            AggCertificate::from_acks(&fixture.verifier, &acks, &Sequential).expect("full acks");
        assert!(matches!(
            full_cert.certificate,
            SchnorrCertificate::Aggregate { .. }
        ));
        assert_eq!(full_cert.item, subject);
        assert!(full_cert.verify(&mut rng, &fixture.verifier, &Sequential));

        // Below-quorum ack sets do not form a certificate.
        assert!(AggCertificate::from_acks(&fixture.verifier, &acks[..2], &Sequential).is_none());
    }
}

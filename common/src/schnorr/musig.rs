//! MuSig2 two-round aggregate signing over secp256k1, with **plain (linear) key
//! aggregation** (see the parent module for the rogue-key/PoP rationale).
//!
//! Round 1 (nonce exchange) is message-independent and can be pre-processed off the
//! critical path; round 2 (partial signatures) is a single-shot per-signer operation once
//! the coordinator broadcasts the signing context. The [`Coordinator`] runs on the router;
//! each [`Participant`] runs on a node.
//!
//! The produced [`AggregateSignature`] verifies with the on-chain `ecrecover` identity in
//! [`super::verify_aggregate`], so the same signature that this protocol assembles is the
//! one the Solidity `SchnorrStakeRegistry` accepts.

use super::{
    AggregateSignature, Entropy, MESSAGE_LEN, PrivateKey, PublicKey, challenge,
    eth_address_of_point, random_scalar,
};
use alloy_primitives::keccak256;
use k256::elliptic_curve::bigint::U256;
use k256::elliptic_curve::group::prime::PrimeCurveAffine;
use k256::elliptic_curve::ops::Reduce;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::{AffinePoint, ProjectivePoint, Scalar};

use super::NONCE_COEFF_TAG;

/// A signer's secret nonce pair `(k1, k2)`. **Never reuse** across two signing sessions —
/// reuse leaks the private key. Each pair is consumed exactly once.
#[derive(Clone)]
pub struct SecNonce {
    k1: Scalar,
    k2: Scalar,
}

impl SecNonce {
    /// Builds a nonce pair from raw scalars, rejecting zeros. Used by the deterministic
    /// pre-committed-batch derivation ([`super::precommit`]); the "consume exactly once"
    /// rule applies unchanged — determinism makes reuse *reproducible*, not safe, so
    /// callers MUST gate every derivation behind a spend journal.
    pub fn from_scalars(k1: Scalar, k2: Scalar) -> Option<Self> {
        if bool::from(k1.is_zero()) || bool::from(k2.is_zero()) {
            return None;
        }
        Some(Self { k1, k2 })
    }

    /// The public half `(k1·G, k2·G)`.
    pub fn pub_nonce(&self) -> PubNonce {
        PubNonce {
            r1: (ProjectivePoint::GENERATOR * self.k1).to_affine(),
            r2: (ProjectivePoint::GENERATOR * self.k2).to_affine(),
        }
    }
}

/// The public half of a nonce pair: `(R1 = k1·G, R2 = k2·G)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PubNonce {
    r1: AffinePoint,
    r2: AffinePoint,
}

impl PubNonce {
    /// 66-byte wire encoding: compressed `R1 ‖ R2`.
    pub fn to_bytes(&self) -> [u8; 66] {
        let mut out = [0u8; 66];
        out[..33].copy_from_slice(&super::compress_point(&self.r1));
        out[33..].copy_from_slice(&super::compress_point(&self.r2));
        out
    }

    /// Decodes the 66-byte wire encoding (rejects invalid points and identity).
    pub fn from_bytes(bytes: &[u8; 66]) -> Option<Self> {
        let r1 = super::decompress_point(&bytes[..33].try_into().unwrap())?;
        let r2 = super::decompress_point(&bytes[33..].try_into().unwrap())?;
        Some(Self { r1, r2 })
    }
}

/// Generates a fresh secret/public nonce pair.
pub fn gen_nonce(fill: &mut impl Entropy) -> (SecNonce, PubNonce) {
    loop {
        let k1 = random_scalar(fill);
        let k2 = random_scalar(fill);
        if bool::from(k1.is_zero()) || bool::from(k2.is_zero()) {
            continue;
        }
        let r1 = (ProjectivePoint::GENERATOR * k1).to_affine();
        let r2 = (ProjectivePoint::GENERATOR * k2).to_affine();
        return (SecNonce { k1, k2 }, PubNonce { r1, r2 });
    }
}

/// Sums a set of public nonces into `(R1_agg, R2_agg)`.
fn aggregate_pubnonces<'a>(
    nonces: impl IntoIterator<Item = &'a PubNonce>,
) -> Option<(AffinePoint, AffinePoint)> {
    let mut acc1 = ProjectivePoint::IDENTITY;
    let mut acc2 = ProjectivePoint::IDENTITY;
    let mut any = false;
    for n in nonces {
        acc1 += ProjectivePoint::from(n.r1);
        acc2 += ProjectivePoint::from(n.r2);
        any = true;
    }
    if !any {
        return None;
    }
    Some((acc1.to_affine(), acc2.to_affine()))
}

/// The MuSig2 nonce coefficient `b = H_non(R1_agg ‖ R2_agg ‖ X_agg ‖ m) mod n`.
///
/// This binds the effective nonce `R = R1_agg + b·R2_agg` to the aggregate key and message,
/// which is what defeats Wagner-style attacks on the two-nonce construction. It is internal
/// (never consumed on-chain), so the encoding is our own compressed-point layout.
fn nonce_coefficient(
    r1_agg: &AffinePoint,
    r2_agg: &AffinePoint,
    x_agg: &PublicKey,
    message: &[u8; MESSAGE_LEN],
) -> Scalar {
    let mut pre = Vec::with_capacity(NONCE_COEFF_TAG.len() + 33 * 3 + MESSAGE_LEN);
    pre.extend_from_slice(NONCE_COEFF_TAG);
    pre.extend_from_slice(r1_agg.to_encoded_point(true).as_bytes());
    pre.extend_from_slice(r2_agg.to_encoded_point(true).as_bytes());
    pre.extend_from_slice(x_agg.point().to_encoded_point(true).as_bytes());
    pre.extend_from_slice(message);
    Scalar::reduce(U256::from_be_slice(keccak256(pre).as_slice()))
}

/// The effective aggregate nonce `R = R1_agg + b·R2_agg`.
fn effective_nonce(r1_agg: &AffinePoint, r2_agg: &AffinePoint, b: &Scalar) -> AffinePoint {
    (ProjectivePoint::from(*r1_agg) + ProjectivePoint::from(*r2_agg) * *b).to_affine()
}

/// The context a [`Coordinator`] broadcasts to signers at the start of round 2.
///
/// It carries only the **public aggregate nonces** `R1_agg`/`R2_agg` (not the derived
/// `b`/`e` scalars): each signer recomputes `b` and the challenge `e` itself in
/// [`partial_sign`], so it never signs under a coordinator-chosen coefficient. This closes
/// the "blindly trust the coordinator's opaque (b, e)" gap.
///
/// SECURITY — single-use nonces: even with local `b`/`e` recomputation, an aggregate Schnorr
/// signature is only secure if **each [`SecNonce`] is used in at most one session**. A
/// malicious coordinator that gets a victim to sign twice under the *same* `SecNonce` (by
/// varying the other signers' nonces so `R1_agg`/`R2_agg` differ) obtains two linear
/// equations and can solve for the victim's key. Consume-by-value enforces single-use within
/// one process; sessions live only in memory, so a node restart forgets in-flight secret
/// nonces rather than risk replaying one (see `node::schnorr_participant`).
#[derive(Clone)]
pub struct SigningContext {
    /// The chosen aggregate key `X_agg(S)` over the signer subset `S`.
    pub x_agg: PublicKey,
    /// Aggregate first nonce `R1_agg = Σ R_i1` over the subset.
    pub r1_agg: AffinePoint,
    /// Aggregate second nonce `R2_agg = Σ R_i2` over the subset.
    pub r2_agg: AffinePoint,
    /// `address(R)` as computed by the coordinator (re-derived and checked by each signer).
    pub r_addr: alloy_primitives::Address,
    /// The signed message.
    pub message: [u8; MESSAGE_LEN],
}

impl SigningContext {
    /// Derives the full context — aggregate nonces, effective `R`, `address(R)` — from an
    /// aggregate key, the signer subset's public nonces, and the message. Returns `None`
    /// on an empty nonce set or a degenerate aggregate (`R` = identity / zero address).
    ///
    /// This is the deterministic core shared by the interactive coordinator
    /// ([`Coordinator::build_context`]) and the pre-committed-nonce scheme
    /// ([`super::scheme`]), where every party derives the same context locally instead of
    /// receiving it from a coordinator.
    pub fn derive<'a>(
        x_agg: PublicKey,
        nonces: impl IntoIterator<Item = &'a PubNonce>,
        message: &[u8; MESSAGE_LEN],
    ) -> Option<Self> {
        let (r1_agg, r2_agg) = aggregate_pubnonces(nonces)?;
        let b = nonce_coefficient(&r1_agg, &r2_agg, &x_agg, message);
        let r_point = effective_nonce(&r1_agg, &r2_agg, &b);
        if bool::from(r_point.is_identity()) {
            return None;
        }
        let r_addr = eth_address_of_point(&r_point);
        if r_addr == alloy_primitives::Address::ZERO {
            return None;
        }
        Some(Self {
            x_agg,
            r1_agg,
            r2_agg,
            r_addr,
            message: *message,
        })
    }

    /// The aggregate nonce pair `(R1_agg, R2_agg)` as a [`PubNonce`] (wire form).
    pub fn agg_nonces(&self) -> PubNonce {
        PubNonce {
            r1: self.r1_agg,
            r2: self.r2_agg,
        }
    }

    /// Rebuilds a context from wire parts (node side).
    ///
    /// SECURITY: the caller MUST have computed `x_agg` itself from an authenticated
    /// signer set (never trust a coordinator-supplied aggregate key); `partial_sign`
    /// then re-derives `b`/`e`/`R` from these parts and cross-checks `r_addr`.
    pub fn from_wire(
        x_agg: PublicKey,
        agg_nonces: &PubNonce,
        r_addr: alloy_primitives::Address,
        message: [u8; MESSAGE_LEN],
    ) -> Self {
        Self {
            x_agg,
            r1_agg: agg_nonces.r1,
            r2_agg: agg_nonces.r2,
            r_addr,
            message,
        }
    }
}

/// Produces a signer's partial signature `s_i = k1 + b·k2 − e·x_i (mod n)`.
///
/// `b` and `e` are **recomputed locally** from the context's public aggregate nonces and
/// message — the coordinator cannot substitute its own coefficients. If the coordinator's
/// claimed `r_addr` disagrees with the one derived from `R1_agg`/`R2_agg`, signing aborts
/// (returns `None`) rather than producing a partial under an inconsistent challenge.
///
/// The [`SecNonce`] is consumed by value to make single-use enforceable at the type level.
pub fn partial_sign(
    nonce: SecNonce,
    private_key: &PrivateKey,
    ctx: &SigningContext,
) -> Option<Scalar> {
    let b = nonce_coefficient(&ctx.r1_agg, &ctx.r2_agg, &ctx.x_agg, &ctx.message);
    let r_point = effective_nonce(&ctx.r1_agg, &ctx.r2_agg, &b);
    if bool::from(r_point.is_identity()) {
        return None;
    }
    let r_addr = eth_address_of_point(&r_point);
    if r_addr != ctx.r_addr {
        return None; // coordinator's R is inconsistent with the aggregate nonces
    }
    let e = challenge(&ctx.x_agg, &ctx.message, r_addr);
    Some(nonce.k1 + b * nonce.k2 - e * private_key.scalar())
}

/// Sums partial signatures into the aggregate response scalar `s = Σ s_i`.
fn aggregate_partials(partials: impl IntoIterator<Item = Scalar>) -> Scalar {
    partials.into_iter().fold(Scalar::ZERO, |a, s| a + s)
}

/// Router-side coordinator: given the public nonces the available signers submitted, fixes
/// the signer subset and builds the [`SigningContext`], then aggregates the returned partial
/// signatures into a single [`AggregateSignature`].
pub struct Coordinator;

impl Coordinator {
    /// Round 1 → 2 transition. `contributions` is `(public_key, pub_nonce)` for each signer
    /// that submitted a nonce for this session. Returns the signing context to broadcast,
    /// or `None` if no signer contributed or the aggregate degenerates.
    pub fn build_context(
        contributions: &[(PublicKey, PubNonce)],
        message: &[u8; MESSAGE_LEN],
    ) -> Option<SigningContext> {
        if contributions.is_empty() {
            return None;
        }
        let x_agg = PublicKey::aggregate(contributions.iter().map(|(pk, _)| pk))?;
        SigningContext::derive(x_agg, contributions.iter().map(|(_, n)| n), message)
    }

    /// Verifies a single signer's partial signature against its nonce commitment:
    /// `s_i·G == R1_i + b·R2_i − e·X_i`.
    ///
    /// Lets the coordinator attribute a bad partial to the exact signer (and drop
    /// it from the next attempt) instead of only discovering that the aggregate
    /// fails to verify in [`Coordinator::assemble`].
    pub fn verify_partial(
        ctx: &SigningContext,
        signer: &PublicKey,
        nonce: &PubNonce,
        partial: &Scalar,
    ) -> bool {
        let b = nonce_coefficient(&ctx.r1_agg, &ctx.r2_agg, &ctx.x_agg, &ctx.message);
        let e = challenge(&ctx.x_agg, &ctx.message, ctx.r_addr);
        let lhs = ProjectivePoint::GENERATOR * *partial;
        let rhs = ProjectivePoint::from(nonce.r1) + ProjectivePoint::from(nonce.r2) * b
            - ProjectivePoint::from(signer.point()) * e;
        lhs.to_affine() == rhs.to_affine()
    }

    /// Aggregates the partial signatures into the final signature. The caller is responsible
    /// for collecting exactly the partials from the subset committed in `build_context`.
    pub fn assemble(
        ctx: &SigningContext,
        partials: impl IntoIterator<Item = Scalar>,
    ) -> Option<AggregateSignature> {
        let s = aggregate_partials(partials);
        if bool::from(s.is_zero()) {
            return None;
        }
        let sig = AggregateSignature {
            s,
            r_addr: ctx.r_addr,
        };
        // Fail fast if assembly did not produce a valid signature (e.g. a wrong/missing
        // partial). The coordinator never submits an unverified certificate on-chain.
        if super::verify_aggregate(&ctx.x_agg, &ctx.message, &sig) {
            Some(sig)
        } else {
            None
        }
    }
}

/// Node-side participant: holds a private key and a pool of pre-generated nonces.
pub struct Participant {
    private_key: PrivateKey,
}

impl Participant {
    pub fn new(private_key: PrivateKey) -> Self {
        Self { private_key }
    }

    pub fn public_key(&self) -> PublicKey {
        self.private_key.public_key()
    }

    /// Round 0: pre-generate a nonce pair to advertise. The [`SecNonce`] must be retained
    /// (keyed by session) until the matching [`SigningContext`] arrives.
    pub fn new_nonce(&self, fill: &mut impl Entropy) -> (SecNonce, PubNonce) {
        gen_nonce(fill)
    }

    /// Round 2: produce this node's partial signature. Consumes the secret nonce. Returns
    /// `None` if the coordinator's context is inconsistent (see [`partial_sign`]).
    pub fn sign(&self, nonce: SecNonce, ctx: &SigningContext) -> Option<Scalar> {
        partial_sign(nonce, &self.private_key, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schnorr::verify_aggregate;
    use rand::rngs::StdRng;
    use rand::{RngCore, SeedableRng};

    /// Deterministic entropy source for tests.
    fn seeded(seed: u64) -> impl Entropy {
        let mut rng = StdRng::seed_from_u64(seed);
        move |b: &mut [u8]| rng.fill_bytes(b)
    }

    /// Runs the full two-round protocol for a given subset of participants and returns the
    /// aggregate signature plus the subset's aggregate key.
    fn run_session(
        participants: &[Participant],
        signer_indices: &[usize],
        message: &[u8; MESSAGE_LEN],
        fill: &mut impl Entropy,
    ) -> (PublicKey, AggregateSignature) {
        // Round 0/1: each participating signer advertises a nonce.
        let mut secnonces = Vec::new();
        let mut contributions = Vec::new();
        for &i in signer_indices {
            let (sec, pubn) = participants[i].new_nonce(fill);
            contributions.push((participants[i].public_key(), pubn));
            secnonces.push((i, sec));
        }
        // Coordinator fixes the subset and broadcasts the context.
        let ctx = Coordinator::build_context(&contributions, message).expect("context");
        // Round 2: each signer returns a partial.
        let partials: Vec<Scalar> = secnonces
            .into_iter()
            .map(|(i, sec)| participants[i].sign(sec, &ctx).expect("participant signs"))
            .collect();
        let sig = Coordinator::assemble(&ctx, partials).expect("assembled");
        (ctx.x_agg, sig)
    }

    #[test]
    fn full_participation_verifies() {
        let mut fill = seeded(7);
        let participants: Vec<Participant> = (0..5)
            .map(|i| Participant::new(PrivateKey::from_seed(100 + i)))
            .collect();
        let msg = keccak256(b"full participation task").0;
        let (x_agg, sig) = run_session(&participants, &[0, 1, 2, 3, 4], &msg, &mut fill);
        assert!(verify_aggregate(&x_agg, &msg, &sig));
    }

    #[test]
    fn subset_participation_verifies_against_subset_key() {
        let mut fill = seeded(11);
        let participants: Vec<Participant> = (0..5)
            .map(|i| Participant::new(PrivateKey::from_seed(200 + i)))
            .collect();
        let msg = keccak256(b"subset task").0;

        // Only signers {0, 2, 4} participate (1 and 3 are "offline").
        let subset = [0usize, 2, 4];
        let (x_agg, sig) = run_session(&participants, &subset, &msg, &mut fill);
        assert!(verify_aggregate(&x_agg, &msg, &sig));

        // The subset key equals the plain sum of the participating keys, and equals
        // (all keys) − (non-signers) — exactly the on-chain non-signer-subtraction path.
        let subset_keys: Vec<PublicKey> = subset
            .iter()
            .map(|&i| participants[i].public_key())
            .collect();
        let expected = PublicKey::aggregate(&subset_keys).unwrap();
        assert_eq!(x_agg.point(), expected.point());

        let all: Vec<PublicKey> = participants.iter().map(|p| p.public_key()).collect();
        let all_agg = PublicKey::aggregate(&all).unwrap();
        let minus_nonsigners = {
            let mut acc = ProjectivePoint::from(all_agg.point());
            acc -= ProjectivePoint::from(participants[1].public_key().point());
            acc -= ProjectivePoint::from(participants[3].public_key().point());
            PublicKey::from_affine(acc.to_affine()).unwrap()
        };
        assert_eq!(minus_nonsigners.point(), x_agg.point());

        // A signature for the subset must NOT verify against the full-set key.
        assert!(!verify_aggregate(&all_agg, &msg, &sig));
    }

    #[test]
    fn wrong_message_rejected() {
        let mut fill = seeded(3);
        let participants: Vec<Participant> = (0..3)
            .map(|i| Participant::new(PrivateKey::from_seed(300 + i)))
            .collect();
        let msg = keccak256(b"real task").0;
        let (x_agg, sig) = run_session(&participants, &[0, 1, 2], &msg, &mut fill);
        let other = keccak256(b"forged task").0;
        assert!(!verify_aggregate(&x_agg, &other, &sig));
    }

    #[test]
    fn partial_verification_attributes_bad_partials() {
        let mut fill = seeded(21);
        let participants: Vec<Participant> = (0..3)
            .map(|i| Participant::new(PrivateKey::from_seed(700 + i)))
            .collect();
        let msg = keccak256(b"attributable partials").0;

        let mut secnonces = Vec::new();
        let mut contributions = Vec::new();
        for p in &participants {
            let (sec, pubn) = p.new_nonce(&mut fill);
            contributions.push((p.public_key(), pubn));
            secnonces.push(sec);
        }
        let ctx = Coordinator::build_context(&contributions, &msg).unwrap();
        for (i, sec) in secnonces.into_iter().enumerate() {
            let partial = participants[i].sign(sec, &ctx).expect("participant signs");
            let (pk, pubn) = &contributions[i];
            assert!(Coordinator::verify_partial(&ctx, pk, pubn, &partial));
            // The same partial must NOT verify against another signer's identity.
            let (other_pk, other_nonce) = &contributions[(i + 1) % contributions.len()];
            assert!(!Coordinator::verify_partial(
                &ctx,
                other_pk,
                other_nonce,
                &partial
            ));
            // A tampered partial is rejected for the right signer.
            let tampered = partial + Scalar::ONE;
            assert!(!Coordinator::verify_partial(&ctx, pk, pubn, &tampered));
        }
    }

    #[test]
    fn missing_partial_fails_assembly() {
        // If a committed signer's partial is dropped, aggregation must not yield a valid
        // signature for the committed subset key (assemble self-verifies and returns None).
        let mut fill = seeded(5);
        let participants: Vec<Participant> = (0..4)
            .map(|i| Participant::new(PrivateKey::from_seed(400 + i)))
            .collect();
        let msg = keccak256(b"dropped partial").0;

        let mut secnonces = Vec::new();
        let mut contributions = Vec::new();
        for p in &participants {
            let (sec, pubn) = p.new_nonce(&mut fill);
            contributions.push((p.public_key(), pubn));
            secnonces.push(sec);
        }
        let ctx = Coordinator::build_context(&contributions, &msg).unwrap();
        // Drop signer 3's partial.
        let partials: Vec<Scalar> = secnonces
            .into_iter()
            .take(3)
            .enumerate()
            .map(|(i, sec)| participants[i].sign(sec, &ctx).expect("participant signs"))
            .collect();
        assert!(Coordinator::assemble(&ctx, partials).is_none());
    }

    // A coordinator that lies about R (r_addr inconsistent with the aggregate nonces) is
    // rejected by every honest signer — partial_sign recomputes R and refuses to sign.
    #[test]
    fn lying_coordinator_rejected() {
        let mut fill = seeded(77);
        let participants: Vec<Participant> = (0..3)
            .map(|i| Participant::new(PrivateKey::from_seed(600 + i)))
            .collect();
        let msg = keccak256(b"lying coordinator").0;

        let mut secnonces = Vec::new();
        let mut contributions = Vec::new();
        for (i, p) in participants.iter().enumerate() {
            let (sec, pubn) = p.new_nonce(&mut fill);
            contributions.push((p.public_key(), pubn));
            secnonces.push((i, sec));
        }
        let mut ctx = Coordinator::build_context(&contributions, &msg).unwrap();
        // Tamper with the coordinator's claimed R.
        ctx.r_addr = address_of_seed(999);
        for (i, sec) in secnonces {
            assert!(
                participants[i].sign(sec, &ctx).is_none(),
                "honest signer must refuse an inconsistent R"
            );
        }
    }

    fn address_of_seed(seed: u64) -> alloy_primitives::Address {
        PrivateKey::from_seed(seed).public_key().eth_address()
    }

    #[test]
    fn nonce_reuse_would_leak_but_fresh_nonces_differ() {
        // Sanity: two fresh nonce pairs differ (defends the "never reuse" invariant by
        // making reuse an explicit caller error, not an accident of determinism).
        let mut fill = seeded(99);
        let p = Participant::new(PrivateKey::from_seed(500));
        let (_, n1) = p.new_nonce(&mut fill);
        let (_, n2) = p.new_nonce(&mut fill);
        assert_ne!(n1, n2);
    }
}

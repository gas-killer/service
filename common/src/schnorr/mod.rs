//! secp256k1 **aggregate Schnorr** signatures for the Gas Killer AVS.
//!
//! An aggregate Schnorr signature is a **single** `(s, R)` pair verified on-chain
//! in constant gas with one `ecrecover` — regardless of how many operators signed.
//!
//! # Convention (audited: Chronicle/MakerDAO "Scribe")
//!
//! We reuse Scribe's `ecrecover`-trick verification so the EVM never runs a secp256k1
//! scalar multiplication in Solidity. For an aggregate public key `X` (x-coordinate
//! `Xx`, y-parity `Xp`), 32-byte message `m`, and signature `(s, Raddr)` where
//! `Raddr = address(R)`:
//!
//! * challenge `e = keccak256(Xx ‖ Xp ‖ m ‖ Raddr) mod n`   (`Xp` a single 0/1 byte)
//! * signing:  `s = k − e·x   (mod n)`   ⇒   `R = s·G + e·X`
//! * verify:   `ecrecover(−s·Xx mod n, 27+Xp, Xx, e·Xx mod n) == Raddr`
//!
//! The derivation (see `verify_aggregate`): setting `r = Xx` forces `ecrecover`'s internal
//! point to `X`, and the `Xx` factors cancel, leaving `addr(e·X + s·G) = addr(R) = Raddr`.
//!
//! # Rogue-key defense
//!
//! Aggregation is **plain/linear** (`X_agg = Σ X_i`, no MuSig per-key coefficients) so that
//! the on-chain verifier can recover any signer subset's aggregate key by **subtracting
//! non-signers** from a checkpointed all-operators aggregate. Plain aggregation is only
//! safe against rogue-key attacks if every registered key carries a **proof of possession**
//! ([`Pop`]) — the same guarantee EigenLayer's `BLSApkRegistry` enforces for BLS. The
//! `SchnorrStakeRegistry` requires a valid [`Pop`] at registration.
//!
//! # What this module is (and is not)
//!
//! This is the cryptographic core + the [`musig`] two-round signing protocol, fully unit
//! tested against the exact on-chain `ecrecover` identity. Wiring the interactive
//! protocol into the live node/router p2p binaries lives in
//! `node::schnorr_participant` and `router::schnorr_coordinator`.

pub mod batches;
pub mod journal;
pub mod musig;
pub mod onchain;
pub mod precommit;
pub mod scheme;
pub mod wire;

use alloy_primitives::{Address, U256 as AlloyU256, keccak256};
use k256::elliptic_curve::PrimeField;
use k256::elliptic_curve::bigint::U256;
use k256::elliptic_curve::group::prime::PrimeCurveAffine;
use k256::elliptic_curve::ops::Reduce;
use k256::elliptic_curve::point::AffineCoordinates;
use k256::elliptic_curve::sec1::{FromEncodedPoint, ToEncodedPoint};
use k256::{AffinePoint, ProjectivePoint, Scalar};

/// Length of the message/digest that gets signed.
pub const MESSAGE_LEN: usize = 32;

/// secp256k1 group order `n` as an [`AlloyU256`], for the on-chain-mirroring arithmetic in
/// [`verify_aggregate`] (`ecrecover` reduces `r`/`s`/`h` modulo `n`).
pub const CURVE_ORDER: AlloyU256 = AlloyU256::from_limbs([
    0xBFD25E8CD0364141,
    0xBAAEDCE6AF48A03B,
    0xFFFFFFFFFFFFFFFE,
    0xFFFFFFFFFFFFFFFF,
]);

/// Domain-separation tags (kept out of on-chain preimages, which follow Scribe exactly).
const POP_TAG: &[u8] = b"gas-killer/schnorr/pop/v1";
const NONCE_COEFF_TAG: &[u8] = b"gas-killer/schnorr/musig2/nonce-coeff/v1";

/// Entropy source: a closure that fills the given buffer with cryptographically secure
/// random bytes (e.g. `|b| rng.fill_bytes(b)`).
///
/// The crypto core takes entropy this way rather than binding a `rand_core` trait, so it is
/// independent of which `rand`/`rand_core` major the surrounding workspace pins (they
/// diverge across the commonware / k256 / alloy dependency trees).
pub trait Entropy: FnMut(&mut [u8]) {}
impl<F: FnMut(&mut [u8])> Entropy for F {}

/// Samples a scalar by reducing 32 fresh bytes modulo the group order.
///
/// The reduction bias is `~2^-128` (negligible), matching what mainstream libraries accept
/// for ephemeral scalars; callers that require non-zero retry.
pub(crate) fn random_scalar(fill: &mut impl Entropy) -> Scalar {
    let mut b = [0u8; 32];
    fill(&mut b);
    Scalar::reduce(U256::from_be_slice(&b))
}

/// A private key: a non-zero secp256k1 scalar.
#[derive(Clone)]
pub struct PrivateKey(Scalar);

impl PrivateKey {
    /// Wraps a scalar, rejecting zero.
    pub fn new(scalar: Scalar) -> Option<Self> {
        if bool::from(scalar.is_zero()) {
            None
        } else {
            Some(Self(scalar))
        }
    }

    /// Generates a fresh random private key from the given entropy source.
    pub fn random(fill: &mut impl Entropy) -> Self {
        loop {
            let s = random_scalar(fill);
            if let Some(k) = Self::new(s) {
                return k;
            }
        }
    }

    /// Deterministic key from a seed, for tests/fixtures.
    pub fn from_seed(seed: u64) -> Self {
        // Hash the seed into a scalar; retry on the (impossible) zero.
        let mut ctr = seed;
        loop {
            let h =
                keccak256([b"gas-killer/schnorr/seed/v1".as_slice(), &ctr.to_be_bytes()].concat());
            let s = Scalar::reduce(U256::from_be_slice(h.as_slice()));
            if let Some(k) = Self::new(s) {
                return k;
            }
            ctr = ctr.wrapping_add(1);
        }
    }

    /// The scalar value.
    pub fn scalar(&self) -> Scalar {
        self.0
    }

    /// The corresponding public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey((ProjectivePoint::GENERATOR * self.0).to_affine())
    }

    /// Produces a single-key Schnorr signature over `message` (the aggregate convention
    /// with one signer), verifiable on-chain via `SchnorrVerify.verify` against this key.
    /// Used for proofs of possession and nonce-batch registrations.
    pub fn sign_single(
        &self,
        message: &[u8; MESSAGE_LEN],
        fill: &mut impl Entropy,
    ) -> AggregateSignature {
        let x = self.public_key();
        // Single-signer Schnorr: k·G = R, s = k − e·x.
        loop {
            let k = random_scalar(fill);
            if bool::from(k.is_zero()) {
                continue;
            }
            let r_point = (ProjectivePoint::GENERATOR * k).to_affine();
            let r_addr = eth_address_of_point(&r_point);
            if r_addr == Address::ZERO {
                continue;
            }
            let e = challenge(&x, message, r_addr);
            let s = k - e * self.0;
            if bool::from(s.is_zero()) {
                continue;
            }
            return AggregateSignature { s, r_addr };
        }
    }

    /// Produces a proof of possession binding this key to its own public key.
    ///
    /// The PoP is itself a single-key Schnorr signature (same convention) over the message
    /// `keccak256(POP_TAG ‖ eth_address(X))`, verifiable on-chain against `X`.
    pub fn prove_possession(&self, fill: &mut impl Entropy) -> Pop {
        let msg = pop_message(&self.public_key());
        Pop(self.sign_single(&msg, fill))
    }
}

/// Parses a private key from an operator key-file value: `0x`-prefixed hex, bare
/// 64-char hex, or a legacy decimal scalar.
///
/// The Schnorr key IS the operator's existing secp256k1 key — same scalar, same
/// public-key point, and (because the registry identity is `keccak256(x ‖ y)[12..]`)
/// the same Ethereum address as the operator's EigenLayer identity. Returns `None`
/// on malformed input, a zero scalar, or a value ≥ the group order.
pub fn private_key_from_hex(key: &str) -> Option<PrivateKey> {
    let key = key.trim();
    let bytes: [u8; 32] = if let Some(hex) = key
        .strip_prefix("0x")
        .or_else(|| (key.len() == 64 && key.chars().all(|c| c.is_ascii_hexdigit())).then_some(key))
    {
        let mut out = [0u8; 32];
        if hex.len() != 64 {
            return None;
        }
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let s = std::str::from_utf8(chunk).ok()?;
            out[i] = u8::from_str_radix(s, 16).ok()?;
        }
        out
    } else {
        // Legacy decimal scalar.
        let v: AlloyU256 = key.parse().ok()?;
        v.to_be_bytes()
    };
    let scalar = Option::<Scalar>::from(Scalar::from_repr(bytes.into()))?;
    PrivateKey::new(scalar)
}

/// Compresses an affine point to the 33-byte SEC1 form used on the wire.
pub(crate) fn compress_point(point: &AffinePoint) -> [u8; 33] {
    point
        .to_encoded_point(true)
        .as_bytes()
        .try_into()
        .expect("compressed SEC1 point is 33 bytes")
}

/// Decompresses a 33-byte SEC1 point, rejecting invalid encodings and the identity.
pub(crate) fn decompress_point(bytes: &[u8; 33]) -> Option<AffinePoint> {
    let encoded = k256::EncodedPoint::from_bytes(bytes).ok()?;
    let point = Option::<AffinePoint>::from(AffinePoint::from_encoded_point(&encoded))?;
    if bool::from(point.is_identity()) {
        return None;
    }
    Some(point)
}

/// A public key: an affine secp256k1 point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PublicKey(AffinePoint);

impl PublicKey {
    /// Wraps an affine point (must not be the identity).
    pub fn from_affine(point: AffinePoint) -> Option<Self> {
        if bool::from(point.is_identity()) {
            None
        } else {
            Some(Self(point))
        }
    }

    /// The underlying affine point.
    pub fn point(&self) -> AffinePoint {
        self.0
    }

    /// The 32-byte big-endian x-coordinate.
    pub fn x_bytes(&self) -> [u8; 32] {
        self.0.x().into()
    }

    /// The 32-byte big-endian y-coordinate (for on-chain point storage / subtraction).
    pub fn y_bytes(&self) -> [u8; 32] {
        let ep = self.0.to_encoded_point(false);
        ep.y()
            .expect("uncompressed point has y")
            .as_slice()
            .try_into()
            .unwrap()
    }

    /// y-parity: `0` if y is even, `1` if odd (Scribe's `pubKeyYParity`).
    pub fn y_parity(&self) -> u8 {
        u8::from(bool::from(self.0.y_is_odd()))
    }

    /// The Ethereum address of this point (`keccak256(x ‖ y)[12..]`).
    pub fn eth_address(&self) -> Address {
        eth_address_of_point(&self.0)
    }

    /// The 33-byte compressed SEC1 encoding (wire form).
    pub fn to_compressed(&self) -> [u8; 33] {
        compress_point(&self.0)
    }

    /// Decodes a 33-byte compressed SEC1 encoding (rejects invalid points and identity).
    pub fn from_compressed(bytes: &[u8; 33]) -> Option<Self> {
        decompress_point(bytes).and_then(Self::from_affine)
    }

    /// Builds a key from affine big-endian coordinates (as stored by the on-chain
    /// registries), rejecting off-curve points and the identity.
    pub fn from_coordinates(x: &[u8; 32], y: &[u8; 32]) -> Option<Self> {
        let encoded = k256::EncodedPoint::from_affine_coordinates(x.into(), y.into(), false);
        let point = Option::<AffinePoint>::from(AffinePoint::from_encoded_point(&encoded))?;
        Self::from_affine(point)
    }

    /// Sums a set of public keys into their plain aggregate (`Σ X_i`).
    ///
    /// Returns `None` if the input is empty or the sum is the identity.
    pub fn aggregate<'a>(keys: impl IntoIterator<Item = &'a PublicKey>) -> Option<PublicKey> {
        let mut acc = ProjectivePoint::IDENTITY;
        let mut any = false;
        for k in keys {
            acc += ProjectivePoint::from(k.0);
            any = true;
        }
        if !any {
            return None;
        }
        PublicKey::from_affine(acc.to_affine())
    }

    /// Verifies a proof of possession for this key.
    pub fn verify_possession(&self, pop: &Pop) -> bool {
        let msg = pop_message(self);
        verify_aggregate(self, &msg, &pop.0)
    }
}

/// An aggregate Schnorr signature: the response scalar `s` and the nonce point's Ethereum
/// address `Raddr = address(R)`. Constant size (52 bytes), independent of signer count.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AggregateSignature {
    /// Response scalar `s = k − e·x (mod n)`.
    pub s: Scalar,
    /// `address(R)` — the only form in which the nonce `R` appears on-chain.
    pub r_addr: Address,
}

impl AggregateSignature {
    /// 52-byte wire encoding: `s (32) ‖ Raddr (20)`.
    pub fn to_bytes(&self) -> [u8; 52] {
        let mut out = [0u8; 52];
        out[..32].copy_from_slice(&self.s.to_bytes());
        out[32..].copy_from_slice(self.r_addr.as_slice());
        out
    }

    /// Decodes the 52-byte wire encoding, rejecting `s ≥ n` and the zero address.
    pub fn from_bytes(bytes: &[u8; 52]) -> Option<Self> {
        let s = Option::<Scalar>::from(Scalar::from_repr(
            <[u8; 32]>::try_from(&bytes[..32]).unwrap().into(),
        ))?;
        if bool::from(s.is_zero()) {
            return None;
        }
        let r_addr = Address::from_slice(&bytes[32..]);
        if r_addr == Address::ZERO {
            return None;
        }
        Some(Self { s, r_addr })
    }
}

/// A proof of possession — structurally a single-key aggregate signature over the key's own
/// address.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pop(pub AggregateSignature);

/// The PoP message: `keccak256(POP_TAG ‖ eth_address(X))`.
fn pop_message(x: &PublicKey) -> [u8; 32] {
    keccak256([POP_TAG, x.eth_address().as_slice()].concat()).into()
}

/// Ethereum address of an affine point: `keccak256(x ‖ y)[12..]`.
pub fn eth_address_of_point(point: &AffinePoint) -> Address {
    let ep = point.to_encoded_point(false);
    let x = ep.x().expect("non-identity point has x");
    let y = ep.y().expect("uncompressed point has y");
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(x);
    buf[32..].copy_from_slice(y);
    let h = keccak256(buf);
    Address::from_slice(&h[12..])
}

/// The Scribe challenge `e = keccak256(Xx ‖ Xp ‖ m ‖ Raddr) mod n`.
///
/// `Xp` is a single byte (0/1). This preimage layout is consumed **verbatim** by the
/// Solidity verifier — do not reorder or repad.
pub fn challenge(x_agg: &PublicKey, message: &[u8; MESSAGE_LEN], r_addr: Address) -> Scalar {
    let mut preimage = Vec::with_capacity(32 + 1 + 32 + 20);
    preimage.extend_from_slice(&x_agg.x_bytes());
    preimage.push(x_agg.y_parity());
    preimage.extend_from_slice(message);
    preimage.extend_from_slice(r_addr.as_slice());
    Scalar::reduce(U256::from_be_slice(keccak256(preimage).as_slice()))
}

/// Verifies an aggregate signature against an aggregate key, using the **exact** on-chain
/// `ecrecover` identity (see [`ecrecover_address`]) so a passing Rust check guarantees a
/// passing Solidity check.
pub fn verify_aggregate(
    x_agg: &PublicKey,
    message: &[u8; MESSAGE_LEN],
    sig: &AggregateSignature,
) -> bool {
    if sig.r_addr == Address::ZERO || bool::from(sig.s.is_zero()) {
        return false;
    }
    let xx = AlloyU256::from_be_bytes::<32>(x_agg.x_bytes());
    // Scribe requires the aggregate key's x-coordinate to be a valid `r`, i.e. `< n`.
    // (Probability an aggregate lands in [n, p) is ~2^-128; such a subset is unverifiable
    // on-chain and must be reported, never silently accepted.)
    if xx >= CURVE_ORDER {
        return false;
    }
    let e = scalar_to_u256(&challenge(x_agg, message, sig.r_addr));
    let s = scalar_to_u256(&sig.s);
    // h = −s·Xx (mod n); s' = e·Xx (mod n).
    let h = mulmod_neg(s, xx);
    let sp = e.mul_mod(xx, CURVE_ORDER);
    if h == AlloyU256::ZERO || sp == AlloyU256::ZERO {
        return false;
    }
    let v = 27 + x_agg.y_parity();
    match ecrecover_address(
        &h.to_be_bytes::<32>(),
        v,
        &xx.to_be_bytes::<32>(),
        &sp.to_be_bytes::<32>(),
    ) {
        Some(addr) => addr == sig.r_addr,
        None => false,
    }
}

/// A faithful software reimplementation of the EVM `ecrecover` precompile, used to mirror
/// the Solidity verification path exactly.
///
/// Given `(h, v, r, s)` it reconstructs the point `R'` whose x-coordinate is `r (mod p)`
/// with parity `v`, then returns `address( r^{-1}·(s·R' − h·G) )` with `r` inverted modulo
/// the group order `n`. Returns `None` on any invalid input (matching `ecrecover`'s
/// zero-address failure).
pub fn ecrecover_address(h: &[u8; 32], v: u8, r: &[u8; 32], s: &[u8; 32]) -> Option<Address> {
    if v != 27 && v != 28 {
        return None;
    }
    let r_u = AlloyU256::from_be_bytes(*r);
    let s_u = AlloyU256::from_be_bytes(*s);
    if r_u == AlloyU256::ZERO || r_u >= CURVE_ORDER || s_u == AlloyU256::ZERO || s_u >= CURVE_ORDER
    {
        return None;
    }
    // Reconstruct R' from x = r (interpreted in the field) and the parity bit in v.
    let r_point = point_from_x(r, v == 28)?;
    let r_scalar = u256_to_scalar(r_u)?; // r mod n, nonzero (checked above)
    let h_scalar = u256_to_scalar_reduced(h);
    let s_scalar = u256_to_scalar(s_u)?;
    let r_inv = Option::<Scalar>::from(r_scalar.invert())?;
    // Q = r^{-1}·(s·R' − h·G)
    let q =
        (ProjectivePoint::from(r_point) * s_scalar - ProjectivePoint::GENERATOR * h_scalar) * r_inv;
    let q_aff = q.to_affine();
    if bool::from(q_aff.is_identity()) {
        return None;
    }
    Some(eth_address_of_point(&q_aff))
}

/// Reconstructs the affine point with the given big-endian x-coordinate and y-parity, or
/// `None` if x is not on the curve.
fn point_from_x(x: &[u8; 32], y_odd: bool) -> Option<AffinePoint> {
    // SEC1 compressed encoding: 0x02 (even y) / 0x03 (odd y) ‖ x.
    let mut enc = [0u8; 33];
    enc[0] = if y_odd { 0x03 } else { 0x02 };
    enc[1..].copy_from_slice(x);
    let ep = k256::EncodedPoint::from_bytes(enc).ok()?;
    Option::<AffinePoint>::from(AffinePoint::from_encoded_point(&ep))
}

// --- small helpers bridging k256 scalars and ruint U256 (ruint provides mul_mod natively) ---

fn scalar_to_u256(s: &Scalar) -> AlloyU256 {
    AlloyU256::from_be_slice(s.to_bytes().as_slice())
}

/// `v mod n` as a scalar; `v` is known `< n` at every call site (`from_repr` enforces it).
fn u256_to_scalar(v: AlloyU256) -> Option<Scalar> {
    Option::<Scalar>::from(Scalar::from_repr(k256::FieldBytes::from(
        v.to_be_bytes::<32>(),
    )))
}

/// Reduce arbitrary 32 bytes modulo the group order into a scalar.
fn u256_to_scalar_reduced(bytes: &[u8; 32]) -> Scalar {
    Scalar::reduce(U256::from_be_slice(bytes))
}

/// `(−(a · b)) mod n` = `n − ((a·b) mod n)` (0 stays 0).
fn mulmod_neg(a: AlloyU256, b: AlloyU256) -> AlloyU256 {
    let m = a.mul_mod(b, CURVE_ORDER);
    if m == AlloyU256::ZERO {
        AlloyU256::ZERO
    } else {
        CURVE_ORDER - m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::{RecoveryId, Signature as EcdsaSig, SigningKey, VerifyingKey};
    use rand::rngs::StdRng;
    use rand::{RngCore, SeedableRng};

    /// Deterministic entropy source for tests: a `FnMut(&mut [u8])` over a seeded RNG.
    fn seeded(seed: u64) -> impl Entropy {
        let mut rng = StdRng::seed_from_u64(seed);
        move |b: &mut [u8]| rng.fill_bytes(b)
    }

    // ecrecover mirror must match a real k256 ECDSA recovery.
    #[test]
    fn ecrecover_mirror_matches_k256() {
        let sk_bytes = PrivateKey::from_seed(1).scalar().to_bytes();
        let sk = SigningKey::from_bytes(&sk_bytes).unwrap();
        let vk = sk.verifying_key();
        let expected = eth_address_of_point(&vk.as_affine().to_owned());

        let digest = keccak256(b"ecrecover parity");
        let (sig, recid): (EcdsaSig, RecoveryId) =
            sk.sign_prehash_recoverable(digest.as_slice()).unwrap();
        let r: [u8; 32] = sig.r().to_bytes().as_slice().try_into().unwrap();
        let s: [u8; 32] = sig.s().to_bytes().as_slice().try_into().unwrap();
        let v = 27 + recid.to_byte();

        // Sanity: k256 recovers the same key.
        let recovered_vk =
            VerifyingKey::recover_from_prehash(digest.as_slice(), &sig, recid).unwrap();
        assert_eq!(recovered_vk, *vk);

        let got = ecrecover_address(&digest.0, v, &r, &s).expect("mirror recovers");
        assert_eq!(
            got, expected,
            "ecrecover mirror must equal on-chain ecrecover"
        );
    }

    #[test]
    fn single_key_sign_verify_roundtrip() {
        let mut fill = seeded(42);
        let sk = PrivateKey::random(&mut fill);
        let pk = sk.public_key();
        let msg = keccak256(b"single-key schnorr").0;

        // Directly sign as a 1-of-1 aggregate: k random, s = k − e·x.
        let k = random_scalar(&mut fill);
        let r_point = (ProjectivePoint::GENERATOR * k).to_affine();
        let r_addr = eth_address_of_point(&r_point);
        let e = challenge(&pk, &msg, r_addr);
        let s = k - e * sk.scalar();
        let sig = AggregateSignature { s, r_addr };

        assert!(verify_aggregate(&pk, &msg, &sig));
        // Wrong message rejected.
        let other = keccak256(b"other").0;
        assert!(!verify_aggregate(&pk, &other, &sig));
        // Wrong key rejected.
        let pk2 = PrivateKey::from_seed(9).public_key();
        assert!(!verify_aggregate(&pk2, &msg, &sig));
    }

    #[test]
    fn proof_of_possession_roundtrip() {
        let mut fill = seeded(7);
        let sk = PrivateKey::random(&mut fill);
        let pk = sk.public_key();
        let pop = sk.prove_possession(&mut fill);
        assert!(pk.verify_possession(&pop));

        // A PoP does not verify against a different key.
        let other = PrivateKey::from_seed(123).public_key();
        assert!(!other.verify_possession(&pop));

        // Tampered s rejected.
        let mut bad = pop;
        bad.0.s += Scalar::ONE;
        assert!(!pk.verify_possession(&bad));
    }

    #[test]
    fn aggregate_key_is_sum() {
        let ks: Vec<PrivateKey> = (0..5).map(PrivateKey::from_seed).collect();
        let pks: Vec<PublicKey> = ks.iter().map(|k| k.public_key()).collect();
        let agg = PublicKey::aggregate(&pks).unwrap();

        // Σ x_i · G == Σ X_i
        let sum_scalar = ks.iter().fold(Scalar::ZERO, |a, k| a + k.scalar());
        let expected = (ProjectivePoint::GENERATOR * sum_scalar).to_affine();
        assert_eq!(agg.point(), expected);

        // Subtracting a key from the aggregate yields the sub-aggregate (on-chain path).
        let sub = {
            let full = ProjectivePoint::from(agg.point());
            let minus = full - ProjectivePoint::from(pks[0].point());
            PublicKey::from_affine(minus.to_affine()).unwrap()
        };
        let expected_sub = PublicKey::aggregate(&pks[1..]).unwrap();
        assert_eq!(sub.point(), expected_sub.point());
    }

    #[test]
    fn signature_wire_roundtrip() {
        let mut fill = seeded(99);
        let sk = PrivateKey::random(&mut fill);
        let pk = sk.public_key();
        let msg = keccak256(b"wire").0;
        let k = random_scalar(&mut fill);
        let r_addr = eth_address_of_point(&(ProjectivePoint::GENERATOR * k).to_affine());
        let e = challenge(&pk, &msg, r_addr);
        let sig = AggregateSignature {
            s: k - e * sk.scalar(),
            r_addr,
        };
        let bytes = sig.to_bytes();
        let back = AggregateSignature::from_bytes(&bytes).unwrap();
        assert_eq!(sig, back);
        assert!(verify_aggregate(&pk, &msg, &back));
    }
}

//! BN254 key types and EigenLayer-compatible signing.
//!
//! Vendored from `commonware_avs_core::bn254` (BreadchainCoop/commonware-restaking,
//! commonware 0.0.63) and ported to the commonware 2026.5.0 trait shapes. The
//! cryptographic behavior is preserved EXACTLY — this is what keeps certificates
//! verifiable by the deployed EigenLayer contracts (`GasKillerSDK.verifyAndUpdate` →
//! `BLSSignatureChecker`):
//!
//! - Signing maps a 32-byte sha256 digest onto G1 with EigenLayer's `map_to_curve`
//!   (Ethereum-style try-and-increment, NOT a standard hash-to-curve) and multiplies
//!   by the private scalar. Verification is the pairing check
//!   `e(H(m), pk_G2) == e(sig, G2::generator())`.
//! - Namespace semantics (see [`signing_payload`]): a `None` namespace with an exactly
//!   32-byte message means "the message IS the digest" and is signed raw — this is the
//!   on-chain path. Any other input is hashed (with `union_unique` domain separation
//!   when a namespace is present) — this is the path the 2026.5.0 `Signer`/`Verifier`
//!   traits use (the p2p handshake always provides a namespace).
//!
//! Type surface:
//! - [`PrivateKey`]: the BN254 scalar (32-byte ark compressed encoding).
//! - [`Bn254`]: the signer (implements the 2026.5.0 [`commonware_cryptography::Signer`]
//!   trait, so it can be the p2p identity signer for `authenticated::lookup`).
//! - [`PublicKey`]: a G2 point, 64-byte ark compressed encoding. The operator's
//!   registered EigenLayer BN254 key, the p2p identity, AND the participant identity
//!   key ordering the certificate scheme's participant set.
//! - [`Signature`]: a G1 point, 32-byte ark compressed encoding.
//! - [`G1PublicKey`]: a G1 point, 32-byte ark compressed encoding. The router maps
//!   certified signer bitmaps to these when assembling the on-chain
//!   `NonSignerStakesAndSignature` submission.
//!
//! Changes vs. the vendored original (behavior-preserving):
//! - `Signer::sign`/`Verifier::verify` take `namespace: &[u8]` (no longer `Option`);
//!   the old `Option<&[u8]>` bodies live on as [`Bn254::sign_message`] /
//!   [`PublicKey::verify_message`], and the raw-digest path is exposed first-class as
//!   [`Bn254::sign_digest`] / [`PublicKey::verify_digest`].
//! - `Read`/`TryFrom` no longer panic on malformed bytes (these run on untrusted
//!   network input via `Lazy`/`Set` decoding — a panic would be a remote DoS); curve
//!   membership, subgroup, and non-zero checks moved into `read_cfg`.
//! - [`Bn254`] implements `commonware_math::algebra::Random` (new `Signer` supertrait).

pub mod scheme;

pub use scheme::{Bn254Certificate, Bn254Scheme};

use ark_bn254::{Fq, Fq2, Fr as Scalar, G1Affine, G1Projective, G2Affine, G2Projective};
use ark_ec::{AffineRepr, CurveGroup, PrimeGroup, pairing::Pairing};
use ark_ff::{AdditiveGroup, UniformRand};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use bytes::{Buf, BufMut};
use commonware_codec::{Error, FixedSize, Read, Write};
use commonware_cryptography::{
    Hasher as _, PublicKey as CPublicKey, Sha256, Signature as CSignature, Signer, Verifier,
};
use commonware_math::algebra::Random;
use commonware_utils::{Array, Span, union_unique};
use eigen_crypto_bn254::utils::map_to_curve;
use rand_core::CryptoRngCore;
use std::borrow::Cow;
use std::fmt::{Debug, Display};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::str::FromStr;

/// Length of a sha256 digest in bytes.
const DIGEST_LENGTH: usize = 32;
/// Compressed size of the BN254 scalar field element in bytes.
const PRIVATE_KEY_LENGTH: usize = 32;
/// Compressed size of a BN254 G1 point in bytes.
const G1_LENGTH: usize = 32;
/// Compressed size of a [`Signature`] (G1) in bytes.
const SIGNATURE_LENGTH: usize = G1_LENGTH;
/// Compressed size of a BN254 G2 point in bytes.
const G2_LENGTH: usize = 64;
/// Compressed size of a [`PublicKey`] (G2) in bytes.
const PUBLIC_KEY_LENGTH: usize = G2_LENGTH;

/// Computes the 32-byte digest that gets mapped onto G1 for signing/verification.
///
/// INVARIANT (on-chain compatibility): `namespace == None` and a 32-byte message
/// means the message is a pre-hashed digest and is used as-is — bit-identical to the
/// old `Bn254::sign(None, digest)`. Every other combination is hashed:
/// `sha256(union_unique(namespace, message))` when a namespace is present (the p2p
/// handshake path), `sha256(message)` otherwise.
fn signing_payload(namespace: Option<&[u8]>, message: &[u8]) -> [u8; DIGEST_LENGTH] {
    if namespace.is_none() && message.len() == DIGEST_LENGTH {
        return message.try_into().expect("length checked above");
    }
    let payload = match namespace {
        Some(namespace) => Cow::Owned(union_unique(namespace, message)),
        None => Cow::Borrowed(message),
    };
    let mut hasher = Sha256::new();
    hasher.update(payload.as_ref());
    let digest = hasher.finalize();
    digest
        .as_ref()
        .try_into()
        .expect("sha256 digest is 32 bytes")
}

/// A BN254 signer: private scalar plus the derived G2 public key.
///
/// Implements the 2026.5.0 [`Signer`] trait (namespaced path) so it can serve as the
/// p2p identity signer, and exposes [`Bn254::sign_digest`] for the raw on-chain path.
#[derive(Clone)]
pub struct Bn254 {
    private: Scalar,
    public: G2Affine,
}

impl Bn254 {
    /// Builds a signer from a [`PrivateKey`], deriving the G2 public key.
    pub fn new(private_key: PrivateKey) -> Self {
        Self::from_scalar(private_key.key)
    }

    /// Builds a signer directly from a scalar.
    pub fn from_scalar(scalar: Scalar) -> Self {
        let public = (G2Projective::generator() * scalar).into_affine();
        Bn254 {
            private: scalar,
            public,
        }
    }

    /// Returns the private key.
    pub fn private_key(&self) -> PrivateKey {
        PrivateKey::from(self.private)
    }

    /// Returns the G1 public key (`g1 * sk`), as registered on-chain alongside the
    /// G2 key.
    pub fn public_g1(&self) -> G1PublicKey {
        let pk = G1Projective::generator() * self.private;
        G1PublicKey::from(pk.into_affine())
    }

    /// Signs a raw 32-byte digest: `map_to_curve(digest) * sk`, no namespace, no
    /// extra hashing. This is the EigenLayer on-chain signing path — byte-identical
    /// to the old `Bn254::sign(None, digest)`.
    pub fn sign_digest(&self, digest: &[u8; DIGEST_LENGTH]) -> Signature {
        let msg_on_g1 = map_to_curve(digest);
        let sig = msg_on_g1 * self.private;
        Signature::from(sig.into_affine())
    }

    /// Signs with the old `Option<&[u8]>` namespace semantics (see
    /// [`signing_payload`]). The 2026.5.0 [`Signer`] trait delegates here with
    /// `Some(namespace)`; internal digest signing uses `None`.
    pub fn sign_message(&self, namespace: Option<&[u8]>, message: &[u8]) -> Signature {
        self.sign_digest(&signing_payload(namespace, message))
    }
}

impl Signer for Bn254 {
    type Signature = Signature;
    type PublicKey = PublicKey;

    fn sign(&self, namespace: &[u8], message: &[u8]) -> Signature {
        self.sign_message(Some(namespace), message)
    }

    fn public_key(&self) -> PublicKey {
        PublicKey::from(self.public)
    }
}

impl Random for Bn254 {
    fn random(mut rng: impl CryptoRngCore) -> Self {
        // ark's `UniformRand` is rand-0.8 compatible, and every rand_core 0.6
        // `CryptoRngCore` is a rand-0.8 `Rng` via the blanket impl.
        Self::from_scalar(Scalar::rand(&mut rng))
    }
}

impl Debug for Bn254 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the private scalar.
        f.debug_struct("Bn254")
            .field("public", &PublicKey::from(self.public))
            .finish_non_exhaustive()
    }
}

/// A BN254 private key: the scalar plus its 32-byte ark compressed encoding.
#[derive(Clone, Eq, PartialEq)]
pub struct PrivateKey {
    raw: [u8; PRIVATE_KEY_LENGTH],
    key: Scalar,
}

impl FixedSize for PrivateKey {
    const SIZE: usize = PRIVATE_KEY_LENGTH;
}

impl Write for PrivateKey {
    fn write(&self, buf: &mut impl BufMut) {
        self.raw.write(buf);
    }
}

impl Read for PrivateKey {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _cfg: &()) -> Result<Self, Error> {
        let raw = <[u8; PRIVATE_KEY_LENGTH]>::read_cfg(buf, &())?;
        let key = Scalar::deserialize_compressed(raw.as_slice())
            .map_err(|_| Error::Invalid("bn254::PrivateKey", "invalid scalar"))?;
        if key == Scalar::ZERO {
            return Err(Error::Invalid("bn254::PrivateKey", "zero scalar"));
        }
        Ok(PrivateKey { raw, key })
    }
}

impl Hash for PrivateKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl AsRef<[u8]> for PrivateKey {
    fn as_ref(&self) -> &[u8] {
        &self.raw
    }
}

impl From<Scalar> for PrivateKey {
    fn from(key: Scalar) -> Self {
        let mut raw = [0u8; PRIVATE_KEY_LENGTH];
        key.serialize_compressed(&mut raw[..])
            .expect("scalar compressed encoding is exactly 32 bytes");
        Self { raw, key }
    }
}

impl TryFrom<&[u8]> for PrivateKey {
    type Error = Error;
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let raw: [u8; PRIVATE_KEY_LENGTH] = value
            .try_into()
            .map_err(|_| Error::InvalidLength(value.len()))?;
        let key = Scalar::deserialize_compressed(value)
            .map_err(|_| Error::Invalid("bn254::PrivateKey", "invalid scalar"))?;
        if key == Scalar::ZERO {
            return Err(Error::Invalid("bn254::PrivateKey", "zero scalar"));
        }
        Ok(Self { raw, key })
    }
}

impl TryFrom<&Vec<u8>> for PrivateKey {
    type Error = Error;
    fn try_from(value: &Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(value.as_slice())
    }
}

impl TryFrom<Vec<u8>> for PrivateKey {
    type Error = Error;
    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(value.as_slice())
    }
}

impl Debug for PrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        write!(f, "bn254::PrivateKey(REDACTED)")
    }
}

impl Display for PrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A BN254 G2 public key (64-byte ark compressed encoding).
///
/// This is the operator's registered EigenLayer BN254 key and doubles as the p2p
/// identity AND the participant identity key of [`Bn254Scheme`]. Raw bytes are cached
/// alongside the affine point so ordering/eq are byte comparisons — participant
/// indices derive from this ordering, so it must be identical on every process.
#[derive(Clone, Eq, PartialEq)]
pub struct PublicKey {
    raw: [u8; PUBLIC_KEY_LENGTH],
    key: G2Affine,
}

impl Span for PublicKey {}

impl Array for PublicKey {}

impl FixedSize for PublicKey {
    const SIZE: usize = PUBLIC_KEY_LENGTH;
}

impl Write for PublicKey {
    fn write(&self, buf: &mut impl BufMut) {
        self.raw.write(buf);
    }
}

impl Read for PublicKey {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _cfg: &()) -> Result<Self, Error> {
        let raw = <[u8; PUBLIC_KEY_LENGTH]>::read_cfg(buf, &())?;
        // `deserialize_compressed` validates curve membership and subgroup; reject
        // the identity separately (it is never a valid operator key and would act
        // as a "free pass" under aggregate verification).
        let key = G2Affine::deserialize_compressed(raw.as_slice())
            .map_err(|_| Error::Invalid("bn254::PublicKey", "invalid G2 point"))?;
        if key.is_zero() {
            return Err(Error::Invalid("bn254::PublicKey", "identity G2 point"));
        }
        Ok(PublicKey { raw, key })
    }
}

impl Hash for PublicKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl Ord for PublicKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.raw.cmp(&other.raw)
    }
}

impl PartialOrd for PublicKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AsRef<[u8]> for PublicKey {
    fn as_ref(&self) -> &[u8] {
        &self.raw
    }
}

impl Deref for PublicKey {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.raw
    }
}

impl From<G2Affine> for PublicKey {
    fn from(key: G2Affine) -> Self {
        let mut raw = [0u8; PUBLIC_KEY_LENGTH];
        key.serialize_compressed(&mut raw[..])
            .expect("G2 compressed encoding is exactly 64 bytes");
        Self { raw, key }
    }
}

impl TryFrom<&[u8]> for PublicKey {
    type Error = Error;
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() != PUBLIC_KEY_LENGTH {
            return Err(Error::InvalidLength(value.len()));
        }
        Self::read_cfg(&mut &*value, &())
    }
}

impl TryFrom<&Vec<u8>> for PublicKey {
    type Error = Error;
    fn try_from(value: &Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(value.as_slice())
    }
}

impl TryFrom<Vec<u8>> for PublicKey {
    type Error = Error;
    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(value.as_slice())
    }
}

impl Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.raw))
    }
}

impl Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.raw))
    }
}

impl Verifier for PublicKey {
    type Signature = Signature;

    fn verify(&self, namespace: &[u8], message: &[u8], signature: &Signature) -> bool {
        self.verify_message(Some(namespace), message, signature)
    }
}

impl CPublicKey for PublicKey {}

impl PublicKey {
    /// Builds a public key from decimal-string G2 coordinates as found in
    /// `public_orchestrator.json` (`g2_x1`, `g2_x2`, `g2_y1`, `g2_y2`).
    ///
    /// Returns `None` if any coordinate fails to parse. Matches the old crate's
    /// behavior of trusting the coordinates (no on-curve check) — the source is
    /// operator-provided config, not network input.
    pub fn create_from_g2_coordinates(x1: &str, x2: &str, y1: &str, y2: &str) -> Option<Self> {
        let x1_fq = Fq::from_str(x1).ok()?;
        let x2_fq = Fq::from_str(x2).ok()?;
        let y1_fq = Fq::from_str(y1).ok()?;
        let y2_fq = Fq::from_str(y2).ok()?;

        let x_fq2 = Fq2::new(x1_fq, x2_fq);
        let y_fq2 = Fq2::new(y1_fq, y2_fq);

        let g2_point = G2Affine::new_unchecked(x_fq2, y_fq2);
        Some(PublicKey::from(g2_point))
    }

    /// Returns the raw `G2Affine` point.
    pub fn get_point(&self) -> G2Affine {
        self.key
    }

    /// Verifies a signature over a raw 32-byte digest (the EigenLayer on-chain
    /// path): `e(map_to_curve(digest), pk) == e(sig, G2::generator())`.
    pub fn verify_digest(&self, digest: &[u8; DIGEST_LENGTH], signature: &Signature) -> bool {
        let msg_on_g1 = map_to_curve(digest);
        let lhs = ark_bn254::Bn254::pairing(msg_on_g1, self.key);
        let rhs = ark_bn254::Bn254::pairing(signature.sig, G2Affine::generator());
        lhs == rhs
    }

    /// Verifies with the old `Option<&[u8]>` namespace semantics (see
    /// [`signing_payload`]). The 2026.5.0 [`Verifier`] trait delegates here with
    /// `Some(namespace)`; internal digest verification uses `None`.
    pub fn verify_message(
        &self,
        namespace: Option<&[u8]>,
        message: &[u8],
        signature: &Signature,
    ) -> bool {
        self.verify_digest(&signing_payload(namespace, message), signature)
    }
}

/// A BN254 signature: a G1 point (32-byte ark compressed encoding).
#[derive(Clone, Eq, PartialEq)]
pub struct Signature {
    raw: [u8; SIGNATURE_LENGTH],
    sig: G1Affine,
}

impl Span for Signature {}

impl Array for Signature {}

impl FixedSize for Signature {
    const SIZE: usize = SIGNATURE_LENGTH;
}

impl Write for Signature {
    fn write(&self, buf: &mut impl BufMut) {
        self.raw.write(buf);
    }
}

impl Read for Signature {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _cfg: &()) -> Result<Self, Error> {
        let raw = <[u8; SIGNATURE_LENGTH]>::read_cfg(buf, &())?;
        // This runs on untrusted network bytes (via `Lazy<Signature>` decode and
        // certificate decode) — it must error, never panic. `deserialize_compressed`
        // validates curve membership and subgroup.
        let sig = G1Affine::deserialize_compressed(raw.as_slice())
            .map_err(|_| Error::Invalid("bn254::Signature", "invalid G1 point"))?;
        Ok(Signature { raw, sig })
    }
}

impl Hash for Signature {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl Ord for Signature {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.raw.cmp(&other.raw)
    }
}

impl PartialOrd for Signature {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AsRef<[u8]> for Signature {
    fn as_ref(&self) -> &[u8] {
        &self.raw
    }
}

impl Deref for Signature {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.raw
    }
}

impl From<G1Affine> for Signature {
    fn from(sig: G1Affine) -> Self {
        let mut raw = [0u8; SIGNATURE_LENGTH];
        sig.serialize_compressed(&mut raw[..])
            .expect("G1 compressed encoding is exactly 32 bytes");
        Self { raw, sig }
    }
}

impl TryFrom<&[u8]> for Signature {
    type Error = Error;
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() != SIGNATURE_LENGTH {
            return Err(Error::InvalidLength(value.len()));
        }
        Self::read_cfg(&mut &*value, &())
    }
}

impl TryFrom<&Vec<u8>> for Signature {
    type Error = Error;
    fn try_from(value: &Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(value.as_slice())
    }
}

impl TryFrom<Vec<u8>> for Signature {
    type Error = Error;
    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(value.as_slice())
    }
}

impl Debug for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.raw))
    }
}

impl Display for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.raw))
    }
}

impl CSignature for Signature {}

impl Signature {
    /// Returns the raw `G1Affine` point (the router needs it to build the on-chain
    /// sigma `G1Point`).
    pub fn get_point(&self) -> G1Affine {
        self.sig
    }
}

/// A BN254 G1 public key (32-byte ark compressed encoding).
///
/// The router maps certified signer bitmaps to these points when assembling the
/// on-chain `NonSignerStakesAndSignature` call.
#[derive(Clone, Eq, PartialEq)]
pub struct G1PublicKey {
    raw: [u8; G1_LENGTH],
    key: G1Affine,
}

impl Span for G1PublicKey {}

impl Array for G1PublicKey {}

impl FixedSize for G1PublicKey {
    const SIZE: usize = G1_LENGTH;
}

impl Write for G1PublicKey {
    fn write(&self, buf: &mut impl BufMut) {
        self.raw.write(buf);
    }
}

impl Read for G1PublicKey {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _cfg: &()) -> Result<Self, Error> {
        let raw = <[u8; G1_LENGTH]>::read_cfg(buf, &())?;
        let key = G1Affine::deserialize_compressed(raw.as_slice())
            .map_err(|_| Error::Invalid("bn254::G1PublicKey", "invalid G1 point"))?;
        if key.is_zero() {
            return Err(Error::Invalid("bn254::G1PublicKey", "identity G1 point"));
        }
        Ok(G1PublicKey { raw, key })
    }
}

impl Hash for G1PublicKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl Ord for G1PublicKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.raw.cmp(&other.raw)
    }
}

impl PartialOrd for G1PublicKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AsRef<[u8]> for G1PublicKey {
    fn as_ref(&self) -> &[u8] {
        &self.raw
    }
}

impl Deref for G1PublicKey {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.raw
    }
}

impl From<G1Affine> for G1PublicKey {
    fn from(key: G1Affine) -> Self {
        let mut raw = [0u8; G1_LENGTH];
        key.serialize_compressed(&mut raw[..])
            .expect("G1 compressed encoding is exactly 32 bytes");
        Self { raw, key }
    }
}

impl G1PublicKey {
    /// Builds a G1 public key from decimal-string coordinates (e.g. from on-chain
    /// registration data or operator config).
    ///
    /// Returns `None` if a coordinate fails to parse. No on-curve check — same trust
    /// model as [`PublicKey::create_from_g2_coordinates`].
    pub fn create_from_g1_coordinates(x: &str, y: &str) -> Option<Self> {
        let x_fq = Fq::from_str(x).ok()?;
        let y_fq = Fq::from_str(y).ok()?;
        let g1_affine = G1Affine::new_unchecked(x_fq, y_fq);
        Some(G1PublicKey::from(g1_affine))
    }

    /// Returns the x-coordinate of the G1 point as a decimal string.
    pub fn get_x(&self) -> String {
        self.key.x.to_string()
    }

    /// Returns the y-coordinate of the G1 point as a decimal string.
    pub fn get_y(&self) -> String {
        self.key.y.to_string()
    }

    /// Returns the raw `G1Affine` point.
    pub fn get_point(&self) -> G1Affine {
        self.key
    }
}

impl TryFrom<&[u8]> for G1PublicKey {
    type Error = Error;
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() != G1_LENGTH {
            return Err(Error::InvalidLength(value.len()));
        }
        Self::read_cfg(&mut &*value, &())
    }
}

impl TryFrom<&Vec<u8>> for G1PublicKey {
    type Error = Error;
    fn try_from(value: &Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(value.as_slice())
    }
}

impl TryFrom<Vec<u8>> for G1PublicKey {
    type Error = Error;
    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(value.as_slice())
    }
}

impl Debug for G1PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.raw))
    }
}

impl Display for G1PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.raw))
    }
}

/// Aggregates (point-adds) G1 public keys, G2 public keys, and signatures into the
/// three points the on-chain submission flow needs.
pub fn get_points(
    g1: &[G1PublicKey],
    g2: &[PublicKey],
    signatures: &[Signature],
) -> Option<(G1Affine, G2Affine, G1Affine)> {
    let mut agg_public_g1 = G1Projective::ZERO;
    for public in g1 {
        agg_public_g1 += public.key.into_group();
    }
    let agg_public_g1 = agg_public_g1.into_affine();

    let mut agg_public_g2 = G2Projective::ZERO;
    for public in g2 {
        agg_public_g2 += public.key.into_group();
    }
    let agg_public_g2 = agg_public_g2.into_affine();

    let mut agg_signature = G1Projective::ZERO;
    for signature in signatures {
        agg_signature += signature.sig.into_group();
    }
    let agg_signature = agg_signature.into_affine();
    Some((agg_public_g1, agg_public_g2, agg_signature))
}

/// Aggregates signatures by G1 point addition.
///
/// Plain point-addition aggregation is safe here ONLY because operators prove
/// possession of their BN254 keys at EigenLayer registration time (rogue-key
/// defense) — do not reuse with unregistered keys.
pub fn aggregate_signatures(signatures: &[Signature]) -> Option<Signature> {
    let mut agg_signature = G1Projective::ZERO;
    for signature in signatures {
        agg_signature += signature.sig.into_group();
    }
    Some(Signature::from(agg_signature.into_affine()))
}

/// Verifies an aggregated signature against the point-added G2 public keys with a
/// single pairing check. Namespace semantics per [`signing_payload`] (`None` + 32-byte
/// message = raw digest — the on-chain path).
pub fn aggregate_verify(
    publics: &[PublicKey],
    namespace: Option<&[u8]>,
    message: &[u8],
    signature: &Signature,
) -> bool {
    let mut agg_public = G2Projective::ZERO;
    for public in publics {
        agg_public += public.key.into_group();
    }
    let agg_public = PublicKey::from(agg_public.into_affine());
    agg_public.verify_message(namespace, message, signature)
}

/// Builds a signer from a decimal-string private key (the operator key-file format).
///
/// # Panics
///
/// Panics on a malformed key. Only call with operator-provided configuration at
/// startup, never with network input.
pub fn get_signer(key: &str) -> Bn254 {
    let fr = Scalar::from_str(key).expect("Invalid decimal string for private key");
    Bn254::from_scalar(fr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_codec::{DecodeExt, Encode};
    use commonware_utils::test_rng;

    /// A fixed digest for deterministic tests.
    fn digest() -> [u8; DIGEST_LENGTH] {
        let mut hasher = Sha256::new();
        hasher.update(b"gas-killer bn254 test digest");
        hasher.finalize().as_ref().try_into().unwrap()
    }

    #[test]
    fn sign_digest_roundtrip() {
        let signer = get_signer("12345");
        let sig = signer.sign_digest(&digest());
        assert!(signer.public_key().verify_digest(&digest(), &sig));

        // Wrong digest is rejected.
        let mut wrong = digest();
        wrong[0] ^= 0x01;
        assert!(!signer.public_key().verify_digest(&wrong, &sig));

        // Wrong key is rejected.
        let other = get_signer("54321");
        assert!(!other.public_key().verify_digest(&digest(), &sig));
    }

    #[test]
    fn sign_digest_matches_legacy_none_namespace_semantics() {
        // The old code path was `sign(None, digest)` with the "None + 32 bytes =>
        // raw digest" special case; `sign_digest` and `sign_message(None, ..)` must
        // agree bit-for-bit.
        let signer = get_signer("999");
        let raw = signer.sign_digest(&digest());
        let legacy = signer.sign_message(None, &digest());
        assert_eq!(raw.as_ref(), legacy.as_ref());
    }

    #[test]
    fn namespaced_signing_is_domain_separated() {
        let signer = get_signer("777");
        let public = signer.public_key();
        let message = b"some message";

        let sig = Signer::sign(&signer, b"ns-a", message);
        assert!(Verifier::verify(&public, b"ns-a", message, &sig));
        // Different namespace or message fails.
        assert!(!Verifier::verify(&public, b"ns-b", message, &sig));
        assert!(!Verifier::verify(&public, b"ns-a", b"other message", &sig));

        // The namespaced path never collides with the raw digest path, even when
        // the message is 32 bytes.
        let sig32 = Signer::sign(&signer, b"ns-a", &digest());
        assert_ne!(sig32.as_ref(), signer.sign_digest(&digest()).as_ref());
    }

    #[test]
    fn none_namespace_non_digest_message_is_hashed() {
        // A `None` namespace with a message that is NOT exactly 32 bytes hashes the
        // message (legacy behavior).
        let signer = get_signer("31337");
        let message = b"not thirty-two bytes";
        let sig = signer.sign_message(None, message);

        let mut hasher = Sha256::new();
        hasher.update(message);
        let hashed: [u8; DIGEST_LENGTH] = hasher.finalize().as_ref().try_into().unwrap();
        assert_eq!(sig.as_ref(), signer.sign_digest(&hashed).as_ref());
    }

    #[test]
    fn aggregate_sign_and_verify() {
        let signers: Vec<Bn254> = ["1", "2", "3", "4"].iter().map(|k| get_signer(k)).collect();
        let publics: Vec<PublicKey> = signers.iter().map(|s| s.public_key()).collect();
        let signatures: Vec<Signature> =
            signers.iter().map(|s| s.sign_digest(&digest())).collect();

        let aggregate = aggregate_signatures(&signatures).unwrap();
        assert!(aggregate_verify(&publics, None, &digest(), &aggregate));

        // Missing one signer's key fails the pairing check.
        assert!(!aggregate_verify(&publics[..3], None, &digest(), &aggregate));

        // Aggregate of a subset does not verify against the full set.
        let partial = aggregate_signatures(&signatures[..3]).unwrap();
        assert!(!aggregate_verify(&publics, None, &digest(), &partial));
    }

    #[test]
    fn get_points_matches_aggregate_signatures() {
        let signers: Vec<Bn254> = ["11", "22", "33"].iter().map(|k| get_signer(k)).collect();
        let g1: Vec<G1PublicKey> = signers.iter().map(|s| s.public_g1()).collect();
        let g2: Vec<PublicKey> = signers.iter().map(|s| s.public_key()).collect();
        let signatures: Vec<Signature> =
            signers.iter().map(|s| s.sign_digest(&digest())).collect();

        let (_, _, agg_sig_point) = get_points(&g1, &g2, &signatures).unwrap();
        let aggregate = aggregate_signatures(&signatures).unwrap();
        assert_eq!(Signature::from(agg_sig_point).as_ref(), aggregate.as_ref());
    }

    #[test]
    fn public_key_codec_roundtrip() {
        let public = get_signer("424242").public_key();
        let encoded = public.encode();
        assert_eq!(encoded.len(), PublicKey::SIZE);
        let decoded = PublicKey::decode(encoded).unwrap();
        assert_eq!(decoded, public);
    }

    #[test]
    fn signature_codec_roundtrip() {
        let sig = get_signer("424242").sign_digest(&digest());
        let encoded = sig.encode();
        assert_eq!(encoded.len(), Signature::SIZE);
        let decoded = Signature::decode(encoded).unwrap();
        assert_eq!(decoded, sig);
    }

    #[test]
    fn g1_public_key_codec_roundtrip() {
        let g1 = get_signer("424242").public_g1();
        let encoded = g1.encode();
        assert_eq!(encoded.len(), G1PublicKey::SIZE);
        let decoded = G1PublicKey::decode(encoded).unwrap();
        assert_eq!(decoded, g1);
    }

    #[test]
    fn private_key_codec_roundtrip() {
        let private = get_signer("13").private_key();
        let encoded = private.encode();
        assert_eq!(encoded.len(), PrivateKey::SIZE);
        let decoded = PrivateKey::decode(encoded).unwrap();
        assert_eq!(decoded, private);
        // Re-derived signer produces identical signatures.
        let rebuilt = Bn254::new(decoded);
        assert_eq!(
            rebuilt.sign_digest(&digest()).as_ref(),
            get_signer("13").sign_digest(&digest()).as_ref()
        );
    }

    #[test]
    fn malformed_bytes_error_instead_of_panicking() {
        // 0xff-filled buffers are not valid compressed points/scalars for any of
        // the fixed-size types. The old vendored code panicked here; these run on
        // untrusted network bytes now and must return errors.
        assert!(PublicKey::decode(&[0xffu8; PUBLIC_KEY_LENGTH][..]).is_err());
        assert!(Signature::decode(&[0xffu8; SIGNATURE_LENGTH][..]).is_err());
        assert!(G1PublicKey::decode(&[0xffu8; G1_LENGTH][..]).is_err());
        assert!(PrivateKey::decode(&[0xffu8; PRIVATE_KEY_LENGTH][..]).is_err());
    }

    #[test]
    fn try_from_rejects_bad_lengths() {
        assert!(matches!(
            PublicKey::try_from(&[0u8; 5][..]),
            Err(Error::InvalidLength(5))
        ));
        assert!(matches!(
            Signature::try_from(&[0u8; 33][..]),
            Err(Error::InvalidLength(33))
        ));
        assert!(matches!(
            G1PublicKey::try_from(&[0u8; 0][..]),
            Err(Error::InvalidLength(0))
        ));
        assert!(matches!(
            PrivateKey::try_from(&[0u8; 31][..]),
            Err(Error::InvalidLength(31))
        ));
    }

    #[test]
    fn zero_keys_rejected() {
        // The identity element serializes to the "infinity flag" compressed
        // encoding; decoding it must fail for key types.
        let mut zero_g2 = [0u8; PUBLIC_KEY_LENGTH];
        G2Affine::zero().serialize_compressed(&mut zero_g2[..]).unwrap();
        assert!(PublicKey::decode(&zero_g2[..]).is_err());

        let zero_scalar = [0u8; PRIVATE_KEY_LENGTH];
        assert!(PrivateKey::decode(&zero_scalar[..]).is_err());
    }

    #[test]
    fn coordinate_constructors_match_key_derivation() {
        // `create_from_g{1,2}_coordinates` (config-file path) must reproduce the
        // same compressed bytes as deriving the keys from the private scalar.
        let signer = get_signer("31415926");
        let g1 = signer.public_g1();
        let g2_point = signer.public_key().get_point();

        let g1_rebuilt = G1PublicKey::create_from_g1_coordinates(&g1.get_x(), &g1.get_y()).unwrap();
        assert_eq!(g1_rebuilt, g1);

        let g2_rebuilt = PublicKey::create_from_g2_coordinates(
            &g2_point.x.c0.to_string(),
            &g2_point.x.c1.to_string(),
            &g2_point.y.c0.to_string(),
            &g2_point.y.c1.to_string(),
        )
        .unwrap();
        assert_eq!(g2_rebuilt, signer.public_key());
    }

    #[test]
    fn random_signers_are_distinct_and_functional() {
        let mut rng = test_rng();
        let a = Bn254::random(&mut rng);
        let b = Bn254::random(&mut rng);
        assert_ne!(a.public_key(), b.public_key());
        let sig = a.sign_digest(&digest());
        assert!(a.public_key().verify_digest(&digest(), &sig));

        // `from_seed` (provided by the Signer trait via Random) is deterministic.
        let s1 = Bn254::from_seed(7);
        let s2 = Bn254::from_seed(7);
        assert_eq!(s1.public_key(), s2.public_key());
    }

    #[test]
    fn g1_and_g2_keys_share_the_discrete_log() {
        // e(pk_G1, g2) == e(g1, pk_G2) iff both keys were derived from the same
        // scalar — the consistency the on-chain registration relies on.
        let signer = get_signer("2718281828");
        let lhs = ark_bn254::Bn254::pairing(
            signer.public_g1().get_point(),
            G2Affine::generator(),
        );
        let rhs = ark_bn254::Bn254::pairing(
            G1Affine::generator(),
            signer.public_key().get_point(),
        );
        assert_eq!(lhs, rhs);
    }
}

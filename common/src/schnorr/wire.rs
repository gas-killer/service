//! Wire protocol for the interactive MuSig2 aggregate-Schnorr signing rounds
//! (p2p channel 2, `SIGNATURE_SCHEME=schnorr` mode).
//!
//! A signing **session** is identified by `(height, attempt)`: the aggregation height
//! the sequencer assigned, and the coordinator's retry counter for that height. Every
//! attempt uses **fresh nonces** — a `SecNonce` is bound to exactly one session and
//! consumed on signing (see the nonce-reuse discussion in [`super::musig`]).
//!
//! Flow (router = coordinator, nodes = participants):
//!
//! ```text
//! router --NonceRequest{h,a}-->  all operators
//! node   --NonceCommit{h,a,pubkey,nonce}--> router      (idempotent re-send of the
//!                                                        SAME pubnonce on duplicates)
//! router --SignRequest{h,a,message,signers,agg_nonces,r_addr}--> subset S
//! node   --PartialSig{h,a,partial}--> router
//! ```
//!
//! Authentication model: the p2p layer authenticates peers (a node only accepts channel-2
//! traffic from the router; the router only from tracked operators). Key material inside
//! the messages is bound to those identities by address: `NonceCommit.pubkey` must satisfy
//! `eth_address(pubkey) == sender`, and every `SignRequest.signers` entry must map to a
//! known operator address — the same `keccak256(x ‖ y)[12..]` identity the on-chain
//! `SchnorrStakeRegistry` registers keys under (with a proof of possession), so a
//! coordinator cannot smuggle an unregistered key into the aggregate.

use alloy_primitives::Address;
use bytes::{Buf, BufMut};
use commonware_codec::varint::UInt;
use commonware_codec::{EncodeSize, Error, Read, ReadExt, Write};
use k256::Scalar;
use k256::elliptic_curve::PrimeField;

use super::musig::PubNonce;
use super::{MESSAGE_LEN, PublicKey};

/// Wire tag for [`SchnorrMsg::NonceRequest`].
const TAG_NONCE_REQUEST: u8 = 0;
/// Wire tag for [`SchnorrMsg::NonceCommit`].
const TAG_NONCE_COMMIT: u8 = 1;
/// Wire tag for [`SchnorrMsg::SignRequest`].
const TAG_SIGN_REQUEST: u8 = 2;
/// Wire tag for [`SchnorrMsg::PartialSig`].
const TAG_PARTIAL_SIG: u8 = 3;

/// Upper bound on the signer list length — bounds allocation before the p2p
/// `max_message_size` check would (33 bytes/point × 4096 ≈ 132 KB, well inside the
/// 1 MB channel limit).
const MAX_SIGNERS: usize = 4096;

/// Round-2 broadcast: everything a participant needs to (re)derive the signing
/// context locally. See the module docs for the authentication model.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SignRequest {
    pub height: u64,
    pub attempt: u32,
    /// The 32-byte digest being signed. Participants refuse to sign unless this
    /// equals the digest they derived themselves from their own TaskBook/validator.
    pub message: [u8; MESSAGE_LEN],
    /// The signer subset `S` as compressed public-key points (the coordinator's
    /// nonce-round responders). Participants recompute `X_agg = Σ signers` after
    /// validating each point against the known operator set.
    pub signers: Vec<PublicKey>,
    /// Aggregate nonce pair `(R1_agg, R2_agg)` over the subset.
    pub agg_nonces: PubNonce,
    /// The coordinator's claimed `address(R)` (re-derived and checked by signers).
    pub r_addr: Address,
}

impl SignRequest {
    /// A collision-resistant digest of the full signing context, used by
    /// participants to distinguish "the same request again" (idempotent partial
    /// re-send) from "a DIFFERENT context for a session that already signed"
    /// (refused — that shape is a nonce-reuse attack).
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut pre = Vec::with_capacity(32 + 20 + 66 + self.signers.len() * 33);
        pre.extend_from_slice(&self.message);
        pre.extend_from_slice(self.r_addr.as_slice());
        pre.extend_from_slice(&self.agg_nonces.to_bytes());
        for signer in &self.signers {
            pre.extend_from_slice(&signer.to_compressed());
        }
        alloy_primitives::keccak256(pre).0
    }
}

/// A message on the Schnorr protocol channel.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SchnorrMsg {
    /// Router → operators: open session `(height, attempt)`, request a fresh nonce.
    NonceRequest { height: u64, attempt: u32 },
    /// Node → router: the node's public nonce pair (and its public key, so the
    /// router learns the point behind the p2p address).
    NonceCommit {
        height: u64,
        attempt: u32,
        pubkey: PublicKey,
        nonce: PubNonce,
    },
    /// Router → subset: the signing context (round 2 open).
    SignRequest(SignRequest),
    /// Node → router: the partial signature scalar `s_i`.
    PartialSig {
        height: u64,
        attempt: u32,
        partial: Scalar,
    },
}

impl SchnorrMsg {
    /// The session height this message addresses.
    pub fn height(&self) -> u64 {
        match self {
            SchnorrMsg::NonceRequest { height, .. } => *height,
            SchnorrMsg::NonceCommit { height, .. } => *height,
            SchnorrMsg::SignRequest(r) => r.height,
            SchnorrMsg::PartialSig { height, .. } => *height,
        }
    }

    /// The session attempt this message addresses.
    pub fn attempt(&self) -> u32 {
        match self {
            SchnorrMsg::NonceRequest { attempt, .. } => *attempt,
            SchnorrMsg::NonceCommit { attempt, .. } => *attempt,
            SchnorrMsg::SignRequest(r) => r.attempt,
            SchnorrMsg::PartialSig { attempt, .. } => *attempt,
        }
    }
}

/// Wire tag for [`PrecommitMsg::BatchAnnounce`].
const TAG_BATCH_ANNOUNCE: u8 = 0;
/// Wire tag for [`PrecommitMsg::BatchRequest`].
const TAG_BATCH_REQUEST: u8 = 1;
/// Wire tag for [`PrecommitMsg::CompletionPartial`].
const TAG_COMPLETION_PARTIAL: u8 = 2;

/// Maximum nonce pairs per [`PrecommitMsg::BatchAnnounce`] chunk
/// (512 × 66 B ≈ 33 KB, well inside the 1 MB channel limit).
pub const MAX_CHUNK_NONCES: usize = 512;

/// Messages for the pre-committed-nonce mode (`docs/schnorr-nonce-registry.md`): batch
/// distribution plus the completion round for `Attested` certificates. Session traffic
/// (attempt-0 partials) rides the aggregation engine's own channel, not this one.
///
/// Authentication model: as with [`SchnorrMsg`], the p2p layer authenticates peers;
/// `BatchAnnounce` is additionally self-authenticating — receivers verify the batch's
/// recomputed Merkle root against the on-chain `SchnorrNonceRegistry` registration for
/// `eth_address(pubkey)` (and may check `signature` even before on-chain confirmation).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PrecommitMsg {
    /// Operator → peers: one chunk of a registered nonce batch.
    BatchAnnounce {
        /// The operator's Schnorr public key (identity = its Ethereum address).
        pubkey: PublicKey,
        /// The batch's position in the operator's on-chain list.
        batch_index: u64,
        /// First absolute slot the batch covers.
        start_slot: u64,
        /// Total slots in the batch (all chunks).
        total: u64,
        /// The batch registration signature (over the Rust/Solidity `batch_message`).
        signature: crate::schnorr::AggregateSignature,
        /// Offset of this chunk's first nonce within the batch.
        chunk_offset: u64,
        /// This chunk's nonce pairs (`≤ MAX_CHUNK_NONCES`).
        nonces: Vec<PubNonce>,
    },
    /// Peer → operator (or any holder): request a missing chunk.
    BatchRequest {
        /// Operator identity address whose batch is requested.
        operator: Address,
        batch_index: u64,
        chunk_offset: u64,
    },
    /// Completion round for an `Attested` certificate: the sender's partial for the
    /// certified signer set at `(height, attempt)` — no request message precedes this;
    /// the set is read from the engine certificate everyone holds.
    CompletionPartial {
        height: u64,
        attempt: u32,
        /// The sender's view of `address(R)` for the completion context (cross-check).
        r_addr: Address,
        partial: Scalar,
    },
}

impl Write for PrecommitMsg {
    fn write(&self, buf: &mut impl BufMut) {
        match self {
            PrecommitMsg::BatchAnnounce {
                pubkey,
                batch_index,
                start_slot,
                total,
                signature,
                chunk_offset,
                nonces,
            } => {
                TAG_BATCH_ANNOUNCE.write(buf);
                buf.put_slice(&pubkey.to_compressed());
                UInt(*batch_index).write(buf);
                UInt(*start_slot).write(buf);
                UInt(*total).write(buf);
                buf.put_slice(&signature.to_bytes());
                UInt(*chunk_offset).write(buf);
                (nonces.len() as u32).write(buf);
                for nonce in nonces {
                    buf.put_slice(&nonce.to_bytes());
                }
            }
            PrecommitMsg::BatchRequest {
                operator,
                batch_index,
                chunk_offset,
            } => {
                TAG_BATCH_REQUEST.write(buf);
                buf.put_slice(operator.as_slice());
                UInt(*batch_index).write(buf);
                UInt(*chunk_offset).write(buf);
            }
            PrecommitMsg::CompletionPartial {
                height,
                attempt,
                r_addr,
                partial,
            } => {
                TAG_COMPLETION_PARTIAL.write(buf);
                UInt(*height).write(buf);
                attempt.write(buf);
                buf.put_slice(r_addr.as_slice());
                buf.put_slice(&partial_to_bytes(partial));
            }
        }
    }
}

impl Read for PrecommitMsg {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _: &()) -> Result<Self, Error> {
        let tag = u8::read(buf)?;
        match tag {
            TAG_BATCH_ANNOUNCE => {
                let pubkey = read_pubkey(buf)?;
                let batch_index: u64 = UInt::read(buf)?.into();
                let start_slot: u64 = UInt::read(buf)?.into();
                let total: u64 = UInt::read(buf)?.into();
                let sig_bytes: [u8; 52] = read_array(buf)?;
                let signature = crate::schnorr::AggregateSignature::from_bytes(&sig_bytes)
                    .ok_or(Error::Invalid("PrecommitMsg", "invalid batch signature"))?;
                let chunk_offset: u64 = UInt::read(buf)?.into();
                let count = u32::read(buf)? as usize;
                if count == 0 || count > MAX_CHUNK_NONCES {
                    return Err(Error::InvalidLength(count));
                }
                if buf.remaining() < count * 66 {
                    return Err(Error::EndOfBuffer);
                }
                let mut nonces = Vec::with_capacity(count);
                for _ in 0..count {
                    nonces.push(read_pubnonce(buf)?);
                }
                // Chunk must lie inside the declared batch.
                let end = chunk_offset
                    .checked_add(count as u64)
                    .ok_or(Error::Invalid("PrecommitMsg", "chunk overflow"))?;
                if end > total {
                    return Err(Error::Invalid("PrecommitMsg", "chunk beyond batch"));
                }
                Ok(PrecommitMsg::BatchAnnounce {
                    pubkey,
                    batch_index,
                    start_slot,
                    total,
                    signature,
                    chunk_offset,
                    nonces,
                })
            }
            TAG_BATCH_REQUEST => {
                let operator = Address::from(read_array::<20>(buf)?);
                let batch_index: u64 = UInt::read(buf)?.into();
                let chunk_offset: u64 = UInt::read(buf)?.into();
                Ok(PrecommitMsg::BatchRequest {
                    operator,
                    batch_index,
                    chunk_offset,
                })
            }
            TAG_COMPLETION_PARTIAL => {
                let height: u64 = UInt::read(buf)?.into();
                let attempt = u32::read(buf)?;
                let r_addr = Address::from(read_array::<20>(buf)?);
                let bytes: [u8; 32] = read_array(buf)?;
                let partial = partial_from_bytes(&bytes).ok_or(Error::Invalid(
                    "PrecommitMsg",
                    "partial scalar out of range",
                ))?;
                Ok(PrecommitMsg::CompletionPartial {
                    height,
                    attempt,
                    r_addr,
                    partial,
                })
            }
            other => Err(Error::InvalidEnum(other)),
        }
    }
}

impl EncodeSize for PrecommitMsg {
    fn encode_size(&self) -> usize {
        match self {
            PrecommitMsg::BatchAnnounce {
                batch_index,
                start_slot,
                total,
                chunk_offset,
                nonces,
                ..
            } => {
                1 + 33
                    + UInt(*batch_index).encode_size()
                    + UInt(*start_slot).encode_size()
                    + UInt(*total).encode_size()
                    + 52
                    + UInt(*chunk_offset).encode_size()
                    + 4
                    + nonces.len() * 66
            }
            PrecommitMsg::BatchRequest {
                batch_index,
                chunk_offset,
                ..
            } => 1 + 20 + UInt(*batch_index).encode_size() + UInt(*chunk_offset).encode_size(),
            PrecommitMsg::CompletionPartial {
                height, attempt, ..
            } => 1 + UInt(*height).encode_size() + attempt.encode_size() + 20 + 32,
        }
    }
}

/// Serializes a partial-signature scalar (32-byte big-endian).
pub fn partial_to_bytes(partial: &Scalar) -> [u8; 32] {
    partial.to_bytes().into()
}

/// Deserializes a partial-signature scalar, rejecting values ≥ the group order.
/// Zero is accepted: it is an (astronomically unlikely but) valid partial.
pub fn partial_from_bytes(bytes: &[u8; 32]) -> Option<Scalar> {
    Option::<Scalar>::from(Scalar::from_repr((*bytes).into()))
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

fn read_pubkey(buf: &mut impl Buf) -> Result<PublicKey, Error> {
    let bytes: [u8; 33] = read_array(buf)?;
    PublicKey::from_compressed(&bytes).ok_or(Error::Invalid(
        "SchnorrMsg",
        "invalid compressed public key",
    ))
}

fn read_pubnonce(buf: &mut impl Buf) -> Result<PubNonce, Error> {
    let bytes: [u8; 66] = read_array(buf)?;
    PubNonce::from_bytes(&bytes).ok_or(Error::Invalid("SchnorrMsg", "invalid nonce points"))
}

impl Write for SchnorrMsg {
    fn write(&self, buf: &mut impl BufMut) {
        match self {
            SchnorrMsg::NonceRequest { height, attempt } => {
                TAG_NONCE_REQUEST.write(buf);
                UInt(*height).write(buf);
                attempt.write(buf);
            }
            SchnorrMsg::NonceCommit {
                height,
                attempt,
                pubkey,
                nonce,
            } => {
                TAG_NONCE_COMMIT.write(buf);
                UInt(*height).write(buf);
                attempt.write(buf);
                buf.put_slice(&pubkey.to_compressed());
                buf.put_slice(&nonce.to_bytes());
            }
            SchnorrMsg::SignRequest(r) => {
                TAG_SIGN_REQUEST.write(buf);
                UInt(r.height).write(buf);
                r.attempt.write(buf);
                buf.put_slice(&r.message);
                (r.signers.len() as u32).write(buf);
                for signer in &r.signers {
                    buf.put_slice(&signer.to_compressed());
                }
                buf.put_slice(&r.agg_nonces.to_bytes());
                buf.put_slice(r.r_addr.as_slice());
            }
            SchnorrMsg::PartialSig {
                height,
                attempt,
                partial,
            } => {
                TAG_PARTIAL_SIG.write(buf);
                UInt(*height).write(buf);
                attempt.write(buf);
                buf.put_slice(&partial_to_bytes(partial));
            }
        }
    }
}

impl Read for SchnorrMsg {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _: &()) -> Result<Self, Error> {
        let tag = u8::read(buf)?;
        let height: u64 = UInt::read(buf)?.into();
        let attempt = u32::read(buf)?;
        match tag {
            TAG_NONCE_REQUEST => Ok(SchnorrMsg::NonceRequest { height, attempt }),
            TAG_NONCE_COMMIT => {
                let pubkey = read_pubkey(buf)?;
                let nonce = read_pubnonce(buf)?;
                Ok(SchnorrMsg::NonceCommit {
                    height,
                    attempt,
                    pubkey,
                    nonce,
                })
            }
            TAG_SIGN_REQUEST => {
                let message: [u8; MESSAGE_LEN] = read_array(buf)?;
                let count = u32::read(buf)? as usize;
                if count == 0 || count > MAX_SIGNERS {
                    return Err(Error::InvalidLength(count));
                }
                if buf.remaining() < count * 33 {
                    return Err(Error::EndOfBuffer);
                }
                let mut signers = Vec::with_capacity(count);
                for _ in 0..count {
                    signers.push(read_pubkey(buf)?);
                }
                let agg_nonces = read_pubnonce(buf)?;
                let addr_bytes: [u8; 20] = read_array(buf)?;
                Ok(SchnorrMsg::SignRequest(SignRequest {
                    height,
                    attempt,
                    message,
                    signers,
                    agg_nonces,
                    r_addr: Address::from(addr_bytes),
                }))
            }
            TAG_PARTIAL_SIG => {
                let bytes: [u8; 32] = read_array(buf)?;
                let partial = partial_from_bytes(&bytes)
                    .ok_or(Error::Invalid("SchnorrMsg", "partial scalar out of range"))?;
                Ok(SchnorrMsg::PartialSig {
                    height,
                    attempt,
                    partial,
                })
            }
            other => Err(Error::InvalidEnum(other)),
        }
    }
}

impl EncodeSize for SchnorrMsg {
    fn encode_size(&self) -> usize {
        let header = 1 + UInt(self.height()).encode_size() + self.attempt().encode_size();
        match self {
            SchnorrMsg::NonceRequest { .. } => header,
            SchnorrMsg::NonceCommit { .. } => header + 33 + 66,
            SchnorrMsg::SignRequest(r) => header + MESSAGE_LEN + 4 + r.signers.len() * 33 + 66 + 20,
            SchnorrMsg::PartialSig { .. } => header + 32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schnorr::musig::gen_nonce;
    use crate::schnorr::{Entropy, PrivateKey};
    use commonware_codec::{DecodeExt, Encode};
    use rand::rngs::StdRng;
    use rand::{RngCore, SeedableRng};

    fn seeded(seed: u64) -> impl Entropy {
        let mut rng = StdRng::seed_from_u64(seed);
        move |b: &mut [u8]| rng.fill_bytes(b)
    }

    #[test]
    fn nonce_request_roundtrip() {
        let original = SchnorrMsg::NonceRequest {
            height: u64::MAX,
            attempt: 7,
        };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        assert_eq!(SchnorrMsg::decode(encoded).unwrap(), original);
    }

    #[test]
    fn nonce_commit_roundtrip() {
        let mut fill = seeded(1);
        let key = PrivateKey::from_seed(42);
        let (_, pubnonce) = gen_nonce(&mut fill);
        let original = SchnorrMsg::NonceCommit {
            height: 9,
            attempt: 1,
            pubkey: key.public_key(),
            nonce: pubnonce,
        };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        assert_eq!(SchnorrMsg::decode(encoded).unwrap(), original);
    }

    #[test]
    fn sign_request_roundtrip() {
        let mut fill = seeded(2);
        let keys: Vec<PrivateKey> = (0..3).map(PrivateKey::from_seed).collect();
        let (_, pubnonce) = gen_nonce(&mut fill);
        let original = SchnorrMsg::SignRequest(SignRequest {
            height: 1_000_000,
            attempt: 3,
            message: [0xab; 32],
            signers: keys.iter().map(|k| k.public_key()).collect(),
            agg_nonces: pubnonce,
            r_addr: Address::from([0x11; 20]),
        });
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        assert_eq!(SchnorrMsg::decode(encoded).unwrap(), original);
    }

    #[test]
    fn partial_sig_roundtrip() {
        let key = PrivateKey::from_seed(5);
        let original = SchnorrMsg::PartialSig {
            height: 3,
            attempt: 2,
            partial: key.scalar(),
        };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        assert_eq!(SchnorrMsg::decode(encoded).unwrap(), original);
    }

    #[test]
    fn empty_signer_set_rejected() {
        let mut fill = seeded(3);
        let (_, pubnonce) = gen_nonce(&mut fill);
        let msg = SchnorrMsg::SignRequest(SignRequest {
            height: 1,
            attempt: 1,
            message: [0; 32],
            signers: vec![],
            agg_nonces: pubnonce,
            r_addr: Address::from([1; 20]),
        });
        assert!(matches!(
            SchnorrMsg::decode(msg.encode()),
            Err(Error::InvalidLength(0))
        ));
    }

    #[test]
    fn unknown_tag_rejected() {
        let mut bytes = SchnorrMsg::NonceRequest {
            height: 1,
            attempt: 1,
        }
        .encode_mut();
        bytes[0] = 9;
        assert!(matches!(
            SchnorrMsg::decode(bytes.freeze()),
            Err(Error::InvalidEnum(9))
        ));
    }

    #[test]
    fn truncated_commit_rejected() {
        let mut fill = seeded(4);
        let key = PrivateKey::from_seed(6);
        let (_, pubnonce) = gen_nonce(&mut fill);
        let encoded = SchnorrMsg::NonceCommit {
            height: 1,
            attempt: 1,
            pubkey: key.public_key(),
            nonce: pubnonce,
        }
        .encode();
        let truncated = encoded.slice(0..encoded.len() - 1);
        assert!(SchnorrMsg::decode(truncated).is_err());
    }

    #[test]
    fn invalid_point_rejected() {
        let key = PrivateKey::from_seed(7);
        let mut fill = seeded(5);
        let (_, pubnonce) = gen_nonce(&mut fill);
        let mut bytes = SchnorrMsg::NonceCommit {
            height: 1,
            attempt: 1,
            pubkey: key.public_key(),
            nonce: pubnonce,
        }
        .encode_mut();
        // Corrupt the compressed pubkey's prefix byte (valid prefixes are 0x02/0x03).
        let pk_offset = bytes.len() - 33 - 66;
        bytes[pk_offset] = 0xff;
        assert!(SchnorrMsg::decode(bytes.freeze()).is_err());
    }

    #[test]
    fn partial_scalar_roundtrips_through_bytes() {
        let s = PrivateKey::from_seed(11).scalar();
        assert_eq!(partial_from_bytes(&partial_to_bytes(&s)).unwrap(), s);
        // Group order itself must be rejected (≥ n).
        let n_bytes: [u8; 32] = crate::schnorr::CURVE_ORDER.to_be_bytes();
        assert!(partial_from_bytes(&n_bytes).is_none());
    }

    // ---- PrecommitMsg (batch gossip + completion round) ----

    fn announce_fixture() -> PrecommitMsg {
        let mut fill = seeded(21);
        let key = PrivateKey::from_seed(21);
        let batch = crate::schnorr::precommit::NonceBatch::generate(
            crate::schnorr::precommit::BatchDomain {
                chain_id: 31337,
                registry: Address::repeat_byte(0x42),
                operator: key.public_key().eth_address(),
            },
            &[3u8; 32],
            0,
            0,
            8,
        )
        .unwrap();
        let signature = batch.sign_registration(&key, &mut fill).unwrap();
        PrecommitMsg::BatchAnnounce {
            pubkey: key.public_key(),
            batch_index: 0,
            start_slot: 0,
            total: batch.count(),
            signature,
            chunk_offset: 2,
            nonces: batch.nonces[2..6].to_vec(),
        }
    }

    #[test]
    fn batch_announce_roundtrip() {
        let original = announce_fixture();
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        assert_eq!(PrecommitMsg::decode(encoded).unwrap(), original);
    }

    #[test]
    fn batch_announce_rejects_out_of_batch_chunk() {
        let PrecommitMsg::BatchAnnounce {
            pubkey,
            batch_index,
            start_slot,
            signature,
            chunk_offset,
            nonces,
            ..
        } = announce_fixture()
        else {
            unreachable!()
        };
        // Declared total smaller than chunk end → decode rejects.
        let bad = PrecommitMsg::BatchAnnounce {
            pubkey,
            batch_index,
            start_slot,
            total: 3,
            signature,
            chunk_offset,
            nonces,
        };
        assert!(PrecommitMsg::decode(bad.encode()).is_err());
    }

    #[test]
    fn batch_request_roundtrip() {
        let original = PrecommitMsg::BatchRequest {
            operator: Address::repeat_byte(0x77),
            batch_index: u64::MAX,
            chunk_offset: 12345,
        };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        assert_eq!(PrecommitMsg::decode(encoded).unwrap(), original);
    }

    #[test]
    fn completion_partial_roundtrip() {
        let original = PrecommitMsg::CompletionPartial {
            height: 99,
            attempt: 2,
            r_addr: Address::repeat_byte(0x11),
            partial: PrivateKey::from_seed(13).scalar(),
        };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        assert_eq!(PrecommitMsg::decode(encoded).unwrap(), original);

        // Out-of-range scalar rejected at decode.
        let mut bytes = original.encode_mut();
        let len = bytes.len();
        bytes[len - 32..].fill(0xff);
        assert!(PrecommitMsg::decode(bytes.freeze()).is_err());
    }
}

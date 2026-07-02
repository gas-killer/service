//! Task directives broadcast by the router to the nodes (p2p channel 1).
//!
//! The router assigns each task a monotonically increasing aggregation height and
//! announces it with [`TaskDirective::Announce`]. Because the aggregation engine's
//! tip only advances when every height certifies (see the liveness model in the
//! design notes), a height the router abandons MUST still resolve — that is what
//! [`TaskDirective::Skip`] and [`skip_digest`] are for: nodes sign `skip_digest(h)`
//! so the height certifies with a sentinel digest instead of stalling the pipeline.

use bytes::{Buf, BufMut};
use commonware_codec::varint::UInt;
use commonware_codec::{EncodeSize, Error, Read, ReadExt, Write};
use commonware_cryptography::sha256::Digest;
use commonware_cryptography::{Hasher, Sha256};

use crate::task_data::GasKillerTaskData;

/// Wire tag for [`TaskDirective::Announce`].
const TAG_ANNOUNCE: u8 = 0;
/// Wire tag for [`TaskDirective::Skip`].
const TAG_SKIP: u8 = 1;

/// Domain separator for [`skip_digest`].
///
/// Versioned so a future format change cannot collide with old skip certificates.
const SKIP_DIGEST_NAMESPACE: &[u8] = b"GK_SKIP_V1";

/// A router → node broadcast assigning (or abandoning) an aggregation height.
///
/// Wire format: tag `u8` + varint `UInt(height)` + (`Announce` only) the
/// [`GasKillerTaskData`] payload. The nested task read is bounded the same way
/// `task_data.rs` bounds it: length prefixes are checked against the remaining
/// buffer, and the p2p channel's `max_message_size` caps the overall size.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskDirective {
    /// Assigns `task` to aggregation height `height`; nodes validate the task and
    /// sign its expected digest.
    Announce {
        height: u64,
        task: GasKillerTaskData,
    },
    /// Abandons `height`; nodes sign [`skip_digest`]`(height)` so the height still
    /// certifies and the pipeline advances.
    Skip { height: u64 },
}

impl TaskDirective {
    /// The height this directive addresses.
    pub fn height(&self) -> u64 {
        match self {
            TaskDirective::Announce { height, .. } => *height,
            TaskDirective::Skip { height } => *height,
        }
    }
}

impl Write for TaskDirective {
    fn write(&self, buf: &mut impl BufMut) {
        match self {
            TaskDirective::Announce { height, task } => {
                TAG_ANNOUNCE.write(buf);
                UInt(*height).write(buf);
                task.write(buf);
            }
            TaskDirective::Skip { height } => {
                TAG_SKIP.write(buf);
                UInt(*height).write(buf);
            }
        }
    }
}

impl Read for TaskDirective {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _: &()) -> Result<Self, Error> {
        let tag = u8::read(buf)?;
        let height: u64 = UInt::read(buf)?.into();
        match tag {
            TAG_ANNOUNCE => {
                let task = GasKillerTaskData::read(buf)?;
                Ok(TaskDirective::Announce { height, task })
            }
            TAG_SKIP => Ok(TaskDirective::Skip { height }),
            other => Err(Error::InvalidEnum(other)),
        }
    }
}

impl EncodeSize for TaskDirective {
    fn encode_size(&self) -> usize {
        let tag_and_height = 1 + UInt(self.height()).encode_size();
        match self {
            TaskDirective::Announce { task, .. } => tag_and_height + task.encode_size(),
            TaskDirective::Skip { .. } => tag_and_height,
        }
    }
}

/// The sentinel digest a quorum signs to certify that height `height` carries no
/// task: `sha256("GK_SKIP_V1" || height.to_be_bytes())`.
///
/// The router treats a certificate whose digest equals `skip_digest(h)` as "task at
/// `h` abandoned by quorum" and never submits it on-chain. The height is bound into
/// the preimage so skip certificates are not replayable across heights (unlike task
/// digests, which are protected by the contract's transition-index ordering instead).
pub fn skip_digest(height: u64) -> Digest {
    let mut hasher = Sha256::new();
    hasher.update(SKIP_DIGEST_NAMESPACE);
    hasher.update(&height.to_be_bytes());
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use commonware_codec::{DecodeExt, Encode};

    fn sample_task() -> GasKillerTaskData {
        GasKillerTaskData {
            storage_updates: vec![0xaa, 0xbb, 0xcc].into(),
            transition_index: 7,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(42u64),
            block_height: 123_456,
            chain_id: 31337,
        }
    }

    #[test]
    fn announce_roundtrip() {
        let original = TaskDirective::Announce {
            height: u64::MAX,
            task: sample_task(),
        };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        let decoded = TaskDirective::decode(encoded).expect("decode failed");
        assert_eq!(decoded, original);
    }

    #[test]
    fn skip_roundtrip() {
        let original = TaskDirective::Skip { height: 0 };
        let encoded = original.encode();
        assert_eq!(encoded.len(), original.encode_size());
        let decoded = TaskDirective::decode(encoded).expect("decode failed");
        assert_eq!(decoded, original);
    }

    #[test]
    fn unknown_tag_rejected() {
        let mut bytes = TaskDirective::Skip { height: 5 }.encode_mut();
        bytes[0] = 2;
        assert!(matches!(
            TaskDirective::decode(bytes.freeze()),
            Err(Error::InvalidEnum(2))
        ));
    }

    #[test]
    fn truncated_announce_rejected() {
        let encoded = TaskDirective::Announce {
            height: 9,
            task: sample_task(),
        }
        .encode();
        let truncated = encoded.slice(0..encoded.len() - 1);
        assert!(TaskDirective::decode(truncated).is_err());
    }

    #[test]
    fn skip_digest_is_deterministic() {
        assert_eq!(skip_digest(42), skip_digest(42));
    }

    #[test]
    fn skip_digest_distinct_per_height() {
        assert_ne!(skip_digest(0), skip_digest(1));
        assert_ne!(skip_digest(1), skip_digest(256));
        // Big-endian height bytes: byte-shifted heights must not collide.
        assert_ne!(skip_digest(1 << 8), skip_digest(1 << 16));
    }
}

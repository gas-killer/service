//! Nested settlement: a task whose call reaches other SDK consumers settles as one tree, one
//! frame per consumer, under one quorum signature over the tree's Merkle root.
//!
//! The router decides per task whether to nest and, if so, fixes the tree's expiry before any
//! node traces it ([`NestedSpec`]); every node and the router then split the same trace the
//! same way and sign the same root. A task whose trace yields a single frame signs today's
//! digest and settles through `verifyAndUpdate`, so nothing about it changes.

use std::collections::BTreeSet;

use alloy_primitives::{Address, Bytes};
use anyhow::{Result, bail};
use bytes::{Buf, BufMut};
use commonware_codec::{EncodeSize, Read, ReadExt, Write};
use commonware_cryptography::sha256::Digest;
use gas_analyzer::nested::{EncodedTree, FrameProgram, encode_frame_tree};
use serde::{Deserialize, Serialize};

use crate::task_data::GasKillerTaskData;

/// What a nested signing round fixes up front, carried to every node with the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NestedSpec {
    /// The last block at which the tree may settle. Signed as a leaf of the tree, so it bounds
    /// the payload's lifetime no matter which reference block a submitter picks.
    pub expiry_block: u64,
}

impl Write for NestedSpec {
    fn write(&self, buf: &mut impl BufMut) {
        self.expiry_block.write(buf);
    }
}

impl Read for NestedSpec {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _: &()) -> Result<Self, commonware_codec::Error> {
        Ok(Self {
            expiry_block: u64::read(buf)?,
        })
    }
}

impl EncodeSize for NestedSpec {
    fn encode_size(&self) -> usize {
        std::mem::size_of::<u64>()
    }
}

/// The last block a tree announced at `head` may settle in: `buffer` blocks out, but never so far
/// that a payload referencing the block before head goes stale under the root's
/// `blockStaleMeasure`, and never on or past the registry's next possible set change.
pub fn nested_expiry(head: u64, buffer: u64, stale_measure: u64, horizon: u64) -> u64 {
    head.saturating_add(buffer.min(stale_measure.saturating_sub(1)))
        .min(horizon.saturating_sub(1))
}

/// A task's trace split into frames, with what signing and settling it need.
#[derive(Debug, Clone)]
pub struct TreeTrace {
    pub frames: Vec<FrameProgram>,
    /// Every contract whose transition counter the call moves: the contracts the settlement
    /// pins, and so the ones the router locks.
    pub counter_moves: BTreeSet<Address>,
    /// The root frame's encoded program, which is the whole program for a one-frame tree.
    pub root_program: Bytes,
    /// The tree the quorum signs; `None` when the trace yielded a single frame.
    pub encoded: Option<EncodedTree>,
    /// What the quorum signs: the tree's root, or today's digest for a single frame.
    pub digest: Digest,
}

impl TreeTrace {
    pub fn is_nested(&self) -> bool {
        self.encoded.is_some()
    }
}

/// Builds the signed form of `frames` for `task`.
///
/// The root leaf binds the router's announced transition index, so a trace whose root index
/// differs (the call was traced against a state the root has since moved past) yields no
/// digest at all rather than one built on old state.
pub fn build_tree_trace(
    task: &GasKillerTaskData,
    frames: Vec<FrameProgram>,
    counter_moves: BTreeSet<Address>,
    spec: &NestedSpec,
) -> Result<TreeTrace> {
    let Some(root) = frames.first() else {
        bail!("a frame tree needs its root frame");
    };
    let root_program = gas_analyzer::nested::encoded_program(&root.updates);
    if frames.len() == 1 {
        let digest = task.build_payload_hash(&root_program);
        return Ok(TreeTrace {
            frames,
            counter_moves,
            root_program,
            encoded: None,
            digest,
        });
    }

    let traced_index = root
        .transition_index
        .ok_or_else(|| anyhow::anyhow!("the root frame never incremented its counter"))?;
    if traced_index != alloy_primitives::U256::from(task.transition_index) {
        bail!(
            "the trace puts the root at transition {traced_index}, the task at {}",
            task.transition_index
        );
    }
    let encoded = encode_frame_tree(
        &frames,
        task.function_selector(),
        task.chain_id,
        spec.expiry_block,
    )?;
    let digest = Digest::from(encoded.root.0);
    Ok(TreeTrace {
        frames,
        counter_moves,
        root_program,
        encoded: Some(encoded),
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, U256};
    use commonware_codec::{DecodeExt, Encode};
    use gas_analyzer::nested::{IStateUpdateTypes, StateUpdate};

    fn nested_op(target: Address) -> StateUpdate {
        StateUpdate::Nested(IStateUpdateTypes::Nested {
            target,
            value: U256::ZERO,
            childLeaf: B256::ZERO,
        })
    }

    fn frame(target: Address, index: u64, children: Vec<usize>) -> FrameProgram {
        FrameProgram {
            target,
            caller: Address::ZERO,
            value: U256::ZERO,
            calldata_hash: B256::ZERO,
            transition_index: Some(U256::from(index)),
            updates: Vec::new(),
            children,
        }
    }

    fn task(index: u64) -> GasKillerTaskData {
        GasKillerTaskData {
            transition_index: index,
            target_address: Address::repeat_byte(0xaa),
            call_data: vec![0xde, 0xad, 0xbe, 0xef],
            chain_id: 1,
            ..Default::default()
        }
    }

    #[test]
    fn expiry_stays_inside_the_buffer_the_stale_measure_and_the_horizon() {
        assert_eq!(nested_expiry(1000, 50, 300, u64::MAX), 1050);
        assert_eq!(nested_expiry(1000, 50, 20, u64::MAX), 1019);
        assert_eq!(nested_expiry(1000, 50, 300, 1010), 1009);
    }

    #[test]
    fn spec_roundtrips() {
        let spec = NestedSpec { expiry_block: 1060 };
        let encoded = spec.encode();
        assert_eq!(encoded.len(), spec.encode_size());
        assert_eq!(NestedSpec::decode(encoded).unwrap(), spec);
    }

    #[test]
    fn a_single_frame_signs_todays_digest() {
        let t = task(4);
        let trace = build_tree_trace(
            &t,
            vec![frame(t.target_address, 4, vec![])],
            BTreeSet::new(),
            &NestedSpec { expiry_block: 99 },
        )
        .unwrap();
        assert!(!trace.is_nested());
        assert_eq!(trace.digest, t.build_payload_hash(&trace.root_program));
    }

    #[test]
    fn a_tree_signs_its_root() {
        let t = task(4);
        let mut root = frame(t.target_address, 4, vec![1]);
        root.updates.push(nested_op(Address::repeat_byte(0xbb)));
        let child = frame(Address::repeat_byte(0xbb), 0, vec![]);
        let trace = build_tree_trace(
            &t,
            vec![root, child],
            BTreeSet::new(),
            &NestedSpec { expiry_block: 99 },
        )
        .unwrap();
        let encoded = trace.encoded.as_ref().expect("nested");
        assert_eq!(trace.digest, Digest::from(encoded.root.0));
        assert_ne!(trace.digest, t.build_payload_hash(&trace.root_program));
    }

    #[test]
    fn a_tree_traced_at_another_root_index_has_no_digest() {
        let t = task(4);
        let mut root = frame(t.target_address, 3, vec![1]);
        root.updates.push(nested_op(Address::repeat_byte(0xbb)));
        let child = frame(Address::repeat_byte(0xbb), 0, vec![]);
        assert!(
            build_tree_trace(
                &t,
                vec![root, child],
                BTreeSet::new(),
                &NestedSpec { expiry_block: 99 }
            )
            .is_err()
        );
    }
}

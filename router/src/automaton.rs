//! Router automaton: answers the verifier-only engine's digest requests.
//!
//! The aggregation engine calls `propose(h)` for every height in its window even
//! though the router never signs. Resolving the oneshot with the sequencer's
//! expected digest lets the engine mark the height `Pending::Verified` and filter
//! buffered acks against it; for unassigned heights the sender is DROPPED — the
//! engine records `AppProposeCanceled` (a warn, nothing more) and still assembles
//! certificates for that height purely from network acks. Dropping (rather than
//! parking) the sender keeps the engine's futures pool from accumulating
//! never-resolving proposals.
//!
//! Note the inherent race: the engine proposes `[tip, tip+window)` eagerly at
//! startup, usually before the sequencer assigns anything, so most proposals
//! resolve as dropped. That is expected and harmless (see the report on the
//! verifier-only trace) — certificates form from acks regardless.

use commonware_consensus::Automaton;
use commonware_consensus::types::Height;
use commonware_cryptography::sha256::Digest;
use commonware_utils::channel::oneshot;
use tracing::{debug, error};

use crate::sequencer::SharedAssignments;

/// Automaton for the router's verifier-only aggregation engine.
///
/// Cheap to clone (the engine requires `Clone`): the only state is the shared
/// assignment map written by the sequencer.
#[derive(Clone)]
pub struct RouterAutomaton {
    assignments: SharedAssignments,
}

impl RouterAutomaton {
    pub fn new(assignments: SharedAssignments) -> Self {
        Self { assignments }
    }
}

impl Automaton for RouterAutomaton {
    type Context = Height;
    type Digest = Digest;

    async fn propose(&mut self, height: Height) -> oneshot::Receiver<Digest> {
        let (sender, receiver) = oneshot::channel();
        let digest = match self.assignments.read() {
            Ok(assignments) => assignments.get(&height.get()).map(|a| a.digest),
            Err(_) => {
                // Poisoned lock: a writer panicked. Treat as unassigned rather
                // than propagating the panic into the engine.
                error!(height = height.get(), "assignments lock poisoned");
                None
            }
        };
        match digest {
            Some(digest) => {
                debug!(height = height.get(), %digest, "resolving propose with expected digest");
                let _ = sender.send(digest);
            }
            None => {
                // Unassigned: drop the sender so the engine observes a canceled
                // proposal immediately instead of holding a pending future.
                drop(sender);
            }
        }
        receiver
    }

    async fn verify(&mut self, _context: Height, _payload: Digest) -> oneshot::Receiver<bool> {
        // The aggregation engine never calls verify (propose-only contract);
        // resolve trivially to satisfy the trait.
        gas_killer_common::trivial_verify()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::{Assignment, shared_assignments};
    use alloy_primitives::{Address, U256};
    use gas_killer_common::GasKillerTaskData;

    fn sample_task() -> GasKillerTaskData {
        GasKillerTaskData {
            storage_updates: vec![0x01, 0x02].into(),
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78],
            from_address: Address::from([2u8; 20]),
            value: U256::ZERO,
            block_height: 10,
            chain_id: 31337,
        }
    }

    #[tokio::test]
    async fn propose_resolves_assigned_height_and_drops_unassigned() {
        let assignments = shared_assignments();
        let task = sample_task();
        let digest = task.build_payload_hash(&task.storage_updates);
        assignments
            .write()
            .unwrap()
            .insert(7, Assignment { digest, task });

        let mut automaton = RouterAutomaton::new(assignments);

        // Assigned: resolves with the expected digest.
        let receiver = automaton.propose(Height::new(7)).await;
        assert_eq!(receiver.await.unwrap(), digest);

        // Unassigned: the sender is dropped, surfacing a RecvError.
        let receiver = automaton.propose(Height::new(8)).await;
        assert!(receiver.await.is_err());
    }

    #[tokio::test]
    async fn verify_resolves_true() {
        let mut automaton = RouterAutomaton::new(shared_assignments());
        let receiver = automaton
            .verify(Height::new(1), Digest::from([0u8; 32]))
            .await;
        assert!(receiver.await.unwrap());
    }
}

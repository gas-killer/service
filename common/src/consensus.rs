//! Shared glue for the commonware-consensus aggregation engine used by both the
//! node (signing participant) and the router (verifier-only).

use commonware_consensus::Monitor;
use commonware_consensus::types::Epoch;
use commonware_cryptography::certificate::Signers;
use commonware_cryptography::sha256::Digest;
use commonware_utils::channel::{mpsc, oneshot};
use commonware_utils::sync::Mutex;
use std::sync::Arc;

/// Scheme-agnostic read surface over an aggregation certificate, so the node/router
/// reporters can stay generic across [`crate::EcdsaScheme`] and
/// [`crate::schnorr::scheme::SchnorrScheme`].
pub trait CertificateInspect {
    /// The signer bitmap.
    fn signer_bitmap(&self) -> &Signers;

    /// The final constant-size aggregate Schnorr signature, when this certificate is
    /// directly on-chain-submittable as one (`None` for ECDSA certificates and for
    /// `Attested` Schnorr certificates awaiting their completion round).
    fn schnorr_aggregate(&self) -> Option<crate::schnorr::AggregateSignature> {
        None
    }

    /// Whether a completion round is required to finalize (Schnorr `Attested`).
    fn needs_completion(&self) -> bool {
        false
    }
}

impl CertificateInspect for crate::EcdsaCertificate {
    fn signer_bitmap(&self) -> &Signers {
        &self.signers
    }
}

impl CertificateInspect for crate::schnorr::scheme::SchnorrCertificate {
    fn signer_bitmap(&self) -> &Signers {
        self.signers()
    }

    fn schnorr_aggregate(&self) -> Option<crate::schnorr::AggregateSignature> {
        self.aggregate_signature()
    }

    fn needs_completion(&self) -> bool {
        matches!(
            self,
            crate::schnorr::scheme::SchnorrCertificate::Attested { .. }
        )
    }
}

/// A certificate observation forwarded by the reporters to a scheme-specific
/// consumer (the precommit actors): everything needed to trigger a completion round
/// (node) or submit/collect (router) without holding the generic certificate type.
#[derive(Debug, Clone)]
pub struct CertifiedSummary {
    pub height: u64,
    pub digest: Digest,
    pub signers: Signers,
    /// `Some` when directly submittable (Schnorr `Aggregate`); `None` for ECDSA
    /// certificates and Schnorr `Attested` ones.
    pub signature: Option<crate::schnorr::AggregateSignature>,
    /// Whether the certificate awaits a completion round.
    pub needs_completion: bool,
}

/// Heights this far below the engine tip are pruned from the node's directive log
/// and dedupe sets, and drive the router's certificate bookkeeping.
///
/// Must comfortably exceed the engine's `activity_timeout` (default 256): heights
/// the engine can still re-request after a restart replay must remain answerable.
/// Shared here so the node's `TaskBook`/reporter and the router's reporter cannot
/// drift to different values.
pub const PRUNE_SLACK: u64 = 1024;

/// Resolves a [`commonware_consensus::Automaton::verify`] call trivially.
///
/// The aggregation engine never calls `verify` (it only requests digests via
/// `propose`), but the trait requires an implementation. Both the node and router
/// automatons delegate here.
pub fn trivial_verify() -> oneshot::Receiver<bool> {
    let (sender, receiver) = oneshot::channel();
    let _ = sender.send(true);
    receiver
}

/// [`Monitor`] pinned to a single static epoch: `subscribe` returns `Epoch::zero()`
/// and a channel that never fires (this deployment has no epoch transitions).
///
/// The engine exits ("epoch subscription failed") if the sender side of the
/// subscription drops, so every sender handed out is retained for the process
/// lifetime. A `Vec` of senders (rather than a single take-once `Option`) tolerates
/// the engine subscribing more than once without panicking.
#[derive(Clone, Default)]
pub struct StaticEpochMonitor {
    subscribers: Arc<Mutex<Vec<mpsc::Sender<Epoch>>>>,
}

impl StaticEpochMonitor {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Monitor for StaticEpochMonitor {
    type Index = Epoch;

    async fn subscribe(&mut self) -> (Epoch, mpsc::Receiver<Epoch>) {
        let (sender, receiver) = mpsc::channel(1);
        self.subscribers.lock().push(sender);
        (Epoch::zero(), receiver)
    }
}

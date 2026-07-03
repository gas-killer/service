//! CertReporter actor: the router's sink for aggregation-engine activity.
//!
//! The verifier-only engine reports [`Activity::Certified`] once per assembled
//! certificate and [`Activity::Tip`] on every tip fast-forward — and, on restart,
//! REPLAYS every journaled activity before the network starts. The reporter must
//! therefore be non-blocking (`report()` enqueues to a mailbox that never drops)
//! and replay-idempotent (certificates are deduplicated by height; tips are folded
//! with `max`).
//!
//! Certified heights are defensively re-verified against the verifier scheme and
//! forwarded to the [`crate::submitter::Submitter`] over an mpsc channel. Replayed
//! certificates from a previous router life are forwarded too — the submitter finds
//! no assignment for them and resolves them without an on-chain call, which is
//! exactly how the sequencer learns about pre-restart heights.

use ::tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use commonware_actor::{Feedback, mailbox};
use commonware_consensus::Reporter;
use commonware_consensus::aggregation::types::{Activity, Certificate};
use commonware_consensus::types::Height;
use commonware_cryptography::sha256::Digest;
use commonware_parallel::Sequential;
use commonware_runtime::{Supervisor, tokio};
use commonware_utils::NZUsize;
use commonware_utils::channel::oneshot;
use gas_killer_common::bn254::{Bn254Certificate, Bn254Scheme};
use std::collections::{BTreeMap, VecDeque};
use tracing::{debug, error, info, trace, warn};

/// A certificate observation handed to the submitter.
#[derive(Clone, Debug)]
pub struct CertifiedHeight {
    /// The aggregation height the certificate covers.
    pub height: u64,
    /// The certified digest — either a task's expected payload hash or
    /// `skip_digest(height)`.
    pub digest: Digest,
    /// The BN254 certificate (signer bitmap + aggregated G1 signature) as
    /// assembled by the engine and re-verified by this reporter.
    pub certificate: Bn254Certificate,
}

pub type CertifiedSender = UnboundedSender<CertifiedHeight>;
pub type CertifiedReceiver = UnboundedReceiver<CertifiedHeight>;

/// Channel carrying verified certificates from the reporter to the submitter.
pub fn certified_channel() -> (CertifiedSender, CertifiedReceiver) {
    ::tokio::sync::mpsc::unbounded_channel()
}

/// Heights this far below the observed tip are pruned from the dedupe map.
/// Shared with the node's actors via `gas_killer_common` so they cannot drift.
use gas_killer_common::PRUNE_SLACK;

/// Mailbox capacity before messages spill to the unbounded overflow queue.
///
/// Overflow never drops: `report()` is called inline in the engine loop (and
/// during journal replay before the network starts) and must not lose activity.
const MAILBOX_CAPACITY: usize = 1024;

/// Messages processed by the [`CertReporter`] actor.
enum Message {
    /// A newly assembled (or replayed) certificate from the engine.
    Certified(Certificate<Bn254Scheme, Digest>),
    /// A tip fast-forward from the engine.
    Tip(Height),
    /// Query: the next height the engine needs a certificate for.
    GetTip(oneshot::Sender<u64>),
    /// Query: the certified digest for a height, if observed.
    Get(u64, oneshot::Sender<Option<Digest>>),
}

impl mailbox::Policy for Message {
    type Overflow = VecDeque<Self>;

    fn handle(overflow: &mut VecDeque<Self>, message: Self) {
        // Never drop: losing a Certified would wedge the sequencer on its
        // in-flight height and losing a query would leave a caller pending.
        overflow.push_back(message);
    }
}

/// Handle for reporting to and querying the [`CertReporter`] actor. Cheap to
/// clone; the clone handed to the engine is its `Reporter`.
#[derive(Clone)]
pub struct CertReporterMailbox {
    sender: mailbox::Sender<Message>,
}

impl Reporter for CertReporterMailbox {
    type Activity = Activity<Bn254Scheme, Digest>;

    fn report(&mut self, activity: Self::Activity) -> Feedback {
        match activity {
            Activity::Certified(certificate) => {
                self.sender.enqueue(Message::Certified(certificate))
            }
            Activity::Tip(height) => self.sender.enqueue(Message::Tip(height)),
            // Own acks are journaled and re-reported on replay, but a
            // verifier-only scheme never signs — tolerate and ignore.
            Activity::Ack(_) => Feedback::Ok,
        }
    }
}

impl CertReporterMailbox {
    /// Returns the engine's observed tip: the next height needing a certificate
    /// (max of reported tips and `certified height + 1`). `0` until the engine
    /// reports anything, or when the actor is gone (shutdown).
    pub async fn get_tip(&self) -> u64 {
        let (responder, receiver) = oneshot::channel();
        if !self.sender.enqueue(Message::GetTip(responder)).accepted() {
            warn!("cert reporter closed; tip query unanswered");
            return 0;
        }
        receiver.await.unwrap_or(0)
    }

    /// Returns the certified digest for `height`, if a certificate was observed
    /// (and not yet pruned). `None` when the actor is gone.
    pub async fn get(&self, height: u64) -> Option<Digest> {
        let (responder, receiver) = oneshot::channel();
        if !self
            .sender
            .enqueue(Message::Get(height, responder))
            .accepted()
        {
            warn!(height, "cert reporter closed; digest query unanswered");
            return None;
        }
        receiver.await.unwrap_or(None)
    }
}

/// Actor owning the certificate log. The context doubles as the RNG for the
/// defensive certificate re-verification.
pub struct CertReporter {
    context: tokio::Context,
    mailbox: mailbox::Receiver<Message>,
    /// Verifier scheme (`me() == None`) used to re-verify certificates.
    scheme: Bn254Scheme,
    /// Certified digest per height: dedupe (replay-idempotency) + query source.
    certified: BTreeMap<u64, Digest>,
    /// Next height the engine needs a certificate for (see [`CertReporterMailbox::get_tip`]).
    tip: u64,
    /// Verified certificates bound for the submitter.
    submit: CertifiedSender,
}

impl CertReporter {
    /// Creates the actor and its mailbox handle.
    ///
    /// `context` labels the mailbox metrics and supplies the verification RNG;
    /// `scheme` must be the same verifier instance the engine runs with.
    pub fn new(
        context: tokio::Context,
        scheme: Bn254Scheme,
        submit: CertifiedSender,
    ) -> (Self, CertReporterMailbox) {
        let (sender, receiver) = mailbox::new(context.child("mailbox"), NZUsize!(MAILBOX_CAPACITY));
        (
            Self {
                context,
                mailbox: receiver,
                scheme,
                certified: BTreeMap::new(),
                tip: 0,
                submit,
            },
            CertReporterMailbox { sender },
        )
    }

    /// Runs until every mailbox handle is dropped.
    pub async fn run(mut self) {
        while let Some(message) = self.mailbox.recv().await {
            match message {
                Message::Certified(certificate) => self.handle_certified(certificate),
                Message::Tip(height) => self.advance_tip(height.get()),
                Message::GetTip(responder) => {
                    let _ = responder.send(self.tip);
                }
                Message::Get(height, responder) => {
                    let _ = responder.send(self.certified.get(&height).cloned());
                }
            }
        }
        info!("cert reporter mailbox closed; exiting");
    }

    fn handle_certified(&mut self, certificate: Certificate<Bn254Scheme, Digest>) {
        let height = certificate.item.height.get();
        let digest = certificate.item.digest;

        // Replay-idempotency: the engine re-reports every journaled certificate
        // on restart, and a certificate is unique per height (same digest
        // guaranteed by quorum intersection) — first observation wins.
        if let Some(existing) = self.certified.get(&height) {
            if *existing != digest {
                // Quorum equivocation across restarts — impossible under the
                // fault assumptions; surface loudly but keep the first.
                error!(
                    height,
                    existing = %existing,
                    conflicting = %digest,
                    "conflicting certificate digests for one height (BUG)"
                );
            } else {
                trace!(height, "duplicate certificate ignored");
            }
            return;
        }

        // Defensive re-verification with the verifier scheme. The engine already
        // verified every contributing ack, so a failure here means journal
        // corruption or a scheme mismatch: refuse to submit (the on-chain check
        // would reject it anyway) and keep the height unresolved so the wedge is
        // operator-visible via the sequencer's rebroadcast warnings.
        if !certificate.verify(&mut self.context, &self.scheme, &Sequential) {
            error!(
                height,
                digest = %digest,
                "certificate failed defensive verification; dropping (BUG)"
            );
            return;
        }

        debug!(height, digest = %digest, "certificate observed");
        self.certified.insert(height, digest);
        self.advance_tip(height + 1);

        // Forward to the submitter. A closed channel means the submitter died —
        // the router cannot make progress without it.
        let certified = CertifiedHeight {
            height,
            digest,
            certificate: certificate.certificate,
        };
        if self.submit.send(certified).is_err() {
            error!(height, "submitter channel closed; certificate dropped");
        }
    }

    /// Folds a tip observation (monotonic max) and prunes old dedupe entries.
    fn advance_tip(&mut self, tip: u64) {
        if tip <= self.tip {
            return;
        }
        self.tip = tip;
        let floor = tip.saturating_sub(PRUNE_SLACK);
        // `split_off` keeps entries >= floor.
        let kept = self.certified.split_off(&floor);
        self.certified = kept;
    }
}

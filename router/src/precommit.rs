//! Router-side actor for the pre-committed-nonce mode
//! (`SIGNATURE_SCHEME=schnorr-precommit`): the verifier engine's reporter, the
//! sequencer's certificate index, the channel-2 batch-gossip peer, and the
//! completion-round collector — one actor, because every input converges on the same
//! per-height state (see `docs/schnorr-nonce-registry.md` §7.3).
//!
//! Inputs and what they drive:
//! * **Engine activity** (mailbox): `Certified` certificates are deduped by height and
//!   defensively re-verified (mirroring `CertReporter`); `Aggregate` ones (and skip
//!   heights) resolve immediately into [`SchnorrCertified`] observations for the
//!   submitter; `Attested` ones open a [`CompletionCollector`].
//! * **Channel-2 p2p**: batch announces/requests feed and serve the [`BatchStore`]
//!   (the router needs every operator's committed nonces to verify partials);
//!   [`PrecommitMsg::CompletionPartial`]s feed the open collector for their height,
//!   and a completed collector emits the final observation.
//! * **A pull tick** requests batches the store still lacks.

use commonware_actor::{Feedback, mailbox};
use commonware_codec::{DecodeExt, Encode};
use commonware_consensus::Reporter;
use commonware_consensus::aggregation::types::{Activity, Certificate};
use commonware_consensus::types::Height;
use commonware_cryptography::sha256::Digest;
use commonware_p2p::{Receiver, Recipients, Sender};
use commonware_parallel::Sequential;
use commonware_runtime::{Supervisor as _, tokio};
use commonware_utils::NZUsize;
use commonware_utils::channel::oneshot;
use gas_killer_common::ecdsa::PublicKey;
use gas_killer_common::schnorr::batches::{Added, BatchStore, CompletionCollector, Ingest};
use gas_killer_common::schnorr::scheme::SchnorrScheme;
use gas_killer_common::schnorr::wire::{MAX_CHUNK_NONCES, PrecommitMsg};
use gas_killer_common::{PRUNE_SLACK, skip_digest};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, trace, warn};

use crate::reporter::CertIndex;
use crate::schnorr_coordinator::{SchnorrCertified, SchnorrCertifiedSender};

/// The completion attempt this router collects (must match the node actor).
const COMPLETION_ATTEMPT: u32 = 1;

/// Mailbox capacity before messages spill to the unbounded overflow queue.
const MAILBOX_CAPACITY: usize = 1024;

/// Messages processed by the actor (engine activity + sequencer queries).
enum Message {
    Certified(Certificate<SchnorrScheme, Digest>),
    Tip(Height),
    GetTip(oneshot::Sender<u64>),
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

/// Handle for reporting to and querying the actor. The clone handed to the engine is
/// its `Reporter`; another clone is the sequencer's [`CertIndex`].
#[derive(Clone)]
pub struct SchnorrPrecommitMailbox {
    sender: mailbox::Sender<Message>,
}

impl Reporter for SchnorrPrecommitMailbox {
    type Activity = Activity<SchnorrScheme, Digest>;

    fn report(&mut self, activity: Self::Activity) -> Feedback {
        match activity {
            Activity::Certified(certificate) => {
                self.sender.enqueue(Message::Certified(certificate))
            }
            Activity::Tip(height) => self.sender.enqueue(Message::Tip(height)),
            // A verifier-only scheme never signs — tolerate and ignore.
            Activity::Ack(_) => Feedback::Ok,
        }
    }
}

impl CertIndex for SchnorrPrecommitMailbox {
    async fn get_tip(&self) -> u64 {
        let (responder, receiver) = oneshot::channel();
        if !self.sender.enqueue(Message::GetTip(responder)).accepted() {
            warn!("schnorr precommit actor closed; tip query unanswered");
            return 0;
        }
        receiver.await.unwrap_or(0)
    }

    async fn get(&self, height: u64) -> Option<Digest> {
        let (responder, receiver) = oneshot::channel();
        if !self
            .sender
            .enqueue(Message::Get(height, responder))
            .accepted()
        {
            warn!(
                height,
                "schnorr precommit actor closed; digest query unanswered"
            );
            return None;
        }
        receiver.await.unwrap_or(None)
    }
}

/// An open completion round for one height.
struct OpenCompletion {
    collector: CompletionCollector,
    digest: Digest,
    non_signers: Vec<alloy::primitives::Address>,
}

/// The actor. See the module docs for the input/state model.
pub struct SchnorrPrecommitRouter<R, S>
where
    R: Receiver<PublicKey = PublicKey>,
    S: Sender<PublicKey = PublicKey>,
{
    context: tokio::Context,
    mailbox: mailbox::Receiver<Message>,
    /// Verifier scheme (`me() == None`), same instance the engine runs with.
    scheme: SchnorrScheme,
    store: Arc<BatchStore>,
    operator_addresses: Vec<alloy::primitives::Address>,
    /// Certified digest per height: dedupe (replay-idempotency) + query source.
    certified: BTreeMap<u64, Digest>,
    /// Next height the engine needs a certificate for.
    tip: u64,
    /// Open completion rounds by height.
    completions: BTreeMap<u64, OpenCompletion>,
    /// Resolved observations bound for the schnorr submitter.
    observations: SchnorrCertifiedSender,
    receiver: R,
    sender: S,
    pull_interval: Duration,
}

impl<R, S> SchnorrPrecommitRouter<R, S>
where
    R: Receiver<PublicKey = PublicKey>,
    S: Sender<PublicKey = PublicKey>,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: tokio::Context,
        scheme: SchnorrScheme,
        store: Arc<BatchStore>,
        operator_addresses: Vec<alloy::primitives::Address>,
        observations: SchnorrCertifiedSender,
        receiver: R,
        sender: S,
        pull_interval: Duration,
    ) -> (Self, SchnorrPrecommitMailbox) {
        let (sender_handle, mailbox) =
            mailbox::new(context.child("mailbox"), NZUsize!(MAILBOX_CAPACITY));
        (
            Self {
                context,
                mailbox,
                scheme,
                store,
                operator_addresses,
                certified: BTreeMap::new(),
                tip: 0,
                completions: BTreeMap::new(),
                observations,
                receiver,
                sender,
                pull_interval,
            },
            SchnorrPrecommitMailbox {
                sender: sender_handle,
            },
        )
    }

    /// Runs until the engine (mailbox) or the p2p channel is gone.
    pub async fn run(mut self) {
        let mut pull = ::tokio::time::interval(self.pull_interval);
        pull.set_missed_tick_behavior(::tokio::time::MissedTickBehavior::Delay);
        loop {
            ::tokio::select! {
                message = self.mailbox.recv() => {
                    let Some(message) = message else {
                        info!("engine mailbox closed; exiting");
                        return;
                    };
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
                incoming = self.receiver.recv() => {
                    let Ok((peer, bytes)) = incoming else {
                        info!("p2p channel closed; exiting");
                        return;
                    };
                    self.handle_p2p(peer, bytes);
                }
                _ = pull.tick() => {
                    for operator in &self.operator_addresses {
                        if !self.store.has(*operator, 0) {
                            let msg = PrecommitMsg::BatchRequest {
                                operator: *operator,
                                batch_index: 0,
                                chunk_offset: 0,
                            };
                            let _ = self.sender.send(Recipients::All, msg.encode(), false);
                        }
                    }
                }
            }
        }
    }

    fn handle_certified(&mut self, certificate: Certificate<SchnorrScheme, Digest>) {
        let height = certificate.item.height.get();
        let digest = certificate.item.digest;

        // Replay-idempotency: first observation per height wins (the engine replays
        // its journal on restart).
        if let Some(existing) = self.certified.get(&height) {
            if *existing != digest {
                tracing::error!(
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

        // Defensive re-verification against the verifier scheme (mirrors CertReporter).
        if !certificate.verify(&mut self.context, &self.scheme, &Sequential) {
            warn!(height, "engine-reported certificate failed re-verification");
            return;
        }

        self.certified.insert(height, digest);
        self.advance_tip(height + 1);

        // Skip heights resolve without any on-chain interest — the submitter only
        // needs the observation to resolve the assignment (old coordinator shape).
        if digest == skip_digest(height) {
            debug!(height, "height certified as skipped");
            self.emit(SchnorrCertified {
                height,
                digest,
                signature: None,
                non_signers: Vec::new(),
            });
            return;
        }

        let signers = certificate.certificate.signers().clone();
        let non_signers = self.scheme.non_signer_identity_addresses(&signers);
        if let Some(signature) = certificate.certificate.aggregate_signature() {
            // Full-S₀ participation: final signature straight from the engine.
            info!(height, signers = signers.count(), "aggregate certified");
            self.emit(SchnorrCertified {
                height,
                digest,
                signature: Some(signature),
                non_signers,
            });
            return;
        }

        // Attested: open the completion round for exactly the certified set. The
        // nodes see the same certificate locally and send their partials unprompted.
        let digest_bytes: [u8; 32] = match digest.as_ref().try_into() {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        match CompletionCollector::new(
            self.scheme.clone(),
            height,
            COMPLETION_ATTEMPT,
            digest_bytes,
            &signers,
        ) {
            Some(collector) => {
                info!(
                    height,
                    signers = signers.count(),
                    "attested certificate; awaiting completion partials"
                );
                self.completions.insert(
                    height,
                    OpenCompletion {
                        collector,
                        digest,
                        non_signers,
                    },
                );
            }
            // Missing coverage at the completion slot (exhaustion / divergent
            // directory): the height falls to the sequencer's deadline/skip path.
            None => warn!(height, "cannot build completion context; height will skip"),
        }
    }

    fn handle_p2p(&mut self, peer: PublicKey, bytes: impl bytes::Buf) {
        let msg = match PrecommitMsg::decode(bytes) {
            Ok(msg) => msg,
            Err(error) => {
                warn!(?peer, ?error, "undecodable precommit message");
                return;
            }
        };
        match msg {
            PrecommitMsg::BatchAnnounce {
                pubkey,
                batch_index,
                start_slot,
                total,
                signature,
                chunk_offset,
                nonces,
            } => {
                match self.store.ingest(
                    peer.address(),
                    pubkey,
                    batch_index,
                    start_slot,
                    total,
                    signature,
                    chunk_offset,
                    nonces,
                ) {
                    Ingest::Completed => {
                        debug!(operator = ?peer.address(), batch_index, "nonce batch verified")
                    }
                    Ingest::Pending => {}
                    Ingest::Rejected(reason) => {
                        warn!(operator = ?peer.address(), batch_index, ?reason, "announce rejected")
                    }
                }
            }
            PrecommitMsg::BatchRequest {
                operator,
                batch_index,
                chunk_offset,
            } => {
                let mut offset = chunk_offset;
                while let Some(reply) = self.store.serve(operator, batch_index, offset) {
                    let _ = self
                        .sender
                        .send(Recipients::One(peer.clone()), reply.encode(), false);
                    offset += MAX_CHUNK_NONCES as u64;
                }
            }
            PrecommitMsg::CompletionPartial {
                height,
                attempt,
                r_addr,
                partial,
            } => {
                if attempt != COMPLETION_ATTEMPT {
                    return;
                }
                let Some(open) = self.completions.get_mut(&height) else {
                    trace!(height, "completion partial for no open round");
                    return;
                };
                match open.collector.add(peer.address(), r_addr, partial) {
                    Added::Complete(signature) => {
                        let open = self
                            .completions
                            .remove(&height)
                            .expect("open completion just accessed");
                        info!(height, "completion round assembled the final signature");
                        self.emit(SchnorrCertified {
                            height,
                            digest: open.digest,
                            signature: Some(signature),
                            non_signers: open.non_signers,
                        });
                    }
                    Added::Accepted | Added::Duplicate => {}
                    Added::Rejected => {
                        debug!(height, ?peer, "completion partial rejected")
                    }
                }
            }
        }
    }

    fn emit(&self, observation: SchnorrCertified) {
        if self.observations.send(observation).is_err() {
            warn!("schnorr submitter observation channel closed");
        }
    }

    fn advance_tip(&mut self, tip: u64) {
        if tip <= self.tip {
            return;
        }
        self.tip = tip;
        let floor = tip.saturating_sub(PRUNE_SLACK);
        if floor > 0 {
            self.certified = self.certified.split_off(&floor);
            self.completions = self.completions.split_off(&floor);
        }
    }
}

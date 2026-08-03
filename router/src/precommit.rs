//! Router-side actor for the pre-committed-nonce mode
//! (`SIGNATURE_SCHEME=schnorr-precommit`): the channel-2 batch-gossip peer and the
//! completion-round collector, translating engine certificates into the
//! [`SchnorrCertified`] observations the existing Schnorr submitter consumes (see
//! `docs/schnorr-nonce-registry.md` §7.3).
//!
//! The reporter half — mailbox, height dedupe across journal replays, defensive
//! certificate re-verification, tip tracking and the sequencer's `CertIndex` — is
//! upstream's scheme-generic [`CertReporter`], the same actor the BLS arm runs. This
//! actor consumes its verified output, so the only state here is what the completion
//! round needs.
//!
//! [`CertReporter`]: commonware_avs_router::reporter::CertReporter
//!
//! Inputs and what they drive:
//! * **Verified certificates** (from the reporter's channel): `Aggregate` ones — and
//!   skip heights — resolve immediately into [`SchnorrCertified`]; `Attested` ones open
//!   a [`CompletionCollector`] for exactly the certified signer set.
//! * **Channel-2 p2p**: batch announces/requests feed and serve the [`BatchStore`] (the
//!   router needs every operator's committed nonces to verify partials);
//!   [`PrecommitMsg::CompletionPartial`]s feed the open collector for their height, and
//!   a completed collector emits the final observation.
//! * **A pull tick** requests batches the store still lacks.

use commonware_avs_core::bn254::PublicKey;
use commonware_avs_core::consensus::PRUNE_SLACK;
use commonware_avs_core::wire::skip_digest;
use commonware_avs_router::reporter::{CertifiedHeight, CertifiedReceiver};
use commonware_codec::{DecodeExt, Encode};
use commonware_cryptography::sha256::Digest;
use commonware_p2p::{Receiver, Recipients, Sender};
use gas_killer_common::schnorr::batches::{Added, BatchStore, CompletionCollector, Ingest};
use gas_killer_common::schnorr::scheme::{SchnorrCertificate, SchnorrScheme};
use gas_killer_common::schnorr::wire::{MAX_CHUNK_NONCES, PrecommitMsg};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, trace, warn};

use crate::schnorr_coordinator::{SchnorrCertified, SchnorrCertifiedSender};

/// The completion attempt this router collects (must match the node actor).
const COMPLETION_ATTEMPT: u32 = 1;

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
    /// Verifier scheme (`me() == None`), same instance the engine runs with.
    scheme: SchnorrScheme,
    store: Arc<BatchStore>,
    operator_addresses: Vec<alloy::primitives::Address>,
    /// Application namespace, for recognizing skip digests.
    namespace: Vec<u8>,
    /// Verified certificates from the engine's reporter.
    certified: CertifiedReceiver<SchnorrScheme>,
    /// Highest certified height seen, for pruning open rounds.
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
        scheme: SchnorrScheme,
        store: Arc<BatchStore>,
        operator_addresses: Vec<alloy::primitives::Address>,
        namespace: Vec<u8>,
        certified: CertifiedReceiver<SchnorrScheme>,
        observations: SchnorrCertifiedSender,
        receiver: R,
        sender: S,
        pull_interval: Duration,
    ) -> Self {
        Self {
            scheme,
            store,
            operator_addresses,
            namespace,
            certified,
            tip: 0,
            completions: BTreeMap::new(),
            observations,
            receiver,
            sender,
            pull_interval,
        }
    }

    /// Runs until the reporter's channel or the p2p channel is gone.
    pub async fn run(mut self) {
        let mut pull = ::tokio::time::interval(self.pull_interval);
        pull.set_missed_tick_behavior(::tokio::time::MissedTickBehavior::Delay);
        loop {
            ::tokio::select! {
                certified = self.certified.recv() => {
                    let Some(certified) = certified else {
                        info!("certificate channel closed; exiting");
                        return;
                    };
                    self.handle_certified(certified);
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

    /// Turns one verified certificate into an observation, or opens a completion round.
    ///
    /// Dedupe and re-verification already happened in the reporter, so a certificate
    /// arriving here is verified and first-of-its-height.
    fn handle_certified(&mut self, certified: CertifiedHeight<SchnorrScheme>) {
        let CertifiedHeight {
            height,
            digest,
            certificate,
        } = certified;
        self.advance_tip(height + 1);

        // Skip heights resolve without any on-chain interest — the submitter only
        // needs the observation to resolve the assignment.
        if digest == skip_digest(&self.namespace, height) {
            debug!(height, "height certified as skipped");
            self.emit(SchnorrCertified {
                height,
                digest,
                signature: None,
                non_signers: Vec::new(),
            });
            return;
        }

        let signers = certificate.signers().clone();
        let non_signers = self.scheme.non_signer_identity_addresses(&signers);
        match &certificate {
            // Full-S₀ participation: final signature straight from the engine.
            SchnorrCertificate::Aggregate { .. } => {
                let Some(signature) = certificate.aggregate_signature() else {
                    warn!(height, "aggregate certificate carries no signature (BUG)");
                    return;
                };
                info!(height, signers = signers.count(), "aggregate certified");
                self.emit(SchnorrCertified {
                    height,
                    digest,
                    signature: Some(signature),
                    non_signers,
                });
            }
            // Attested: open the completion round for exactly the certified set. The
            // nodes see the same certificate locally and send their partials unprompted.
            SchnorrCertificate::Attested { .. } => {
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
                // The batch authenticates itself (registration signature over the
                // recomputed root), so `peer` is only the relaying stream's key.
                let operator = pubkey.eth_address();
                match self.store.ingest(
                    &peer,
                    pubkey,
                    batch_index,
                    start_slot,
                    total,
                    signature,
                    chunk_offset,
                    nonces,
                ) {
                    Ingest::Completed => {
                        debug!(?operator, batch_index, "nonce batch verified")
                    }
                    Ingest::Pending => {}
                    Ingest::Rejected(reason) => {
                        warn!(?operator, ?peer, batch_index, ?reason, "announce rejected")
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
                match open.collector.add(&peer, r_addr, partial) {
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

    /// Raises the highest observed height and drops completion rounds far below it —
    /// an unfinished round that old has already lost its height to the deadline path.
    fn advance_tip(&mut self, tip: u64) {
        if tip <= self.tip {
            return;
        }
        self.tip = tip;
        let floor = tip.saturating_sub(PRUNE_SLACK);
        if floor > 0 {
            self.completions = self.completions.split_off(&floor);
        }
    }
}

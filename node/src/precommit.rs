//! Precommit actor: the node's channel-2 side of the pre-committed-nonce mode
//! (`SIGNATURE_SCHEME=schnorr-precommit`; see `docs/schnorr-nonce-registry.md` §6.2/§7.3).
//!
//! Signing itself rides the aggregation engine (the node's partial IS its ack —
//! see `SchnorrScheme`); this actor handles everything around it:
//!
//! 1. **Batch distribution**: announce our own registered nonce batches at startup,
//!    ingest peers' announces into the shared [`BatchStore`] (self-authenticating:
//!    sender identity + registration signature over the recomputed root), serve
//!    [`PrecommitMsg::BatchRequest`]s, and periodically pull batches we still lack —
//!    which heals the startup race where an announce beats the p2p mesh.
//! 2. **Completion rounds**: when the engine certifies an `Attested` certificate
//!    (forwarded by the reporter tap), produce our completion partial for the
//!    certified set via [`SchnorrScheme::sign_completion`] — journal-gated
//!    (invariant N1) and idempotent, so replayed taps after a restart are safe —
//!    and send it to the router.

use commonware_avs_core::bn254::PublicKey;
use commonware_avs_node::reporter::{CertificateObservation, ObservationReceiver};
use commonware_codec::{DecodeExt, Encode};
use commonware_p2p::{Receiver, Recipients, Sender};
use gas_killer_common::schnorr::batches::{BatchStore, Ingest};
use gas_killer_common::schnorr::scheme::{SchnorrCertificate, SchnorrScheme};
use gas_killer_common::schnorr::wire::{MAX_CHUNK_NONCES, PrecommitMsg};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// The completion attempt this actor answers. Attempt 0 is the engine ack; shrinking
/// retries beyond the first completion attempt are future hardening (the deadline/skip
/// path covers them).
const COMPLETION_ATTEMPT: u32 = 1;

/// Runs the actor until the p2p channel or the reporter tap closes.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run<R, S>(
    scheme: SchnorrScheme,
    store: Arc<BatchStore>,
    own_announces: Vec<PrecommitMsg>,
    operator_addresses: Vec<alloy::primitives::Address>,
    router: PublicKey,
    pull_interval: Duration,
    mut certified: ObservationReceiver<SchnorrScheme>,
    mut receiver: R,
    mut sender: S,
) where
    R: Receiver<PublicKey = PublicKey>,
    S: Sender<PublicKey = PublicKey>,
{
    // Announce our own batches once; late joiners pull via BatchRequest.
    for msg in &own_announces {
        let _ = sender.send(Recipients::All, msg.encode(), false);
    }
    info!(
        chunks = own_announces.len(),
        "announced own nonce batches to peers"
    );

    let mut pull = tokio::time::interval(pull_interval);
    pull.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            observation = certified.recv() => {
                let Some(CertificateObservation { height, digest, certificate }) = observation else {
                    info!("reporter tap closed; exiting");
                    return;
                };
                // Only `Attested` needs finishing: `Aggregate` is already the final
                // signature, and skip heights carry no signing session at all.
                let SchnorrCertificate::Attested { signers, .. } = &certificate else {
                    continue;
                };
                let digest: [u8; 32] = match digest.as_ref().try_into() {
                    Ok(digest) => digest,
                    Err(_) => continue,
                };
                match scheme.sign_completion(height, COMPLETION_ATTEMPT, &digest, signers) {
                    Some((r_addr, partial)) => {
                        debug!(height, "sending completion partial");
                        let msg = PrecommitMsg::CompletionPartial {
                            height,
                            attempt: COMPLETION_ATTEMPT,
                            r_addr,
                            partial,
                        };
                        let _ = sender.send(Recipients::One(router.clone()), msg.encode(), true);
                    }
                    // Not a member of the certified set, missing coverage, or the
                    // journal refused (a different context already consumed the
                    // slot — the one refusal that must stay loud).
                    None => warn!(height, "did not produce a completion partial"),
                }
            }
            incoming = receiver.recv() => {
                let Ok((peer, bytes)) = incoming else {
                    info!("p2p channel closed; exiting");
                    return;
                };
                handle_message(&store, &mut sender, peer, bytes);
            }
            _ = pull.tick() => {
                // Pull the initial batch of any operator we still lack coverage
                // for (batch auto-rotation re-registration is future hardening).
                for operator in &operator_addresses {
                    if !store.has(*operator, 0) {
                        let msg = PrecommitMsg::BatchRequest {
                            operator: *operator,
                            batch_index: 0,
                            chunk_offset: 0,
                        };
                        let _ = sender.send(Recipients::All, msg.encode(), false);
                    }
                }
            }
        }
    }
}

/// Handles one inbound channel-2 message (shared shape with the router's actor).
pub(crate) fn handle_message<S>(
    store: &Arc<BatchStore>,
    sender: &mut S,
    peer: PublicKey,
    bytes: impl bytes::Buf,
) where
    S: Sender<PublicKey = PublicKey>,
{
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
            // recomputed root), so `peer` is only the relaying stream's key — the
            // operator identity comes from the announced pubkey.
            let operator = pubkey.eth_address();
            let outcome = store.ingest(
                &peer,
                pubkey,
                batch_index,
                start_slot,
                total,
                signature,
                chunk_offset,
                nonces,
            );
            match outcome {
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
            // Serve every chunk from the requested offset (requesters do not know
            // the batch size up front).
            let mut offset = chunk_offset;
            while let Some(reply) = store.serve(operator, batch_index, offset) {
                let _ = sender.send(Recipients::One(peer.clone()), reply.encode(), false);
                offset += MAX_CHUNK_NONCES as u64;
            }
        }
        // Completion partials are addressed to the router; nodes ignore them.
        PrecommitMsg::CompletionPartial { .. } => {}
    }
}

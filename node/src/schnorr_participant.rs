//! Schnorr participant actor: the node's side of the two-round MuSig2 aggregate
//! signing protocol (`SIGNATURE_SCHEME=schnorr` mode, p2p channel 2).
//!
//! Replaces the aggregation engine + reporter of BLS mode. Task discovery is
//! unchanged — the router still announces tasks on channel 1 and the TaskBook
//! resolves heights — but instead of unilaterally signing TipAcks, the node
//! answers the router coordinator's session messages:
//!
//! 1. `NonceRequest{h, a}`: generate (or re-send) a fresh nonce pair for the
//!    session. Nonces are message-independent, so this needs no task validation.
//! 2. `SignRequest{h, a, …}`: derive the digest for `h` LOCALLY (TaskBook +
//!    EVMSketch, same [`DigestResolver`] the BLS automaton mirrors), refuse unless
//!    it equals the request's message, authenticate the signer set (every point
//!    must map to a known operator address — the identity the on-chain registry
//!    binds with a proof of possession), recompute `X_agg`, then produce the
//!    partial signature. `partial_sign` re-derives the nonce coefficient and
//!    challenge itself and aborts on an inconsistent coordinator `R`.
//!
//! # Nonce safety (the invariant everything here serves)
//!
//! A `SecNonce` signs AT MOST ONE context, ever:
//! - sessions live only in memory — a restart forgets secret nonces, so a rebooted
//!   node simply refuses in-flight sessions (it becomes a non-signer and the
//!   coordinator retries with fresh nonces);
//! - duplicate `NonceRequest`s re-send the SAME public nonce (idempotent — the
//!   secret is still unused);
//! - signing consumes the secret nonce by value; the result is cached, and a
//!   duplicate `SignRequest` with the SAME context fingerprint re-sends the cached
//!   partial, while a DIFFERENT context for an already-signed session is refused
//!   loudly (that shape is exactly a nonce-reuse attack).

use alloy::primitives::Address;
use commonware_avs_core::bn254::PublicKey;
use commonware_codec::{DecodeExt, Encode};
use commonware_p2p::{Receiver, Recipients, Sender};
use commonware_runtime::{Spawner, Supervisor, tokio};
use gas_killer_common::schnorr::musig::{Participant, PubNonce, SigningContext};
use gas_killer_common::schnorr::wire::{SchnorrMsg, SignRequest, partial_from_bytes};
use gas_killer_common::schnorr::{self, PrivateKey};
use rand::TryRngCore;
use rand::rngs::OsRng;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

use crate::digest::DigestResolver;

/// Sessions for heights this far below the highest height seen are pruned. Far
/// smaller than the directive log's `PRUNE_SLACK`: a session is useless the moment
/// the coordinator moves on (it will never ask for that `(height, attempt)` again).
const SESSION_SLACK: u64 = 64;

/// Per-session signing state. See the module docs for the nonce-safety rules each
/// transition enforces.
#[allow(clippy::large_enum_variant)] // few sessions live at once; boxing buys nothing
enum Session {
    /// Nonce issued, secret retained until the matching `SignRequest`.
    Issued {
        sec: gas_killer_common::schnorr::musig::SecNonce,
        pubn: PubNonce,
    },
    /// A spawned sign task owns the secret nonce and is resolving the digest.
    Signing,
    /// Signed under `fingerprint`; the partial is cached for idempotent re-sends.
    Signed {
        fingerprint: [u8; 32],
        partial_bytes: [u8; 32],
    },
    /// Refused (digest mismatch / bad signer set / inconsistent context). Terminal:
    /// the secret nonce is gone, and this session will never sign.
    Refused,
}

/// Shared state between the actor loop and its spawned sign tasks.
struct Shared {
    sessions: Mutex<HashMap<(u64, u32), Session>>,
    participant: Participant,
    own_pubkey: schnorr::PublicKey,
    /// The operator identity addresses (same set the p2p layer tracks); signer
    /// points in a `SignRequest` must map into this set.
    operator_addresses: HashSet<Address>,
    resolver: DigestResolver,
}

/// Runs the participant actor until the channel closes. Spawns one child task per
/// `SignRequest` (digest resolution can block up to the validation retry budget;
/// the actor loop must stay responsive to later sessions meanwhile).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run<R, S>(
    context: tokio::Context,
    private_key: PrivateKey,
    router: PublicKey,
    operator_addresses: HashSet<Address>,
    resolver: DigestResolver,
    engine_tip: Arc<AtomicU64>,
    mut receiver: R,
    sender: S,
) where
    R: Receiver<PublicKey = PublicKey>,
    S: Sender<PublicKey = PublicKey> + Clone + Send + Sync + 'static,
{
    let own_pubkey = private_key.public_key();
    let shared = Arc::new(Shared {
        sessions: Mutex::new(HashMap::new()),
        participant: Participant::new(private_key),
        own_pubkey,
        operator_addresses,
        resolver,
    });
    let mut max_height = 0u64;

    info!(
        operator = %shared.own_pubkey.eth_address(),
        "schnorr participant running"
    );

    loop {
        let (peer, bytes) = match receiver.recv().await {
            Ok(msg) => msg,
            Err(error) => {
                info!(?error, "schnorr channel closed; exiting");
                return;
            }
        };
        if peer != router {
            warn!(peer = %peer, "schnorr message from non-router peer; ignored");
            continue;
        }
        let msg = match SchnorrMsg::decode(bytes) {
            Ok(msg) => msg,
            Err(error) => {
                warn!(%error, "malformed schnorr message; ignored");
                continue;
            }
        };

        // Track the highest height the coordinator is working on: it feeds the
        // channel-1 TipReport recovery (`engine_tip`) and bounds session storage.
        let height = msg.height();
        if height > max_height {
            max_height = height;
            engine_tip.store(max_height, Ordering::Relaxed);
            prune_sessions(&shared, max_height);
        }

        match msg {
            SchnorrMsg::NonceRequest { height, attempt } => {
                handle_nonce_request(&shared, &sender, &router, height, attempt);
            }
            SchnorrMsg::SignRequest(request) => {
                handle_sign_request(&context, &shared, &sender, &router, request);
            }
            SchnorrMsg::NonceCommit { .. } | SchnorrMsg::PartialSig { .. } => {
                // Node → router messages; the router never sends these.
                debug!(height, "unexpected node-bound schnorr message; ignored");
            }
        }
    }
}

/// Issues (or idempotently re-sends) the public nonce for a session.
fn handle_nonce_request<S>(
    shared: &Arc<Shared>,
    sender: &S,
    router: &PublicKey,
    height: u64,
    attempt: u32,
) where
    S: Sender<PublicKey = PublicKey> + Clone,
{
    let pubn = {
        let mut sessions = shared.sessions.lock().expect("sessions lock");
        match sessions.get(&(height, attempt)) {
            None => {
                // OS entropy: nonce secrecy is what the whole protocol's key
                // safety rests on, so nothing weaker is acceptable here.
                let (sec, pubn) = shared.participant.new_nonce(&mut |b: &mut [u8]| {
                    OsRng.try_fill_bytes(b).expect("OS entropy unavailable");
                });
                sessions.insert((height, attempt), Session::Issued { sec, pubn });
                debug!(height, attempt, "issued nonce");
                pubn
            }
            // Duplicate request (lost reply / router rebroadcast): re-send the SAME
            // public nonce — the secret is still unused, so this stays single-use.
            Some(Session::Issued { pubn, .. }) => *pubn,
            // Already signing/signed/refused: never issue a second nonce for the
            // same session (the coordinator must escalate to a new attempt).
            Some(_) => {
                debug!(
                    height,
                    attempt, "nonce request for consumed session; ignored"
                );
                return;
            }
        }
    };
    let reply = SchnorrMsg::NonceCommit {
        height,
        attempt,
        pubkey: shared.own_pubkey,
        nonce: pubn,
    };
    let mut sender = sender.clone();
    let _ = sender.send(Recipients::One(router.clone()), reply.encode(), true);
}

/// Validates a `SignRequest` and spawns the sign task that resolves the digest and
/// produces the partial signature.
fn handle_sign_request<S>(
    context: &tokio::Context,
    shared: &Arc<Shared>,
    sender: &S,
    router: &PublicKey,
    request: SignRequest,
) where
    S: Sender<PublicKey = PublicKey> + Clone + Send + Sync + 'static,
{
    let (height, attempt) = (request.height, request.attempt);
    let fingerprint = request.fingerprint();

    // Take the secret nonce (or serve the idempotent-resend / refuse cases).
    let sec = {
        let mut sessions = shared.sessions.lock().expect("sessions lock");
        match sessions.get(&(height, attempt)) {
            Some(Session::Issued { .. }) => {
                let Some(Session::Issued { sec, .. }) =
                    sessions.insert((height, attempt), Session::Signing)
                else {
                    unreachable!("entry checked above");
                };
                sec
            }
            Some(Session::Signed {
                fingerprint: signed_fp,
                partial_bytes,
            }) => {
                if *signed_fp == fingerprint {
                    // Lost reply: re-send the cached partial for the SAME context.
                    let Some(partial) = partial_from_bytes(partial_bytes) else {
                        return;
                    };
                    let reply = SchnorrMsg::PartialSig {
                        height,
                        attempt,
                        partial,
                    };
                    let mut sender = sender.clone();
                    let _ = sender.send(Recipients::One(router.clone()), reply.encode(), true);
                } else {
                    // Same session, different context, after we already signed:
                    // signing again would reuse the nonce and leak the key.
                    warn!(
                        height,
                        attempt,
                        "sign request with a DIFFERENT context for an already-signed session; refused (possible nonce-reuse attempt)"
                    );
                }
                return;
            }
            Some(Session::Signing) => {
                debug!(height, attempt, "sign already in progress; ignored");
                return;
            }
            Some(Session::Refused) | None => {
                debug!(
                    height,
                    attempt, "sign request without an issuable nonce; ignored"
                );
                return;
            }
        }
    };

    let shared = Arc::clone(shared);
    let sender = sender.clone();
    let router = router.clone();
    drop(context.child("sign").spawn(move |_| async move {
        let outcome = sign(&shared, sec, &request).await;
        let mut sessions = shared.sessions.lock().expect("sessions lock");
        match outcome {
            Some(partial_bytes) => {
                sessions.insert(
                    (height, attempt),
                    Session::Signed {
                        fingerprint,
                        partial_bytes,
                    },
                );
                drop(sessions);
                let Some(partial) = partial_from_bytes(&partial_bytes) else {
                    return;
                };
                let reply = SchnorrMsg::PartialSig {
                    height,
                    attempt,
                    partial,
                };
                let mut sender = sender;
                let _ = sender.send(Recipients::One(router), reply.encode(), true);
                debug!(height, attempt, "partial signature sent");
            }
            None => {
                sessions.insert((height, attempt), Session::Refused);
            }
        }
    }));
}

/// The signing decision: authenticate the signer set, derive the digest locally,
/// and produce the partial. Returns `None` to refuse (reasons logged inside).
async fn sign(
    shared: &Shared,
    sec: gas_killer_common::schnorr::musig::SecNonce,
    request: &SignRequest,
) -> Option<[u8; 32]> {
    let (height, attempt) = (request.height, request.attempt);

    // Every signer point must map to a known operator identity address — the same
    // keccak256(x ‖ y) identity the registry requires a proof of possession for —
    // and our own key must be in the subset (otherwise our partial is for a key
    // sum we are not part of).
    let mut includes_self = false;
    for signer in &request.signers {
        if !shared.operator_addresses.contains(&signer.eth_address()) {
            warn!(
                height,
                attempt,
                signer = %signer.eth_address(),
                "sign request includes a non-operator key; refused"
            );
            return None;
        }
        if *signer == shared.own_pubkey {
            includes_self = true;
        }
    }
    if !includes_self {
        warn!(height, attempt, "sign request excludes our key; refused");
        return None;
    }
    let x_agg = schnorr::PublicKey::aggregate(request.signers.iter())?;

    // Never sign a digest we did not derive ourselves. The resolver returns the
    // validated task digest or the skip digest; the coordinator only runs signing
    // sessions for real task digests, so any mismatch (including "we think this
    // height should skip") is a refusal.
    let local_digest = shared.resolver.resolve(height).await?;
    if local_digest.as_ref() != request.message.as_slice() {
        warn!(
            height,
            attempt,
            ours = ?local_digest,
            theirs = ?alloy::primitives::hex::encode(request.message),
            "coordinator message does not match locally derived digest; refused"
        );
        return None;
    }

    let ctx =
        SigningContext::from_wire(x_agg, &request.agg_nonces, request.r_addr, request.message);
    let Some(partial) = shared.participant.sign(sec, &ctx) else {
        warn!(
            height,
            attempt, "inconsistent signing context (coordinator R mismatch); refused"
        );
        return None;
    };
    Some(gas_killer_common::schnorr::wire::partial_to_bytes(&partial))
}

/// Drops sessions too far below the coordinator's working height to ever complete.
fn prune_sessions(shared: &Shared, max_height: u64) {
    let floor = max_height.saturating_sub(SESSION_SLACK);
    if floor == 0 {
        return;
    }
    shared
        .sessions
        .lock()
        .expect("sessions lock")
        .retain(|(h, _), _| *h >= floor);
}

//! Schnorr participant actor: the node's side of the two-round MuSig2 aggregate
//! signing protocol (p2p channel 2).
//!
//! The node answers the router coordinator's session messages:
//!
//! 1. `CommitRequest{h, a, task}`: derive the task's digest LOCALLY (EVMSketch, via
//!    [`DigestResolver`]), then commit a fresh nonce pair together with that digest. The
//!    coordinator signs the digest a quorum of commits agrees on, so the node's trace is what
//!    the round waits for, not the router's. A task whose digest cannot be derived gets no
//!    commit.
//! 2. `SignRequest{h, a, …}`: refuse unless the message equals the digest this session
//!    committed to, authenticate the signer set (every point must map to a known operator
//!    address — the identity the on-chain registry binds with a proof of possession),
//!    recompute `X_agg`, then produce the partial signature. `partial_sign` re-derives the
//!    nonce coefficient and challenge itself and aborts on an inconsistent coordinator `R`.
//!
//! # Nonce safety (the invariant everything here serves)
//!
//! A `SecNonce` signs AT MOST ONE context, ever:
//! - sessions live only in memory — a restart forgets secret nonces, so a rebooted
//!   node simply refuses in-flight sessions (it becomes a non-signer and the
//!   coordinator retries with fresh nonces);
//! - duplicate `CommitRequest`s re-send the SAME public nonce (idempotent — the
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
use gas_killer_common::task_data::GasKillerTaskData;
use rand::TryRngCore;
use rand::rngs::OsRng;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

use crate::digest::DigestResolver;

/// Sessions for heights this far below the highest height seen are pruned: a session is
/// useless the moment the coordinator moves on (it will never ask for that
/// `(height, attempt)` again). It must stay above the number of sessions the router runs at
/// once, or a live session is pruned under a newer one.
const SESSION_SLACK: u64 = 64;

/// Per-session signing state. See the module docs for the nonce-safety rules each
/// transition enforces.
#[allow(clippy::large_enum_variant)] // few sessions live at once; boxing buys nothing
enum Session {
    /// A spawned task is resolving the digest; no nonce exists yet.
    Resolving,
    /// Nonce issued for `digest`, secret retained until the matching `SignRequest`.
    Issued {
        sec: gas_killer_common::schnorr::musig::SecNonce,
        pubn: PubNonce,
        digest: [u8; 32],
    },
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
/// `CommitRequest`: digest resolution can block up to the validation retry budget, and
/// the actor loop must stay responsive to later sessions meanwhile.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run<R, S>(
    context: tokio::Context,
    private_key: PrivateKey,
    router: PublicKey,
    operator_addresses: HashSet<Address>,
    resolver: DigestResolver,
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

        // Track the highest height the coordinator is working on: it bounds session storage.
        let height = msg.height();
        if height > max_height {
            max_height = height;
            prune_sessions(&shared, max_height);
        }

        match msg {
            SchnorrMsg::CommitRequest {
                height,
                attempt,
                task,
            } => {
                handle_commit_request(&context, &shared, &sender, &router, height, attempt, task);
            }
            SchnorrMsg::SignRequest(request) => {
                handle_sign_request(&shared, &sender, &router, request);
            }
            SchnorrMsg::Commit { .. } | SchnorrMsg::PartialSig { .. } => {
                // Node → router messages; the router never sends these.
                debug!(height, "unexpected node-bound schnorr message; ignored");
            }
        }
    }
}

/// Resolves the session's digest and commits a fresh nonce for it, or re-sends the
/// commit a duplicate request is asking for.
fn handle_commit_request<S>(
    context: &tokio::Context,
    shared: &Arc<Shared>,
    sender: &S,
    router: &PublicKey,
    height: u64,
    attempt: u32,
    task: GasKillerTaskData,
) where
    S: Sender<PublicKey = PublicKey> + Clone + Send + Sync + 'static,
{
    {
        let mut sessions = shared.sessions.lock().expect("sessions lock");
        match sessions.get(&(height, attempt)) {
            None => {
                sessions.insert((height, attempt), Session::Resolving);
            }
            // Duplicate request (lost reply / router rebroadcast): re-send the SAME
            // public nonce — the secret is still unused, so this stays single-use.
            Some(Session::Issued { pubn, digest, .. }) => {
                send_commit(shared, sender, router, height, attempt, *pubn, *digest);
                return;
            }
            // Resolving answers once the digest is known; a consumed session never issues
            // a second nonce (the coordinator must escalate to a new attempt).
            Some(_) => {
                debug!(
                    height,
                    attempt, "commit request for a session in progress or consumed; ignored"
                );
                return;
            }
        }
    }

    let shared = Arc::clone(shared);
    let sender = sender.clone();
    let router = router.clone();
    drop(context.child("commit").spawn(move |_| async move {
        let Some(digest) = shared.resolver.resolve(height, &task).await else {
            shared
                .sessions
                .lock()
                .expect("sessions lock")
                .insert((height, attempt), Session::Refused);
            return;
        };
        let digest: [u8; 32] = digest
            .as_ref()
            .try_into()
            .expect("sha256 digest is 32 bytes");

        let pubn = {
            let mut sessions = shared.sessions.lock().expect("sessions lock");
            // Pruned while resolving: the coordinator has moved past this session.
            if !matches!(sessions.get(&(height, attempt)), Some(Session::Resolving)) {
                return;
            }
            // OS entropy: nonce secrecy is what the whole protocol's key
            // safety rests on, so nothing weaker is acceptable here.
            let (sec, pubn) = shared.participant.new_nonce(&mut |b: &mut [u8]| {
                OsRng.try_fill_bytes(b).expect("OS entropy unavailable");
            });
            sessions.insert((height, attempt), Session::Issued { sec, pubn, digest });
            pubn
        };
        debug!(height, attempt, "issued nonce");
        send_commit(&shared, &sender, &router, height, attempt, pubn, digest);
    }));
}

fn send_commit<S>(
    shared: &Shared,
    sender: &S,
    router: &PublicKey,
    height: u64,
    attempt: u32,
    nonce: PubNonce,
    digest: [u8; 32],
) where
    S: Sender<PublicKey = PublicKey> + Clone,
{
    let reply = SchnorrMsg::Commit {
        height,
        attempt,
        pubkey: shared.own_pubkey,
        nonce,
        digest,
    };
    let mut sender = sender.clone();
    let _ = sender.send(Recipients::One(router.clone()), reply.encode(), true);
}

/// Validates a `SignRequest` and answers it with the partial signature, a re-send of the
/// cached one, or a refusal.
fn handle_sign_request<S>(shared: &Shared, sender: &S, router: &PublicKey, request: SignRequest)
where
    S: Sender<PublicKey = PublicKey> + Clone,
{
    let (height, attempt) = (request.height, request.attempt);
    let fingerprint = request.fingerprint();

    let key = (height, attempt);
    let mut sessions = shared.sessions.lock().expect("sessions lock");
    let partial_bytes = match sessions.get(&key) {
        Some(Session::Signed {
            fingerprint: signed_fp,
            partial_bytes,
        }) => {
            if *signed_fp != fingerprint {
                // Same session, different context, after we already signed:
                // signing again would reuse the nonce and leak the key.
                warn!(
                    height,
                    attempt,
                    "sign request with a DIFFERENT context for an already-signed session; refused (possible nonce-reuse attempt)"
                );
                return;
            }
            // Lost reply: re-send the cached partial for the SAME context.
            *partial_bytes
        }
        Some(Session::Issued { .. }) => {
            let Some(Session::Issued { sec, digest, .. }) = sessions.remove(&key) else {
                unreachable!("entry checked above");
            };
            // Signing consumes the secret nonce, so the session is terminal either way.
            let Some(partial_bytes) = sign(shared, sec, &digest, &request) else {
                sessions.insert(key, Session::Refused);
                return;
            };
            sessions.insert(
                key,
                Session::Signed {
                    fingerprint,
                    partial_bytes,
                },
            );
            partial_bytes
        }
        _ => {
            debug!(
                height,
                attempt, "sign request without an issuable nonce; ignored"
            );
            return;
        }
    };
    drop(sessions);

    let Some(partial) = partial_from_bytes(&partial_bytes) else {
        return;
    };
    let reply = SchnorrMsg::PartialSig {
        height,
        attempt,
        partial,
    };
    let mut sender = sender.clone();
    let _ = sender.send(Recipients::One(router.clone()), reply.encode(), true);
    debug!(height, attempt, "partial signature sent");
}

/// The signing decision: authenticate the signer set, check the message against the digest
/// this session committed to, and produce the partial. Returns `None` to refuse (reasons
/// logged inside).
fn sign(
    shared: &Shared,
    sec: gas_killer_common::schnorr::musig::SecNonce,
    digest: &[u8; 32],
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

    // Never sign a digest we did not derive ourselves: a coordinator that picked another
    // group's digest gets a refusal, not our key behind it.
    if *digest != request.message {
        warn!(
            height,
            attempt,
            ours = %alloy::primitives::hex::encode(digest),
            theirs = %alloy::primitives::hex::encode(request.message),
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

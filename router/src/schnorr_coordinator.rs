//! Schnorr coordinator actor: the router's side of the two-round MuSig2 aggregate
//! signing protocol (`SIGNATURE_SCHEME=schnorr` mode, p2p channel 2).
//!
//! Replaces the aggregation engine + RouterAutomaton + CertReporter of BLS mode.
//! The sequencer is unchanged: it still assigns one height at a time into
//! [`SharedAssignments`], polls its [`CertIndex`] for a certificate, and waits for
//! the submitter's resolution. This actor supplies both ends: it watches the
//! assignments map, drives signing sessions over channel 2, and emits
//! [`SchnorrCertified`] observations to the schnorr submitter.
//!
//! # Session flow (per assigned height)
//!
//! ```text
//! attempt = 1, 2, … (fresh nonces each — a nonce is bound to one session):
//!   NonceRequest{h,a}  → all operators
//!   collect NonceCommit until all reply or the stage timeout; verify each
//!     commit's pubkey point maps to the sender's operator address
//!   participation ≥ ceil(N·num/den)?  else next attempt
//!   build_context → SignRequest{h,a, digest, signer points, R aggregates} → subset
//!   collect PartialSig from exactly the subset; each partial is verified against
//!     the signer's own nonce commitment (bad partials are attributed and the
//!     signer is excluded from the next attempt)
//!   all partials → assemble (self-verifies) → Certified{h, digest, sig, nonSigners}
//! deadline (ROUND_TIMEOUT from first sight of the assignment) →
//!   Certified{h, skip_digest(h), no signature} — the sequencer's own deadline is
//!   broadcasting Skip{h} on channel 1 by then; nothing downstream consumes skip
//!   proofs, so no skip signing session is run.
//! ```
//!
//! # p2p identity vs signing identity
//!
//! The p2p transport key is BN254 (the network's operator identity), but a
//! Schnorr signer is a secp256k1 point whose Ethereum address is the operator's
//! registry identity. The two are bound at registration; here the coordinator
//! carries both directions of the map so it can (a) authenticate a `NonceCommit`
//! (the committed point's address must equal the sender's registered address) and
//! (b) address round-2 `SignRequest`s to the p2p keys of a subset chosen by
//! address.
//!
//! The certified log lives in memory only (no journal): after a router restart the
//! sequencer recovers its height from node TipReports exactly as in BLS mode.

use alloy_primitives::Address;
use commonware_avs_core::bn254::PublicKey;
use commonware_avs_core::consensus::PRUNE_SLACK;
use commonware_avs_core::wire::skip_digest;
use commonware_avs_router::reporter::CertIndex;
use commonware_avs_router::sequencer::{Assignment, SharedAssignments};
use commonware_codec::{DecodeExt, Encode};
use commonware_cryptography::sha256::Digest;
use commonware_p2p::{Receiver, Recipients, Sender};
use gas_killer_common::schnorr::musig::{Coordinator, PubNonce};
use gas_killer_common::schnorr::wire::{SchnorrMsg, SignRequest};
use gas_killer_common::schnorr::{self, AggregateSignature};
use gas_killer_common::task_data::GasKillerTaskData;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::{debug, info, warn};

/// How often the actor re-checks the assignments map for new work.
const ASSIGNMENT_POLL: Duration = Duration::from_millis(250);

/// An aggregate-signature observation handed to the schnorr submitter.
#[derive(Debug, Clone)]
pub struct SchnorrCertified {
    pub height: u64,
    /// The digest the quorum signed — or [`skip_digest`] when the round deadline
    /// passed without a signature (in which case `signature` is `None`).
    pub digest: Digest,
    /// The verified aggregate signature (`None` only for skips).
    pub signature: Option<AggregateSignature>,
    /// Operator identity addresses that did NOT sign, strictly ascending — the
    /// exact list `SchnorrStakeRegistry.isValidSignature` subtracts on-chain.
    pub non_signers: Vec<Address>,
}

pub type SchnorrCertifiedSender = UnboundedSender<SchnorrCertified>;
pub type SchnorrCertifiedReceiver = UnboundedReceiver<SchnorrCertified>;

pub fn schnorr_certified_channel() -> (SchnorrCertifiedSender, SchnorrCertifiedReceiver) {
    unbounded_channel()
}

/// Shared certified log answering the sequencer's [`CertIndex`] queries.
#[derive(Default)]
struct CertLog {
    certified: BTreeMap<u64, Digest>,
    tip: u64,
}

/// Cheap-to-clone handle over the coordinator's certified log.
#[derive(Clone, Default)]
pub struct SchnorrCoordinatorMailbox {
    log: Arc<RwLock<CertLog>>,
}

impl CertIndex for SchnorrCoordinatorMailbox {
    async fn get_tip(&self) -> u64 {
        self.log.read().expect("cert log lock").tip
    }

    async fn get(&self, height: u64) -> Option<Digest> {
        self.log
            .read()
            .expect("cert log lock")
            .certified
            .get(&height)
            .copied()
    }
}

impl SchnorrCoordinatorMailbox {
    /// Records a certified height and advances the tip, pruning old entries.
    fn record(&self, height: u64, digest: Digest) {
        let mut log = self.log.write().expect("cert log lock");
        log.certified.insert(height, digest);
        log.tip = log.tip.max(height + 1);
        let floor = log.tip.saturating_sub(PRUNE_SLACK);
        if floor > 0 {
            log.certified = log.certified.split_off(&floor);
        }
    }
}

/// The coordinator actor. Owns the channel-2 endpoints; single-threaded per
/// height (the sequencer only keeps one assignment in flight).
pub struct SchnorrCoordinator<S, R>
where
    S: Sender<PublicKey = PublicKey>,
    R: Receiver<PublicKey = PublicKey>,
{
    assignments: SharedAssignments<GasKillerTaskData>,
    mailbox: SchnorrCoordinatorMailbox,
    certified_out: SchnorrCertifiedSender,
    sender: S,
    receiver: R,
    /// Operator p2p keys, the round-1 `NonceRequest` recipients.
    operator_keys: Vec<PublicKey>,
    /// p2p key → operator address: authenticates an incoming commit/partial by the
    /// sender's registered identity.
    peer_to_address: HashMap<PublicKey, Address>,
    /// operator address → p2p key: addresses round-2 `SignRequest`s to a subset
    /// selected by address.
    address_to_peer: HashMap<Address, PublicKey>,
    /// All operator identity addresses (the non-signer complement is drawn from here).
    operator_addresses: HashSet<Address>,
    /// Application namespace mixed into the skip digest; kept in lockstep with the
    /// node's `APPLICATION_NAMESPACE`.
    namespace: Vec<u8>,
    /// Local participation floor `num/den` before a signing round is attempted
    /// (the authoritative stake check is the on-chain registry threshold).
    threshold: (u64, u64),
    stage_timeout: Duration,
    round_timeout: Duration,
}

impl<S, R> SchnorrCoordinator<S, R>
where
    S: Sender<PublicKey = PublicKey>,
    R: Receiver<PublicKey = PublicKey>,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        assignments: SharedAssignments<GasKillerTaskData>,
        certified_out: SchnorrCertifiedSender,
        sender: S,
        receiver: R,
        operators: Vec<(PublicKey, Address)>,
        namespace: Vec<u8>,
        threshold: (u64, u64),
        stage_timeout: Duration,
        round_timeout: Duration,
    ) -> (Self, SchnorrCoordinatorMailbox) {
        let mailbox = SchnorrCoordinatorMailbox::default();
        let operator_keys: Vec<PublicKey> = operators.iter().map(|(k, _)| k.clone()).collect();
        let peer_to_address: HashMap<PublicKey, Address> = operators.iter().cloned().collect();
        let address_to_peer: HashMap<Address, PublicKey> =
            operators.iter().map(|(k, a)| (*a, k.clone())).collect();
        let operator_addresses: HashSet<Address> = operators.iter().map(|(_, a)| *a).collect();
        (
            Self {
                assignments,
                mailbox: mailbox.clone(),
                certified_out,
                sender,
                receiver,
                operator_keys,
                peer_to_address,
                address_to_peer,
                operator_addresses,
                namespace,
                threshold,
                stage_timeout,
                round_timeout,
            },
            mailbox,
        )
    }

    /// The minimum number of signers worth running a round for:
    /// `ceil(N·num/den)`. Weights are uniform in this deployment, so the count
    /// approximates the registry's stake fraction; a too-small subset would only
    /// waste a round on an on-chain threshold revert.
    fn min_signers(&self) -> usize {
        let n = self.operator_keys.len() as u64;
        let (num, den) = self.threshold;
        (n.saturating_mul(num).div_ceil(den)).max(1) as usize
    }

    pub async fn run(mut self) {
        info!(
            operators = self.operator_keys.len(),
            min_signers = self.min_signers(),
            stage_timeout_secs = self.stage_timeout.as_secs_f64(),
            "schnorr coordinator running"
        );
        loop {
            let Some((height, assignment)) = self.next_assignment().await else {
                // Assignments lock poisoned — the process is on its way down.
                return;
            };
            self.drive_height(height, assignment).await;
        }
    }

    /// Waits for an unprocessed assignment at or above the tip.
    async fn next_assignment(&self) -> Option<(u64, Assignment<GasKillerTaskData>)> {
        loop {
            {
                let tip = self.mailbox.get_tip().await;
                let assignments = self.assignments.read().ok()?;
                if let Some((&height, assignment)) = assignments.iter().find(|(h, _)| **h >= tip) {
                    return Some((height, assignment.clone()));
                }
            }
            tokio::time::sleep(ASSIGNMENT_POLL).await;
        }
    }

    /// Runs signing attempts for a height until success or the round deadline,
    /// then records the outcome (task certificate or skip) and notifies the
    /// submitter.
    async fn drive_height(&mut self, height: u64, assignment: Assignment<GasKillerTaskData>) {
        let deadline = Instant::now() + self.round_timeout;
        let message: [u8; 32] = assignment
            .digest
            .as_ref()
            .try_into()
            .expect("sha256 digest is 32 bytes");

        // Partial-stage offenders (no partial, or an invalid one) are excluded
        // from later attempts' subsets — a node that commits nonces but never
        // signs would otherwise stall every attempt until the deadline. They are
        // re-admitted only if the compliant set alone cannot reach the floor.
        let mut suspects: HashSet<Address> = HashSet::new();

        let mut attempt: u32 = 0;
        while Instant::now() < deadline {
            attempt += 1;
            match self
                .run_attempt(height, attempt, &message, &mut suspects, deadline)
                .await
            {
                Some((signature, non_signers)) => {
                    info!(
                        height,
                        attempt,
                        non_signers = non_signers.len(),
                        "aggregate schnorr signature assembled"
                    );
                    self.mailbox.record(height, assignment.digest);
                    let _ = self.certified_out.send(SchnorrCertified {
                        height,
                        digest: assignment.digest,
                        signature: Some(signature),
                        non_signers,
                    });
                    return;
                }
                None => {
                    debug!(height, attempt, "signing attempt failed; retrying");
                }
            }
        }

        // Round deadline: resolve the height as skipped so the pipeline advances
        // (the sequencer is broadcasting Skip{h} on channel 1 by now). No skip
        // signature is assembled — nothing downstream consumes skip proofs.
        warn!(
            height,
            attempts = attempt,
            timeout_secs = self.round_timeout.as_secs_f64(),
            "no aggregate signature before round timeout, skipping height"
        );
        let skip = skip_digest(&self.namespace, height);
        self.mailbox.record(height, skip);
        let _ = self.certified_out.send(SchnorrCertified {
            height,
            digest: skip,
            signature: None,
            non_signers: Vec::new(),
        });
    }

    /// One full two-round attempt. Returns the verified signature and the sorted
    /// non-signer list, or `None` (reasons logged; `suspects` updated).
    async fn run_attempt(
        &mut self,
        height: u64,
        attempt: u32,
        message: &[u8; 32],
        suspects: &mut HashSet<Address>,
        deadline: Instant,
    ) -> Option<(AggregateSignature, Vec<Address>)> {
        // Round 1: fresh nonces from everyone (suspects included — flapping nodes
        // recover here; they are filtered at subset selection below).
        let request = SchnorrMsg::NonceRequest { height, attempt }.encode();
        let _ = self
            .sender
            .send(Recipients::Some(self.operator_keys.clone()), request, true);

        let stage_deadline = (Instant::now() + self.stage_timeout).min(deadline);
        let mut commits: HashMap<Address, (schnorr::PublicKey, PubNonce)> = HashMap::new();
        while commits.len() < self.operator_keys.len() {
            let Some(msg) = self.recv_until(stage_deadline).await else {
                break;
            };
            let (peer, msg) = msg;
            if let SchnorrMsg::NonceCommit {
                height: h,
                attempt: a,
                pubkey,
                nonce,
            } = msg
            {
                if h != height || a != attempt {
                    continue; // stale session traffic
                }
                // The pubkey point must be the sender's registered identity:
                // address(point) == the sender's operator address.
                let Some(peer_addr) = self.peer_to_address.get(&peer).copied() else {
                    continue; // not a known operator
                };
                if pubkey.eth_address() != peer_addr {
                    warn!(height, attempt, peer = %peer, "nonce commit pubkey does not match sender; ignored");
                    continue;
                }
                commits.insert(peer_addr, (pubkey, nonce));
            }
        }

        // Subset selection: nonce responders minus suspects — unless that
        // undershoots the floor, in which case re-admit everyone who responded
        // (better a possibly-stalling attempt than none).
        let min_signers = self.min_signers();
        let mut subset: Vec<Address> = commits
            .keys()
            .filter(|addr| !suspects.contains(*addr))
            .copied()
            .collect();
        if subset.len() < min_signers {
            subset = commits.keys().copied().collect();
        }
        if subset.len() < min_signers {
            debug!(
                height,
                attempt,
                responders = commits.len(),
                min_signers,
                "not enough nonce responders for a quorum"
            );
            return None;
        }

        let contributions: Vec<(schnorr::PublicKey, PubNonce)> =
            subset.iter().map(|addr| commits[addr]).collect();
        let ctx = Coordinator::build_context(&contributions, message)?;

        // Round 2: the signing context goes to exactly the subset.
        let sign_request = SchnorrMsg::SignRequest(SignRequest {
            height,
            attempt,
            message: *message,
            signers: contributions.iter().map(|(pk, _)| *pk).collect(),
            agg_nonces: ctx.agg_nonces(),
            r_addr: ctx.r_addr,
        })
        .encode();
        let recipients: Vec<PublicKey> = subset
            .iter()
            .map(|addr| self.address_to_peer[addr].clone())
            .collect();
        let _ = self
            .sender
            .send(Recipients::Some(recipients), sign_request, true);

        let stage_deadline = (Instant::now() + self.stage_timeout).min(deadline);
        // (address, partial scalar) pairs; the scalar type is inferred so the
        // router crate does not need a direct k256 dependency.
        let mut partials = Vec::new();
        while partials.len() < subset.len() {
            let Some((peer, msg)) = self.recv_until(stage_deadline).await else {
                break;
            };
            if let SchnorrMsg::PartialSig {
                height: h,
                attempt: a,
                partial,
            } = msg
            {
                if h != height || a != attempt {
                    continue;
                }
                let Some(addr) = self.peer_to_address.get(&peer).copied() else {
                    continue; // not a known operator
                };
                let Some((pk, nonce)) = commits.get(&addr) else {
                    continue; // not a nonce responder for this session
                };
                if !subset.contains(&addr) || partials.iter().any(|(a, _)| *a == addr) {
                    continue; // uninvited, or a duplicate partial
                }
                // Attribute bad partials to the exact signer instead of letting
                // them poison the aggregate.
                if !Coordinator::verify_partial(&ctx, pk, nonce, &partial) {
                    warn!(height, attempt, signer = %addr, "invalid partial signature; excluding signer");
                    suspects.insert(addr);
                    continue;
                }
                partials.push((addr, partial));
            }
        }

        if partials.len() < subset.len() {
            for addr in &subset {
                if !partials.iter().any(|(a, _)| a == addr) {
                    debug!(height, attempt, signer = %addr, "no partial before stage timeout");
                    suspects.insert(*addr);
                }
            }
            return None;
        }

        // Every invited signer produced a verified partial: assembly cannot fail
        // (assemble still self-verifies the aggregate as a final guard).
        let signature = Coordinator::assemble(&ctx, partials.into_iter().map(|(_, s)| s))?;

        let mut non_signers: Vec<Address> = self
            .operator_addresses
            .iter()
            .filter(|addr| !subset.contains(*addr))
            .copied()
            .collect();
        non_signers.sort();
        Some((signature, non_signers))
    }

    /// Receives the next channel-2 message before `deadline`, or `None` on
    /// timeout / channel close. Non-operator senders are dropped.
    async fn recv_until(&mut self, deadline: Instant) -> Option<(PublicKey, SchnorrMsg)> {
        loop {
            let remaining = deadline.checked_duration_since(Instant::now())?;
            let received = tokio::time::timeout(remaining, self.receiver.recv())
                .await
                .ok()?;
            let (peer, bytes) = received.ok()?;
            if !self.peer_to_address.contains_key(&peer) {
                warn!(peer = %peer, "schnorr message from unknown peer; ignored");
                continue;
            }
            match SchnorrMsg::decode(bytes) {
                Ok(msg) => return Some((peer, msg)),
                Err(error) => {
                    warn!(%error, "malformed schnorr message; ignored");
                    continue;
                }
            }
        }
    }
}

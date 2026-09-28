//! Schnorr coordinator: the router's side of the two-round MuSig2 aggregate signing protocol
//! (p2p channel 2). The scheduler hands it one session at a time — a height and its task — and
//! gets back how the session ended.
//!
//! # Session flow
//!
//! ```text
//! attempt = 1, 2, … (fresh nonces each — a nonce is bound to one session):
//!   CommitRequest{h,a,task}  → all operators, re-sent every REBROADCAST_INTERVAL to the
//!     operators that have not committed yet
//!   collect Commit{…, digest} (each node commits once it has traced the task) until
//!     all reply or the trace timeout, cut to one stage timeout once some digest has
//!     ceil(N·num/den) commits; verify each commit's pubkey point maps to the sender's
//!     operator address
//!   the largest digest group reaches ceil(N·num/den)?  else next attempt
//!   build_context → SignRequest{h,a, digest, signer points, R aggregates} → subset of that group
//!   collect PartialSig from exactly the subset until the sign stage timeout; each
//!     partial is verified against the signer's own nonce commitment (bad partials
//!     are attributed and the signer is excluded from the next attempt, and the
//!     attempt is abandoned at once rather than waiting out a stage it cannot win)
//!   all partials → assemble (self-verifies) → Signed{digest, sig, nonSigners}
//! ROUND_TIMEOUT from the session's start → TimedOut
//! the router's own trace of the task failing → TraceFailed
//! ```
//!
//! The router never waits on its own trace to sign: the digest comes from the nodes'
//! commits, and the render gate checks it against the router trace before anything is
//! handed out. A router trace that fails ends the session at once, so a task every node
//! would also fail to trace does not hold the pipeline for a whole round.
//!
//! # p2p identity vs signing identity
//!
//! The p2p transport key is BN254 (the network's operator identity), but a
//! Schnorr signer is a secp256k1 point whose Ethereum address is the operator's
//! registry identity. The two are bound at registration; here the coordinator
//! carries both directions of the map so it can (a) authenticate a `Commit`
//! (the committed point's address must equal the sender's registered address) and
//! (b) address round-2 `SignRequest`s to the p2p keys of a subset chosen by
//! address.

use crate::sequencer::RouterTrace;
use alloy_primitives::Address;
use commonware_avs_core::bn254::PublicKey;
use commonware_codec::{DecodeExt, Encode};
use commonware_p2p::{Receiver, Recipients, Sender};
use gas_killer_common::schnorr::musig::{Coordinator, PubNonce};
use gas_killer_common::schnorr::wire::{SchnorrMsg, SignRequest};
use gas_killer_common::schnorr::{self, AggregateSignature};
use gas_killer_common::task_data::GasKillerTaskData;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Which invited signers to hold against the next attempt after round 2 came up short.
///
/// Silence is evidence only once the stage has actually run its course. `foreclosed` marks
/// the attempt that ended early because a signer was proven bad: the signers that had not
/// answered yet were never given their full window, so blaming them would drop them from the
/// next attempt's subset and pad the on-chain non-signer list for no reason. The proven-bad
/// signer is recorded by the caller at the point it is detected, not here.
fn silent_signers<T>(
    subset: &[Address],
    partials: &[(Address, T)],
    foreclosed: bool,
) -> Vec<Address> {
    if foreclosed {
        return Vec::new();
    }
    subset
        .iter()
        .filter(|addr| !partials.iter().any(|(seen, _)| seen == *addr))
        .copied()
        .collect()
}

/// The digest to sign: the one the most commits agree on, if at least `min_signers` do.
///
/// A digest is signable only when a quorum derived it independently, so a minority that
/// traced differently cannot steer the round. Ties break toward the lower digest so repeated
/// runs over the same commits pick the same group.
fn agreed_digest<'a>(
    digests: impl Iterator<Item = &'a [u8; 32]>,
    min_signers: usize,
) -> Option<[u8; 32]> {
    let mut counts: BTreeMap<[u8; 32], usize> = BTreeMap::new();
    for digest in digests {
        *counts.entry(*digest).or_default() += 1;
    }
    let (digest, count) = counts
        .into_iter()
        .max_by(|(a, x), (b, y)| x.cmp(y).then_with(|| b.cmp(a)))?;
    (count >= min_signers).then_some(digest)
}

/// The height a router life starts at: Unix time in milliseconds.
///
/// Heights only name sessions (neither the task digest nor the chain sees them), so a start far
/// above the last height costs nothing. What matters is never reusing one: a node keys its
/// nonce sessions by `(height, attempt)` and never signs twice for the same key. The scheduler
/// never hands out a height above the clock, so a previous life's heights are all below the
/// clock when the next life starts. The wall clock is not monotonic, so a backward step across
/// a restart would reuse recorded heights and the nodes would refuse those sessions until
/// restarted.
pub fn clock_tip() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_millis() as u64,
        Err(_) => {
            warn!("system clock is before the Unix epoch; schnorr heights start at 0");
            0
        }
    }
}

/// How a signing session ended.
#[derive(Debug, Clone)]
pub enum SessionOutcome {
    /// A quorum signed `digest`.
    Signed {
        /// The digest the quorum signed: the one a quorum of commits carried.
        digest: [u8; 32],
        /// The verified aggregate signature.
        signature: AggregateSignature,
        /// Operator identity addresses that did NOT sign, strictly ascending — the exact
        /// list `SchnorrStakeRegistry.isValidSignature` subtracts on-chain.
        non_signers: Vec<Address>,
    },
    /// No attempt assembled a signature before `ROUND_TIMEOUT`.
    TimedOut,
    /// The router's own trace of the task failed, so the session could never render.
    TraceFailed(String),
}

/// The coordinator. Owns the channel-2 endpoints and drives one session at a time.
pub struct SchnorrCoordinator<S, R>
where
    S: Sender<PublicKey = PublicKey>,
    R: Receiver<PublicKey = PublicKey>,
{
    sender: S,
    receiver: R,
    /// Operator p2p keys, the round-1 `CommitRequest` recipients.
    operator_keys: Vec<PublicKey>,
    /// p2p key → operator address: authenticates an incoming commit/partial by the
    /// sender's registered identity.
    peer_to_address: HashMap<PublicKey, Address>,
    /// operator address → p2p key: addresses round-2 `SignRequest`s to a subset
    /// selected by address.
    address_to_peer: HashMap<Address, PublicKey>,
    /// All operator identity addresses (the non-signer complement is drawn from here).
    operator_addresses: HashSet<Address>,
    /// Local participation floor `num/den` before a signing round is attempted
    /// (the authoritative stake check is the on-chain registry threshold).
    threshold: (u64, u64),
    /// The round-trip deadline: partial collection, and the wait for the remaining commits
    /// once a digest has enough to sign.
    stage_timeout: Duration,
    /// Commit deadline. A node commits only once it has traced the task, so this
    /// has to cover a full cold trace.
    trace_timeout: Duration,
    round_timeout: Duration,
    /// How often an unanswered `CommitRequest` is re-sent. A lost request would otherwise
    /// cost the whole trace budget before the next attempt asks again.
    resend_interval: Duration,
    /// Set once the channel-2 receiver has closed: the network is shutting down, so no session
    /// can finish and waiting out its deadlines would only spin.
    closed: bool,
}

impl<S, R> SchnorrCoordinator<S, R>
where
    S: Sender<PublicKey = PublicKey>,
    R: Receiver<PublicKey = PublicKey>,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sender: S,
        receiver: R,
        operators: Vec<(PublicKey, Address)>,
        threshold: (u64, u64),
        stage_timeout: Duration,
        trace_timeout: Duration,
        round_timeout: Duration,
        resend_interval: Duration,
    ) -> Self {
        let operator_keys: Vec<PublicKey> = operators.iter().map(|(k, _)| k.clone()).collect();
        let peer_to_address: HashMap<PublicKey, Address> = operators.iter().cloned().collect();
        let address_to_peer: HashMap<Address, PublicKey> =
            operators.iter().map(|(k, a)| (*a, k.clone())).collect();
        let operator_addresses: HashSet<Address> = operators.iter().map(|(_, a)| *a).collect();
        let coordinator = Self {
            sender,
            receiver,
            operator_keys,
            peer_to_address,
            address_to_peer,
            operator_addresses,
            threshold,
            stage_timeout,
            trace_timeout,
            round_timeout,
            resend_interval,
            closed: false,
        };
        info!(
            operators = coordinator.operator_keys.len(),
            min_signers = coordinator.min_signers(),
            stage_timeout_secs = stage_timeout.as_secs_f64(),
            trace_timeout_secs = trace_timeout.as_secs_f64(),
            "schnorr coordinator running"
        );
        coordinator
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

    /// Runs signing attempts for `task` at `height` until one assembles a signature, the round
    /// deadline passes, or the router's own trace fails. `None` means the channel closed, so the
    /// session could not run to an outcome.
    pub async fn drive_height(
        &mut self,
        height: u64,
        task: &GasKillerTaskData,
        trace: &RouterTrace,
    ) -> Option<SessionOutcome> {
        let deadline = Instant::now() + self.round_timeout;
        let trace = trace.clone();
        let trace_failed = async move {
            match trace.wait().await {
                Err(reason) => reason,
                Ok(_) => std::future::pending().await,
            }
        };
        tokio::pin!(trace_failed);

        // Partial-stage offenders (no partial, or an invalid one) are excluded
        // from later attempts' subsets — a node that commits nonces but never
        // signs would otherwise stall every attempt until the deadline. They are
        // re-admitted only if the compliant set alone cannot reach the floor.
        let mut suspects: HashSet<Address> = HashSet::new();

        let mut attempt: u32 = 0;
        while Instant::now() < deadline {
            if self.closed {
                warn!(height, "schnorr channel closed; abandoning session");
                return None;
            }
            attempt += 1;
            // Biased toward the trace: a session whose router trace has failed cannot render,
            // so when both are ready it ends as a trace failure rather than as a signed round
            // that fails at the render gate.
            tokio::select! {
                biased;
                reason = &mut trace_failed => {
                    warn!(height, %reason, "router trace failed, abandoning session");
                    return Some(SessionOutcome::TraceFailed(reason));
                }
                outcome = self.run_attempt(height, attempt, task, &mut suspects, deadline) => {
                    let Some((signature, non_signers, digest)) = outcome else {
                        debug!(height, attempt, "signing attempt failed; retrying");
                        continue;
                    };
                    info!(
                        height,
                        attempt,
                        non_signers = non_signers.len(),
                        "aggregate schnorr signature assembled"
                    );
                    return Some(SessionOutcome::Signed {
                        digest,
                        signature,
                        non_signers,
                    });
                }
            }
        }

        warn!(
            height,
            attempts = attempt,
            timeout_secs = self.round_timeout.as_secs_f64(),
            "no aggregate signature before round timeout, abandoning session"
        );
        Some(SessionOutcome::TimedOut)
    }

    /// One full two-round attempt. Returns the verified signature, the sorted non-signer
    /// list and the digest signed, or `None` (reasons logged; `suspects` updated).
    async fn run_attempt(
        &mut self,
        height: u64,
        attempt: u32,
        task: &GasKillerTaskData,
        suspects: &mut HashSet<Address>,
        deadline: Instant,
    ) -> Option<(AggregateSignature, Vec<Address>, [u8; 32])> {
        // Round 1: fresh nonces from everyone (suspects included — flapping nodes
        // recover here; they are filtered at subset selection below).
        let request = SchnorrMsg::CommitRequest {
            height,
            attempt,
            task: task.clone(),
        }
        .encode();
        let _ = self.sender.send(
            Recipients::Some(self.operator_keys.clone()),
            request.clone(),
            true,
        );

        let min_signers = self.min_signers();
        let trace_deadline = (Instant::now() + self.trace_timeout).min(deadline);
        let mut stage_deadline = trace_deadline;
        let mut commits: HashMap<Address, (schnorr::PublicKey, PubNonce, [u8; 32])> =
            HashMap::new();
        let mut next_resend = Instant::now() + self.resend_interval;
        while commits.len() < self.operator_keys.len() {
            let Some(msg) = self.recv_until(stage_deadline.min(next_resend)).await else {
                if self.closed || Instant::now() >= stage_deadline {
                    break;
                }
                // A node dedupes by session, so the ones already tracing just ignore it.
                let silent: Vec<PublicKey> = self
                    .operator_keys
                    .iter()
                    .filter(|key| {
                        !self
                            .peer_to_address
                            .get(*key)
                            .is_some_and(|addr| commits.contains_key(addr))
                    })
                    .cloned()
                    .collect();
                let _ = self
                    .sender
                    .send(Recipients::Some(silent), request.clone(), true);
                next_resend = Instant::now() + self.resend_interval;
                continue;
            };
            let (peer, msg) = msg;
            if let SchnorrMsg::Commit {
                height: h,
                attempt: a,
                pubkey,
                nonce,
                digest,
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
                    warn!(height, attempt, peer = %peer, "commit pubkey does not match sender; ignored");
                    continue;
                }
                commits.insert(peer_addr, (pubkey, nonce, digest));
                // Once a digest can be signed, the operators still tracing get one round trip
                // to catch up rather than the rest of the trace budget.
                if stage_deadline == trace_deadline
                    && agreed_digest(commits.values().map(|(_, _, d)| d), min_signers).is_some()
                {
                    stage_deadline = (Instant::now() + self.stage_timeout).min(trace_deadline);
                }
            }
        }

        let Some(message) = agreed_digest(commits.values().map(|(_, _, d)| d), min_signers) else {
            debug!(
                height,
                attempt,
                responders = commits.len(),
                min_signers,
                "no digest has enough commits for a quorum"
            );
            return None;
        };
        for (addr, (_, _, digest)) in &commits {
            if *digest != message {
                warn!(
                    height,
                    attempt,
                    operator = %addr,
                    theirs = %alloy_primitives::hex::encode(digest),
                    agreed = %alloy_primitives::hex::encode(message),
                    "operator committed to a different digest than the quorum"
                );
            }
        }
        commits.retain(|_, (_, _, digest)| *digest == message);

        // Subset selection: agreeing responders minus suspects — unless that
        // undershoots the floor, in which case re-admit everyone who agreed
        // (better a possibly-stalling attempt than none).
        let mut subset: Vec<Address> = commits
            .keys()
            .filter(|addr| !suspects.contains(*addr))
            .copied()
            .collect();
        if subset.len() < min_signers {
            subset = commits.keys().copied().collect();
        }

        let contributions: Vec<(schnorr::PublicKey, PubNonce)> = subset
            .iter()
            .map(|addr| {
                let (pk, nonce, _) = commits[addr];
                (pk, nonce)
            })
            .collect();
        let ctx = Coordinator::build_context(&contributions, &message)?;

        // Round 2: the signing context goes to exactly the subset.
        let sign_request = SchnorrMsg::SignRequest(SignRequest {
            height,
            attempt,
            message,
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
        // Set when a signer is proven bad, which forecloses this attempt: assembly needs a
        // verified partial from every invited signer, so waiting out the rest of the stage
        // could only reach the same failure later. Distinguished from a plain shortfall
        // because the two attribute blame differently below.
        let mut foreclosed = false;
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
                let Some((pk, nonce, _)) = commits.get(&addr) else {
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
                    foreclosed = true;
                    break;
                }
                partials.push((addr, partial));
            }
        }

        if partials.len() < subset.len() {
            for addr in silent_signers(&subset, &partials, foreclosed) {
                debug!(height, attempt, signer = %addr, "no partial before stage timeout");
                suspects.insert(addr);
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
        Some((signature, non_signers, message))
    }

    /// Receives the next channel-2 message before `deadline`, or `None` on timeout or channel
    /// close (which also sets [`Self::closed`]). Non-operator senders are dropped.
    async fn recv_until(&mut self, deadline: Instant) -> Option<(PublicKey, SchnorrMsg)> {
        loop {
            let remaining = deadline.checked_duration_since(Instant::now())?;
            let received = tokio::time::timeout(remaining, self.receiver.recv())
                .await
                .ok()?;
            let Ok((peer, bytes)) = received else {
                self.closed = true;
                return None;
            };
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

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_actor::{Feedback, Unreliable};
    use commonware_avs_core::bn254::Bn254;
    use commonware_cryptography::Signer as _;
    use commonware_p2p::{CheckedSender, LimitedSender, Message};
    use commonware_runtime::IoBufs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    /// A channel-2 receiver whose network has already shut down.
    #[derive(Debug)]
    struct ClosedReceiver;

    impl Receiver for ClosedReceiver {
        type Error = std::io::Error;
        type PublicKey = PublicKey;

        async fn recv(&mut self) -> Result<Message<PublicKey>, Self::Error> {
            Err(std::io::Error::other("channel closed"))
        }
    }

    /// A channel-2 sender that delivers to every recipient and counts its sends.
    #[derive(Clone, Default)]
    struct CountingSender {
        sends: Arc<AtomicUsize>,
    }

    struct CountingChecked {
        recipients: Vec<PublicKey>,
        sends: Arc<AtomicUsize>,
    }

    impl CheckedSender for CountingChecked {
        type PublicKey = PublicKey;

        fn recipients(&self) -> Vec<PublicKey> {
            self.recipients.clone()
        }

        fn send(self, _message: impl Into<IoBufs> + Send, _priority: bool) -> Unreliable<Feedback> {
            self.sends.fetch_add(1, Ordering::Relaxed);
            Unreliable::new(Feedback::Ok)
        }
    }

    impl LimitedSender for CountingSender {
        type PublicKey = PublicKey;
        type Checked<'a>
            = CountingChecked
        where
            Self: 'a;

        fn check(
            &mut self,
            recipients: Recipients<PublicKey>,
        ) -> Result<CountingChecked, SystemTime> {
            let recipients = match recipients {
                Recipients::Some(peers) => peers,
                Recipients::One(peer) => vec![peer],
                Recipients::All => Vec::new(),
            };
            Ok(CountingChecked {
                recipients,
                sends: Arc::clone(&self.sends),
            })
        }
    }

    /// A coordinator over three operators whose channel-2 receiver is already closed.
    fn closed_coordinator(
        sender: CountingSender,
    ) -> SchnorrCoordinator<CountingSender, ClosedReceiver> {
        let operators = (0..3u8)
            .map(|i| (Bn254::from_seed(u64::from(i)).public_key(), addr(i + 1)))
            .collect();
        SchnorrCoordinator::new(
            sender,
            ClosedReceiver,
            operators,
            (2, 3),
            Duration::from_secs(5),
            Duration::from_secs(60),
            Duration::from_secs(120),
            Duration::from_millis(10),
        )
    }

    /// A closed channel ends the session at once instead of re-sending in a hot loop until the
    /// trace and round deadlines run out.
    #[tokio::test]
    async fn a_closed_channel_ends_the_session_without_spinning() {
        let sender = CountingSender::default();
        let mut coordinator = closed_coordinator(sender.clone());
        let trace = RouterTrace::spawn(std::future::pending());

        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            coordinator.drive_height(1, &GasKillerTaskData::default(), &trace),
        )
        .await
        .expect("a closed channel must not hold the session until its deadlines");

        assert!(outcome.is_none());
        assert_eq!(
            sender.sends.load(Ordering::Relaxed),
            1,
            "only the first request goes out"
        );
    }

    /// A stage that ran its course blames everyone who stayed silent, which is what keeps a
    /// dead operator out of the next attempt's subset.
    #[test]
    fn a_stage_that_timed_out_blames_every_signer_that_never_answered() {
        let subset = [addr(1), addr(2), addr(3)];
        let partials = [(addr(1), ())];

        assert_eq!(
            silent_signers(&subset, &partials, false),
            vec![addr(2), addr(3)]
        );
    }

    /// Leaving early on a proven-bad partial says nothing about the signers still working, so
    /// none of them are blamed — they would otherwise be dropped from the next attempt and
    /// padded onto the on-chain non-signer list without ever having missed a deadline.
    #[test]
    fn an_attempt_abandoned_early_blames_nobody_for_silence() {
        let subset = [addr(1), addr(2), addr(3)];
        let partials = [(addr(1), ())];

        assert!(silent_signers(&subset, &partials, true).is_empty());
    }

    /// The nodes never sign twice for one session key, so a restarted router must start above
    /// every height its previous life used, the in-flight one included.
    #[tokio::test]
    async fn a_restarted_router_starts_above_every_height_its_previous_life_announced() {
        let first = clock_tip();
        let highest = first + 2;
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(clock_tip() > highest);
    }

    #[test]
    fn the_digest_a_quorum_committed_to_is_signed() {
        let commits = [[1; 32], [1; 32], [2; 32]];
        assert_eq!(agreed_digest(commits.iter(), 2), Some([1; 32]));
    }

    /// A minority that traced differently cannot steer the round, and neither can a plurality
    /// short of the floor.
    #[test]
    fn no_digest_is_signed_without_a_quorum_behind_it() {
        let commits = [[1; 32], [2; 32], [3; 32]];
        assert_eq!(agreed_digest(commits.iter(), 2), None);
        assert_eq!(agreed_digest(std::iter::empty(), 1), None);
    }

    #[test]
    fn tied_digests_resolve_the_same_way_every_time() {
        let commits = [[2; 32], [1; 32], [2; 32], [1; 32]];
        assert_eq!(agreed_digest(commits.iter(), 2), Some([1; 32]));
        assert_eq!(agreed_digest(commits.iter().rev(), 2), Some([1; 32]));
    }

    #[test]
    fn a_complete_subset_leaves_nobody_to_blame() {
        let subset = [addr(1), addr(2)];
        let partials = [(addr(1), ()), (addr(2), ())];

        assert!(silent_signers(&subset, &partials, false).is_empty());
    }
}

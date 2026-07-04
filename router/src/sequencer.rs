//! Task sequencer: assigns aggregation heights to ingress tasks and drives them to
//! resolution (absorbs the old `creator.rs`).
//!
//! The aggregation engine's tip only advances when every height certifies (see the
//! liveness model in `.context/DESIGN.md`), so the sequencer follows two hard rules:
//!
//! 1. **Exactly one outstanding height at a time.** `next_height` starts at the
//!    reporter tip observed after a short settle delay (journal replay) and only
//!    advances when the current height resolves — a certificate was observed (real
//!    digest or skip digest) AND, for real digests, on-chain execution finished
//!    (successfully or not).
//! 2. **Every assigned height MUST resolve to a certificate.** The sequencer
//!    rebroadcasts `TaskDirective::Announce` every `REBROADCAST_INTERVAL`; after
//!    `ROUND_TIMEOUT` without a certificate it switches to broadcasting
//!    `TaskDirective::Skip` (and keeps rebroadcasting until the height certifies —
//!    with either digest; a late real-digest certificate is still submitted while the
//!    assignment is cached).
//!
//! Pre-restart leftovers: a certificate for the current height whose digest is
//! neither the expected digest nor `skip_digest(h)` (a node resolved the height with
//! a directive from a previous router life) consumes the height — the in-flight task
//! is re-assigned to the next height. Heights certified without an assignment simply
//! advance `next_height` past them. Heights between the tip and the first assignment
//! that never certified (non-contiguous pre-restart certificates) are resolved by the
//! nodes' auto-skip rule once they see this router's first directive for a higher
//! height.

use crate::ingress::GasKillerTaskRequest;
use crate::metrics::MetricsCollector;
use crate::reporter::CertReporterMailbox;
use gas_killer_common::GasKillerValidator;
use gas_killer_common::task_data::GasKillerTaskData;
use gas_killer_common::tasks::TaskDirective;
use gas_killer_common::{rebroadcast_interval, round_timeout};

use alloy_primitives::{B256, Bytes};
use anyhow::Result;
use commonware_codec::{DecodeExt, Encode};
use commonware_cryptography::sha256::Digest;
use commonware_cryptography::{Hasher, Sha256};
use commonware_p2p::{Receiver as NetworkReceiver, Recipients, Sender as NetworkSender};
use gas_killer_common::ecdsa::PublicKey;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, warn};

/// How long the sequencer waits after startup before reading the reporter tip.
///
/// Covers the engine's journal replay (which re-reports certified heights and the
/// tip into the [`crate::reporter::CertReporter`]) so `next_height` starts where the
/// previous router life stopped instead of re-assigning already-certified heights.
const SETTLE_DELAY: Duration = Duration::from_secs(2);

/// Per-height dispatch timestamps: the sequencer inserts `height -> Instant::now()`
/// when it assigns a task, and the executor removes the matching entry in
/// `handle_verification` to compute the P2P round-trip duration.
///
/// Keying by height (rather than a single shared slot) means a height that fails
/// without calling `handle_verification` cannot bleed its stale timestamp into the
/// next height's measurement. Heights advance monotonically and the sequencer runs
/// one at a time, so the sequencer evicts any entry older than the height it is
/// dispatching, keeping the map bounded even when heights repeatedly fail.
pub type DispatchTime = Arc<Mutex<HashMap<u64, Instant>>>;

/// Records `height`'s dispatch instant for later round-trip measurement, first
/// evicting any entry from an earlier height. Heights advance monotonically and the
/// sequencer runs one at a time, so an older entry belongs to a height that failed
/// without completing; dropping it here keeps the map bounded.
pub(crate) fn stamp_dispatch_time(times: &DispatchTime, height: u64) {
    if let Ok(mut times) = times.lock() {
        times.retain(|&h, _| h >= height);
        times.insert(height, Instant::now());
    }
}

/// Removes and returns `height`'s dispatch instant, if one was recorded. Consuming
/// the entry both yields the round-trip start and prevents the completed height from
/// lingering in the map.
pub(crate) fn take_dispatch_time(times: &DispatchTime, height: u64) -> Option<Instant> {
    times
        .lock()
        .ok()
        .and_then(|mut times| times.remove(&height))
}

pub type TaskSender = UnboundedSender<GasKillerTaskRequest>;
pub type TaskReceiver = UnboundedReceiver<GasKillerTaskRequest>;
/// Shared atomic counter tracking tasks in flight between the ingress sender and
/// sequencer receiver.
pub type TaskQueueDepth = Arc<AtomicUsize>;

pub fn task_channel() -> (TaskSender, TaskReceiver) {
    mpsc::unbounded_channel()
}

pub fn task_queue_depth() -> TaskQueueDepth {
    Arc::new(AtomicUsize::new(0))
}

/// The expected outcome for an assigned height, shared between the sequencer (which
/// writes it), the automaton (which resolves `propose(h)` with `digest`), and the
/// submitter (which needs `task` for the on-chain `verifyAndUpdate` call).
#[derive(Clone)]
pub struct Assignment {
    /// The digest a correct node signs for this height
    /// (`task.build_payload_hash(task.storage_updates)`).
    pub digest: Digest,
    /// The enriched task data announced on channel 1.
    pub task: GasKillerTaskData,
}

/// Height → assignment map. Entries live from assignment until the height resolves;
/// the sequencer removes them, so the map holds at most the single in-flight height.
pub type SharedAssignments = Arc<RwLock<BTreeMap<u64, Assignment>>>;

pub fn shared_assignments() -> SharedAssignments {
    Arc::new(RwLock::new(BTreeMap::new()))
}

/// How a certified height was consumed, reported by the submitter to the sequencer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionKind {
    /// The certificate carried the assignment's expected digest and the on-chain
    /// submission finished (`success` reflects the verifyAndUpdate outcome).
    Executed { success: bool },
    /// The certificate carried `skip_digest(height)`: the quorum abandoned the
    /// height and the task (if any) was dropped.
    Skipped,
    /// The certificate carried a digest that is neither the expected digest nor the
    /// skip digest, or the height had no assignment (pre-restart leftovers). The
    /// height is consumed; an in-flight task must be re-assigned.
    Foreign,
}

/// A certified height's final disposition (submitter → sequencer).
#[derive(Debug, Clone, Copy)]
pub struct Resolution {
    pub height: u64,
    pub kind: ResolutionKind,
}

pub type ResolutionSender = UnboundedSender<Resolution>;
pub type ResolutionReceiver = UnboundedReceiver<Resolution>;

pub fn resolution_channel() -> (ResolutionSender, ResolutionReceiver) {
    mpsc::unbounded_channel()
}

/// Enriched task data that includes computed storage updates and block height.
struct EnrichedTask {
    task: GasKillerTaskRequest,
    storage_updates: Bytes,
    block_height: u64,
    /// Hash of the anchor block (`block_height`) the storage updates were computed against.
    anchor_hash: B256,
    /// Resolved transition index (sentinel `None` → concrete count from chain).
    transition_index: u64,
    /// Actual EVM chain ID (e.g. 1 = Ethereum mainnet, 100 = Gnosis, 31337 = Anvil).
    chain_id: u64,
}

impl EnrichedTask {
    fn into_task_data(self) -> GasKillerTaskData {
        GasKillerTaskData {
            storage_updates: self.storage_updates,
            transition_index: self.transition_index,
            target_address: self.task.body.target_address,
            call_data: self.task.body.call_data,
            from_address: self.task.body.from_address,
            value: self.task.body.value,
            block_height: self.block_height,
            anchor_hash: self.anchor_hash,
            chain_id: self.chain_id,
        }
    }
}

/// Why [`Sequencer::drive_height`] returned.
enum HeightOutcome {
    /// The height resolved (certificate + execution); see [`ResolutionKind`].
    Resolved(ResolutionKind),
    /// Node tip reports prove the quorum is already past this height (the router
    /// lost its journal and assigned a height the nodes will never propose) — the
    /// task must be re-assigned at the reported tip.
    Superseded,
    /// The resolution channel closed (submitter died) — shut down.
    Closed,
}

/// Node tip reports (channel 1, node → router), keyed by operator identity.
///
/// A node that receives a directive for a height below its own engine tip replies
/// with [`TaskDirective::TipReport`] — evidence this router lost its journal and is
/// driving heights the nodes will never propose (their engines only propose
/// `[tip, tip+window)`). [`TipReports::safe_tip`] applies the same trust rule as
/// the engine's safe-tip: the `(f+1)`-th highest report is reachable only if at
/// least one honest node reached it (`f = (n-1)/3`, the N3f1 fault bound).
///
/// Callers must only [`TipReports::record`] reports from authenticated quorum
/// participants (see [`ingest_tip_reports`]).
#[derive(Clone)]
pub struct TipReports {
    /// Highest tip reported per participant (monotonic).
    reports: Arc<RwLock<BTreeMap<PublicKey, u64>>>,
    /// Faults tolerated by the engine's N3f1 quorum rule.
    max_faults: usize,
}

impl TipReports {
    pub fn new(participant_count: usize) -> Self {
        Self {
            reports: Arc::new(RwLock::new(BTreeMap::new())),
            max_faults: participant_count.saturating_sub(1) / 3,
        }
    }

    /// Records `tip` for `peer`, keeping the per-peer maximum.
    pub fn record(&self, peer: PublicKey, tip: u64) {
        if let Ok(mut reports) = self.reports.write() {
            let entry = reports.entry(peer).or_insert(0);
            *entry = (*entry).max(tip);
        }
    }

    /// The `(f+1)`-th highest reported tip (0 until that many nodes reported).
    pub fn safe_tip(&self) -> u64 {
        let Ok(reports) = self.reports.read() else {
            return 0;
        };
        let mut tips: Vec<u64> = reports.values().copied().collect();
        tips.sort_unstable_by(|a, b| b.cmp(a));
        tips.get(self.max_faults).copied().unwrap_or(0)
    }
}

/// Consumes the router's side of p2p channel 1, recording node tip reports.
///
/// Only quorum participants may influence the sequencer's height choice; other
/// directive variants (the router's own broadcasts echoed back by a buggy peer)
/// are ignored. Malformed payloads are logged and dropped.
pub async fn ingest_tip_reports<R>(
    mut receiver: R,
    participants: HashSet<PublicKey>,
    reports: TipReports,
) where
    R: NetworkReceiver<PublicKey = PublicKey>,
{
    loop {
        match receiver.recv().await {
            Ok((peer, bytes)) => {
                if !participants.contains(&peer) {
                    warn!(peer = %peer, "tip report from non-participant; ignored");
                    continue;
                }
                match TaskDirective::decode(bytes) {
                    Ok(TaskDirective::TipReport { height }) => {
                        debug!(peer = %peer, height, "node tip report");
                        reports.record(peer, height);
                    }
                    Ok(directive) => {
                        debug!(
                            height = directive.height(),
                            "non-report directive on router side; ignored"
                        );
                    }
                    Err(error) => {
                        warn!(%error, "malformed directive on router side; ignored");
                    }
                }
            }
            Err(error) => {
                info!(?error, "directive channel closed; exiting");
                return;
            }
        }
    }
}

/// Owns the ingress task queue and drives one aggregation height at a time.
pub struct Sequencer<S: NetworkSender<PublicKey = PublicKey>> {
    receiver: TaskReceiver,
    queue_depth: TaskQueueDepth,
    validator: Arc<GasKillerValidator>,
    metrics: Option<Arc<MetricsCollector>>,
    /// Shared with the executor to measure P2P round-trip duration.
    dispatch_time: DispatchTime,
    /// Shared with the automaton and submitter.
    assignments: SharedAssignments,
    /// Certificate observations (tip + per-height digests) from the engine.
    reporter: CertReporterMailbox,
    /// Final dispositions from the submitter.
    resolutions: ResolutionReceiver,
    /// Broadcasts [`TaskDirective`]s to the nodes on p2p channel 1.
    directive_sender: S,
    /// Explicit directive recipients (the participant/operator keys).
    ///
    /// Directives are sent to these keys via `Recipients::Some` rather than
    /// `Recipients::All`: the p2p `LimitedSender` resolves `Recipients::All` from
    /// a lazily-populated connected-peer snapshot that stays empty on the router's
    /// send side (its inbound-heavy channel-1 usage never warms it), silently
    /// dropping every `Announce`. Addressing the known operator set directly
    /// bypasses that snapshot.
    recipients: Vec<PublicKey>,
    /// Node tip reports; a safe tip above the driven height supersedes it.
    tip_reports: TipReports,
    /// Next height to assign; only advances when the current height resolves.
    next_height: u64,
    round_timeout: Duration,
    rebroadcast_interval: Duration,
}

impl<S: NetworkSender<PublicKey = PublicKey>> Sequencer<S> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        receiver: TaskReceiver,
        queue_depth: TaskQueueDepth,
        validator: Arc<GasKillerValidator>,
        metrics: Option<Arc<MetricsCollector>>,
        dispatch_time: DispatchTime,
        assignments: SharedAssignments,
        reporter: CertReporterMailbox,
        resolutions: ResolutionReceiver,
        directive_sender: S,
        recipients: Vec<PublicKey>,
        tip_reports: TipReports,
    ) -> Self {
        Self {
            receiver,
            queue_depth,
            validator,
            metrics,
            dispatch_time,
            assignments,
            reporter,
            resolutions,
            directive_sender,
            recipients,
            tip_reports,
            next_height: 0,
            round_timeout: round_timeout(),
            rebroadcast_interval: rebroadcast_interval(),
        }
    }

    /// Main loop: dequeue → enrich → assign height → drive to resolution → repeat.
    pub async fn run(mut self) {
        // Let the engine replay its journal into the reporter before choosing the
        // first height, so we resume where the previous router life stopped.
        tokio::time::sleep(SETTLE_DELAY).await;
        self.next_height = self
            .reporter
            .get_tip()
            .await
            .max(self.tip_reports.safe_tip());
        info!(
            next_height = self.next_height,
            "sequencer starting at reporter tip"
        );

        loop {
            // Consume any resolutions that arrived while idle (heights certified
            // without an assignment — pre-restart leftovers); never re-assign them.
            self.drain_resolutions();

            let Some(task) = self.wait_for_task().await else {
                info!("task channel closed, sequencer shutting down");
                return;
            };

            let enriched = match self.enrich(task).await {
                Ok(enriched) => enriched,
                Err(e) => {
                    error!(error = %e, "failed to enrich task, dropping request");
                    continue;
                }
            };
            let task_data = enriched.into_task_data();

            // The wire codec asserts the combined calldata + storage-updates size
            // (ingress only bounds the calldata; EVMSketch produces the updates),
            // so reject gracefully here instead of panicking mid-broadcast.
            if let Err(e) = task_data.validate() {
                error!(
                    error = %e,
                    target = %task_data.target_address,
                    "enriched task exceeds wire limits, dropping request"
                );
                continue;
            }

            // Prime the validator's digest cache so the router automaton (and any
            // digest re-derivation) skips EVMSketch: the sequencer already ran it.
            self.validator
                .prime_cache(&task_data, &task_data.storage_updates)
                .await;
            let digest = task_data.build_payload_hash(&task_data.storage_updates);

            // Assign heights until the task is consumed. A foreign-digest
            // certificate consumes the height but not the task, so re-assign it to
            // the next height (liveness rule 5).
            let mut pending = Some(task_data);
            while let Some(task_data) = pending.take() {
                self.drain_resolutions();
                // Never assign below the engine tip (e.g. tip fast-forwarded past
                // heights certified elsewhere while we were executing) or below the
                // nodes' reported tips (journal loss on this router).
                self.next_height = self
                    .next_height
                    .max(self.reporter.get_tip().await)
                    .max(self.tip_reports.safe_tip());
                let height = self.next_height;
                self.next_height += 1;

                if let Ok(mut assignments) = self.assignments.write() {
                    assignments.insert(
                        height,
                        Assignment {
                            digest,
                            task: task_data.clone(),
                        },
                    );
                } else {
                    error!("assignments lock poisoned, dropping task");
                    break;
                }
                stamp_dispatch_time(&self.dispatch_time, height);
                info!(
                    height,
                    digest = %digest,
                    target = %task_data.target_address,
                    transition_index = task_data.transition_index,
                    "assigned task to height"
                );

                let outcome = self.drive_height(height, &task_data).await;
                if let Ok(mut assignments) = self.assignments.write() {
                    assignments.remove(&height);
                }
                match outcome {
                    HeightOutcome::Resolved(ResolutionKind::Executed { success }) => {
                        info!(height, success, "height executed, releasing sequencer");
                    }
                    HeightOutcome::Resolved(ResolutionKind::Skipped) => {
                        // Liveness rule: a skip certificate for our height means the
                        // quorum abandoned the task — drop it (the client observes
                        // no on-chain effect) and move on.
                        warn!(
                            height,
                            target = %task_data.target_address,
                            "quorum skipped our height, dropping task"
                        );
                    }
                    HeightOutcome::Resolved(ResolutionKind::Foreign) => {
                        // Pre-restart leftover consumed the height; the task is
                        // still ours to deliver — re-assign it to the next height.
                        warn!(
                            height,
                            "height certified with a foreign digest, re-assigning task"
                        );
                        pending = Some(task_data);
                    }
                    HeightOutcome::Superseded => {
                        // Node tip reports prove the quorum is past this height —
                        // it certified (or was skipped) in a previous router life
                        // and can never certify again. Re-assign at the safe tip.
                        warn!(
                            height,
                            safe_tip = self.tip_reports.safe_tip(),
                            "height superseded by node tip reports, re-assigning task"
                        );
                        pending = Some(task_data);
                    }
                    HeightOutcome::Closed => {
                        error!("resolution channel closed, sequencer shutting down");
                        return;
                    }
                }
            }
        }
    }

    /// Applies buffered resolutions to `next_height` without blocking.
    fn drain_resolutions(&mut self) {
        while let Ok(resolution) = self.resolutions.try_recv() {
            debug!(
                height = resolution.height,
                kind = ?resolution.kind,
                "observed resolution for unassigned height"
            );
            self.next_height = self.next_height.max(resolution.height + 1);
        }
    }

    /// Broadcasts (and rebroadcasts) the directive for `height` until it resolves.
    ///
    /// Announces `task_data`; after `ROUND_TIMEOUT` without a certificate, switches
    /// to `Skip{height}` so the height still certifies and the pipeline advances.
    /// Timeout granularity is the rebroadcast interval. Rebroadcasting stops once
    /// the reporter has a certificate for the height (waiting only on execution).
    async fn drive_height(&mut self, height: u64, task_data: &GasKillerTaskData) -> HeightOutcome {
        // Announce the task WITHOUT the router's computed storage_updates: nodes
        // independently recompute them with EVMSketch (that is the whole trust
        // model — see GasKillerValidator::expected_digest_for_task), so shipping
        // them is both a validation-bypass smell and dead weight. Dropping the
        // ~700-byte diff shrinks the Announce ~4x, which is the difference between
        // it reliably fitting a single unreliable p2p frame and being dropped in
        // favor of the tiny Skip. The digest is unaffected (the node builds it from
        // its own recomputed updates).
        let announce_task = GasKillerTaskData {
            storage_updates: Bytes::new(),
            ..task_data.clone()
        };
        let announce = TaskDirective::Announce {
            height,
            task: announce_task,
        }
        .encode();
        let skip = TaskDirective::Skip { height }.encode();
        let deadline = Instant::now() + self.round_timeout;
        let mut skipping = false;
        // Latches once the height certifies, so the post-certification wait for the
        // submitter's resolution (execution may take minutes) stops re-polling the
        // reporter actor every tick.
        let mut certified = false;

        self.broadcast(announce.clone());
        loop {
            tokio::select! {
                resolution = self.resolutions.recv() => {
                    let Some(resolution) = resolution else {
                        return HeightOutcome::Closed;
                    };
                    if resolution.height == height {
                        return HeightOutcome::Resolved(resolution.kind);
                    }
                    // A different height resolved (pre-restart leftover certified
                    // while we drive ours) — never assign it again.
                    debug!(
                        height = resolution.height,
                        kind = ?resolution.kind,
                        "observed resolution for another height while driving"
                    );
                    self.next_height = self.next_height.max(resolution.height + 1);
                }
                _ = tokio::time::sleep(self.rebroadcast_interval) => {
                    // Once certified, the directive is moot — the submitter's
                    // resolution is all that remains (execution may take minutes;
                    // don't broadcast Skip for an already-certified height). Poll the
                    // reporter only until it certifies, then latch and idle.
                    if certified || self.reporter.get(height).await.is_some() {
                        certified = true;
                        continue;
                    }
                    // Nodes past this height will never sign it (their engines
                    // only propose at-or-above their tips) — stop driving it.
                    if self.tip_reports.safe_tip() > height {
                        return HeightOutcome::Superseded;
                    }
                    if !skipping && Instant::now() >= deadline {
                        warn!(
                            height,
                            timeout_secs = self.round_timeout.as_secs_f64(),
                            "no certificate before round timeout, broadcasting skip"
                        );
                        skipping = true;
                    }
                    let directive = if skipping { skip.clone() } else { announce.clone() };
                    self.broadcast(directive);
                }
            }
        }
    }

    /// Broadcasts an encoded [`TaskDirective`] to the operator set on channel 1.
    ///
    /// Uses `Recipients::Some(self.recipients)` — the explicit participant keys —
    /// rather than `Recipients::All`, whose connected-peer snapshot stays empty on
    /// the router's send side (see the `recipients` field). An empty result
    /// (everyone rate-limited or momentarily disconnected) is expected transiently;
    /// the rebroadcast loop retries.
    fn broadcast(&mut self, message: bytes::Bytes) {
        let sent =
            self.directive_sender
                .send(Recipients::Some(self.recipients.clone()), message, true);
        if sent.is_empty() {
            debug!("directive broadcast reached no peers (will rebroadcast)");
        }
    }

    /// Blocks until a task arrives, maintaining the queue-depth metric.
    ///
    /// Returns `None` when the ingress side of the channel closed.
    async fn wait_for_task(&mut self) -> Option<GasKillerTaskRequest> {
        let task = self.receiver.recv().await?;
        let depth = self
            .queue_depth
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            })
            .unwrap_or(0)
            .saturating_sub(1);
        if let Some(m) = &self.metrics {
            m.task_queue_depth.set(depth as i64);
        }
        Some(task)
    }

    /// Computes storage updates (EVMSketch) and resolves the transition index for a
    /// dequeued task. Lifted nearly verbatim from the old creator.
    async fn enrich(&self, task: GasKillerTaskRequest) -> Result<EnrichedTask> {
        info!(
            target = format!("{:?}", task.body.target_address),
            from = format!("{:?}", task.body.from_address),
            transition_index = ?task.body.transition_index,
            call_data_len = task.body.call_data.len(),
            "Sequencer received task"
        );

        if let Some(m) = &self.metrics {
            m.tasks_created.inc();
        }

        debug!(
            "Computing storage updates for target {}",
            task.body.target_address
        );

        // For explicit indices, run storage computation alone (count not needed).
        // For auto mode, run stateTransitionCount() concurrently with EVMSketch: the
        // count RPC call (~200ms) is fully hidden behind EVMSketch (seconds), so the
        // auto path adds zero observable latency compared to the explicit-index path.
        let (
            storage_updates,
            block_height,
            anchor_hash,
            numeric_chain_id,
            resolved_transition_index,
            storage_elapsed,
        ) = if let Some(idx) = task.body.transition_index {
            let start = Instant::now();
            // compute_storage_updates_for_tx detects the chain, runs EVMSketch, and also
            // calls eth_chainId on the same RPC — returns the numeric chain ID directly.
            let (updates, height, anchor_hash, chain_id) = self
                .validator
                .compute_storage_updates_for_tx(
                    task.body.target_address,
                    &task.body.call_data,
                    Some(task.body.from_address),
                    Some(task.body.value),
                    task.body.block_height,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Failed to compute storage updates: {}", e))?;
            (updates, height, anchor_hash, chain_id, idx, start.elapsed())
        } else {
            // Detect chain once so all concurrent futures skip redundant eth_getCode probes.
            let chain_role = self
                .validator
                .detect_chain_for_address(task.body.target_address)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to detect chain: {}", e))?;
            let rpc_url = self
                .validator
                .rpc_url_for_chain(chain_role)
                .ok_or_else(|| anyhow::anyhow!("No RPC URL for chain {}", chain_role))?
                .to_owned();

            info!(
                target_address = %task.body.target_address,
                chain = %chain_role,
                "Resolving auto transition_index concurrently with EVMSketch"
            );

            let count_validator = Arc::clone(&self.validator);
            let chain_id_validator = Arc::clone(&self.validator);
            let target = task.body.target_address;
            let count_fut = async move {
                count_validator
                    .get_state_transition_count_on_chain(target, chain_role)
                    .await
            };
            let storage_fut = async {
                let start = Instant::now();
                self.validator
                    .analyze_transaction(
                        &rpc_url,
                        task.body.target_address,
                        &task.body.call_data,
                        Some(task.body.from_address),
                        Some(task.body.value),
                        task.body.block_height,
                    )
                    .await
                    .map(|r| {
                        (
                            r.storage_updates,
                            r.block_height,
                            r.anchor_hash,
                            start.elapsed(),
                        )
                    })
            };
            // eth_chainId runs concurrently — completes in ~50ms, well before EVMSketch.
            let chain_id_fut = async move { chain_id_validator.get_chain_id_for(chain_role).await };
            let (count, (updates, height, anchor_hash, storage_elapsed), chain_id) =
                tokio::try_join!(count_fut, storage_fut, chain_id_fut)?;

            info!(
                target_address = %task.body.target_address,
                chain = %chain_role,
                count,
                "Resolved auto transition_index from chain"
            );
            (
                updates,
                height,
                anchor_hash,
                chain_id,
                count,
                storage_elapsed,
            )
        };

        if let Some(m) = &self.metrics {
            m.storage_computation_seconds
                .observe(storage_elapsed.as_secs_f64());
        }

        // Debug: Log hash of full storage_updates to detect differences vs validators
        let mut storage_hasher = Sha256::new();
        storage_hasher.update(&storage_updates);
        let storage_hash = storage_hasher.finalize();
        let storage_hash_hex = hex::encode(&storage_hash[..8]);
        info!(
            storage_updates_len = storage_updates.len(),
            storage_updates_hash = %storage_hash_hex,
            block_height = block_height,
            transition_index = resolved_transition_index,
            target_address = %task.body.target_address,
            target_function = %task.body.call_data.get(..4).map(hex::encode).unwrap_or_default(),
            chain_id = numeric_chain_id,
            "Sequencer computed storage updates"
        );

        Ok(EnrichedTask {
            task,
            storage_updates: storage_updates.into(),
            block_height,
            anchor_hash,
            transition_index: resolved_transition_index,
            chain_id: numeric_chain_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, U256};

    #[test]
    fn test_dispatch_time_evicts_failed_heights_and_isolates_measurements() {
        let times: DispatchTime = Arc::new(Mutex::new(HashMap::new()));

        // Height 0 is dispatched but never certifies/executes, so it is never consumed.
        stamp_dispatch_time(&times, 0);
        assert_eq!(times.lock().unwrap().len(), 1);

        // Dispatching height 1 evicts the stale height-0 entry rather than letting it
        // accumulate or bleed into height 1's measurement.
        stamp_dispatch_time(&times, 1);
        {
            let map = times.lock().unwrap();
            assert_eq!(map.len(), 1, "stale failed-height entry should be evicted");
            assert!(map.contains_key(&1));
            assert!(!map.contains_key(&0));
        }

        // The executor consumes height 1's own timestamp exactly once; the failed
        // height 0 is gone.
        assert!(take_dispatch_time(&times, 0).is_none());
        assert!(take_dispatch_time(&times, 1).is_some());
        assert!(take_dispatch_time(&times, 1).is_none());
        assert!(times.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_channel_send_recv() {
        let (sender, mut receiver) = task_channel();
        let task = GasKillerTaskRequest {
            body: crate::ingress::GasKillerTaskRequestBody {
                target_address: Address::from([1u8; 20]),
                call_data: vec![0x12, 0x34, 0x56, 0x78],
                transition_index: Some(1),
                from_address: Address::from([2u8; 20]),
                value: U256::from(1000),
                block_height: 12345,
            },
        };

        sender.send(task.clone()).unwrap();
        let received = receiver.try_recv().unwrap();
        assert_eq!(received.body.transition_index, Some(1));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn test_task_data_from_request() {
        let task = GasKillerTaskRequest {
            body: crate::ingress::GasKillerTaskRequestBody {
                target_address: Address::from([1u8; 20]),
                call_data: vec![0x12, 0x34, 0x56, 0x78],
                transition_index: Some(42),
                from_address: Address::from([2u8; 20]),
                value: U256::from(1000),
                block_height: 12345,
            },
        };

        let enriched = EnrichedTask {
            task,
            storage_updates: vec![0x01, 0x02, 0x03, 0x04].into(), // computed by GasAnalyzer
            block_height: 12345,
            anchor_hash: B256::from([7u8; 32]),
            transition_index: 42,
            chain_id: 1u64,
        };
        let task_data = enriched.into_task_data();

        assert_eq!(task_data.transition_index, 42);
        assert_eq!(task_data.target_address, Address::from([1u8; 20]));
        assert_eq!(task_data.anchor_hash, B256::from([7u8; 32]));
        assert_eq!(task_data.chain_id, 1);
    }

    #[test]
    fn test_assignment_map_shares_digest_and_task() {
        let assignments = shared_assignments();
        let task = GasKillerTaskData {
            storage_updates: vec![0x01].into(),
            transition_index: 3,
            target_address: Address::from([9u8; 20]),
            call_data: vec![1, 2, 3, 4],
            from_address: Address::from([8u8; 20]),
            value: U256::ZERO,
            block_height: 77,
            anchor_hash: B256::from([5u8; 32]),
            chain_id: 31337,
        };
        let digest = task.build_payload_hash(&task.storage_updates);
        assignments.write().unwrap().insert(
            5,
            Assignment {
                digest,
                task: task.clone(),
            },
        );

        let read = assignments.read().unwrap();
        let assignment = read.get(&5).expect("assignment present");
        assert_eq!(assignment.digest, digest);
        assert_eq!(assignment.task.transition_index, 3);
        assert!(read.get(&6).is_none());
    }
}

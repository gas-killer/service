//! Scheduler: turns ingress tasks into signing sessions and settles each.
//!
//! A session is a height, the task, and its router trace. The height only names the session:
//! the nodes key their nonce state by `(height, attempt)`, so it must never repeat within or
//! across router lives. Every height is handed out at or below the wall clock in milliseconds
//! ([`clock_tip`]), which is where the next router life starts.
//!
//! Up to `MAX_IN_FLIGHT_TASKS` sessions run at once, but a contract is only ever pinned by one
//! of them. A settlement pins every contract whose transition counter it moves: the target, and
//! for a nested tree every frame contract. A task starts holding its target; its session's sign
//! gate takes the rest once the router's trace names them, all or nothing, and defers the task
//! when one is busy. A rendered task keeps its locks until its payload lands or can no longer
//! land, so the next task touching any of those contracts is traced against the state the first
//! one leaves behind. A task that waited for a lock is traced at head rather than at its
//! client's block.

use crate::height_metrics::HeightObserver;
use crate::metrics::{MetricsCollector, operator_labels};
use crate::schnorr_coordinator::{Permit, SchnorrCoordinator, SessionOutcome, SignGate, clock_tip};
use crate::schnorr_submitter::SchnorrSubmitter;
use crate::sequencer::{DispatchedTask, QueuedTask, TaskDispatcher, TaskQueue, Traced};
use crate::store::SqliteStore;
use alloy_primitives::Address;
use anyhow::Result;
use commonware_avs_core::bn254::PublicKey;
use commonware_p2p::Sender;
use gas_killer_common::{ChainRole, GasKillerValidator, TaskBundle};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinSet;
use tracing::{error, info, warn};

/// How often a rendered task's locks check whether its payload landed or expired.
const LOCK_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How far back a restart looks for rendered tasks whose locks it restores. Far longer than any
/// payload's validity window; older tasks are certain to have landed or expired.
const RESTORE_WINDOW: Duration = Duration::from_secs(6 * 60 * 60);

/// Longest the scheduler waits for the clock to reach the next height. Only a burst of sessions
/// that each start within a millisecond waits at all; a gap this large means the wall clock
/// stepped backwards, and waiting it out would stall the pipeline for as long.
const MAX_CLOCK_WAIT: Duration = Duration::from_secs(1);

/// How long to wait before handing out `next` when the clock reads `now`, or `None` when the
/// gap is too large to wait out.
fn clock_wait(next: u64, now: u64) -> Option<Duration> {
    let ahead = Duration::from_millis(next.saturating_sub(now));
    (ahead <= MAX_CLOCK_WAIT).then_some(ahead)
}

/// Session heights, shared by every live session.
struct Heights {
    next: Mutex<u64>,
}

impl Heights {
    fn starting_at_the_clock() -> Self {
        let next = clock_tip();
        info!(tip = next, "schnorr heights start at the clock");
        Self {
            next: Mutex::new(next),
        }
    }

    /// The next session's height: never below the previous one, and not above the clock, so a
    /// restarted router (which starts at the clock) is above every height this life used. A
    /// backwards clock step larger than [`MAX_CLOCK_WAIT`] breaks the second property until the
    /// clock catches up; stalling for the step instead would stop the pipeline.
    async fn take(&self) -> u64 {
        let mut next = self.next.lock().await;
        let now = clock_tip();
        match clock_wait(*next, now) {
            Some(wait) if !wait.is_zero() => tokio::time::sleep(wait).await,
            Some(_) => {}
            None => warn!(
                next_height = *next,
                clock = now,
                "session heights are ahead of the clock; the wall clock stepped backwards"
            ),
        }
        let height = (*next).max(clock_tip());
        *next = height + 1;
        height
    }
}

/// Contracts pinned by live tasks, by the id of the task pinning each.
///
/// Beyond the transition index, a lock keeps two live sessions from ever signing the same
/// digest: it binds only `(transitionIndex, target, selector, storageUpdates)`, so two tasks for
/// one target resolved against the same state could share one.
#[derive(Default)]
struct LockTable {
    held: HashMap<Address, String>,
}

impl LockTable {
    /// Takes every contract in `wanted` for `owner`, or none of them if any is held by another
    /// task. Taking all at once is what keeps two tasks wanting overlapping sets from each
    /// holding part and waiting on the other.
    fn try_take(&mut self, owner: &str, wanted: &BTreeSet<Address>) -> bool {
        if wanted
            .iter()
            .any(|contract| self.held.get(contract).is_some_and(|held| held != owner))
        {
            return false;
        }
        for contract in wanted {
            self.held.insert(*contract, owner.to_owned());
        }
        true
    }

    fn release(&mut self, owner: &str) {
        self.held.retain(|_, held| held != owner);
    }
}

/// The lock table, shared by the scheduler, each session's sign gate, and the watchers holding
/// rendered tasks' locks until their payloads land.
#[derive(Clone, Default)]
pub(crate) struct Locks {
    table: Arc<std::sync::Mutex<LockTable>>,
    released: Arc<Notify>,
}

impl Locks {
    fn try_take(&self, owner: &str, wanted: &BTreeSet<Address>) -> bool {
        self.table
            .lock()
            .expect("lock table poisoned")
            .try_take(owner, wanted)
    }

    fn release(&self, owner: &str) {
        self.table
            .lock()
            .expect("lock table poisoned")
            .release(owner);
        self.released.notify_one();
    }
}

/// How a session left its task.
pub(crate) struct SessionEnd {
    /// `false` when the schnorr channel closed.
    keep_going: bool,
    /// The task, back for another session once every contract it records is free.
    requeue: Option<QueuedTask>,
    /// The task's locks outlive the session, released by whatever it handed them to.
    holds_locks: bool,
}

impl SessionEnd {
    fn settled() -> Self {
        Self {
            keep_going: true,
            requeue: None,
            holds_locks: false,
        }
    }

    fn channel_closed() -> Self {
        Self {
            keep_going: false,
            ..Self::settled()
        }
    }
}

/// Dequeued tasks waiting for their locks, oldest first.
#[derive(Default)]
struct Waiting {
    tasks: VecDeque<QueuedTask>,
}

impl Waiting {
    fn hold(&mut self, task: QueuedTask) {
        self.tasks.push_back(task);
    }

    /// A deferred task goes ahead of everything that arrived after it.
    fn hold_first(&mut self, task: QueuedTask) {
        self.tasks.push_front(task);
    }

    /// The oldest waiting task whose starting locks are all free, taking them. A task passed
    /// over is marked to re-anchor: by the time it starts, its client's block may predate
    /// whatever the task ahead of it settled.
    ///
    /// A contract an older waiting task wants is off limits to younger ones, or a tree pinning
    /// several contracts would rarely find them all free at once behind a stream of tasks that
    /// each want one of them.
    fn next_ready(&mut self, locks: &Locks) -> Option<QueuedTask> {
        let mut table = locks.table.lock().expect("lock table poisoned");
        let mut reserved = BTreeSet::new();
        for index in 0..self.tasks.len() {
            let task = &mut self.tasks[index];
            let wanted = task.starting_locks();
            if wanted.is_disjoint(&reserved) && table.try_take(&task.task_id, &wanted) {
                return self.tasks.remove(index);
            }
            task.reanchor = true;
            reserved.extend(wanted);
        }
        None
    }
}

/// Runs `session` for each task from `queue`, at most `max_in_flight` at once and never two
/// pinning the same contract, until the ingress side closes and every session has ended. A
/// session reporting the schnorr channel closed stops anything new from starting, and the live
/// sessions are drained rather than aborted, so one that already signed still settles its task.
async fn schedule<F, Fut>(mut queue: TaskQueue, max_in_flight: usize, locks: Locks, session: F)
where
    F: Fn(QueuedTask) -> Fut,
    Fut: Future<Output = SessionEnd> + Send + 'static,
{
    let mut waiting = Waiting::default();
    let mut live = JoinSet::new();
    let mut owners = HashMap::new();
    let mut ingress_open = true;
    let mut channel_closed = false;
    loop {
        while !channel_closed
            && live.len() < max_in_flight
            && let Some(task) = waiting.next_ready(&locks)
        {
            queue.started();
            owners.insert(live.spawn(session(task.clone())).id(), task.task_id);
        }
        if live.is_empty() && (channel_closed || !ingress_open) {
            if channel_closed {
                info!("schnorr channel closed; scheduler exiting");
            } else {
                info!("task channel closed; scheduler exiting");
            }
            return;
        }
        tokio::select! {
            Some(joined) = live.join_next_with_id(), if !live.is_empty() => {
                let (id, end) = match joined {
                    Ok(ended) => ended,
                    // The task stays `processing` until the next router life re-queues it; its
                    // locks are freed so its contracts are not blocked for the rest of this one.
                    Err(panicked) => {
                        error!(error = %panicked, "signing session panicked");
                        (panicked.id(), SessionEnd::settled())
                    }
                };
                if let Some(owner) = owners.remove(&id)
                    && !end.holds_locks
                {
                    locks.release(&owner);
                }
                if let Some(task) = end.requeue {
                    queue.requeued();
                    waiting.hold_first(task);
                }
                let keep_going = end.keep_going;
                if !keep_going && !channel_closed {
                    // Sessions still signing see their inbox closed and end at once; only a
                    // session that already signed keeps the drain waiting, while it settles.
                    info!(live = live.len(), "schnorr channel closed; draining live sessions");
                    channel_closed = true;
                }
            }
            queued = queue.next(), if !channel_closed && ingress_open && live.len() < max_in_flight => {
                match queued {
                    Some(task) => waiting.hold(task),
                    None => ingress_open = false,
                }
            }
            // A watcher let go of a landed task's locks; something waiting may start now.
            _ = locks.released.notified() => {}
        }
    }
}

pub struct Scheduler<S>
where
    S: Sender<PublicKey = PublicKey>,
{
    queue: TaskQueue,
    sessions: Sessions<S>,
    max_in_flight: usize,
}

impl<S> Scheduler<S>
where
    S: Sender<PublicKey = PublicKey>,
{
    pub fn new(
        queue: TaskQueue,
        dispatcher: TaskDispatcher,
        coordinator: SchnorrCoordinator<S>,
        submitter: SchnorrSubmitter,
        observer: HeightObserver,
        metrics: Arc<MetricsCollector>,
        max_in_flight: usize,
    ) -> Self {
        info!(max_in_flight, "scheduler running");
        Self {
            queue,
            sessions: Sessions {
                dispatcher,
                coordinator,
                submitter,
                observer,
                metrics,
                heights: Arc::new(Heights::starting_at_the_clock()),
                locks: Locks::default(),
            },
            max_in_flight: max_in_flight.max(1),
        }
    }

    /// Re-takes the locks of tasks a previous router life rendered whose payloads may still
    /// land, so no new task traces against state one of them is about to change. Run before
    /// [`Self::run`].
    pub async fn restore_held_locks(&self, store: &SqliteStore) {
        let bundles = match store.recent_ready_bundles(RESTORE_WINDOW).await {
            Ok(bundles) => bundles,
            Err(e) => {
                error!(error = %e, "could not read rendered tasks; their locks are not restored");
                return;
            }
        };
        let validator = Arc::clone(self.sessions.dispatcher.validator());
        let mut restored = 0usize;
        for (task_id, bundle) in bundles {
            let bundle: TaskBundle = match serde_json::from_str(&bundle) {
                Ok(bundle) => bundle,
                Err(e) => {
                    warn!(task_id, error = %e, "unreadable bundle; its locks are not restored");
                    continue;
                }
            };
            let chain = match validator
                .detect_chain_for_address(bundle.target_address)
                .await
            {
                Ok(chain) => chain,
                Err(e) => {
                    warn!(task_id, error = %e, "unknown chain; its locks are not restored");
                    continue;
                }
            };
            let (pinned, until) = bundle_hold(&bundle);
            if validator
                .chain_head(chain)
                .await
                .is_ok_and(|head| head > until)
            {
                continue;
            }
            // Rendered tasks pinned disjoint sets when they were signed, so this only fails for
            // a set a later task already took, which then holds the newer state anyway.
            if !self.sessions.locks.try_take(&task_id, &pinned) {
                continue;
            }
            tokio::spawn(hold_until_landed(
                Held {
                    locks: self.sessions.locks.clone(),
                    validator: Arc::clone(&validator),
                    task_id,
                    root: bundle.target_address,
                    chain,
                    transition_index: bundle.transition_index,
                    until,
                },
                LOCK_POLL_INTERVAL,
            ));
            restored += 1;
        }
        info!(
            restored,
            "restored the locks of rendered tasks from a previous router life"
        );
    }

    /// Runs sessions until the ingress side of the task channel closes.
    pub async fn run(self) {
        let sessions = self.sessions;
        let locks = sessions.locks.clone();
        schedule(self.queue, self.max_in_flight, locks, move |task| {
            sessions.clone().run(task)
        })
        .await;
    }
}

/// Everything one session needs. Each session runs on its own clone.
struct Sessions<S>
where
    S: Sender<PublicKey = PublicKey>,
{
    dispatcher: TaskDispatcher,
    coordinator: SchnorrCoordinator<S>,
    submitter: SchnorrSubmitter,
    observer: HeightObserver,
    metrics: Arc<MetricsCollector>,
    heights: Arc<Heights>,
    locks: Locks,
}

impl<S> Clone for Sessions<S>
where
    S: Sender<PublicKey = PublicKey>,
{
    fn clone(&self) -> Self {
        Self {
            dispatcher: self.dispatcher.clone(),
            coordinator: self.coordinator.clone(),
            submitter: self.submitter.clone(),
            observer: self.observer.clone(),
            metrics: Arc::clone(&self.metrics),
            heights: Arc::clone(&self.heights),
            locks: self.locks.clone(),
        }
    }
}

impl<S> Sessions<S>
where
    S: Sender<PublicKey = PublicKey>,
{
    /// Dispatches, signs and settles one task.
    async fn run(self, task: QueuedTask) -> SessionEnd {
        let Some(dispatched) = self.dispatcher.dispatch(task.clone()).await else {
            return SessionEnd::settled();
        };
        self.sign_and_settle(dispatched, task).await
    }

    async fn sign_and_settle(
        mut self,
        dispatched: DispatchedTask,
        queued: QueuedTask,
    ) -> SessionEnd {
        let height = self.heights.take().await;
        let started = Instant::now();
        self.observer.start(height, started);
        info!(
            height,
            task_id = dispatched.task_id,
            "assigned task to height"
        );

        // A closed channel means the process is going down: the task stays `processing` and
        // the next router life re-queues it, instead of failing a task that did nothing wrong.
        let mut gate = TaskGate {
            locks: self.locks.clone(),
            validator: Arc::clone(self.dispatcher.validator()),
            task_id: dispatched.task_id.clone(),
            root: dispatched.task.target_address,
            chain: dispatched.chain,
            block_height: dispatched.task.block_height,
            transition_index: dispatched.task.transition_index,
        };
        let Some(outcome) = self
            .coordinator
            .drive_height(
                height,
                &dispatched.task,
                dispatched.nested,
                &dispatched.trace,
                &mut gate,
            )
            .await
        else {
            info!(
                height,
                task_id = dispatched.task_id,
                "schnorr channel closed; session abandoned"
            );
            return SessionEnd::channel_closed();
        };
        let deferred = match &outcome {
            SessionOutcome::Deferred(lock_set) => Some(lock_set.clone()),
            _ => None,
        };
        if let SessionOutcome::Signed {
            non_signers,
            straggler_wait,
            ..
        } = &outcome
        {
            self.metrics
                .p2p_round_trip_seconds
                .observe(started.elapsed().as_secs_f64());
            self.metrics
                .straggler_wait_seconds
                .observe(straggler_wait.as_secs_f64());
            for operator in non_signers {
                self.metrics
                    .non_signers
                    .get_or_create(&operator_labels(*operator))
                    .inc();
            }
        }
        let (settled, valid_until) = self
            .submitter
            .settle(height, &dispatched, started, outcome)
            .await;
        info!(
            height,
            task_id = dispatched.task_id,
            outcome = settled.as_str(),
            "session settled"
        );
        self.observer.finish(height, settled);

        if let Some(lock_set) = deferred {
            return SessionEnd {
                requeue: Some(QueuedTask {
                    lock_set,
                    reanchor: true,
                    ..queued
                }),
                ..SessionEnd::settled()
            };
        }
        let Some(valid_until) = valid_until else {
            return SessionEnd::settled();
        };
        // A submitter picks the reference block, so a tree can land until its signed expiry
        // even after the rendered validity a payload that keeps its reference block observes.
        let until = dispatched
            .nested
            .map_or(valid_until, |spec| spec.expiry_block.max(valid_until));
        tokio::spawn(hold_until_landed(
            Held {
                locks: gate.locks,
                validator: gate.validator,
                task_id: gate.task_id,
                root: gate.root,
                chain: gate.chain,
                transition_index: gate.transition_index,
                until,
            },
            LOCK_POLL_INTERVAL,
        ));
        SessionEnd {
            holds_locks: true,
            ..SessionEnd::settled()
        }
    }
}

/// A session's sign gate: takes every contract the router's trace says the settlement pins,
/// then checks the trace still describes each one's state.
struct TaskGate {
    locks: Locks,
    validator: Arc<GasKillerValidator>,
    task_id: String,
    root: Address,
    chain: ChainRole,
    block_height: u64,
    transition_index: u64,
}

impl SignGate for TaskGate {
    async fn permit(&mut self, traced: &Traced) -> Permit {
        let pinned = traced.counter_moves(self.root);
        if !self.locks.try_take(&self.task_id, &pinned) {
            return Permit::Deferred(pinned);
        }
        match self.check_fresh(&pinned).await {
            Ok(Ok(())) => Permit::Granted,
            Ok(Err(stale)) => Permit::Refused(stale),
            // Nothing was learned about the task, so it is tried again rather than failed.
            Err(e) => {
                warn!(task_id = self.task_id, error = %e, "could not check the trace is current; deferring");
                Permit::Deferred(pinned)
            }
        }
    }
}

impl TaskGate {
    /// A trace taken at `block_height` signs a program built on that block's state, and every
    /// frame's tracker write restores the count it saw there. Only while that count is still
    /// each contract's count at head does the program describe what settling would change.
    ///
    /// Run with every lock held, so no task this router signs can move a count in between.
    /// The outer error is a failed read; the inner one is a trace that is no longer current.
    async fn check_fresh(&self, pinned: &BTreeSet<Address>) -> Result<Result<(), String>> {
        for &contract in pinned {
            let (traced_at, head) = tokio::try_join!(
                self.validator
                    .state_transition_count_at(contract, self.chain, self.block_height),
                self.validator
                    .get_state_transition_count_on_chain(contract, self.chain),
            )?;
            if traced_at != head {
                return Ok(Err(format!(
                    "{contract} settled {} transition(s) after block {}, which the task was \
                     traced at; resubmit the task at a later block",
                    head.saturating_sub(traced_at),
                    self.block_height
                )));
            }
            if contract == self.root && traced_at != self.transition_index {
                return Ok(Err(format!(
                    "the target is at transition {traced_at}, but the task settles transition {}",
                    self.transition_index
                )));
            }
        }
        Ok(Ok(()))
    }
}

/// A rendered task's locks, held until its payload can no longer land.
struct Held {
    locks: Locks,
    validator: Arc<GasKillerValidator>,
    task_id: String,
    root: Address,
    chain: ChainRole,
    transition_index: u64,
    /// The last block the payload can land in.
    until: u64,
}

/// Holds `held`'s locks until the root's count passes the task's transition (it landed) or the
/// chain passes `until` (it never will). A failed read just waits for the next poll: releasing
/// early is what would let a second task trace against state the first is about to change.
async fn hold_until_landed(held: Held, poll: Duration) {
    loop {
        tokio::time::sleep(poll).await;
        let Ok(head) = held.validator.chain_head(held.chain).await else {
            continue;
        };
        // Read at head, the block a waiting task is re-anchored to, so a release is never ahead
        // of what that task's trace can see. A chain that mines only on demand may not produce
        // another block for a while, so waiting for a later one could hold the lock forever.
        let count = held
            .validator
            .state_transition_count_at(held.root, held.chain, head)
            .await;
        if count.is_ok_and(|count| count > held.transition_index) {
            info!(
                task_id = held.task_id,
                "payload landed; releasing its locks"
            );
            break;
        }
        if head > held.until {
            info!(
                task_id = held.task_id,
                until = held.until,
                "payload expired; releasing its locks"
            );
            break;
        }
    }
    held.locks.release(&held.task_id);
}

/// The contracts a rendered bundle pins, and the last block its payload can land in.
fn bundle_hold(bundle: &TaskBundle) -> (BTreeSet<Address>, u64) {
    let mut pinned = BTreeSet::from([bundle.target_address]);
    let mut until = bundle.valid_until_block;
    if let Some(nested) = &bundle.nested {
        pinned.extend(nested.frames.iter().map(|frame| frame.contract));
        until = until.max(nested.expiry_block);
    }
    (pinned, until)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::GasKillerHandler;
    use crate::schnorr_coordinator::testing::{CountingSender, closed_coordinator};
    use crate::sequencer::{RouterTrace, task_channel, task_queue_depth};
    use crate::store::{SqliteStore, TaskStatus};
    use alloy_primitives::U256;
    use alloy_provider::ProviderBuilder;
    use alloy_provider::fillers::{
        BlobGasFiller, ChainIdFiller, GasFiller, JoinFill, NonceFiller, SimpleNonceManager,
    };
    use alloy_signer_local::PrivateKeySigner;
    use gas_killer_common::{GasKillerTaskData, GasKillerValidator};
    use std::collections::HashMap;

    /// An RPC nothing listens on: the paths under test never reach the chain.
    const OFFLINE_RPC: &str = "http://localhost:1";

    fn offline_submitter(store: SqliteStore) -> SchnorrSubmitter {
        let url = url::Url::parse(OFFLINE_RPC).unwrap();
        let wallet = ProviderBuilder::default()
            .filler(JoinFill::new(
                GasFiller,
                JoinFill::new(
                    BlobGasFiller::default(),
                    JoinFill::new(
                        NonceFiller::<SimpleNonceManager>::default(),
                        ChainIdFiller::default(),
                    ),
                ),
            ))
            .wallet(PrivateKeySigner::random())
            .connect_http(url.clone());
        SchnorrSubmitter::new(
            ProviderBuilder::new().connect_http(url),
            GasKillerHandler::new(1, wallet).with_store(store),
        )
    }

    /// Shutting down mid-session must leave the task `processing` for the next router life to
    /// re-queue, not fail it.
    #[tokio::test]
    async fn a_closed_channel_stops_the_scheduler_without_settling_the_task() {
        let store = SqliteStore::connect_in_memory().await.unwrap();
        let key = store.create_api_key(None, None).await.unwrap().id;
        let body = crate::ingress::GasKillerTaskRequestBody {
            target_address: Address::from([0x11; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78],
            transition_index: Some(0),
            from_address: Address::from([0x22; 20]),
            value: U256::ZERO,
            block_height: 1,
        };
        let task = store.create_task(&key, &body).await.unwrap();
        assert!(store.claim_task_for_processing(&task.id).await.unwrap());

        let metrics = Arc::new(MetricsCollector::new());
        let session = Sessions {
            dispatcher: TaskDispatcher::new(
                Arc::new(GasKillerValidator::with_rpc_url(OFFLINE_RPC)),
                None,
                Some(store.clone()),
            ),
            coordinator: closed_coordinator(CountingSender::default()),
            submitter: offline_submitter(store.clone()),
            observer: HeightObserver::new(Arc::clone(&metrics)),
            metrics: Arc::clone(&metrics),
            heights: Arc::new(Heights::starting_at_the_clock()),
            locks: Locks::default(),
        };

        let dispatched = DispatchedTask {
            task_id: task.id.clone(),
            task: GasKillerTaskData::default(),
            nested: None,
            chain: ChainRole::L1,
            trace: RouterTrace::spawn(std::future::pending()),
        };
        let queued = QueuedTask::new(
            task.id.clone(),
            crate::ingress::GasKillerTaskRequest { body },
        );
        let end = tokio::time::timeout(
            Duration::from_secs(1),
            session.sign_and_settle(dispatched, queued),
        )
        .await
        .expect("a closed channel must end the session at once");

        assert!(!end.keep_going);
        assert_eq!(
            store.get_task(&task.id).await.unwrap().unwrap().status,
            TaskStatus::Processing
        );
        assert!(
            !metrics
                .encode()
                .contains("gas_killer_height_outcomes_total{")
        );
    }

    /// How a fake session ends.
    enum Ending {
        Settled,
        ChannelClosed,
        Panic,
        /// Rendered: its locks stay held until the test releases them.
        Holding,
        /// Deferred, to start again once every contract in the set is free.
        Deferred(&'static [u8]),
    }

    /// Fake sessions for [`schedule`]: each records that it started, then runs until the test
    /// ends it.
    #[derive(Clone, Default)]
    struct Harness {
        started: Arc<std::sync::Mutex<Vec<String>>>,
        reanchored: Arc<std::sync::Mutex<Vec<String>>>,
        endings: Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<Ending>>>>,
        locks: Locks,
    }

    impl Harness {
        fn session(
            &self,
        ) -> impl Fn(QueuedTask) -> std::pin::Pin<Box<dyn Future<Output = SessionEnd> + Send>> + use<>
        {
            let harness = self.clone();
            move |task| {
                let harness = harness.clone();
                Box::pin(async move {
                    let (end, ending) = tokio::sync::oneshot::channel::<Ending>();
                    harness
                        .endings
                        .lock()
                        .unwrap()
                        .insert(task.task_id.clone(), end);
                    if task.reanchor {
                        harness
                            .reanchored
                            .lock()
                            .unwrap()
                            .push(task.task_id.clone());
                    }
                    harness.started.lock().unwrap().push(task.task_id.clone());
                    match ending.await {
                        Ok(Ending::Settled) | Err(_) => SessionEnd::settled(),
                        Ok(Ending::ChannelClosed) => SessionEnd::channel_closed(),
                        Ok(Ending::Panic) => panic!("session panicked"),
                        Ok(Ending::Holding) => SessionEnd {
                            holds_locks: true,
                            ..SessionEnd::settled()
                        },
                        Ok(Ending::Deferred(set)) => SessionEnd {
                            requeue: Some(QueuedTask {
                                lock_set: set.iter().map(|b| Address::from([*b; 20])).collect(),
                                reanchor: true,
                                ..task
                            }),
                            ..SessionEnd::settled()
                        },
                    }
                })
            }
        }

        fn schedule(&self, queue: TaskQueue, max_in_flight: usize) -> tokio::task::JoinHandle<()> {
            tokio::spawn(schedule(
                queue,
                max_in_flight,
                self.locks.clone(),
                self.session(),
            ))
        }

        fn end(&self, task_id: &str, ending: Ending) {
            let end = self.endings.lock().unwrap().remove(task_id);
            let _ = end.expect("session is running").send(ending);
        }

        /// Waits for exactly `expected` to have started, then checks nothing else starts.
        async fn assert_started(&self, expected: &[&str]) {
            let deadline = Instant::now() + Duration::from_secs(1);
            while self.started.lock().unwrap().len() < expected.len() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(*self.started.lock().unwrap(), expected);
        }
    }

    fn queued(task_id: &str, target: u8) -> QueuedTask {
        QueuedTask::new(
            task_id.to_owned(),
            crate::ingress::GasKillerTaskRequest {
                body: crate::ingress::GasKillerTaskRequestBody {
                    target_address: Address::from([target; 20]),
                    call_data: vec![],
                    transition_index: Some(0),
                    from_address: Address::ZERO,
                    value: U256::ZERO,
                    block_height: 1,
                },
            },
        )
    }

    fn contracts(bytes: &[u8]) -> BTreeSet<Address> {
        bytes.iter().map(|b| Address::from([*b; 20])).collect()
    }

    #[test]
    fn a_restored_tree_holds_every_frame_until_its_signed_expiry() {
        use gas_killer_common::{BundleProof, NestedBundle, NestedFrame};

        let mut bundle = TaskBundle {
            msg_hash: alloy_primitives::B256::ZERO,
            reference_block_number: 99,
            transition_index: 3,
            target_address: Address::from([1; 20]),
            target_function: alloy_primitives::FixedBytes::ZERO,
            storage_updates: alloy_primitives::Bytes::new(),
            chain_id: 1,
            value: U256::ZERO,
            valid_until_block: 119,
            proof: BundleProof::Retired,
            nested: None,
        };
        assert_eq!(bundle_hold(&bundle), (contracts(&[1]), 119));

        bundle.nested = Some(NestedBundle {
            expiry_block: 140,
            expiry_proof: vec![],
            proof: vec![],
            children: vec![],
            frames: [1, 2, 3]
                .map(|b| NestedFrame {
                    contract: Address::from([b; 20]),
                    transition_index: 0,
                })
                .to_vec(),
        });
        assert_eq!(bundle_hold(&bundle), (contracts(&[1, 2, 3]), 140));
    }

    #[test]
    fn locks_are_taken_all_or_nothing() {
        let mut table = LockTable::default();
        assert!(table.try_take("a", &contracts(&[1, 2])));
        assert!(!table.try_take("b", &contracts(&[2, 3])));
        assert!(
            table.try_take("b", &contracts(&[3])),
            "a refused take holds nothing"
        );
        assert!(
            table.try_take("a", &contracts(&[1, 2])),
            "an owner retakes its own"
        );
        table.release("a");
        assert!(table.try_take("b", &contracts(&[1, 2, 3])));
    }

    /// A rendered task keeps its target until its payload lands, so the next task for that
    /// target waits for the landing, not just the rendering, and is traced at head once it starts.
    #[tokio::test]
    async fn a_rendered_task_holds_its_target_until_released_and_the_next_one_reanchors() {
        let harness = Harness::default();
        let (_sender, queue, _) = ingress(&[queued("a1", 1), queued("a2", 1)]);
        harness.schedule(queue, 2);

        harness.assert_started(&["a1"]).await;
        harness.end("a1", Ending::Holding);
        harness.assert_started(&["a1"]).await;

        harness.locks.release("a1");
        harness.assert_started(&["a1", "a2"]).await;
        assert_eq!(*harness.reanchored.lock().unwrap(), ["a2"]);
    }

    /// A deferred task starts again only once every contract its trace pinned is free, and
    /// meanwhile a task needing none of them is not held up behind it.
    #[tokio::test]
    async fn a_deferred_task_waits_for_its_whole_lock_set() {
        let harness = Harness::default();
        let (_sender, queue, _) = ingress(&[queued("a1", 1), queued("b1", 2)]);
        harness.schedule(queue, 3);

        harness.assert_started(&["a1", "b1"]).await;
        harness.end("a1", Ending::Deferred(&[1, 2]));
        harness.assert_started(&["a1", "b1"]).await;

        harness.end("b1", Ending::Settled);
        harness.assert_started(&["a1", "b1", "a1"]).await;
        assert_eq!(*harness.reanchored.lock().unwrap(), ["a1"]);
    }

    /// Once a tree waits for two contracts, a younger task wanting one of them waits behind it
    /// instead of taking it the moment it frees.
    #[tokio::test]
    async fn a_waiting_tree_is_not_overtaken_on_the_contracts_it_needs() {
        let harness = Harness::default();
        let (sender, queue, _) = ingress(&[queued("a1", 1), queued("b1", 2)]);
        harness.schedule(queue, 3);

        harness.assert_started(&["a1", "b1"]).await;
        harness.end("a1", Ending::Deferred(&[1, 2]));
        sender.send(queued("a2", 1)).unwrap();
        harness.assert_started(&["a1", "b1"]).await;

        harness.end("b1", Ending::Settled);
        harness.assert_started(&["a1", "b1", "a1"]).await;
    }

    /// Two tasks whose trees each pin the other's root both defer once, then settle one after
    /// the other instead of deferring each other forever.
    #[tokio::test]
    async fn tasks_pinning_each_others_roots_settle_in_turn() {
        let harness = Harness::default();
        let (_sender, queue, _) = ingress(&[queued("a1", 1), queued("b1", 2)]);
        harness.schedule(queue, 2);

        harness.assert_started(&["a1", "b1"]).await;
        harness.end("a1", Ending::Deferred(&[1, 2]));
        harness.assert_started(&["a1", "b1"]).await;
        // The most recently deferred task goes first; either order settles both.
        harness.end("b1", Ending::Deferred(&[1, 2]));
        harness.assert_started(&["a1", "b1", "b1"]).await;

        harness.end("b1", Ending::Settled);
        harness.assert_started(&["a1", "b1", "b1", "a1"]).await;
    }

    /// Queues `tasks` as the ingress would, counting each toward the queue depth.
    fn ingress(
        tasks: &[QueuedTask],
    ) -> (
        crate::sequencer::TaskSender,
        TaskQueue,
        crate::sequencer::TaskQueueDepth,
    ) {
        let (sender, receiver) = task_channel();
        let depth = task_queue_depth();
        for task in tasks {
            depth.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            sender.send(task.clone()).unwrap();
        }
        let queue = TaskQueue::new(receiver, Arc::clone(&depth), None);
        (sender, queue, depth)
    }

    #[tokio::test]
    async fn distinct_targets_sign_side_by_side_and_one_target_signs_one_task_at_a_time() {
        let harness = Harness::default();
        let (sender, queue, depth) = ingress(&[queued("a1", 1), queued("a2", 1), queued("b1", 2)]);
        let scheduler = harness.schedule(queue, 2);

        harness.assert_started(&["a1", "b1"]).await;
        assert_eq!(
            depth.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a task held behind its target still counts against the queue"
        );

        harness.end("a1", Ending::Settled);
        harness.assert_started(&["a1", "b1", "a2"]).await;

        harness.end("b1", Ending::Settled);
        harness.end("a2", Ending::Settled);
        drop(sender);
        tokio::time::timeout(Duration::from_secs(1), scheduler)
            .await
            .expect("the scheduler exits once ingress closed and every session ended")
            .unwrap();
    }

    #[tokio::test]
    async fn one_session_at_a_time_signs_tasks_in_arrival_order() {
        let harness = Harness::default();
        let (_sender, queue, _) = ingress(&[queued("a1", 1), queued("b1", 2)]);
        harness.schedule(queue, 1);

        harness.assert_started(&["a1"]).await;
        harness.end("a1", Ending::Settled);
        harness.assert_started(&["a1", "b1"]).await;
    }

    #[tokio::test]
    async fn a_panicked_session_frees_its_target() {
        let harness = Harness::default();
        let (_sender, queue, _) = ingress(&[queued("a1", 1), queued("a2", 1)]);
        harness.schedule(queue, 2);

        harness.assert_started(&["a1"]).await;
        harness.end("a1", Ending::Panic);
        harness.assert_started(&["a1", "a2"]).await;
    }

    /// A session that already signed must settle its task rather than be aborted and signed
    /// again by the next router life, and nothing new may start once the channel is gone.
    #[tokio::test]
    async fn a_closed_channel_drains_live_sessions_and_starts_nothing_new() {
        let harness = Harness::default();
        // a2 waits behind a1, so the close frees a lane it could otherwise take.
        let (_sender, queue, _) = ingress(&[queued("a1", 1), queued("a2", 1), queued("b1", 2)]);
        let scheduler = harness.schedule(queue, 2);

        harness.assert_started(&["a1", "b1"]).await;
        harness.end("a1", Ending::ChannelClosed);
        harness.assert_started(&["a1", "b1"]).await;
        assert!(
            !scheduler.is_finished(),
            "the scheduler waits for the session still settling"
        );

        harness.end("b1", Ending::Settled);
        tokio::time::timeout(Duration::from_secs(1), scheduler)
            .await
            .expect("the scheduler exits once the live sessions drained, with ingress still open")
            .unwrap();
    }

    #[test]
    fn a_height_at_or_below_the_clock_is_handed_out_at_once() {
        assert_eq!(clock_wait(100, 100), Some(Duration::ZERO));
        assert_eq!(clock_wait(90, 100), Some(Duration::ZERO));
    }

    #[test]
    fn a_height_just_ahead_of_the_clock_waits_for_it() {
        assert_eq!(clock_wait(105, 100), Some(Duration::from_millis(5)));
    }

    /// A backwards clock step must not stall the pipeline for the size of the step.
    #[test]
    fn a_height_far_ahead_of_the_clock_is_not_waited_for() {
        assert_eq!(clock_wait(100 + 60_000, 100), None);
    }
}

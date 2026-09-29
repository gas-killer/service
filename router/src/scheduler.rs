//! Scheduler: turns ingress tasks into signing sessions and settles each.
//!
//! A session is a height, the task, and its router trace. The height only names the session:
//! the nodes key their nonce state by `(height, attempt)`, so it must never repeat within or
//! across router lives. Every height is handed out at or below the wall clock in milliseconds
//! ([`clock_tip`]), which is where the next router life starts.
//!
//! Up to `MAX_IN_FLIGHT_TASKS` sessions run at once, but only one per target: a task's
//! transition index is read from its target when the task is resolved, so a second task for the
//! same target must not resolve until the first has settled. A task whose target is busy waits
//! without holding up tasks for other targets.

use crate::height_metrics::HeightObserver;
use crate::metrics::MetricsCollector;
use crate::schnorr_coordinator::{SchnorrCoordinator, SessionOutcome, clock_tip};
use crate::schnorr_submitter::SchnorrSubmitter;
use crate::sequencer::{DispatchedTask, QueuedTask, TaskDispatcher, TaskQueue};
use alloy_primitives::Address;
use commonware_avs_core::bn254::PublicKey;
use commonware_p2p::Sender;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

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

/// Dequeued tasks waiting for their target's lane, and the targets with a live session.
///
/// Beyond the transition index, the lane keeps two live sessions from ever signing the same
/// digest: it binds only `(transitionIndex, target, selector, storageUpdates)`, so two tasks for
/// one target resolved against the same state could share one.
#[derive(Default)]
struct Lanes {
    busy: HashSet<Address>,
    waiting: VecDeque<QueuedTask>,
}

impl Lanes {
    fn hold(&mut self, task: QueuedTask) {
        self.waiting.push_back(task);
    }

    /// The oldest waiting task whose target is free, taking that target's lane.
    fn next_ready(&mut self) -> Option<QueuedTask> {
        let index = self
            .waiting
            .iter()
            .position(|task| !self.busy.contains(&task.request.body.target_address))?;
        let task = self.waiting.remove(index)?;
        self.busy.insert(task.request.body.target_address);
        Some(task)
    }

    fn release(&mut self, target: Address) {
        self.busy.remove(&target);
    }
}

/// Runs `session` for each task from `queue`, at most `max_in_flight` at once and one per
/// target, until the ingress side closes and every session has ended, or a session reports the
/// schnorr channel closed by returning `false`.
async fn schedule<F, Fut>(mut queue: TaskQueue, max_in_flight: usize, session: F)
where
    F: Fn(QueuedTask) -> Fut,
    Fut: Future<Output = bool> + Send + 'static,
{
    let mut lanes = Lanes::default();
    let mut live = JoinSet::new();
    let mut targets = HashMap::new();
    let mut ingress_open = true;
    loop {
        while live.len() < max_in_flight
            && let Some(task) = lanes.next_ready()
        {
            queue.started();
            targets.insert(
                live.spawn(session(task.clone())).id(),
                task.request.body.target_address,
            );
        }
        if !ingress_open && live.is_empty() {
            info!("task channel closed; scheduler exiting");
            return;
        }
        tokio::select! {
            Some(joined) = live.join_next_with_id(), if !live.is_empty() => {
                let (id, keep_going) = match joined {
                    Ok(ended) => ended,
                    // The task stays `processing` until the next router life re-queues it; its
                    // lane is freed so the target is not blocked for the rest of this one.
                    Err(panicked) => {
                        error!(error = %panicked, "signing session panicked");
                        (panicked.id(), true)
                    }
                };
                if let Some(target) = targets.remove(&id) {
                    lanes.release(target);
                }
                if !keep_going {
                    // Aborting the other sessions leaves their tasks `processing` for the next
                    // router life, like the one that saw the close.
                    info!("schnorr channel closed; scheduler exiting");
                    return;
                }
            }
            queued = queue.next(), if ingress_open && live.len() < max_in_flight => {
                match queued {
                    Some(task) => lanes.hold(task),
                    None => ingress_open = false,
                }
            }
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
            },
            max_in_flight: max_in_flight.max(1),
        }
    }

    /// Runs sessions until the ingress side of the task channel closes.
    pub async fn run(self) {
        let sessions = self.sessions;
        schedule(self.queue, self.max_in_flight, move |task| {
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
        }
    }
}

impl<S> Sessions<S>
where
    S: Sender<PublicKey = PublicKey>,
{
    /// Dispatches, signs and settles one task. Returns `false` when the schnorr channel closed.
    async fn run(self, task: QueuedTask) -> bool {
        let Some(dispatched) = self.dispatcher.dispatch(task).await else {
            return true;
        };
        self.sign_and_settle(dispatched).await
    }

    async fn sign_and_settle(mut self, dispatched: DispatchedTask) -> bool {
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
        let Some(outcome) = self
            .coordinator
            .drive_height(height, &dispatched.task, &dispatched.trace)
            .await
        else {
            info!(
                height,
                task_id = dispatched.task_id,
                "schnorr channel closed; session abandoned"
            );
            return false;
        };
        if matches!(outcome, SessionOutcome::Signed { .. }) {
            self.metrics
                .p2p_round_trip_seconds
                .observe(started.elapsed().as_secs_f64());
        }
        let settled = self
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
        true
    }
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
        };

        let dispatched = DispatchedTask {
            task_id: task.id.clone(),
            task: GasKillerTaskData::default(),
            trace: RouterTrace::spawn(std::future::pending()),
        };
        let keep_going =
            tokio::time::timeout(Duration::from_secs(1), session.sign_and_settle(dispatched))
                .await
                .expect("a closed channel must end the session at once");

        assert!(!keep_going);
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
    }

    /// Fake sessions for [`schedule`]: each records that it started, then runs until the test
    /// ends it.
    #[derive(Clone, Default)]
    struct Harness {
        started: Arc<std::sync::Mutex<Vec<String>>>,
        endings: Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<Ending>>>>,
    }

    impl Harness {
        fn session(
            &self,
        ) -> impl Fn(QueuedTask) -> std::pin::Pin<Box<dyn Future<Output = bool> + Send>> + use<>
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
                    harness.started.lock().unwrap().push(task.task_id);
                    match ending.await {
                        Ok(Ending::Settled) | Err(_) => true,
                        Ok(Ending::ChannelClosed) => false,
                        Ok(Ending::Panic) => panic!("session panicked"),
                    }
                })
            }
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
        QueuedTask {
            task_id: task_id.to_owned(),
            request: crate::ingress::GasKillerTaskRequest {
                body: crate::ingress::GasKillerTaskRequestBody {
                    target_address: Address::from([target; 20]),
                    call_data: vec![],
                    transition_index: Some(0),
                    from_address: Address::ZERO,
                    value: U256::ZERO,
                    block_height: 1,
                },
            },
        }
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
        let scheduler = tokio::spawn(schedule(queue, 2, harness.session()));

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
        tokio::spawn(schedule(queue, 1, harness.session()));

        harness.assert_started(&["a1"]).await;
        harness.end("a1", Ending::Settled);
        harness.assert_started(&["a1", "b1"]).await;
    }

    #[tokio::test]
    async fn a_panicked_session_frees_its_target() {
        let harness = Harness::default();
        let (_sender, queue, _) = ingress(&[queued("a1", 1), queued("a2", 1)]);
        tokio::spawn(schedule(queue, 2, harness.session()));

        harness.assert_started(&["a1"]).await;
        harness.end("a1", Ending::Panic);
        harness.assert_started(&["a1", "a2"]).await;
    }

    #[tokio::test]
    async fn a_closed_channel_stops_the_scheduler_with_ingress_still_open() {
        let harness = Harness::default();
        let (_sender, queue, _) = ingress(&[queued("a1", 1), queued("b1", 2)]);
        let scheduler = tokio::spawn(schedule(queue, 2, harness.session()));

        harness.assert_started(&["a1", "b1"]).await;
        harness.end("a1", Ending::ChannelClosed);
        tokio::time::timeout(Duration::from_secs(1), scheduler)
            .await
            .expect("a closed channel ends the scheduler")
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

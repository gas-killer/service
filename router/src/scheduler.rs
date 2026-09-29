//! Scheduler: turns each ingress task into one signing session and settles it.
//!
//! A session is a height, the task, and its router trace. The height only names the session:
//! the nodes key their nonce state by `(height, attempt)`, so it must never repeat within or
//! across router lives. Every height is handed out at or below the wall clock in milliseconds
//! ([`clock_tip`]), which is where the next router life starts. Sessions run one at a time:
//! resolve the task, drive the signing round, settle the task, then take the next one.

use crate::height_metrics::HeightObserver;
use crate::metrics::MetricsCollector;
use crate::schnorr_coordinator::{SchnorrCoordinator, SessionOutcome, clock_tip};
use crate::schnorr_submitter::SchnorrSubmitter;
use crate::sequencer::{DispatchedTask, GasKillerTaskSource};
use commonware_avs_core::bn254::PublicKey;
use commonware_p2p::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Longest the scheduler waits for the clock to reach the next height. Only a burst of sessions
/// that each end within a millisecond waits at all; a gap this large means the wall clock
/// stepped backwards, and waiting it out would stall the pipeline for as long.
const MAX_CLOCK_WAIT: Duration = Duration::from_secs(1);

/// How long to wait before handing out `next` when the clock reads `now`, or `None` when the
/// gap is too large to wait out.
fn clock_wait(next: u64, now: u64) -> Option<Duration> {
    let ahead = Duration::from_millis(next.saturating_sub(now));
    (ahead <= MAX_CLOCK_WAIT).then_some(ahead)
}

pub struct Scheduler<S, R>
where
    S: Sender<PublicKey = PublicKey>,
    R: Receiver<PublicKey = PublicKey>,
{
    source: GasKillerTaskSource,
    coordinator: SchnorrCoordinator<S, R>,
    submitter: SchnorrSubmitter,
    observer: HeightObserver,
    metrics: Arc<MetricsCollector>,
    next_height: u64,
}

impl<S, R> Scheduler<S, R>
where
    S: Sender<PublicKey = PublicKey>,
    R: Receiver<PublicKey = PublicKey>,
{
    pub fn new(
        source: GasKillerTaskSource,
        coordinator: SchnorrCoordinator<S, R>,
        submitter: SchnorrSubmitter,
        observer: HeightObserver,
        metrics: Arc<MetricsCollector>,
    ) -> Self {
        let next_height = clock_tip();
        info!(tip = next_height, "schnorr heights start at the clock");
        Self {
            source,
            coordinator,
            submitter,
            observer,
            metrics,
            next_height,
        }
    }

    /// Runs sessions until the ingress side of the task channel closes.
    pub async fn run(mut self) {
        while let Some(dispatched) = self.source.next_task().await {
            if !self.run_session(dispatched).await {
                return;
            }
        }
        info!("task channel closed; scheduler exiting");
    }

    /// Signs and settles one task. Returns `false` when the schnorr channel closed, which ends
    /// the scheduler.
    async fn run_session(&mut self, dispatched: DispatchedTask) -> bool {
        let height = self.take_height().await;
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
                "schnorr channel closed; scheduler exiting"
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

    /// The next session's height: never below the previous one, and not above the clock, so a
    /// restarted router (which starts at the clock) is above every height this life used. A
    /// backwards clock step larger than [`MAX_CLOCK_WAIT`] breaks the second property until the
    /// clock catches up; stalling for the step instead would stop the pipeline.
    async fn take_height(&mut self) -> u64 {
        let now = clock_tip();
        match clock_wait(self.next_height, now) {
            Some(wait) if !wait.is_zero() => tokio::time::sleep(wait).await,
            Some(_) => {}
            None => warn!(
                next_height = self.next_height,
                clock = now,
                "session heights are ahead of the clock; the wall clock stepped backwards"
            ),
        }
        let height = self.next_height.max(clock_tip());
        self.next_height = height + 1;
        height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::GasKillerHandler;
    use crate::schnorr_coordinator::testing::{CountingSender, closed_coordinator};
    use crate::sequencer::{RouterTrace, task_channel};
    use crate::store::{SqliteStore, TaskStatus};
    use alloy_primitives::{Address, U256};
    use alloy_provider::ProviderBuilder;
    use alloy_provider::fillers::{
        BlobGasFiller, ChainIdFiller, GasFiller, JoinFill, NonceFiller, SimpleNonceManager,
    };
    use alloy_signer_local::PrivateKeySigner;
    use gas_killer_common::{GasKillerTaskData, GasKillerValidator};

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
        let (_ingress, receiver) = task_channel();
        let mut scheduler = Scheduler::new(
            GasKillerTaskSource::new(
                receiver,
                Default::default(),
                Arc::new(GasKillerValidator::with_rpc_url(OFFLINE_RPC)),
                None,
                Some(store.clone()),
            ),
            closed_coordinator(CountingSender::default()),
            offline_submitter(store.clone()),
            HeightObserver::new(Arc::clone(&metrics)),
            Arc::clone(&metrics),
        );

        let dispatched = DispatchedTask {
            task_id: task.id.clone(),
            task: GasKillerTaskData::default(),
            trace: RouterTrace::spawn(std::future::pending()),
        };
        let keep_going =
            tokio::time::timeout(Duration::from_secs(1), scheduler.run_session(dispatched))
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

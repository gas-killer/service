//! Scheduler: turns each ingress task into one signing session and settles it.
//!
//! A session is a height, the task, and its router trace. The height only names the session:
//! the nodes key their nonce state by `(height, attempt)`, so it must never repeat within or
//! across router lives ([`clock_tip`] seeds it). Sessions run one at a time: resolve the task,
//! drive the signing round, settle the task, then take the next one.

use crate::height_metrics::HeightObserver;
use crate::metrics::MetricsCollector;
use crate::schnorr_coordinator::{SchnorrCoordinator, SessionOutcome, clock_tip};
use crate::schnorr_submitter::SchnorrSubmitter;
use crate::sequencer::GasKillerTaskSource;
use commonware_avs_core::bn254::PublicKey;
use commonware_p2p::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;
use tracing::info;

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
            let height = self.next_height;
            self.next_height += 1;
            let started = Instant::now();
            self.observer.start(height, started);
            info!(
                height,
                task_id = dispatched.task_id,
                "assigned task to height"
            );

            let outcome = self
                .coordinator
                .drive_height(height, &dispatched.task, &dispatched.trace)
                .await;
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
        }
        info!("task channel closed; scheduler exiting");
    }
}

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
use crate::sequencer::GasKillerTaskSource;
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
            let height = self.take_height().await;
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

    /// The next session's height: never below the previous one, and never above the clock, so
    /// a restarted router (which starts at the clock) is above every height this life used.
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

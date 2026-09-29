//! Window and height observability for the signing pipeline.
//!
//! A pipeline that has stopped settling sessions is otherwise indistinguishable from an idle
//! one — the process is up, memory is flat, and nothing restarts. The scheduler reports each
//! session's start and end here; [`HeightObserver`] publishes the live window from that and
//! samples the oldest session's age on a fixed interval, since an age only grows between events.

use crate::metrics::{HeightOutcome, MetricsCollector, outcome_labels};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::warn;

/// How often the oldest live session's age is re-published.
///
/// Rounds take tens of seconds, so one second keeps the gauge accurate to within a scrape
/// interval at negligible cost.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Publishes the window's shape and each session's final disposition. Cloning shares the
/// underlying state.
#[derive(Clone)]
pub struct HeightObserver {
    metrics: Arc<MetricsCollector>,
    /// Live sessions: height → when the session started.
    live: Arc<Mutex<BTreeMap<u64, Instant>>>,
}

impl HeightObserver {
    pub fn new(metrics: Arc<MetricsCollector>) -> Self {
        Self {
            metrics,
            live: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Records a session starting at `height`.
    pub fn start(&self, height: u64, started: Instant) {
        let Ok(mut live) = self.live.lock() else {
            warn!(height, "live-session lock poisoned; session not observed");
            return;
        };
        live.insert(height, started);
        self.publish_window(&live);
        self.raise_highest_assigned(height);
    }

    /// Records the session at `height` ending with `outcome`.
    pub fn finish(&self, height: u64, outcome: HeightOutcome) {
        self.metrics
            .height_outcomes
            .get_or_create(&outcome_labels(outcome))
            .inc();
        let Ok(mut live) = self.live.lock() else {
            warn!(
                height,
                "live-session lock poisoned; session end not observed"
            );
            return;
        };
        live.remove(&height);
        self.publish_window(&live);
        self.publish_age(&live, Instant::now());
    }

    /// Re-publishes the oldest live session's age until the task is aborted.
    pub async fn sample_forever(self, interval: Duration) {
        loop {
            tokio::time::sleep(interval).await;
            if let Ok(live) = self.live.lock() {
                self.publish_age(&live, Instant::now());
            }
        }
    }

    fn publish_window(&self, live: &BTreeMap<u64, Instant>) {
        self.metrics.in_flight_heights.set(live.len() as i64);
        // An idle router has no base to report; the monotonic highest-assigned gauge is what
        // still says where the pipeline got to.
        let base = live.keys().next().copied().map(clamp_to_gauge).unwrap_or(0);
        self.metrics.window_base.set(base);
    }

    fn publish_age(&self, live: &BTreeMap<u64, Instant>, now: Instant) {
        let age = oldest_age(live, now)
            .map(|age| clamp_to_gauge(age.as_secs()))
            .unwrap_or(0);
        self.metrics.height_age_seconds.set(age);
    }

    /// Raises the monotonic highest-assigned gauge, never lowering it.
    fn raise_highest_assigned(&self, height: u64) {
        let height = clamp_to_gauge(height);
        let gauge = &self.metrics.highest_assigned_height;
        if height > gauge.get() {
            gauge.set(height);
        }
    }
}

/// Age of the oldest live session, or `None` when none is live. A clock that appears to run
/// backwards yields zero rather than wrapping.
fn oldest_age(live: &BTreeMap<u64, Instant>, now: Instant) -> Option<Duration> {
    live.values()
        .map(|started| now.saturating_duration_since(*started))
        .max()
}

/// Narrows a height or duration to the gauge's signed width, saturating rather than wrapping.
/// Heights this large are unreachable in practice; saturating keeps a nonsensical value visibly
/// pinned instead of flipping negative.
fn clamp_to_gauge(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observer() -> HeightObserver {
        HeightObserver::new(Arc::new(MetricsCollector::new()))
    }

    #[test]
    fn the_window_follows_live_sessions() {
        let observer = observer();
        let now = Instant::now();
        observer.start(10, now);
        observer.start(11, now);
        assert_eq!(observer.metrics.in_flight_heights.get(), 2);
        assert_eq!(observer.metrics.window_base.get(), 10);

        observer.finish(10, HeightOutcome::Ready);
        assert_eq!(observer.metrics.in_flight_heights.get(), 1);
        assert_eq!(observer.metrics.window_base.get(), 11);

        observer.finish(11, HeightOutcome::TimedOut);
        assert_eq!(observer.metrics.in_flight_heights.get(), 0);
        assert_eq!(observer.metrics.window_base.get(), 0);
    }

    #[test]
    fn each_session_end_is_counted_under_its_outcome() {
        let observer = observer();
        observer.start(1, Instant::now());
        observer.finish(1, HeightOutcome::Ready);
        observer.start(2, Instant::now());
        observer.finish(2, HeightOutcome::TraceFailed);

        let output = observer.metrics.encode();
        assert!(output.contains("gas_killer_height_outcomes_total{outcome=\"ready\"} 1"));
        assert!(output.contains("gas_killer_height_outcomes_total{outcome=\"trace_failed\"} 1"));
    }

    #[test]
    fn oldest_age_tracks_the_longest_running_session() {
        let now = Instant::now();
        let live = BTreeMap::from([
            (1, now - Duration::from_secs(30)),
            (2, now - Duration::from_secs(5)),
        ]);
        assert_eq!(oldest_age(&live, now), Some(Duration::from_secs(30)));
        assert_eq!(oldest_age(&BTreeMap::new(), now), None);
    }

    #[test]
    fn highest_assigned_height_never_falls_back() {
        let observer = observer();
        observer.start(20, Instant::now());
        observer.finish(20, HeightOutcome::Ready);
        observer.start(5, Instant::now());
        assert_eq!(observer.metrics.highest_assigned_height.get(), 20);
    }
}

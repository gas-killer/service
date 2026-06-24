use crate::rolling_window::RollingWindow;
use commonware_avs_router::orchestrator::types::DurationProvider;
use prometheus_client::metrics::gauge::Gauge;
use std::env;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Runtime configuration for the load-adaptive aggregation-window controller.
///
/// The controller derives a target window as `scale_factor × rolling-p95(round-trip)`,
/// clamped to `[min, max]`. When no samples are available yet, it falls back to `max`.
#[derive(Clone)]
pub struct AdaptiveConfig {
    /// Multiplier applied to the rolling p95 round-trip latency.
    pub scale_factor: f64,
    /// Floor for the computed target window.
    pub min: Duration,
    /// Ceiling for the computed target window; also the no-data fallback.
    pub max: Duration,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            scale_factor: 2.0,
            min: Duration::from_secs(5),
            max: Duration::from_secs(300),
        }
    }
}

impl AdaptiveConfig {
    /// Reads config from environment variables, applying defaults for missing or invalid values.
    ///
    /// - `ADAPTIVE_AGGREGATION_SCALE_FACTOR`: p95 multiplier (default 2.0, must be > 0)
    /// - `ADAPTIVE_AGGREGATION_MIN_SECS`: window floor in seconds (default 5)
    /// - `ADAPTIVE_AGGREGATION_MAX_SECS`: window ceiling in seconds (default 300)
    pub fn from_env() -> Self {
        let scale_factor = env::var("ADAPTIVE_AGGREGATION_SCALE_FACTOR")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|&v| v > 0.0 && v.is_finite())
            .unwrap_or(2.0);

        let max_secs = env::var("ADAPTIVE_AGGREGATION_MAX_SECS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|&v| v > 0.0 && v.is_finite())
            .unwrap_or(300.0);

        let min_secs = env::var("ADAPTIVE_AGGREGATION_MIN_SECS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|&v| v > 0.0 && v.is_finite())
            .unwrap_or(5.0)
            .min(max_secs);

        Self {
            scale_factor,
            min: Duration::from_secs_f64(min_secs),
            max: Duration::from_secs_f64(max_secs),
        }
    }
}

/// Returns `true` when the `ADAPTIVE_AGGREGATION_ENABLED` environment variable is set
/// to `true`, `1`, or `yes` (case-insensitive). Defaults to `false` so deployments
/// stay on the static `ROUND_TIMEOUT` until explicitly opted in.
pub fn adaptive_enabled() -> bool {
    env::var("ADAPTIVE_AGGREGATION_ENABLED")
        .map(|v| matches!(v.trim().to_lowercase().as_str(), "true" | "1" | "yes"))
        .unwrap_or(false)
}

/// Adaptive controller that adjusts the orchestrator's aggregation window from
/// observed P2P round-trip latency samples.
///
/// The controller reads from a shared `RollingWindow` that the executor populates on
/// each completed round. It is surfaced to the orchestrator as a `DurationProvider`
/// closure; calling `as_provider()` produces a cheap, cloneable `Arc<dyn Fn>` that
/// the orchestrator samples once per round on the hot path.
///
/// The exported `gas_killer_aggregation_window_seconds` gauge reflects the value
/// returned on the most recent provider call.
pub struct AdaptiveController {
    window: Arc<Mutex<RollingWindow>>,
    gauge: Gauge<f64, AtomicU64>,
    config: AdaptiveConfig,
}

impl AdaptiveController {
    /// Creates a new controller and initializes the gauge to `config.max` so the
    /// exported metric has a meaningful value before the first round completes.
    pub fn new(
        window: Arc<Mutex<RollingWindow>>,
        gauge: Gauge<f64, AtomicU64>,
        config: AdaptiveConfig,
    ) -> Self {
        gauge.set(config.max.as_secs_f64());
        Self {
            window,
            gauge,
            config,
        }
    }

    /// Returns a `DurationProvider` that the orchestrator calls once per round.
    ///
    /// The closure computes `target = scale_factor × p95(window)`, clamps it to
    /// `[min, max]`, updates the exported gauge, and returns the target. Falls back
    /// to `max` when the window is empty (e.g. no rounds have completed yet).
    pub fn as_provider(&self) -> DurationProvider {
        let window = Arc::clone(&self.window);
        let config = self.config.clone();
        let gauge = self.gauge.clone();
        Arc::new(move || {
            let p95 = window.lock().unwrap().percentile(0.95);
            let target = match p95 {
                Some(p95) => {
                    let scaled = Duration::from_secs_f64(p95.as_secs_f64() * config.scale_factor);
                    scaled.max(config.min).min(config.max)
                }
                None => config.max,
            };
            gauge.set(target.as_secs_f64());
            target
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_arc(capacity: usize) -> Arc<Mutex<RollingWindow>> {
        Arc::new(Mutex::new(RollingWindow::new(capacity)))
    }

    fn gauge() -> Gauge<f64, AtomicU64> {
        Gauge::default()
    }

    fn config(scale: f64, min_s: u64, max_s: u64) -> AdaptiveConfig {
        AdaptiveConfig {
            scale_factor: scale,
            min: Duration::from_secs(min_s),
            max: Duration::from_secs(max_s),
        }
    }

    #[test]
    fn returns_max_when_no_samples() {
        let c = AdaptiveController::new(window_arc(50), gauge(), config(2.0, 5, 60));
        let p = c.as_provider();
        assert_eq!(p(), Duration::from_secs(60));
    }

    #[test]
    fn gauge_initialized_to_max() {
        let g = gauge();
        let _ = AdaptiveController::new(window_arc(50), g.clone(), config(2.0, 5, 60));
        assert!((g.get() - 60.0).abs() < 1e-9);
    }

    #[test]
    fn scales_p95_within_bounds() {
        let w = window_arc(50);
        for _ in 0..20 {
            w.lock().unwrap().push(Duration::from_secs(10));
        }
        let c = AdaptiveController::new(w, gauge(), config(2.0, 5, 300));
        // p95 = 10s; 2.0 × 10s = 20s, within [5, 300].
        assert_eq!(c.as_provider()(), Duration::from_secs(20));
    }

    #[test]
    fn clamps_to_min() {
        let w = window_arc(50);
        w.lock().unwrap().push(Duration::from_millis(100));
        let c = AdaptiveController::new(w, gauge(), config(2.0, 10, 300));
        // 2.0 × 0.1s = 0.2s < min(10s).
        assert_eq!(c.as_provider()(), Duration::from_secs(10));
    }

    #[test]
    fn clamps_to_max() {
        let w = window_arc(50);
        for _ in 0..20 {
            w.lock().unwrap().push(Duration::from_secs(200));
        }
        let c = AdaptiveController::new(w, gauge(), config(2.0, 5, 300));
        // 2.0 × 200s = 400s > max(300s).
        assert_eq!(c.as_provider()(), Duration::from_secs(300));
    }

    #[test]
    fn updates_gauge_on_each_call() {
        let w = window_arc(50);
        for _ in 0..10 {
            w.lock().unwrap().push(Duration::from_secs(15));
        }
        let g = gauge();
        let c = AdaptiveController::new(w, g.clone(), config(2.0, 5, 300));
        let p = c.as_provider();
        let _ = p();
        // 2.0 × 15s = 30s.
        assert!((g.get() - 30.0).abs() < 1e-9);
    }

    #[test]
    fn multiple_calls_track_changing_window() {
        let w = window_arc(50);
        let g = gauge();
        let c = AdaptiveController::new(w.clone(), g.clone(), config(2.0, 5, 300));
        let p = c.as_provider();

        // No data → max.
        assert_eq!(p(), Duration::from_secs(300));

        // Push fast samples.
        for _ in 0..20 {
            w.lock().unwrap().push(Duration::from_secs(5));
        }
        // p95 = 5s; target = 10s.
        assert_eq!(p(), Duration::from_secs(10));

        // Push slow samples.
        for _ in 0..20 {
            w.lock().unwrap().push(Duration::from_secs(100));
        }
        // Window now dominated by 100s samples; p95 = 100s; target = 200s.
        assert_eq!(p(), Duration::from_secs(200));
    }
}

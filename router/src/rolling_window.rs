use std::collections::VecDeque;
use std::time::Duration;

/// Fixed-capacity sliding window of `Duration` samples.
///
/// When capacity is reached the oldest sample is evicted on each push, keeping
/// the window anchored to the most recent `capacity` observations. Designed to
/// be held inside a `Mutex` and shared between a writer (executor) and a reader
/// (adaptive controller `DurationProvider` closure).
pub struct RollingWindow {
    samples: VecDeque<Duration>,
    capacity: usize,
}

impl RollingWindow {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "RollingWindow capacity must be > 0");
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, sample: Duration) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Returns the p-th percentile (0.0–1.0) of the stored samples using nearest-rank
    /// rounding, or `None` if the window is empty.
    pub fn percentile(&self, p: f64) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted: Vec<Duration> = self.samples.iter().copied().collect();
        sorted.sort_unstable();
        let idx = ((sorted.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
        Some(sorted[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_at_capacity() {
        let mut w = RollingWindow::new(3);
        for i in 0u64..5 {
            w.push(Duration::from_secs(i));
        }
        // Capacity 3; after 5 pushes the window holds [2s, 3s, 4s].
        assert_eq!(w.len(), 3);
        assert_eq!(w.percentile(0.0), Some(Duration::from_secs(2)));
        assert_eq!(w.percentile(1.0), Some(Duration::from_secs(4)));
    }

    #[test]
    fn percentile_empty_returns_none() {
        let w = RollingWindow::new(10);
        assert_eq!(w.percentile(0.95), None);
    }

    #[test]
    fn p95_single_sample() {
        let mut w = RollingWindow::new(10);
        w.push(Duration::from_secs(5));
        assert_eq!(w.percentile(0.95), Some(Duration::from_secs(5)));
    }

    #[test]
    fn p95_ten_uniform_samples() {
        let mut w = RollingWindow::new(20);
        for i in 1u64..=10 {
            w.push(Duration::from_secs(i));
        }
        // Sorted [1..10], idx = round(9 * 0.95) = round(8.55) = 9 → 10s.
        assert_eq!(w.percentile(0.95), Some(Duration::from_secs(10)));
    }

    #[test]
    fn p50_even_length() {
        let mut w = RollingWindow::new(10);
        for i in [1u64, 2, 3, 4] {
            w.push(Duration::from_secs(i));
        }
        // idx = round(3 * 0.5) = round(1.5) = 2 → sorted[2] = 3s.
        assert_eq!(w.percentile(0.5), Some(Duration::from_secs(3)));
    }
}

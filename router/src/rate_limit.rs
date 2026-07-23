//! Per-API-key request rate limiting for the ingress `POST /tasks` endpoint.
//!
//! Each API key gets an independent GCRA limiter (via `governor`) keyed by its `key_id`, so one
//! abusive client cannot exhaust the shared task queue and starve other keys. A key is limited at
//! the global default rate (`RATE_LIMIT_RPM`) unless it was issued with a per-key override.
//!
//! State is in-memory and per-process: a restart resets every window. This is acceptable for the
//! v1 beta (the queue is bounded independently by `MAX_QUEUE_DEPTH`) and keeps the check off the
//! database — no per-request write on the hot path.

use std::num::NonZeroU32;
use std::sync::Arc;

use dashmap::DashMap;
use governor::clock::{Clock, DefaultClock};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};

/// A keyed request rate limiter: one GCRA limiter per API key.
///
/// A key's limiter is created on first use with that key's effective quota — its override, or the
/// global default — and cached thereafter. Overrides are fixed at key creation, so a key's quota
/// never changes within a process lifetime and the cached limiter stays correct.
///
/// The backing map grows one small entry per distinct key seen since startup. Key ids are
/// admin-minted and bounded, and all state is dropped on restart, so no eviction is needed for v1.
pub struct KeyRateLimiter {
    limiters: DashMap<String, Arc<DefaultDirectRateLimiter>>,
    default_quota: Quota,
    clock: DefaultClock,
}

impl KeyRateLimiter {
    /// Builds a limiter whose default per-key rate is `default_rpm` requests per minute.
    pub fn new(default_rpm: NonZeroU32) -> Self {
        Self {
            limiters: DashMap::new(),
            default_quota: Quota::per_minute(default_rpm),
            clock: DefaultClock::default(),
        }
    }

    /// Records a request against `key_id`'s window.
    ///
    /// Returns `Ok(())` when the request is within the key's rate, or `Err(retry_after_secs)` —
    /// whole seconds to wait before the next request is allowed (always at least 1) — when it
    /// would exceed the limit. `rpm_override` selects the quota the first time a key is seen:
    /// `Some(rpm)` for a key issued with a custom limit, `None` for the global default.
    pub fn check(&self, key_id: &str, rpm_override: Option<NonZeroU32>) -> Result<(), u64> {
        let limiter = self.limiter_for(key_id, rpm_override);
        limiter.check().map_err(|not_until| {
            let wait = not_until.wait_time_from(self.clock.now());
            // Round up to whole seconds so a sub-second wait still advertises a positive
            // Retry-After; a client that honors it never retries before the cell refills.
            (wait.as_millis().div_ceil(1000) as u64).max(1)
        })
    }

    /// Returns the cached limiter for `key_id`, creating it with the key's effective quota on
    /// first use.
    fn limiter_for(
        &self,
        key_id: &str,
        rpm_override: Option<NonZeroU32>,
    ) -> Arc<DefaultDirectRateLimiter> {
        if let Some(existing) = self.limiters.get(key_id) {
            return Arc::clone(existing.value());
        }
        let quota = rpm_override.map_or(self.default_quota, Quota::per_minute);
        // or_insert_with closes the race where two requests for a new key build a limiter
        // concurrently: whichever inserts first wins and both callers share the same one.
        Arc::clone(
            self.limiters
                .entry(key_id.to_owned())
                .or_insert_with(|| Arc::new(RateLimiter::direct(quota)))
                .value(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpm(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).expect("test rpm is non-zero")
    }

    #[test]
    fn allows_within_quota_then_rejects_with_retry_after() {
        // A quota of one per minute permits a single request, then blocks.
        let limiter = KeyRateLimiter::new(rpm(1));

        assert!(limiter.check("key-a", None).is_ok());
        let retry_after = limiter
            .check("key-a", None)
            .expect_err("second request in the same window must be rejected");
        assert!(
            (1..=60).contains(&retry_after),
            "retry-after should be a positive whole-second wait within the replenish window, got {retry_after}"
        );
    }

    #[test]
    fn keys_are_independent() {
        let limiter = KeyRateLimiter::new(rpm(1));

        assert!(limiter.check("key-a", None).is_ok());
        assert!(
            limiter.check("key-a", None).is_err(),
            "key-a is now exhausted"
        );
        assert!(
            limiter.check("key-b", None).is_ok(),
            "a different key must have its own independent window"
        );
    }

    #[test]
    fn per_key_override_takes_precedence_over_default() {
        // Default is a strict one-per-minute; the override key gets a generous burst.
        let limiter = KeyRateLimiter::new(rpm(1));
        let override_rpm = Some(rpm(600));

        // The override key sustains a burst the default rate would reject on the second request.
        for _ in 0..10 {
            assert!(
                limiter.check("vip", override_rpm).is_ok(),
                "override key should not be limited at the default rate"
            );
        }

        // A default key is still capped at one request.
        assert!(limiter.check("regular", None).is_ok());
        assert!(limiter.check("regular", None).is_err());
    }

    #[test]
    fn override_is_pinned_on_first_use() {
        // The quota is fixed the first time a key is seen; a later call with a different override
        // for the same key reuses the original limiter rather than widening the window.
        let limiter = KeyRateLimiter::new(rpm(1));

        assert!(limiter.check("key-a", Some(rpm(1))).is_ok());
        assert!(
            limiter.check("key-a", Some(rpm(600))).is_err(),
            "a key's quota is pinned at first use and cannot be widened by a later call"
        );
    }
}

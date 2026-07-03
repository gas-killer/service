//! GasKillerAutomaton: supplies the digest the aggregation engine signs per height.
//!
//! The engine calls [`commonware_consensus::Automaton::propose`] for every height
//! in its window; the returned oneshot resolves once the TaskBook decides what
//! lives at that height:
//!
//! - `Announce(task)`: validate via EVMSketch ([`GasKillerValidator::
//!   expected_digest_for_task`]) and resolve the expected task digest. Transient
//!   errors (RPC hiccups) are retried with backoff within a budget of
//!   ~`ROUND_TIMEOUT`; once the budget is exhausted the height resolves to
//!   `skip_digest(h)` — by then the router has hit its own round timeout and is
//!   broadcasting `Skip{h}` anyway (DESIGN.md liveness rule 3).
//! - `Skip`: resolve `skip_digest(h)` so the height still certifies and the
//!   pipeline advances.
//!
//! `verify` is never called by the aggregation engine (propose-only contract); it
//! resolves `true` trivially to satisfy the trait.

use commonware_consensus::Automaton;
use commonware_consensus::types::Height;
use commonware_cryptography::sha256::Digest;
use commonware_runtime::{Spawner, Supervisor, tokio};
use commonware_utils::channel::oneshot;
use gas_killer_common::{GasKillerTaskData, GasKillerValidator, skip_digest};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::task_book::{Resolution, TaskBookMailbox};

/// First retry delay after a validation error; doubles per attempt.
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// Ceiling for the exponential retry backoff.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// Automaton for the aggregation engine (`Context = Height`).
///
/// Cheap to clone (the engine requires `Clone`): all state lives behind an `Arc`.
#[derive(Clone)]
pub struct GasKillerAutomaton {
    inner: Arc<Inner>,
}

struct Inner {
    /// Runtime handle used to spawn one resolution task per proposed height.
    /// Children of this context abort with the root task, never individually.
    context: tokio::Context,
    /// Source of per-height directives (fed by the router over channel 1).
    task_book: TaskBookMailbox,
    /// Recomputes storage updates via EVMSketch and hashes the expected payload.
    validator: Arc<GasKillerValidator>,
    /// Total time budget for retrying validation errors (~`ROUND_TIMEOUT`).
    retry_budget: Duration,
}

impl GasKillerAutomaton {
    /// Creates the automaton.
    ///
    /// `retry_budget` bounds how long an announced task is retried on validation
    /// errors before the height resolves to its skip digest; wire it to
    /// `round_timeout()` so the node gives up in lockstep with the router.
    pub fn new(
        context: tokio::Context,
        task_book: TaskBookMailbox,
        validator: Arc<GasKillerValidator>,
        retry_budget: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                context,
                task_book,
                validator,
                retry_budget,
            }),
        }
    }
}

impl Automaton for GasKillerAutomaton {
    type Context = Height;
    type Digest = Digest;

    async fn propose(&mut self, height: Height) -> oneshot::Receiver<Digest> {
        let (sender, receiver) = oneshot::channel();
        let inner = Arc::clone(&self.inner);
        // Detached: dropping the Handle does not abort the task; it runs until the
        // height resolves (or the TaskBook dies). Parked subscriptions for
        // unassigned heights are normal and hold no resources beyond the oneshot.
        drop(
            self.inner
                .context
                .child("propose")
                .spawn(move |_| async move {
                    inner.resolve(height, sender).await;
                }),
        );
        receiver
    }

    async fn verify(&mut self, _context: Height, _payload: Digest) -> oneshot::Receiver<bool> {
        // The aggregation engine never calls verify (it only requests digests via
        // propose); resolve trivially to satisfy the trait.
        gas_killer_common::trivial_verify()
    }
}

impl Inner {
    /// Waits for the TaskBook's resolution of `height` and resolves the engine's
    /// digest request accordingly.
    async fn resolve(&self, height: Height, sender: oneshot::Sender<Digest>) {
        let h = height.get();
        let resolution = match self.task_book.subscribe(h).await {
            Ok(resolution) => resolution,
            Err(_) => {
                // TaskBook actor is gone (process shutting down). Drop the sender
                // so the engine records AppProposeCanceled instead of leaking a
                // forever-pending future.
                warn!(height = h, "task book unavailable; abandoning propose");
                return;
            }
        };

        let digest = match resolution {
            Resolution::Skip => {
                info!(height = h, "height skipped; signing skip digest");
                skip_digest(h)
            }
            Resolution::Announce(task) => self.digest_for_announce(h, &task).await,
        };

        if sender.send(digest).is_err() {
            // The engine dropped the request (e.g. shutdown mid-resolution).
            debug!(height = h, "engine dropped digest request");
        }
    }

    /// Computes the expected digest for an announced task, retrying errors with
    /// backoff until `retry_budget` is spent, then falling back to the skip digest.
    ///
    /// `expected_digest_for_task` returns untyped (anyhow) errors, so transient
    /// RPC failures and deterministic validation failures are indistinguishable
    /// here; both are retried within the budget and both end in `skip_digest(h)`.
    /// That satisfies liveness rule 3 (deterministic failures resolve to skip —
    /// at worst after the budget) without ever wedging on a flaky RPC. The one
    /// cheaply detectable deterministic failure (missing block height) skips
    /// immediately.
    async fn digest_for_announce(&self, height: u64, task: &GasKillerTaskData) -> Digest {
        if task.block_height == 0 {
            // Deterministic: validation requires a fork height, and every honest
            // node rejects this identically. No point burning the retry budget.
            warn!(
                height,
                "announced task has no block height; signing skip digest"
            );
            return skip_digest(height);
        }

        let deadline = Instant::now() + self.retry_budget;
        let mut backoff = INITIAL_RETRY_BACKOFF;
        loop {
            match self.validator.expected_digest_for_task(task).await {
                Ok(digest) => {
                    debug!(
                        height,
                        transition_index = task.transition_index,
                        ?digest,
                        "validated announced task"
                    );
                    return digest;
                }
                Err(error) if Instant::now() + backoff < deadline => {
                    debug!(
                        height,
                        %error,
                        backoff_ms = backoff.as_millis() as u64,
                        "task validation failed; retrying"
                    );
                    ::tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_RETRY_BACKOFF);
                }
                Err(error) => {
                    // Budget exhausted: treat as failed validation and sign the
                    // skip digest so the height can still certify (the router is
                    // broadcasting Skip{h} on its own round timeout by now).
                    warn!(
                        height,
                        %error,
                        budget_secs = self.retry_budget.as_secs_f64(),
                        "task validation budget exhausted; signing skip digest"
                    );
                    return skip_digest(height);
                }
            }
        }
    }
}

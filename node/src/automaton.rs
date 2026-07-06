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
//! The announce-vs-skip decision and the validation retry loop live in
//! [`crate::digest::DigestResolver`], shared with the Schnorr participant so the
//! two signing paths resolve identical digests.
//!
//! `verify` is never called by the aggregation engine (propose-only contract); it
//! resolves `true` trivially to satisfy the trait.

use commonware_consensus::Automaton;
use commonware_consensus::types::Height;
use commonware_cryptography::sha256::Digest;
use commonware_runtime::{Spawner, Supervisor, tokio};
use commonware_utils::channel::oneshot;
use gas_killer_common::GasKillerValidator;
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

use crate::digest::DigestResolver;
use crate::task_book::TaskBookMailbox;

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
    /// Shared announce-vs-skip + validation logic (see module docs).
    resolver: DigestResolver,
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
                resolver: DigestResolver::new(task_book, validator, retry_budget),
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
                    let h = height.get();
                    // None = TaskBook gone (shutdown): drop the sender so the
                    // engine records AppProposeCanceled instead of leaking a
                    // forever-pending future.
                    let Some(digest) = inner.resolver.resolve(h).await else {
                        return;
                    };
                    if sender.send(digest).is_err() {
                        // The engine dropped the request (e.g. shutdown mid-resolution).
                        debug!(height = h, "engine dropped digest request");
                    }
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

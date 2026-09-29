//! Bounded, isolated execution for task traces.
//!
//! A trace is mostly synchronous CPU work with no yield points (struct-log parsing, the revm
//! estimate) wrapped in an `async fn`, so on the node's main runtime it would hold the workers that
//! drive p2p and the signing rounds. The pool runs traces on a runtime of their own, and at most
//! `concurrency` at once: past that they queue in arrival order, because traces sharing a CPU or a
//! trace RPC all finish late together, while queued ones finish in turn.

use gas_killer_common::ValidatorMetrics;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::Semaphore;

/// Owns the runtime traces run on. Keep it alive for the life of the process, and outside any
/// async context: a tokio runtime panics if it is dropped from inside one.
pub(crate) struct TraceRuntime {
    runtime: Runtime,
    concurrency: usize,
}

impl TraceRuntime {
    /// A runtime for `concurrency` traces. One thread more than that keeps request I/O moving while
    /// every trace is parsing.
    pub fn new(concurrency: usize) -> std::io::Result<Self> {
        let concurrency = concurrency.max(1);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(concurrency + 1)
            .thread_name("gk-trace")
            .enable_all()
            .build()?;
        Ok(Self {
            runtime,
            concurrency,
        })
    }

    /// The pool that runs traces here. It holds only a handle, so it can move into async code
    /// while this runtime stays outside it.
    pub fn pool(&self) -> TracePool {
        TracePool {
            runtime: self.runtime.handle().clone(),
            permits: Arc::new(Semaphore::new(self.concurrency)),
            metrics: None,
        }
    }
}

/// Runs traces on the [`TraceRuntime`], `concurrency` at a time. Cheap to clone.
#[derive(Clone)]
pub(crate) struct TracePool {
    runtime: Handle,
    /// FIFO: tokio hands permits out in the order they were requested.
    permits: Arc<Semaphore>,
    metrics: Option<Arc<ValidatorMetrics>>,
}

impl TracePool {
    pub fn with_metrics(mut self, metrics: Arc<ValidatorMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Runs `work` once a slot is free and returns its output.
    ///
    /// Dropping the returned future aborts `work`, queued or running, so a caller that gives up
    /// never leaves a trace burning CPU for nobody. The slot is released when `work` stops, not
    /// when the caller returns.
    pub async fn run<F>(&self, work: F) -> F::Output
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let queued = Queued::enter(self.metrics.as_deref());
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .expect("the trace semaphore is never closed");
        drop(queued);

        let task = self.runtime.spawn(async move {
            let _permit = permit;
            work.await
        });
        let _abort = AbortOnDrop(task.abort_handle());
        match task.await {
            Ok(output) => output,
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(error) => unreachable!("only this future aborts the trace: {error}"),
        }
    }
}

/// Counts one trace in the queue until it gets a slot or its caller gives up.
struct Queued<'a> {
    metrics: Option<&'a ValidatorMetrics>,
    since: Instant,
}

impl<'a> Queued<'a> {
    fn enter(metrics: Option<&'a ValidatorMetrics>) -> Self {
        if let Some(metrics) = metrics {
            metrics.validation_queue_depth.inc();
        }
        Self {
            metrics,
            since: Instant::now(),
        }
    }
}

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        if let Some(metrics) = self.metrics {
            metrics.validation_queue_depth.dec();
            metrics
                .validation_queue_wait_seconds
                .observe(self.since.elapsed().as_secs_f64());
        }
    }
}

struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn traces_past_the_limit_wait_and_start_in_arrival_order() {
        let runtime = TraceRuntime::new(1).unwrap();
        let pool = runtime.pool();
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));

        let mut callers = Vec::new();
        for id in 0..4 {
            let (pool, running, peak, order) = (
                pool.clone(),
                Arc::clone(&running),
                Arc::clone(&peak),
                Arc::clone(&order),
            );
            callers.push(tokio::spawn(async move {
                pool.run(async move {
                    order.lock().unwrap().push(id);
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                })
                .await
            }));
            // Queue in a known order: each caller asks for its slot before the next starts.
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for caller in callers {
            caller.await.unwrap();
        }

        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3]);
        drop(pool);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_caller_that_gives_up_aborts_its_trace_and_frees_the_slot() {
        let runtime = TraceRuntime::new(1).unwrap();
        let metrics = Arc::new(ValidatorMetrics::new());
        let pool = runtime.pool().with_metrics(Arc::clone(&metrics));

        let (dropped_tx, dropped_rx) = oneshot::channel::<()>();
        let abandoned = pool.run(async move {
            // Resolves only if the trace is dropped rather than left running.
            let _signal = DropSignal(Some(dropped_tx));
            std::future::pending::<()>().await
        });
        tokio::time::timeout(Duration::from_millis(50), abandoned)
            .await
            .expect_err("the trace never finishes on its own");
        tokio::time::timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("giving up must abort the running trace")
            .ok();

        let next = tokio::time::timeout(Duration::from_secs(1), pool.run(async { 7 }))
            .await
            .expect("the abandoned trace's slot must be free again");
        assert_eq!(next, 7);
        assert_eq!(metrics.validation_queue_depth.get(), 0);
        drop(pool);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_waiting_trace_is_counted_in_the_queue() {
        let runtime = TraceRuntime::new(1).unwrap();
        let metrics = Arc::new(ValidatorMetrics::new());
        let pool = runtime.pool().with_metrics(Arc::clone(&metrics));

        let (release_tx, release_rx) = oneshot::channel::<()>();
        let holder = tokio::spawn({
            let pool = pool.clone();
            async move {
                pool.run(async move {
                    release_rx.await.ok();
                })
                .await
            }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let waiter = tokio::spawn({
            let pool = pool.clone();
            async move { pool.run(async {}).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(metrics.validation_queue_depth.get(), 1);

        release_tx.send(()).unwrap();
        holder.await.unwrap();
        waiter.await.unwrap();
        assert_eq!(metrics.validation_queue_depth.get(), 0);
        drop(pool);
        tokio::task::spawn_blocking(move || drop(runtime))
            .await
            .unwrap();
    }

    struct DropSignal(Option<oneshot::Sender<()>>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }
}

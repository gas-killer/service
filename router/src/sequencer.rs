//! Task source: [`TaskQueue`] pulls tasks off the ingress queue, and [`TaskDispatcher`] resolves
//! each into [`GasKillerTaskData`] ready for a signing session and starts the router's own
//! EVMSketch trace alongside it.
//!
//! The scheduler that consumes these lives in [`crate::scheduler`].

use crate::ingress::GasKillerTaskRequest;
use crate::metrics::MetricsCollector;
use crate::store::SqliteStore;
use gas_killer_common::task_data::GasKillerTaskData;
use gas_killer_common::{ChainRole, GasKillerValidator, NestedSpec, TreeTrace};
use gas_killer_common::{PayloadView, TaskBundle};

use alloy_primitives::{Address, Bytes};
use anyhow::{Result, bail};
use commonware_cryptography::{Hasher, Sha256};
use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;
use tracing::{error, info};

/// A task queued for the sequencer, carrying the persisted task id alongside the
/// request so status transitions can be attributed back to the right store row.
#[derive(Debug, Clone)]
pub struct QueuedTask {
    pub task_id: String,
    pub request: GasKillerTaskRequest,
    /// Every contract the task must hold before it starts, recorded when a round found one of
    /// them busy. Empty for a task that has never been deferred: it starts holding its root.
    pub lock_set: BTreeSet<Address>,
    /// Set once the task has waited behind a lock: its client's block may predate whatever
    /// settled meanwhile, so it is traced at head instead.
    pub reanchor: bool,
}

impl QueuedTask {
    pub fn new(task_id: String, request: GasKillerTaskRequest) -> Self {
        Self {
            task_id,
            request,
            lock_set: BTreeSet::new(),
            reanchor: false,
        }
    }

    pub fn root(&self) -> Address {
        self.request.body.target_address
    }

    /// The contracts the task needs free to start.
    pub fn starting_locks(&self) -> BTreeSet<Address> {
        let mut locks = self.lock_set.clone();
        locks.insert(self.root());
        locks
    }
}

pub type TaskSender = UnboundedSender<QueuedTask>;
pub type TaskReceiver = UnboundedReceiver<QueuedTask>;
/// Shared atomic counter tracking tasks in flight between the ingress sender and
/// the task source's receiver.
pub type TaskQueueDepth = Arc<AtomicUsize>;

pub fn task_channel() -> (TaskSender, TaskReceiver) {
    mpsc::unbounded_channel()
}

pub fn task_queue_depth() -> TaskQueueDepth {
    Arc::new(AtomicUsize::new(0))
}

/// What the router's own trace of a task produced.
#[derive(Debug, Clone)]
pub enum Traced {
    /// A flat program for `verifyAndUpdate`.
    Flat(Bytes),
    /// The task split into frames; a single-frame tree still settles through `verifyAndUpdate`.
    Tree(Arc<TreeTrace>),
}

impl Traced {
    /// The program `verifyAndUpdate` applies, when the task settles flat.
    pub fn flat_program(&self) -> Option<&Bytes> {
        match self {
            Self::Flat(program) => Some(program),
            Self::Tree(tree) if !tree.is_nested() => Some(&tree.root_program),
            Self::Tree(_) => None,
        }
    }

    /// Every contract whose transition counter settling the task moves, `root` included.
    pub fn counter_moves(&self, root: Address) -> BTreeSet<Address> {
        let mut moved = match self {
            Self::Flat(_) => BTreeSet::new(),
            Self::Tree(tree) => tree.counter_moves.clone(),
        };
        moved.insert(root);
        moved
    }
}

/// The router's own EVMSketch trace for a dispatched task, running alongside the signing
/// round rather than ahead of it.
///
/// Nodes supply the digest the quorum signs, so the round never waits on this trace. It
/// still gates the outcome: the payload is rendered only from these storage updates, and only
/// when they hash to the digest the quorum signed, so the router keeps its independent check
/// of the quorum. The trace runs in its own task, so it outlives whatever future awaited it.
#[derive(Clone)]
pub struct RouterTrace {
    result: watch::Receiver<Option<Result<Traced, String>>>,
}

impl RouterTrace {
    /// Spawns `trace` and returns a handle every consumer can await.
    pub fn spawn<F>(trace: F) -> Self
    where
        F: Future<Output = Result<Traced>> + Send + 'static,
    {
        let (tx, result) = watch::channel(None);
        tokio::spawn(async move {
            let outcome = trace.await.map_err(|e| e.to_string());
            let _ = tx.send(Some(outcome));
        });
        Self { result }
    }

    /// A trace that has already finished, for callers that hold the result up front.
    pub fn finished(outcome: Result<Traced, String>) -> Self {
        let (_, result) = watch::channel(Some(outcome));
        Self { result }
    }

    /// Waits for the trace: what it produced, or why it could not be computed.
    pub async fn wait(&self) -> Result<Traced, String> {
        let mut result = self.result.clone();
        match result.wait_for(Option::is_some).await {
            Ok(outcome) => outcome.clone().expect("waited for a result"),
            Err(_) => Err("router trace ended without a result".to_owned()),
        }
    }
}

/// Claims a dequeued task for this round, moving it to `processing`, and reports whether the
/// claim succeeded. `false` means the task has already settled — the TTL sweep expires tasks
/// while they sit in this channel — so the caller must drop it rather than spend a round
/// producing a payload too stale to land.
///
/// Best-effort task status bookkeeping shared by the task source and the executor: a store error
/// is logged rather than propagated, because failing to record a status transition must never
/// derail aggregation, which is the real work — a missed transition is recoverable (the startup
/// re-queue picks up anything left `queued` or `processing`), whereas aborting the pipeline is
/// not. An unreachable store therefore claims the task: the guard sheds doomed work, it is not
/// the correctness gate — the round's own on-chain validation still rejects a stale payload.
async fn claim_task_for_processing(store: &SqliteStore, task_id: &str) -> bool {
    match store.claim_task_for_processing(task_id).await {
        Ok(claimed) => claimed,
        Err(e) => {
            error!(task_id, error = %e, "failed to mark task processing");
            true
        }
    }
}

/// Settles a task ready, persisting both the rendered transaction-request `payload` and the
/// structured `bundle` it was derived from (each as JSON). A serialization failure is logged and
/// the transition skipped rather than propagated, following the best-effort convention above.
pub(crate) async fn set_task_ready(
    store: &SqliteStore,
    metrics: Option<&MetricsCollector>,
    task_id: &str,
    payload: &PayloadView,
    bundle: &TaskBundle,
) {
    let payload_json = match serde_json::to_string(payload) {
        Ok(json) => json,
        Err(e) => {
            error!(task_id, error = %e, "failed to serialize payload; task not marked ready");
            return;
        }
    };
    let bundle_json = match serde_json::to_string(bundle) {
        Ok(json) => json,
        Err(e) => {
            error!(task_id, error = %e, "failed to serialize bundle; task not marked ready");
            return;
        }
    };
    match store
        .mark_task_ready_with_bundle(task_id, &payload_json, &bundle_json)
        .await
    {
        Ok(elapsed) => observe_task_e2e(metrics, elapsed),
        Err(e) => error!(task_id, error = %e, "failed to mark task ready"),
    }
}

pub(crate) async fn set_task_failed(
    store: &SqliteStore,
    metrics: Option<&MetricsCollector>,
    task_id: &str,
    reason: &str,
) {
    match store.mark_task_failed(task_id, reason).await {
        Ok(elapsed) => observe_task_e2e(metrics, elapsed),
        Err(e) => error!(task_id, error = %e, "failed to mark task failed"),
    }
}

/// Records a settled task's end-to-end latency — ingress acceptance to terminal status — from the
/// elapsed seconds the settling statement reported. `None` means no task carried that id, so
/// there is nothing to time. A clock that moved backwards between the two timestamps is clamped
/// to zero rather than observed as a negative duration.
fn observe_task_e2e(metrics: Option<&MetricsCollector>, elapsed_secs: Option<i64>) {
    if let (Some(m), Some(secs)) = (metrics, elapsed_secs) {
        m.task_e2e_seconds.observe(secs.max(0) as f64);
    }
}

/// A dequeued task with everything the announce needs; the storage updates come later from
/// its [`RouterTrace`].
struct ResolvedTask {
    task: GasKillerTaskRequest,
    /// Resolved transition index (sentinel `None` → concrete count from chain).
    transition_index: u64,
    /// Actual EVM chain ID (e.g. 1 = Ethereum mainnet, 100 = Gnosis, 31337 = Anvil).
    chain_id: u64,
    /// Simulation RPC the trace runs against.
    sim_rpc_url: String,
    chain_role: ChainRole,
    /// The block the call is traced at: the client's, or head once re-anchored.
    block_height: u64,
    nested: Option<NestedSpec>,
}

impl ResolvedTask {
    fn task_data(&self) -> GasKillerTaskData {
        GasKillerTaskData {
            storage_updates: Bytes::new(),
            transition_index: self.transition_index,
            target_address: self.task.body.target_address,
            call_data: self.task.body.call_data.clone(),
            from_address: self.task.body.from_address,
            value: self.task.body.value,
            block_height: self.block_height,
            chain_id: self.chain_id,
        }
    }
}

/// The scheduler's end of the ingress queue.
///
/// A task counts toward the queue depth until the scheduler starts it, not just until it is
/// dequeued, so tasks held back behind a busy target still count against `MAX_QUEUE_DEPTH`.
pub struct TaskQueue {
    receiver: TaskReceiver,
    queue_depth: TaskQueueDepth,
    metrics: Option<Arc<MetricsCollector>>,
}

impl TaskQueue {
    pub fn new(
        receiver: TaskReceiver,
        queue_depth: TaskQueueDepth,
        metrics: Option<Arc<MetricsCollector>>,
    ) -> Self {
        Self {
            receiver,
            queue_depth,
            metrics,
        }
    }

    /// Blocks until a task arrives. Returns `None` when the ingress side of the channel closed.
    pub async fn next(&mut self) -> Option<QueuedTask> {
        self.receiver.recv().await
    }

    /// Records that a task whose session was deferred is waiting again.
    pub fn requeued(&self) {
        let depth = self.queue_depth.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(m) = &self.metrics {
            m.task_queue_depth.set(depth as i64);
        }
    }

    /// Records that a dequeued task has left the queue for a session.
    pub fn started(&self) {
        // Renamed `try_update` in Rust 1.99; kept until no supported toolchain predates it.
        #[allow(deprecated)]
        let depth = self
            .queue_depth
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            })
            .unwrap_or(0)
            .saturating_sub(1);
        if let Some(m) = &self.metrics {
            m.task_queue_depth.set(depth as i64);
        }
    }
}

/// Claims and resolves dequeued tasks for signing sessions. Cheap to clone.
#[derive(Clone)]
pub struct TaskDispatcher {
    validator: Arc<GasKillerValidator>,
    metrics: Option<Arc<MetricsCollector>>,
    /// Durable store used to advance task status as work progresses. `None` in
    /// store-less test/dev harnesses, where status transitions are simply skipped.
    store: Option<SqliteStore>,
    /// Blocks a nested tree may stay settleable for; see [`GasKillerValidator::nested_spec_for`].
    nested_buffer: Option<u64>,
}

impl TaskDispatcher {
    pub fn new(
        validator: Arc<GasKillerValidator>,
        metrics: Option<Arc<MetricsCollector>>,
        store: Option<SqliteStore>,
    ) -> Self {
        Self {
            validator,
            metrics,
            store,
            nested_buffer: None,
        }
    }

    /// Settles tasks whose root supports it as nested trees, each payload expiring within
    /// `buffer` blocks of its announcement.
    pub fn with_nested_settlement(mut self, buffer: u64) -> Self {
        self.nested_buffer = Some(buffer);
        self
    }

    pub fn validator(&self) -> &Arc<GasKillerValidator> {
        &self.validator
    }

    /// Resolves what the announce needs for a dequeued task: its chain, its transition index
    /// and its chain id. Cheap RPC reads only, so a task that cannot be routed never reaches a
    /// height.
    async fn resolve(&self, task: GasKillerTaskRequest, reanchor: bool) -> Result<ResolvedTask> {
        info!(
            target = format!("{:?}", task.body.target_address),
            from = format!("{:?}", task.body.from_address),
            transition_index = ?task.body.transition_index,
            call_data_len = task.body.call_data.len(),
            "Sequencer received task"
        );

        if let Some(m) = &self.metrics {
            m.tasks_created.inc();
        }

        let chain_role = self
            .validator
            .detect_chain_for_address(task.body.target_address)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to detect chain: {}", e))?;
        let sim_rpc_url = self
            .validator
            .sim_rpc_url_for_chain(chain_role)
            .ok_or_else(|| anyhow::anyhow!("No RPC URL for chain {}", chain_role))?
            .to_owned();

        // Read the chain ID from the chain RPC rather than the simulation one: a fork reports its
        // upstream's ID, but it is the settling chain the commitment is bound to.
        let chain_id_fut = self.validator.get_chain_id_for(chain_role);
        let count_fut = async {
            match task.body.transition_index {
                Some(index) => Ok(index),
                None => {
                    self.validator
                        .get_state_transition_count_on_chain(task.body.target_address, chain_role)
                        .await
                }
            }
        };
        let (chain_id, transition_index) = tokio::try_join!(chain_id_fut, count_fut)?;

        // A task that waited behind a lock is traced at head, where every transition that
        // settled while it waited is visible. One whose client fixed its index
        // cannot follow the count forward, so if that index was used meanwhile it fails here.
        let block_height = if reanchor {
            let head = self.validator.chain_head(chain_role).await?;
            if task.body.transition_index.is_some() {
                let current = self
                    .validator
                    .get_state_transition_count_on_chain(task.body.target_address, chain_role)
                    .await?;
                if current != transition_index {
                    bail!(
                        "transition index {transition_index} was used while the task waited \
                         (the target is at {current}); resubmit the task"
                    );
                }
            }
            head
        } else {
            task.body.block_height
        };

        let nested = match self.nested_buffer {
            Some(buffer) => {
                self.validator
                    .nested_spec_for(task.body.target_address, chain_role, buffer)
                    .await?
            }
            None => None,
        };

        info!(
            target_address = %task.body.target_address,
            chain = %chain_role,
            transition_index,
            chain_id,
            "Resolved task for announce"
        );

        Ok(ResolvedTask {
            task,
            transition_index,
            chain_id,
            sim_rpc_url,
            chain_role,
            block_height,
            nested,
        })
    }

    /// Starts the router's EVMSketch trace for a resolved task.
    fn spawn_trace(&self, resolved: &ResolvedTask) -> RouterTrace {
        let validator = Arc::clone(&self.validator);
        let metrics = self.metrics.clone();
        let traced = resolved.task_data();
        let rpc_url = resolved.sim_rpc_url.clone();
        if let Some(spec) = resolved.nested {
            return RouterTrace::spawn(async move {
                let start = Instant::now();
                let tree = validator.trace_tree(&traced, &spec).await?;
                if let Some(m) = &metrics {
                    m.storage_computation_seconds
                        .observe(start.elapsed().as_secs_f64());
                }
                info!(
                    frames = tree.frames.len(),
                    counter_moves = tree.counter_moves.len(),
                    digest = %hex::encode(&tree.digest.as_ref()[..8]),
                    block_height = traced.block_height,
                    transition_index = traced.transition_index,
                    target_address = %traced.target_address,
                    expiry_block = spec.expiry_block,
                    "Sequencer computed frame tree"
                );
                Ok(Traced::Tree(Arc::new(tree)))
            });
        }
        RouterTrace::spawn(async move {
            let start = Instant::now();
            let analysis = validator
                .analyze_transaction(
                    &rpc_url,
                    traced.target_address,
                    &traced.call_data,
                    Some(traced.from_address),
                    Some(traced.value),
                    traced.block_height,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Failed to compute storage updates: {}", e))?;
            if let Some(m) = &metrics {
                m.storage_computation_seconds
                    .observe(start.elapsed().as_secs_f64());
            }
            let storage_updates = analysis.storage_updates;

            let mut storage_hasher = Sha256::new();
            storage_hasher.update(&storage_updates);
            let storage_hash = storage_hasher.finalize();
            info!(
                storage_updates_len = storage_updates.len(),
                storage_updates_hash = %hex::encode(&storage_hash[..8]),
                block_height = traced.block_height,
                transition_index = traced.transition_index,
                target_address = %traced.target_address,
                target_function = %traced.call_data.get(..4).map(hex::encode).unwrap_or_default(),
                chain_id = traced.chain_id,
                "Sequencer computed storage updates"
            );
            Ok(Traced::Flat(storage_updates.into()))
        })
    }
}

/// A dequeued task ready for a signing session: claimed, resolved, and with the router's own
/// trace already running.
pub struct DispatchedTask {
    pub task_id: String,
    /// The task as announced to the operators, without storage updates.
    pub task: GasKillerTaskData,
    /// Announced with the task when it may settle as a nested tree.
    pub nested: Option<NestedSpec>,
    pub chain: ChainRole,
    pub trace: RouterTrace,
}

impl TaskDispatcher {
    /// Claims a dequeued task, resolves it and starts its trace. `None` means the task is not
    /// going to a session: it settled while it waited (the expiry sweep), or it could not be
    /// resolved and is settled as `failed` here.
    pub async fn dispatch(&self, queued: QueuedTask) -> Option<DispatchedTask> {
        let QueuedTask {
            task_id,
            request,
            reanchor,
            ..
        } = queued;

        // A task the expiry sweep settled while it waited is dropped rather than aggregated: its
        // pinned block is stale enough that the round could not produce a submittable payload.
        if let Some(store) = &self.store
            && !claim_task_for_processing(store, &task_id).await
        {
            info!(task_id, "task settled while queued, skipping dispatch");
            return None;
        }

        let resolved = match self.resolve(request, reanchor).await {
            Ok(resolved) => resolved,
            Err(e) => {
                error!(error = %e, task_id, "failed to enrich task, dropping request");
                if let Some(store) = &self.store {
                    set_task_failed(
                        store,
                        self.metrics.as_deref(),
                        &task_id,
                        &format!("task enrichment failed: {e}"),
                    )
                    .await;
                }
                return None;
            }
        };

        if reanchor
            && let Some(store) = &self.store
            && let Err(e) = store
                .set_task_block_height(&task_id, resolved.block_height)
                .await
        {
            error!(task_id, error = %e, "failed to record the re-anchored block height");
        }

        // Operators get the task WITHOUT storage_updates: they independently recompute them
        // with EVMSketch (that is the whole trust model — see
        // GasKillerValidator::expected_digest_for_task), and the router's own come from the
        // trace started here, alongside the round rather than ahead of it.
        let task = resolved.task_data();
        let trace = self.spawn_trace(&resolved);
        Some(DispatchedTask {
            task_id,
            task,
            nested: resolved.nested,
            chain: resolved.chain_role,
            trace,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TaskStatus;
    use alloy::primitives::{Address, B256, FixedBytes, U256};
    use gas_killer_common::BundleProof;

    fn sample_request(transition_index: Option<u64>) -> GasKillerTaskRequest {
        GasKillerTaskRequest {
            body: crate::ingress::GasKillerTaskRequestBody {
                target_address: Address::from([1u8; 20]),
                call_data: vec![0x12, 0x34, 0x56, 0x78],
                transition_index,
                from_address: Address::from([2u8; 20]),
                value: U256::from(1000),
                block_height: 12345,
            },
        }
    }

    #[tokio::test]
    async fn test_channel_send_recv() {
        let (sender, mut receiver) = task_channel();
        let queued = QueuedTask::new("task-1".to_string(), sample_request(Some(1)));

        sender.send(queued.clone()).unwrap();
        let received = receiver.try_recv().unwrap();
        assert_eq!(received.task_id, "task-1");
        assert_eq!(received.request.body.transition_index, Some(1));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn resolved_task_announces_without_storage_updates() {
        let resolved = ResolvedTask {
            task: sample_request(Some(42)),
            transition_index: 42,
            chain_id: 1u64,
            sim_rpc_url: "http://localhost:8545".to_owned(),
            chain_role: ChainRole::L1,
            block_height: 12345,
            nested: None,
        };
        let task_data = resolved.task_data();

        assert_eq!(task_data.transition_index, 42);
        assert_eq!(task_data.target_address, Address::from([1u8; 20]));
        assert_eq!(task_data.block_height, 12345);
        assert_eq!(task_data.chain_id, 1);
        assert!(task_data.storage_updates.is_empty());
    }

    #[tokio::test]
    async fn a_spawned_trace_outlives_the_future_that_started_it() {
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let trace = RouterTrace::spawn(async move {
            released.await.ok();
            Ok(Traced::Flat(Bytes::from(vec![7u8])))
        });

        let first_wait = tokio::time::timeout(std::time::Duration::from_millis(20), trace.wait());
        assert!(first_wait.await.is_err(), "the trace is still running");

        release.send(()).unwrap();
        assert_eq!(
            trace.wait().await.unwrap().flat_program(),
            Some(&Bytes::from(vec![7u8]))
        );
    }

    // -- task lifecycle transitions --

    async fn store() -> SqliteStore {
        SqliteStore::connect_in_memory()
            .await
            .expect("in-memory store should open and migrate")
    }

    async fn key_id(store: &SqliteStore) -> String {
        store
            .create_api_key(None, None)
            .await
            .expect("key creation should succeed")
            .id
    }

    fn request_body() -> crate::ingress::GasKillerTaskRequestBody {
        crate::ingress::GasKillerTaskRequestBody {
            target_address: Address::from([0x11; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78],
            transition_index: Some(0),
            from_address: Address::from([0x22; 20]),
            value: U256::ZERO,
            block_height: 1,
        }
    }

    fn sample_payload() -> PayloadView {
        PayloadView {
            to: Address::from([0x11; 20]),
            data: Bytes::from(vec![0xde, 0xad, 0xbe, 0xef]),
            value: U256::ZERO,
            chain_id: 31337,
            estimated_gas: 21_000,
            valid_until_block: 100,
        }
    }

    fn sample_bundle() -> TaskBundle {
        TaskBundle {
            msg_hash: B256::ZERO,
            reference_block_number: 50,
            transition_index: 0,
            target_address: Address::from([0x11; 20]),
            target_function: FixedBytes::<4>::from([0x12, 0x34, 0x56, 0x78]),
            storage_updates: Bytes::new(),
            chain_id: 31337,
            value: U256::ZERO,
            valid_until_block: 100,
            proof: BundleProof::Schnorr {
                s: U256::ZERO,
                r_addr: Address::ZERO,
                non_signers: vec![],
            },
            nested: None,
        }
    }

    fn unreachable_validator() -> Arc<GasKillerValidator> {
        // Nothing listens on this port; RPC calls fail fast with connection refused
        // rather than hanging, so `resolve` errors quickly and deterministically.
        Arc::new(GasKillerValidator::with_rpc_url("http://localhost:8545"))
    }

    #[tokio::test]
    async fn set_helpers_persist_status_transitions() {
        let store = store().await;
        let key = key_id(&store).await;
        let done = store.create_task(&key, &request_body()).await.unwrap();
        let doomed = store.create_task(&key, &request_body()).await.unwrap();

        assert!(claim_task_for_processing(&store, &done.id).await);
        assert_eq!(
            store.get_task(&done.id).await.unwrap().unwrap().status,
            TaskStatus::Processing
        );

        set_task_ready(&store, None, &done.id, &sample_payload(), &sample_bundle()).await;
        let ready = store.get_task(&done.id).await.unwrap().unwrap();
        assert_eq!(ready.status, TaskStatus::Ready);
        // Both the rendered payload and the structured bundle are persisted as JSON.
        let payload: PayloadView = serde_json::from_str(ready.payload.as_deref().unwrap()).unwrap();
        assert_eq!(payload, sample_payload());
        let bundle: TaskBundle = serde_json::from_str(ready.bundle.as_deref().unwrap()).unwrap();
        assert_eq!(bundle, sample_bundle());

        set_task_failed(&store, None, &doomed.id, "boom").await;
        let failed = store.get_task(&doomed.id).await.unwrap().unwrap();
        assert_eq!(failed.status, TaskStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn settling_a_task_records_its_end_to_end_latency() {
        let store = store().await;
        let key = key_id(&store).await;
        let metrics = MetricsCollector::new();
        let ready = store.create_task(&key, &request_body()).await.unwrap();
        let failed = store.create_task(&key, &request_body()).await.unwrap();

        // Both terminal transitions feed the histogram: a client waits just as long for a failure
        // as for a payload, so leaving failures out would flatter the observed latency.
        set_task_ready(
            &store,
            Some(&metrics),
            &ready.id,
            &sample_payload(),
            &sample_bundle(),
        )
        .await;
        set_task_failed(&store, Some(&metrics), &failed.id, "boom").await;

        let output = metrics.encode();
        assert!(
            output.contains("gas_killer_task_e2e_seconds_count 2"),
            "both settled tasks should be observed, got:\n{output}"
        );

        // Settling a task that does not exist reports no latency, so nothing is observed for it.
        set_task_failed(&store, Some(&metrics), "no-such-task", "boom").await;
        assert!(
            metrics
                .encode()
                .contains("gas_killer_task_e2e_seconds_count 2"),
            "a settle that matched no task must not be observed"
        );
    }

    #[tokio::test]
    async fn dispatch_marks_processing_then_failed_on_enrich_error() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request_body()).await.unwrap();

        let dispatcher = TaskDispatcher::new(unreachable_validator(), None, Some(store.clone()));
        let queued = QueuedTask::new(
            task.id.clone(),
            GasKillerTaskRequest {
                body: request_body(),
            },
        );

        assert!(dispatcher.dispatch(queued).await.is_none());

        let settled = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Failed);
        assert!(
            settled
                .error
                .as_deref()
                .is_some_and(|e| e.contains("task enrichment failed"))
        );
    }

    /// The claim is the handshake between the expiry sweep and dispatch: once a task has settled,
    /// the round it was queued for must go to other work.
    #[tokio::test]
    async fn claim_refuses_a_task_that_already_settled() {
        let store = store().await;
        let key = key_id(&store).await;
        let expired = store.create_task(&key, &request_body()).await.unwrap();
        store
            .mark_task_expired(&expired.id, "QUEUE_TTL_EXCEEDED")
            .await
            .unwrap();

        assert!(!claim_task_for_processing(&store, &expired.id).await);
        assert_eq!(
            store.get_task(&expired.id).await.unwrap().unwrap().status,
            TaskStatus::Expired,
            "a refused claim must not resurrect the task"
        );
        assert!(
            !claim_task_for_processing(&store, "no-such-task").await,
            "an unknown id is not claimable"
        );
    }

    #[tokio::test]
    async fn dispatch_skips_a_task_expired_while_queued() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request_body()).await.unwrap();
        store
            .mark_task_expired(&task.id, "QUEUE_TTL_EXCEEDED: expired by the sweep")
            .await
            .unwrap();

        let dispatcher = TaskDispatcher::new(unreachable_validator(), None, Some(store.clone()));
        let queued = QueuedTask::new(
            task.id.clone(),
            GasKillerTaskRequest {
                body: request_body(),
            },
        );

        assert!(dispatcher.dispatch(queued).await.is_none());

        // Untouched: not re-dispatched (which resolution would have settled as `failed` against
        // the unreachable validator) and still carrying the sweep's reason.
        let settled = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Expired);
        assert_eq!(
            settled.error.as_deref(),
            Some("QUEUE_TTL_EXCEEDED: expired by the sweep")
        );
    }
}

//! Task persistence backed by the [`SqliteStore`].
//!
//! A task is the durable record of one submitted aggregation request. It is created in the
//! [`TaskStatus::Queued`] state, advances to [`TaskStatus::Processing`] while the router
//! aggregates operator signatures, and settles in a terminal state: [`TaskStatus::Ready`]
//! (with an executable `payload`), [`TaskStatus::Failed`], or [`TaskStatus::Expired`] (each
//! carrying an `error`).
//!
//! The full request that created the task is stored alongside its state so the router can
//! rebuild and re-enqueue any task still `queued` or `processing` after a restart — an
//! in-flight request must never be lost when the pod recycles. Every task is scoped to the
//! API key that created it (`key_id`), which drives both ownership checks and per-key listing.

use std::str::FromStr;

use alloy_primitives::{Address, U256};
use anyhow::Context;
use sqlx::FromRow;
use uuid::Uuid;

use super::SqliteStore;
use crate::ingress::GasKillerTaskRequestBody;

/// Columns selected for every task read, in the order the [`TaskRow`] fields expect. sqlx maps
/// by column name, but listing them once keeps the queries consistent and self-documenting.
const TASK_COLUMNS: &str = "id, key_id, status, target_address, call_data, transition_index, \
     from_address, value, block_height, payload, bundle, error, created_at, updated_at";

/// Columns written by every task insert. Shared by [`SqliteStore::create_task`] and
/// [`SqliteStore::create_task_deduplicated`] so a new column is added to both inserts at once
/// rather than diverging silently.
const TASK_INSERT_COLUMNS: &str = "id, key_id, status, target_address, call_data, \
     transition_index, from_address, value, block_height";

/// Lifecycle state of a task as it moves through aggregation.
///
/// The string forms are the on-the-wire values (via serde) and the values persisted in the
/// `status` column; the migration's CHECK constraint pins the same set.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[schema(
    description = "Lifecycle state of a task as it moves through aggregation. `ready` is \
                        the state in which a task carries a submittable payload; `failed` and \
                        `expired` both carry an `error`."
)]
pub enum TaskStatus {
    /// Accepted and waiting in the queue.
    Queued,
    /// Dequeued; the router is aggregating operator signatures.
    Processing,
    /// Aggregation succeeded; `payload` holds the executable calldata.
    Ready,
    /// Aggregation failed; `error` explains why.
    Failed,
    /// The task lapsed before completing; `error` explains why.
    Expired,
}

impl TaskStatus {
    /// The value stored in the `status` column, matching the migration's CHECK constraint.
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Processing => "processing",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Expired => "expired",
        }
    }

    /// Parses a status read back from the store. Errors on any value outside the known set,
    /// which the CHECK constraint should make unreachable in practice.
    fn from_db(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "queued" => Self::Queued,
            "processing" => Self::Processing,
            "ready" => Self::Ready,
            "failed" => Self::Failed,
            "expired" => Self::Expired,
            other => anyhow::bail!("unknown task status in store: {other}"),
        })
    }
}

/// A non-terminal lifecycle stage the TTL expiry sweep can settle, together with the clock it
/// ages against.
///
/// `processing` is deliberately absent: its aggregation height is already assigned and the round
/// must resolve either way, so cancelling the row mid-round would only lose the attribution of
/// the result that is still coming. A task that outlives the TTL while processing is expired once
/// its round settles, not during it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryStage {
    /// Accepted but never dequeued: the queue drained too slowly for the task's pinned block.
    Queued,
    /// Aggregated into a payload the client never collected. Sweeping it stops the router serving
    /// that payload again and frees the deduplication slot for its transition index.
    ///
    /// The default TTL is sized to the payload's own on-chain life, so the sweep normally arrives
    /// after `valid_until_block` has already passed and only clears server state. Under a TTL set
    /// shorter than that window it arrives first, withdrawing a payload the chain would still
    /// accept — which does not invalidate a payload a client already holds, since
    /// `valid_until_block` remains the on-chain authority.
    Ready,
}

impl ExpiryStage {
    /// Every stage the sweep visits, in the order it visits them.
    pub const ALL: [Self; 2] = [Self::Queued, Self::Ready];

    /// The `status` value this stage matches, as a SQL literal. Named literally rather than bound
    /// as a parameter so SQLite can match the partial index covering the stage (see
    /// `0006_task_expiry_index.sql`).
    fn status_literal(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Ready => "ready",
        }
    }

    /// The column the stage's age is measured from: when the request was accepted for `queued`,
    /// and when aggregation recorded the payload for `ready`.
    fn age_column(self) -> &'static str {
        match self {
            Self::Queued => "created_at",
            Self::Ready => "updated_at",
        }
    }

    /// Machine-readable code recorded at the front of the expired row's `error`, so a client
    /// polling the task can tell a TTL sweep from the read path's staleness checks.
    pub fn code(self) -> &'static str {
        match self {
            Self::Queued => "QUEUE_TTL_EXCEEDED",
            Self::Ready => "READY_TTL_EXCEEDED",
        }
    }

    /// The full `error` recorded on rows this stage expires, for a TTL of `ttl_secs`.
    fn reason(self, ttl_secs: i64) -> String {
        let detail = match self {
            Self::Queued => "not dequeued within the",
            Self::Ready => "payload not collected within the",
        };
        format!("{}: {detail} {ttl_secs}s task TTL; re-request", self.code())
    }
}

/// A persisted task: the request that created it, its current lifecycle state, and the outputs
/// produced as it settles.
#[derive(Debug, Clone)]
pub struct Task {
    /// UUID v4 identifying the task globally.
    pub id: String,
    /// Id of the API key that submitted the task; scopes ownership and listing.
    pub key_id: String,
    pub status: TaskStatus,
    /// The original request, preserved so the task can be rebuilt and re-enqueued on restart.
    pub request: GasKillerTaskRequestBody,
    /// Serialized [`gas_killer_common::PayloadView`] — the ready-to-sign transaction request,
    /// populated once the task is [`TaskStatus::Ready`].
    pub payload: Option<String>,
    /// Serialized [`gas_killer_common::TaskBundle`] — the structured completed round the payload
    /// is rendered from, populated alongside `payload` once the task is [`TaskStatus::Ready`].
    pub bundle: Option<String>,
    /// Human-readable failure reason, populated once the task is `failed` or `expired`.
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Statuses a task can still leave, spelled as the complement of the terminal ones.
///
/// Equivalent to `status IN ('queued', 'processing')` under the table's `CHECK` constraint, and
/// the only one of the equivalent spellings SQLite's partial-index prover accepts for
/// `idx_tasks_incomplete`. Must stay in step with that index's own predicate;
/// `the_paged_walk_uses_the_incomplete_index` fails if they drift.
const IN_FLIGHT_PREDICATE: &str = "status NOT IN ('ready', 'failed', 'expired')";

/// Reads the upper bound of a re-queue walk. See [`SqliteStore::last_incomplete_task`].
fn last_incomplete_task_sql() -> String {
    format!(
        "SELECT created_at, id FROM tasks WHERE {IN_FLIGHT_PREDICATE} \
         ORDER BY created_at DESC, id DESC LIMIT 1"
    )
}

/// Reads one page of a re-queue walk. See [`SqliteStore::incomplete_tasks_page`].
fn incomplete_tasks_page_sql() -> String {
    format!(
        "SELECT {TASK_COLUMNS} FROM tasks WHERE {IN_FLIGHT_PREDICATE} \
         AND (created_at, id) > (?1, ?2) AND (created_at, id) <= (?3, ?4) \
         ORDER BY created_at, id LIMIT ?5"
    )
}

/// Position in the `(created_at, id)` ordering the startup re-queue walks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCursor {
    pub created_at: i64,
    pub id: String,
}

/// Outcome of a deduplicated submission: the task the client polls, and whether the submission
/// collapsed onto an existing task (`true`) rather than creating a fresh one (`false`).
#[derive(Debug, Clone)]
pub struct SubmittedTask {
    pub task: Task,
    pub deduplicated: bool,
}

/// Raw column values as read from SQLite, before Ethereum types are parsed back out of their
/// text encodings. Kept private; callers only ever see [`Task`].
#[derive(FromRow)]
struct TaskRow {
    id: String,
    key_id: String,
    status: String,
    target_address: String,
    call_data: Vec<u8>,
    transition_index: Option<i64>,
    from_address: String,
    value: String,
    block_height: i64,
    payload: Option<String>,
    bundle: Option<String>,
    error: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<TaskRow> for Task {
    type Error = anyhow::Error;

    fn try_from(row: TaskRow) -> anyhow::Result<Self> {
        let request = GasKillerTaskRequestBody {
            target_address: Address::from_str(&row.target_address)
                .with_context(|| format!("parsing target_address for task {}", row.id))?,
            call_data: row.call_data,
            transition_index: row.transition_index.map(|i| i as u64),
            from_address: Address::from_str(&row.from_address)
                .with_context(|| format!("parsing from_address for task {}", row.id))?,
            value: U256::from_str(&row.value)
                .with_context(|| format!("parsing value for task {}", row.id))?,
            block_height: row.block_height as u64,
        };

        Ok(Task {
            status: TaskStatus::from_db(&row.status)
                .with_context(|| format!("reading status for task {}", row.id))?,
            id: row.id,
            key_id: row.key_id,
            request,
            payload: row.payload,
            bundle: row.bundle,
            error: row.error,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

impl SqliteStore {
    /// Persists a new task in the [`TaskStatus::Queued`] state for the given API key, assigning
    /// it a fresh UUID v4. The full request is stored so the task can be re-enqueued on restart.
    /// Returns the created [`Task`], including the store-assigned timestamps.
    ///
    /// Fails if `key_id` does not reference an existing key (enforced by the foreign key).
    ///
    /// Inserts unconditionally. Route explicit-`transition_index` submissions through
    /// [`SqliteStore::create_task_deduplicated`] instead, so the one-active-task-per
    /// `(key_id, target_address, transition_index)` invariant that ingress deduplication relies on
    /// keeps holding.
    pub async fn create_task(
        &self,
        key_id: &str,
        request: &GasKillerTaskRequestBody,
    ) -> anyhow::Result<Task> {
        let id = Uuid::new_v4().to_string();

        let (created_at, updated_at): (i64, i64) = sqlx::query_as(&format!(
            "INSERT INTO tasks ({TASK_INSERT_COLUMNS}) \
             VALUES (?1, ?2, 'queued', ?3, ?4, ?5, ?6, ?7, ?8) \
             RETURNING created_at, updated_at",
        ))
        .bind(&id)
        .bind(key_id)
        .bind(request.target_address.to_string())
        .bind(request.call_data.as_slice())
        .bind(request.transition_index.map(|i| i as i64))
        .bind(request.from_address.to_string())
        .bind(request.value.to_string())
        .bind(request.block_height as i64)
        .fetch_one(self.pool())
        .await
        .context("inserting task")?;

        Ok(Task {
            id,
            key_id: key_id.to_string(),
            status: TaskStatus::Queued,
            request: request.clone(),
            payload: None,
            bundle: None,
            error: None,
            created_at,
            updated_at,
        })
    }

    /// Persists a task for the given API key, collapsing a duplicate submission onto the existing
    /// task instead of creating a second one.
    ///
    /// A client that retries after a timeout submits the same logical request twice. Without
    /// deduplication both tasks pass validation, but only the first to reach the chain succeeds —
    /// the second's `transition_index` is already consumed — so the retry races a doomed
    /// transaction. Keying idempotency on `(key_id, target_address, transition_index)` makes the
    /// retry safe: it returns the existing task rather than creating a duplicate.
    ///
    /// A match counts as a duplicate only while it is in flight (`queued`/`processing`) or `ready`.
    /// A `failed` or `expired` task is not a duplicate — its work must be re-run — so a submission
    /// matching one creates a fresh task. Deduplication applies only to an explicit
    /// `transition_index`; an `auto` (NULL) request leaves the slot for the server to resolve at
    /// dequeue time, so two `auto` submissions are distinct requests that each take their own slot
    /// (safe parallel submissions) and are never collapsed.
    ///
    /// The duplicate check and the insert are one atomic statement — an `INSERT ... SELECT` guarded
    /// by `WHERE NOT EXISTS` — so a concurrent retry cannot slip between them. SQLite serializes
    /// writers, so a racer that loses observes the winner's committed row, inserts nothing, and this
    /// method returns that task as the deduplicated result.
    pub async fn create_task_deduplicated(
        &self,
        key_id: &str,
        request: &GasKillerTaskRequestBody,
    ) -> anyhow::Result<SubmittedTask> {
        self.create_task_deduplicated_hooked(key_id, request, || async {})
            .await
    }

    /// Core of [`SqliteStore::create_task_deduplicated`], with a test seam. `on_blocked_insert`
    /// runs after an insert is turned away by the guard and before the duplicate is read back —
    /// the window in which the blocking task can go terminal — letting tests force that transition
    /// and assert the retry yields a fresh task rather than an error.
    async fn create_task_deduplicated_hooked<F, Fut>(
        &self,
        key_id: &str,
        request: &GasKillerTaskRequestBody,
        mut on_blocked_insert: F,
    ) -> anyhow::Result<SubmittedTask>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        // Auto submissions carry no idempotency key, so they always create a fresh task.
        let Some(transition_index) = request.transition_index else {
            let task = self.create_task(key_id, request).await?;
            return Ok(SubmittedTask {
                task,
                deduplicated: false,
            });
        };

        // The guard can block the insert (an active duplicate exists) yet the read-back find
        // nothing, when that duplicate transitions to a terminal status in the window between the
        // two statements. That window is correlated with the slow round the client is retrying, so
        // rather than surface a transient error we re-attempt: the blocker is now terminal, so the
        // next insert's `NOT EXISTS` passes and creates the fresh task the state machine implies.
        // Bounded so a pathologically churning slot cannot loop forever.
        const MAX_INSERT_ATTEMPTS: usize = 3;
        for _ in 0..MAX_INSERT_ATTEMPTS {
            let id = Uuid::new_v4().to_string();
            let inserted: Option<(i64, i64)> = sqlx::query_as(&format!(
                "INSERT INTO tasks ({TASK_INSERT_COLUMNS}) \
                 SELECT ?1, ?2, 'queued', ?3, ?4, ?5, ?6, ?7, ?8 \
                 WHERE NOT EXISTS ( \
                     SELECT 1 FROM tasks \
                     WHERE key_id = ?2 AND target_address = ?3 AND transition_index = ?5 \
                       AND status IN ('queued', 'processing', 'ready') \
                 ) \
                 RETURNING created_at, updated_at",
            ))
            .bind(&id)
            .bind(key_id)
            .bind(request.target_address.to_string())
            .bind(request.call_data.as_slice())
            .bind(transition_index as i64)
            .bind(request.from_address.to_string())
            .bind(request.value.to_string())
            .bind(request.block_height as i64)
            .fetch_optional(self.pool())
            .await
            .context("inserting task with deduplication")?;

            match inserted {
                Some((created_at, updated_at)) => {
                    return Ok(SubmittedTask {
                        task: Task {
                            id,
                            key_id: key_id.to_string(),
                            status: TaskStatus::Queued,
                            request: request.clone(),
                            payload: None,
                            bundle: None,
                            error: None,
                            created_at,
                            updated_at,
                        },
                        deduplicated: false,
                    });
                }
                // The guard's `NOT EXISTS` was false: an active task already covers this work.
                None => {
                    on_blocked_insert().await;
                    if let Some(task) = self
                        .active_duplicate_task(key_id, request.target_address, transition_index)
                        .await?
                    {
                        return Ok(SubmittedTask {
                            task,
                            deduplicated: true,
                        });
                    }
                    // The blocker went terminal between the insert and the read-back; loop to
                    // re-attempt the insert, which now sees no active duplicate.
                }
            }
        }

        anyhow::bail!(
            "deduplication insert blocked without an active duplicate across \
             {MAX_INSERT_ATTEMPTS} attempts"
        )
    }

    /// Finds the active task a duplicate submission collapses onto: the newest `queued`,
    /// `processing`, or `ready` task for `(key_id, target_address, transition_index)`. Terminal
    /// (`failed`/`expired`) tasks are excluded so their work can be re-submitted. Returns `None`
    /// when no such task exists.
    async fn active_duplicate_task(
        &self,
        key_id: &str,
        target_address: Address,
        transition_index: u64,
    ) -> anyhow::Result<Option<Task>> {
        let row: Option<TaskRow> = sqlx::query_as(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks \
             WHERE key_id = ?1 AND target_address = ?2 AND transition_index = ?3 \
               AND status IN ('queued', 'processing', 'ready') \
             ORDER BY created_at DESC, id LIMIT 1",
        ))
        .bind(key_id)
        .bind(target_address.to_string())
        .bind(transition_index as i64)
        .fetch_optional(self.pool())
        .await
        .context("finding active duplicate task")?;

        row.map(Task::try_from).transpose()
    }

    /// Fetches a task by id, or `None` if no such task exists. The returned task carries its
    /// `key_id`, which callers compare against the requesting key to enforce ownership.
    pub async fn get_task(&self, id: &str) -> anyhow::Result<Option<Task>> {
        let row: Option<TaskRow> =
            sqlx::query_as(&format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"))
                .bind(id)
                .fetch_optional(self.pool())
                .await
                .context("fetching task")?;

        row.map(Task::try_from).transpose()
    }

    /// Lists a single API key's tasks, newest first, optionally filtered to one status. `limit`
    /// caps the page size and `offset` skips earlier rows, together paginating the listing.
    pub async fn list_tasks_for_key(
        &self,
        key_id: &str,
        status: Option<TaskStatus>,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<Task>> {
        let rows: Vec<TaskRow> = sqlx::query_as(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks \
             WHERE key_id = ?1 AND (?2 IS NULL OR status = ?2) \
             ORDER BY created_at DESC, id \
             LIMIT ?3 OFFSET ?4",
        ))
        .bind(key_id)
        .bind(status.map(TaskStatus::as_str))
        .bind(limit)
        .bind(offset)
        .fetch_all(self.pool())
        .await
        .context("listing tasks")?;

        rows.into_iter().map(Task::try_from).collect()
    }

    /// Claims a task for aggregation, moving it to [`TaskStatus::Processing`] only while it is
    /// still `queued` or already `processing`. Returns `false` when the id is unknown or the task
    /// has already settled.
    ///
    /// The guard is what keeps the TTL sweep and the sequencer from fighting: a queued task the
    /// sweep expires is still sitting in the sequencer's channel, and an unguarded status write
    /// would resurrect it into a round whose payload is already too stale to land. A caller that
    /// sees `false` must drop the task instead of aggregating it. `processing` is accepted so the
    /// startup re-queue can re-claim a task whose round a restart interrupted.
    pub async fn claim_task_for_processing(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE tasks SET status = 'processing', updated_at = unixepoch() \
             WHERE id = ?1 AND status IN ('queued', 'processing')",
        )
        .bind(id)
        .execute(self.pool())
        .await
        .context("claiming task for processing")?;

        Ok(result.rows_affected() > 0)
    }

    /// Settles a task as [`TaskStatus::Ready`], recording its executable payload and stamping
    /// `updated_at`. Returns `true` if a task with that id existed.
    pub async fn mark_task_ready(&self, id: &str, payload: &str) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE tasks SET status = 'ready', payload = ?2, updated_at = unixepoch() \
             WHERE id = ?1",
        )
        .bind(id)
        .bind(payload)
        .execute(self.pool())
        .await
        .context("marking task ready")?;

        Ok(result.rows_affected() > 0)
    }

    /// Settles a task as [`TaskStatus::Ready`], recording both the rendered transaction-request
    /// `payload` and the structured round `bundle` it was rendered from, and stamping
    /// `updated_at`. Both are stored as JSON.
    ///
    /// Returns how many whole seconds the task spent between being accepted at the ingress and
    /// settling here — its end-to-end latency, measured in the same statement that settles it —
    /// or `None` when no task with that id existed.
    pub async fn mark_task_ready_with_bundle(
        &self,
        id: &str,
        payload: &str,
        bundle: &str,
    ) -> anyhow::Result<Option<i64>> {
        sqlx::query_scalar(
            "UPDATE tasks SET status = 'ready', payload = ?2, bundle = ?3, \
             updated_at = unixepoch() WHERE id = ?1 \
             RETURNING unixepoch() - created_at",
        )
        .bind(id)
        .bind(payload)
        .bind(bundle)
        .fetch_optional(self.pool())
        .await
        .context("marking task ready with bundle")
    }

    /// Settles a task as [`TaskStatus::Expired`], recording why and stamping `updated_at`.
    ///
    /// The read path uses this when a ready payload is no longer submittable — its
    /// `valid_until_block` has passed or the on-chain transition index has advanced — so a later
    /// poll short-circuits to the re-request error without another chain read. Returns `true` if a
    /// task with that id existed.
    pub async fn mark_task_expired(&self, id: &str, reason: &str) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE tasks SET status = 'expired', error = ?2, updated_at = unixepoch() \
             WHERE id = ?1",
        )
        .bind(id)
        .bind(reason)
        .execute(self.pool())
        .await
        .context("marking task expired")?;

        Ok(result.rows_affected() > 0)
    }

    /// Settles every task in `stage` that has aged past `ttl` as [`TaskStatus::Expired`],
    /// recording the stage's reason, and returns the ids it expired (empty when none had).
    ///
    /// This is the store half of the periodic expiry sweep. It bounds how long a task can hold
    /// state that is no longer useful: a `queued` task whose pinned block has gone stale would
    /// spend a full aggregation round producing a payload the chain rejects, and a `ready` payload
    /// nobody collected keeps blocking resubmission for its transition index. Expiring both frees
    /// that capacity for work that can still land.
    ///
    /// Age is measured in the store's own clock (`unixepoch()`), so the sweep is unaffected by
    /// skew between the router process and the database, and it survives a restart: the row's
    /// timestamps, not any in-memory timer, decide what has lapsed.
    pub async fn expire_stale_tasks(
        &self,
        stage: ExpiryStage,
        ttl: std::time::Duration,
    ) -> anyhow::Result<Vec<String>> {
        let ttl_secs = ttl.as_secs().min(i64::MAX as u64) as i64;
        let ids: Vec<(String,)> = sqlx::query_as(&format!(
            "UPDATE tasks SET status = 'expired', error = ?1, updated_at = unixepoch() \
             WHERE status = '{status}' AND {age} <= unixepoch() - ?2 \
             RETURNING id",
            status = stage.status_literal(),
            age = stage.age_column(),
        ))
        .bind(stage.reason(ttl_secs))
        .bind(ttl_secs)
        .fetch_all(self.pool())
        .await
        .with_context(|| format!("expiring {} tasks past their TTL", stage.status_literal()))?;

        Ok(ids.into_iter().map(|(id,)| id).collect())
    }

    /// Settles a task as [`TaskStatus::Failed`], recording the failure reason and stamping
    /// `updated_at`.
    ///
    /// Returns the task's end-to-end latency in whole seconds, as
    /// [`mark_task_ready_with_bundle`](Self::mark_task_ready_with_bundle) does, or `None` when no
    /// task with that id existed.
    pub async fn mark_task_failed(&self, id: &str, error: &str) -> anyhow::Result<Option<i64>> {
        sqlx::query_scalar(
            "UPDATE tasks SET status = 'failed', error = ?2, updated_at = unixepoch() \
             WHERE id = ?1 RETURNING unixepoch() - created_at",
        )
        .bind(id)
        .bind(error)
        .fetch_optional(self.pool())
        .await
        .context("marking task failed")
    }

    /// The last task still in flight in `(created_at, id)` order, or `None` when none are.
    ///
    /// The startup re-queue takes this before the ingress serves and walks up to it, so the walk
    /// covers exactly the backlog a previous router life left and never a row the running ingress
    /// has already put in the channel itself.
    pub async fn last_incomplete_task(&self) -> anyhow::Result<Option<TaskCursor>> {
        let row: Option<(i64, String)> = sqlx::query_as(&last_incomplete_task_sql())
            .fetch_optional(self.pool())
            .await
            .context("loading the last incomplete task")?;

        Ok(row.map(|(created_at, id)| TaskCursor { created_at, id }))
    }

    /// One page of tasks still in flight (`queued` or `processing`), ordered by
    /// `(created_at, id)`, starting after `after` and ending at `through`.
    ///
    /// Keyed on the ordering rather than an offset because the caller settles and re-enqueues
    /// rows as it drains, which moves rows out of the in-flight set mid-walk: a cursor resumes at
    /// the right row regardless, where an offset would skip as many rows as left the set.
    ///
    /// The SQL selects the complement of the terminal statuses, which is the same set under the
    /// table's `CHECK` constraint and the only spelling the partial index serving this walk will
    /// match. See `0007_task_requeue_index.sql`.
    pub async fn incomplete_tasks_page(
        &self,
        after: Option<&TaskCursor>,
        through: &TaskCursor,
        limit: u32,
    ) -> anyhow::Result<Vec<Task>> {
        // A `None` cursor starts before every row: `created_at` is `unixepoch()`, never negative,
        // and no id sorts below the empty string.
        let (after_created_at, after_id) = after
            .map(|c| (c.created_at, c.id.as_str()))
            .unwrap_or((-1, ""));

        let rows: Vec<TaskRow> = sqlx::query_as(&incomplete_tasks_page_sql())
            .bind(after_created_at)
            .bind(after_id)
            .bind(through.created_at)
            .bind(&through.id)
            .bind(limit)
            .fetch_all(self.pool())
            .await
            .context("loading a page of incomplete tasks")?;

        rows.into_iter().map(Task::try_from).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> SqliteStore {
        SqliteStore::connect_in_memory()
            .await
            .expect("in-memory store should open and migrate")
    }

    /// Creates an API key and returns its id, satisfying the tasks table's foreign key.
    async fn key_id(store: &SqliteStore) -> String {
        store
            .create_api_key(None, None)
            .await
            .expect("key creation should succeed")
            .id
    }

    fn request() -> GasKillerTaskRequestBody {
        GasKillerTaskRequestBody {
            target_address: Address::from([0x11; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0xab],
            transition_index: Some(7),
            from_address: Address::from([0x22; 20]),
            value: U256::from(1_000_000u64),
            block_height: 21_000_000,
        }
    }

    #[tokio::test]
    async fn created_task_is_queued_with_uuid_and_timestamps() {
        let store = store().await;
        let key = key_id(&store).await;

        let task = store
            .create_task(&key, &request())
            .await
            .expect("task creation should succeed");

        assert_eq!(task.status, TaskStatus::Queued);
        assert_eq!(task.key_id, key);
        assert!(task.payload.is_none());
        assert!(task.error.is_none());
        assert!(task.created_at > 0);
        assert!(task.updated_at >= task.created_at);

        let parsed = Uuid::parse_str(&task.id).expect("id should be a UUID");
        assert_eq!(parsed.get_version_num(), 4, "task ids must be UUID v4");
    }

    #[tokio::test]
    async fn create_rejects_unknown_key() {
        let store = store().await;
        let result = store.create_task("no-such-key", &request()).await;
        assert!(
            result.is_err(),
            "the foreign key should reject a task for a key that does not exist"
        );
    }

    #[tokio::test]
    async fn get_round_trips_request_fields() {
        let store = store().await;
        let key = key_id(&store).await;
        let created = store.create_task(&key, &request()).await.unwrap();

        let fetched = store
            .get_task(&created.id)
            .await
            .unwrap()
            .expect("created task should be fetchable");

        let req = request();
        assert_eq!(fetched.request.target_address, req.target_address);
        assert_eq!(fetched.request.from_address, req.from_address);
        assert_eq!(fetched.request.call_data, req.call_data);
        assert_eq!(fetched.request.transition_index, req.transition_index);
        assert_eq!(fetched.request.value, req.value);
        assert_eq!(fetched.request.block_height, req.block_height);
    }

    #[tokio::test]
    async fn auto_transition_index_round_trips_as_none() {
        let store = store().await;
        let key = key_id(&store).await;
        let mut req = request();
        req.transition_index = None;

        let created = store.create_task(&key, &req).await.unwrap();
        let fetched = store.get_task(&created.id).await.unwrap().unwrap();
        assert_eq!(fetched.request.transition_index, None);
    }

    #[tokio::test]
    async fn get_unknown_task_is_none() {
        let store = store().await;
        assert!(store.get_task("does-not-exist").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_is_scoped_to_key_and_newest_first() {
        let store = store().await;
        let key_a = key_id(&store).await;
        let key_b = key_id(&store).await;

        store.create_task(&key_a, &request()).await.unwrap();
        store.create_task(&key_a, &request()).await.unwrap();
        store.create_task(&key_b, &request()).await.unwrap();

        let listed = store
            .list_tasks_for_key(&key_a, None, 100, 0)
            .await
            .unwrap();
        assert_eq!(listed.len(), 2, "listing must be scoped to the key");
        assert!(listed.iter().all(|t| t.key_id == key_a));
        // created_at is non-increasing down the list (ties broken by id).
        assert!(
            listed
                .windows(2)
                .all(|w| w[0].created_at >= w[1].created_at),
            "tasks must be ordered newest first"
        );
    }

    #[tokio::test]
    async fn list_filters_by_status() {
        let store = store().await;
        let key = key_id(&store).await;
        let ready = store.create_task(&key, &request()).await.unwrap();
        store.create_task(&key, &request()).await.unwrap();

        store
            .mark_task_ready(&ready.id, "0xdeadbeef")
            .await
            .unwrap();

        let ready_only = store
            .list_tasks_for_key(&key, Some(TaskStatus::Ready), 100, 0)
            .await
            .unwrap();
        assert_eq!(ready_only.len(), 1);
        assert_eq!(ready_only[0].id, ready.id);

        let queued_only = store
            .list_tasks_for_key(&key, Some(TaskStatus::Queued), 100, 0)
            .await
            .unwrap();
        assert_eq!(queued_only.len(), 1);
    }

    #[tokio::test]
    async fn list_paginates_with_limit_and_offset() {
        let store = store().await;
        let key = key_id(&store).await;
        for _ in 0..3 {
            store.create_task(&key, &request()).await.unwrap();
        }

        let first_page = store.list_tasks_for_key(&key, None, 2, 0).await.unwrap();
        assert_eq!(first_page.len(), 2);

        let second_page = store.list_tasks_for_key(&key, None, 2, 2).await.unwrap();
        assert_eq!(second_page.len(), 1);
    }

    #[tokio::test]
    async fn mark_ready_records_payload() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request()).await.unwrap();

        assert!(store.mark_task_ready(&task.id, "0xcafe").await.unwrap());
        let fetched = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::Ready);
        assert_eq!(fetched.payload.as_deref(), Some("0xcafe"));
        assert!(fetched.error.is_none());
    }

    #[tokio::test]
    async fn mark_ready_with_bundle_records_both() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request()).await.unwrap();

        assert!(
            store
                .mark_task_ready_with_bundle(&task.id, "{\"payload\":1}", "{\"bundle\":2}")
                .await
                .unwrap()
                .is_some(),
            "settling an existing task should report its latency"
        );
        let fetched = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::Ready);
        assert_eq!(fetched.payload.as_deref(), Some("{\"payload\":1}"));
        assert_eq!(fetched.bundle.as_deref(), Some("{\"bundle\":2}"));
        assert!(fetched.error.is_none());
    }

    #[tokio::test]
    async fn mark_expired_records_reason() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request()).await.unwrap();
        store
            .mark_task_ready_with_bundle(&task.id, "{}", "{}")
            .await
            .unwrap();

        assert!(
            store
                .mark_task_expired(&task.id, "payload validity window elapsed")
                .await
                .unwrap()
        );
        let fetched = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::Expired);
        assert_eq!(
            fetched.error.as_deref(),
            Some("payload validity window elapsed")
        );

        assert!(
            !store
                .mark_task_expired("does-not-exist", "nope")
                .await
                .unwrap(),
            "expiring an unknown task should report no change"
        );
    }

    #[tokio::test]
    async fn settling_a_task_reports_how_long_it_took() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request()).await.unwrap();

        // Backdate the submission so the reported figure is unambiguous, rather than the 0 a test
        // that runs in well under a second would otherwise produce.
        sqlx::query("UPDATE tasks SET created_at = created_at - 90 WHERE id = ?1")
            .bind(&task.id)
            .execute(store.pool())
            .await
            .unwrap();

        let elapsed = store
            .mark_task_ready_with_bundle(&task.id, "{}", "{}")
            .await
            .unwrap()
            .expect("settling an existing task reports its latency");
        // A one-second window: the statement can land in the second after the backdating.
        assert!(
            (90..=91).contains(&elapsed),
            "expected roughly 90s since submission, got {elapsed}"
        );

        // A task that never existed has no latency to report, so nothing is observed for it.
        assert!(
            store
                .mark_task_failed("no-such-task", "nope")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn mark_failed_records_error() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request()).await.unwrap();

        assert!(
            store
                .mark_task_failed(&task.id, "aggregation timed out")
                .await
                .unwrap()
                .is_some(),
            "settling an existing task should report its latency"
        );
        let fetched = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::Failed);
        assert_eq!(fetched.error.as_deref(), Some("aggregation timed out"));
        assert!(fetched.payload.is_none());
    }

    #[tokio::test]
    async fn incomplete_tasks_returns_only_in_flight() {
        let store = store().await;
        let key = key_id(&store).await;

        let queued = store.create_task(&key, &request()).await.unwrap();
        let processing = store.create_task(&key, &request()).await.unwrap();
        let ready = store.create_task(&key, &request()).await.unwrap();
        let failed = store.create_task(&key, &request()).await.unwrap();

        store
            .claim_task_for_processing(&processing.id)
            .await
            .unwrap();
        store.mark_task_ready(&ready.id, "0x00").await.unwrap();
        store.mark_task_failed(&failed.id, "nope").await.unwrap();

        let through = store
            .last_incomplete_task()
            .await
            .unwrap()
            .expect("two tasks are in flight");
        let incomplete = store
            .incomplete_tasks_page(None, &through, 100)
            .await
            .unwrap();
        let ids: Vec<&str> = incomplete.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            incomplete.len(),
            2,
            "only queued and processing are in flight"
        );
        assert!(ids.contains(&queued.id.as_str()));
        assert!(ids.contains(&processing.id.as_str()));
    }

    /// SQLite only uses a partial index when the query's WHERE implies the index's own, so
    /// [`IN_FLIGHT_PREDICATE`] and `idx_tasks_incomplete`'s predicate must stay in step. Without
    /// the index every page sorts the whole matching set, which is the cost paging exists to
    /// avoid. Both plans are taken from the builders the store itself binds, so this fails on a
    /// change to the real query rather than to a copy of it.
    #[tokio::test]
    async fn the_paged_walk_uses_the_incomplete_index() {
        let store = store().await;
        let key = key_id(&store).await;
        store.create_task(&key, &request()).await.unwrap();
        let through = store.last_incomplete_task().await.unwrap().unwrap();

        let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&format!(
            "EXPLAIN QUERY PLAN {}",
            incomplete_tasks_page_sql()
        ))
        .bind(-1i64)
        .bind("")
        .bind(through.created_at)
        .bind(&through.id)
        .bind(10u32)
        .fetch_all(store.pool())
        .await
        .unwrap();

        let detail = plan
            .iter()
            .map(|(_, _, _, d)| d.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            detail.contains("idx_tasks_incomplete"),
            "the walk should seek the partial index, got: {detail}"
        );
        assert!(
            !detail.contains("TEMP B-TREE"),
            "the index should supply the ordering, got: {detail}"
        );

        // The bound query runs once per startup over the whole table, so it must seek the same
        // index rather than scan.
        let bound: Vec<(i64, i64, i64, String)> = sqlx::query_as(&format!(
            "EXPLAIN QUERY PLAN {}",
            last_incomplete_task_sql()
        ))
        .fetch_all(store.pool())
        .await
        .unwrap();
        let bound_detail = bound
            .iter()
            .map(|(_, _, _, d)| d.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            bound_detail.contains("idx_tasks_incomplete") && !bound_detail.contains("TEMP B-TREE"),
            "the bound query should reverse-scan the index, got: {bound_detail}"
        );
    }

    #[tokio::test]
    async fn last_incomplete_task_is_none_when_nothing_is_in_flight() {
        let store = store().await;
        let key = key_id(&store).await;
        let done = store.create_task(&key, &request()).await.unwrap();
        store.mark_task_ready(&done.id, "0x00").await.unwrap();

        assert!(store.last_incomplete_task().await.unwrap().is_none());
    }

    /// The walk covers the backlog exactly once across pages, in `(created_at, id)` order.
    #[tokio::test]
    async fn paging_walks_the_whole_backlog_without_gaps_or_repeats() {
        let store = store().await;
        let key = key_id(&store).await;
        let mut created = Vec::new();
        for _ in 0..7 {
            created.push(store.create_task(&key, &request()).await.unwrap().id);
        }
        created.sort();

        let through = store.last_incomplete_task().await.unwrap().unwrap();
        let mut cursor = None;
        let mut seen = Vec::new();
        let mut pages = 0;
        loop {
            let page = store
                .incomplete_tasks_page(cursor.as_ref(), &through, 3)
                .await
                .unwrap();
            let Some(last) = page.last() else { break };
            pages += 1;
            cursor = Some(TaskCursor {
                created_at: last.created_at,
                id: last.id.clone(),
            });
            seen.extend(page.into_iter().map(|t| t.id));
        }

        assert_eq!(pages, 3, "7 rows at a page size of 3 takes three pages");
        // Every row shares a second here, so `(created_at, id)` order is uuid order: comparing
        // against the sorted ids unsorted pins the traversal order as well as the coverage.
        assert_eq!(
            seen, created,
            "every row appears exactly once, oldest first"
        );
    }

    /// Settling rows mid-walk is what breaks offset paging; a cursor must still land on the rest.
    #[tokio::test]
    async fn paging_survives_rows_leaving_the_set_mid_walk() {
        let store = store().await;
        let key = key_id(&store).await;
        let mut created = Vec::new();
        for _ in 0..6 {
            created.push(store.create_task(&key, &request()).await.unwrap().id);
        }

        let through = store.last_incomplete_task().await.unwrap().unwrap();
        let first = store
            .incomplete_tasks_page(None, &through, 2)
            .await
            .unwrap();
        assert_eq!(first.len(), 2);

        // Take the whole first page out of the `queued`/`processing` set, as the re-queue does
        // when it settles a task whose transition index is spent.
        for task in &first {
            store.mark_task_expired(&task.id, "spent").await.unwrap();
        }

        let cursor = TaskCursor {
            created_at: first[1].created_at,
            id: first[1].id.clone(),
        };
        let mut seen: Vec<String> = first.iter().map(|t| t.id.clone()).collect();
        let mut next = Some(cursor);
        while let Some(c) = next.take() {
            let page = store
                .incomplete_tasks_page(Some(&c), &through, 2)
                .await
                .unwrap();
            let Some(last) = page.last() else { break };
            next = Some(TaskCursor {
                created_at: last.created_at,
                id: last.id.clone(),
            });
            seen.extend(page.into_iter().map(|t| t.id));
        }

        seen.sort();
        created.sort();
        assert_eq!(
            seen, created,
            "no row is skipped when earlier rows leave the set"
        );
    }

    /// The bound's own tiebreak: a row sharing its second but ordering above it by id is outside
    /// the walk. Bounding on `created_at` alone would sweep it in.
    ///
    /// A row ordering at or below the bound is inside the walk by definition, so the walk cannot
    /// be what excludes a concurrently created one. That is why `create_ingress` reads the bound
    /// before the HTTP server starts, when no row this process accepted can exist yet.
    #[tokio::test]
    async fn paging_stops_above_the_bound_within_the_same_second() {
        let store = store().await;
        let key = key_id(&store).await;
        let existing = store.create_task(&key, &request()).await.unwrap();

        let through = store.last_incomplete_task().await.unwrap().unwrap();

        let later = store.create_task(&key, &request()).await.unwrap();
        let above = "ffffffff-ffff-ffff-ffff-ffffffffffff";
        sqlx::query("UPDATE tasks SET id = ?2, created_at = ?3 WHERE id = ?1")
            .bind(&later.id)
            .bind(above)
            .bind(through.created_at)
            .execute(store.pool())
            .await
            .unwrap();
        assert!(
            above > through.id.as_str(),
            "the planted row must sort above the bound's id for this to test the tiebreak"
        );

        let page = store
            .incomplete_tasks_page(None, &through, 100)
            .await
            .unwrap();
        let ids: Vec<&str> = page.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![existing.id.as_str()],
            "only the id tiebreak separates the planted row from the bound"
        );
    }

    // -- TTL expiry sweep --

    /// Backdates a task's timestamps, standing in for the wall-clock wait the sweep measures.
    async fn age_task(store: &SqliteStore, id: &str, secs: i64) {
        sqlx::query(
            "UPDATE tasks SET created_at = created_at - ?2, updated_at = updated_at - ?2 \
             WHERE id = ?1",
        )
        .bind(id)
        .bind(secs)
        .execute(store.pool())
        .await
        .expect("backdating a task should succeed");
    }

    #[tokio::test]
    async fn expire_stale_tasks_settles_only_rows_past_the_ttl() {
        let store = store().await;
        let key = key_id(&store).await;
        let stale = store.create_task(&key, &request()).await.unwrap();
        let fresh = store.create_task(&key, &request()).await.unwrap();
        age_task(&store, &stale.id, 400).await;

        let expired = store
            .expire_stale_tasks(ExpiryStage::Queued, std::time::Duration::from_secs(300))
            .await
            .unwrap();

        assert_eq!(expired, vec![stale.id.clone()]);
        let settled = store.get_task(&stale.id).await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Expired);
        assert!(
            settled
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("QUEUE_TTL_EXCEEDED") && e.contains("300s")),
            "the reason names the breached TTL: {:?}",
            settled.error
        );
        assert_eq!(
            store.get_task(&fresh.id).await.unwrap().unwrap().status,
            TaskStatus::Queued
        );
    }

    #[tokio::test]
    async fn expire_stale_tasks_is_empty_when_nothing_has_lapsed() {
        let store = store().await;
        let key = key_id(&store).await;
        store.create_task(&key, &request()).await.unwrap();

        for stage in ExpiryStage::ALL {
            assert!(
                store
                    .expire_stale_tasks(stage, std::time::Duration::from_secs(300))
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }

    /// An expired task is not an active duplicate, so sweeping one frees its transition index for
    /// a resubmission — the client's re-request after a `QUEUE_TTL_EXCEEDED` must be accepted.
    #[tokio::test]
    async fn sweeping_a_queued_task_frees_its_deduplication_slot() {
        let store = store().await;
        let key = key_id(&store).await;

        let first = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        age_task(&store, &first.task.id, 400).await;
        store
            .expire_stale_tasks(ExpiryStage::Queued, std::time::Duration::from_secs(300))
            .await
            .unwrap();

        let retry = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        assert!(
            !retry.deduplicated,
            "a resubmission after the sweep is fresh work, not a duplicate"
        );
        assert_ne!(retry.task.id, first.task.id);
    }

    #[tokio::test]
    async fn claim_moves_queued_to_processing_and_is_idempotent() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request()).await.unwrap();

        assert!(store.claim_task_for_processing(&task.id).await.unwrap());
        let claimed = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(claimed.status, TaskStatus::Processing);
        assert!(claimed.updated_at >= task.updated_at);
        assert!(
            store.claim_task_for_processing(&task.id).await.unwrap(),
            "re-claiming a processing task succeeds, so the startup re-queue can resume it"
        );
    }

    #[tokio::test]
    async fn claim_refuses_terminal_and_unknown_tasks() {
        let store = store().await;
        let key = key_id(&store).await;

        let expired = store.create_task(&key, &request()).await.unwrap();
        store.mark_task_expired(&expired.id, "swept").await.unwrap();
        let failed = store.create_task(&key, &request()).await.unwrap();
        store.mark_task_failed(&failed.id, "boom").await.unwrap();
        let ready = store.create_task(&key, &request()).await.unwrap();
        store.mark_task_ready(&ready.id, "0x00").await.unwrap();

        for id in [&expired.id, &failed.id, &ready.id] {
            assert!(
                !store.claim_task_for_processing(id).await.unwrap(),
                "a settled task must not be claimable"
            );
        }
        assert!(
            !store
                .claim_task_for_processing("does-not-exist")
                .await
                .unwrap()
        );
        assert_eq!(
            store.get_task(&expired.id).await.unwrap().unwrap().error,
            Some("swept".to_string()),
            "a refused claim leaves the settled reason intact"
        );
    }

    #[test]
    fn status_round_trips_through_db_encoding() {
        for status in [
            TaskStatus::Queued,
            TaskStatus::Processing,
            TaskStatus::Ready,
            TaskStatus::Failed,
            TaskStatus::Expired,
        ] {
            assert_eq!(TaskStatus::from_db(status.as_str()).unwrap(), status);
        }
        assert!(TaskStatus::from_db("bogus").is_err());
    }

    #[test]
    fn status_serializes_snake_case() {
        let json = serde_json::to_string(&TaskStatus::Processing).unwrap();
        assert_eq!(json, r#""processing""#);
    }

    // -- deduplication --

    #[tokio::test]
    async fn dedup_collapses_onto_queued() {
        let store = store().await;
        let key = key_id(&store).await;

        let first = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        assert!(!first.deduplicated, "the first submission creates a task");

        let second = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        assert!(
            second.deduplicated,
            "a retry while the task is queued collapses onto it"
        );
        assert_eq!(second.task.id, first.task.id);

        let listed = store.list_tasks_for_key(&key, None, 100, 0).await.unwrap();
        assert_eq!(listed.len(), 1, "no duplicate row is created");
    }

    #[tokio::test]
    async fn dedup_collapses_onto_processing() {
        let store = store().await;
        let key = key_id(&store).await;

        let first = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        store
            .claim_task_for_processing(&first.task.id)
            .await
            .unwrap();

        let second = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        assert!(second.deduplicated);
        assert_eq!(second.task.id, first.task.id);
        assert_eq!(second.task.status, TaskStatus::Processing);
    }

    #[tokio::test]
    async fn dedup_collapses_onto_ready() {
        let store = store().await;
        let key = key_id(&store).await;

        let first = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        store
            .mark_task_ready(&first.task.id, "0xcafe")
            .await
            .unwrap();

        let second = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        assert!(second.deduplicated, "a ready task still absorbs a retry");
        assert_eq!(second.task.id, first.task.id);
        assert_eq!(second.task.status, TaskStatus::Ready);
    }

    #[tokio::test]
    async fn dedup_resubmits_after_failed() {
        let store = store().await;
        let key = key_id(&store).await;

        let first = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        store
            .mark_task_failed(&first.task.id, "aggregation timed out")
            .await
            .unwrap();

        let second = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        assert!(
            !second.deduplicated,
            "a failed task is not a duplicate, so its work can be re-submitted"
        );
        assert_ne!(second.task.id, first.task.id);
        assert_eq!(second.task.status, TaskStatus::Queued);
    }

    #[tokio::test]
    async fn dedup_resubmits_after_expired() {
        let store = store().await;
        let key = key_id(&store).await;

        let first = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        store
            .mark_task_expired(&first.task.id, "payload validity window elapsed")
            .await
            .unwrap();

        let second = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();
        assert!(
            !second.deduplicated,
            "an expired task is not a duplicate, so its work can be re-submitted"
        );
        assert_ne!(second.task.id, first.task.id);
    }

    #[tokio::test]
    async fn dedup_is_scoped_per_key() {
        let store = store().await;
        let key_a = key_id(&store).await;
        let key_b = key_id(&store).await;

        let a = store
            .create_task_deduplicated(&key_a, &request())
            .await
            .unwrap();
        let b = store
            .create_task_deduplicated(&key_b, &request())
            .await
            .unwrap();
        assert!(!a.deduplicated);
        assert!(
            !b.deduplicated,
            "the same request under a different key is distinct work"
        );
        assert_ne!(a.task.id, b.task.id);
    }

    #[tokio::test]
    async fn dedup_distinguishes_transition_index() {
        let store = store().await;
        let key = key_id(&store).await;

        let mut first_req = request();
        first_req.transition_index = Some(1);
        let mut second_req = request();
        second_req.transition_index = Some(2);

        let first = store
            .create_task_deduplicated(&key, &first_req)
            .await
            .unwrap();
        let second = store
            .create_task_deduplicated(&key, &second_req)
            .await
            .unwrap();
        assert!(!first.deduplicated);
        assert!(
            !second.deduplicated,
            "a different transition_index is different work"
        );
        assert_ne!(first.task.id, second.task.id);
    }

    #[tokio::test]
    async fn dedup_distinguishes_target_address() {
        let store = store().await;
        let key = key_id(&store).await;

        let mut first_req = request();
        first_req.target_address = Address::from([0xaa; 20]);
        let mut second_req = request();
        second_req.target_address = Address::from([0xbb; 20]);

        let first = store
            .create_task_deduplicated(&key, &first_req)
            .await
            .unwrap();
        let second = store
            .create_task_deduplicated(&key, &second_req)
            .await
            .unwrap();
        assert!(!first.deduplicated);
        assert!(
            !second.deduplicated,
            "a different target_address is different work"
        );
        assert_ne!(first.task.id, second.task.id);
    }

    #[tokio::test]
    async fn auto_transition_index_never_deduplicates() {
        let store = store().await;
        let key = key_id(&store).await;
        let mut req = request();
        req.transition_index = None;

        let first = store.create_task_deduplicated(&key, &req).await.unwrap();
        let second = store.create_task_deduplicated(&key, &req).await.unwrap();
        assert!(!first.deduplicated);
        assert!(
            !second.deduplicated,
            "auto submissions each take their own slot and are never collapsed"
        );
        assert_ne!(first.task.id, second.task.id);

        let listed = store.list_tasks_for_key(&key, None, 100, 0).await.unwrap();
        assert_eq!(
            listed.len(),
            2,
            "both auto submissions persist distinct rows"
        );
    }

    #[tokio::test]
    async fn concurrent_duplicate_submissions_collapse_to_one_task() {
        // A real file-backed store (WAL, multi-connection pool) so the racing submissions run on
        // separate connections — the in-memory store is capped at one connection and would
        // serialize them, hiding the concurrency the atomic insert must survive.
        let dir = tempfile::tempdir().expect("temp dir");
        let store = SqliteStore::connect_at(&dir.path().join("router.db"))
            .await
            .expect("file store should open and migrate");
        let key = key_id(&store).await;

        let mut handles = Vec::new();
        for _ in 0..16 {
            let store = store.clone();
            let key = key.clone();
            handles.push(tokio::spawn(async move {
                store
                    .create_task_deduplicated(&key, &request())
                    .await
                    .expect("concurrent dedup submission should succeed")
            }));
        }

        let mut created = 0usize;
        let mut ids = std::collections::HashSet::new();
        for handle in handles {
            let submission = handle.await.unwrap();
            if !submission.deduplicated {
                created += 1;
            }
            ids.insert(submission.task.id);
        }

        assert_eq!(
            created, 1,
            "exactly one concurrent submission may create the task"
        );
        assert_eq!(
            ids.len(),
            1,
            "every concurrent submission returns the same task id"
        );

        let listed = store.list_tasks_for_key(&key, None, 100, 0).await.unwrap();
        assert_eq!(listed.len(), 1, "only one row exists after the race");
    }

    #[tokio::test]
    async fn dedup_retries_to_fresh_task_when_blocker_goes_terminal_mid_check() {
        let store = store().await;
        let key = key_id(&store).await;

        // An active task occupies the slot, so the next submission's insert is turned away.
        let blocker = store
            .create_task_deduplicated(&key, &request())
            .await
            .unwrap();

        // Drive the blocker terminal exactly once, in the window between the blocked insert and the
        // read-back, reproducing the race: the read-back then finds no active duplicate, so the
        // loop must re-insert rather than error.
        let fired = std::cell::Cell::new(false);
        let submission = store
            .create_task_deduplicated_hooked(&key, &request(), || {
                let store = store.clone();
                let blocker_id = blocker.task.id.clone();
                let first = !fired.replace(true);
                async move {
                    if first {
                        store
                            .mark_task_failed(&blocker_id, "aggregation timed out")
                            .await
                            .unwrap();
                    }
                }
            })
            .await
            .unwrap();

        assert!(
            !submission.deduplicated,
            "the blocker went terminal, so the retry creates a fresh task rather than a 500"
        );
        assert_ne!(submission.task.id, blocker.task.id);
        assert_eq!(submission.task.status, TaskStatus::Queued);

        let active = store
            .list_tasks_for_key(&key, Some(TaskStatus::Queued), 100, 0)
            .await
            .unwrap();
        assert_eq!(active.len(), 1, "only the fresh task remains active");
    }
}

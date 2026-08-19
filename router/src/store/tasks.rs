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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

    /// Transitions a task to `status` and stamps `updated_at`. Sets only the status column, so
    /// it suits state-only moves such as `queued → processing`; use [`Self::mark_task_ready`] or
    /// [`Self::mark_task_failed`] when a payload or error must be recorded too. Returns `true`
    /// if a task with that id existed.
    pub async fn update_task_status(&self, id: &str, status: TaskStatus) -> anyhow::Result<bool> {
        let result =
            sqlx::query("UPDATE tasks SET status = ?2, updated_at = unixepoch() WHERE id = ?1")
                .bind(id)
                .bind(status.as_str())
                .execute(self.pool())
                .await
                .context("updating task status")?;

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
    /// `updated_at`. Both are stored as JSON. Returns `true` if a task with that id existed.
    pub async fn mark_task_ready_with_bundle(
        &self,
        id: &str,
        payload: &str,
        bundle: &str,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE tasks SET status = 'ready', payload = ?2, bundle = ?3, \
             updated_at = unixepoch() WHERE id = ?1",
        )
        .bind(id)
        .bind(payload)
        .bind(bundle)
        .execute(self.pool())
        .await
        .context("marking task ready with bundle")?;

        Ok(result.rows_affected() > 0)
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

    /// Settles a task as [`TaskStatus::Failed`], recording the failure reason and stamping
    /// `updated_at`. Returns `true` if a task with that id existed.
    pub async fn mark_task_failed(&self, id: &str, error: &str) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE tasks SET status = 'failed', error = ?2, updated_at = unixepoch() \
             WHERE id = ?1",
        )
        .bind(id)
        .bind(error)
        .execute(self.pool())
        .await
        .context("marking task failed")?;

        Ok(result.rows_affected() > 0)
    }

    /// Returns every task still in flight — `queued` or `processing` — oldest first. The router
    /// calls this on startup to rebuild and re-enqueue work interrupted by a restart.
    pub async fn incomplete_tasks(&self) -> anyhow::Result<Vec<Task>> {
        let rows: Vec<TaskRow> = sqlx::query_as(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks \
             WHERE status IN ('queued', 'processing') ORDER BY created_at, id",
        ))
        .fetch_all(self.pool())
        .await
        .context("loading incomplete tasks")?;

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
    async fn update_status_transitions_and_reports_existence() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request()).await.unwrap();

        assert!(
            store
                .update_task_status(&task.id, TaskStatus::Processing)
                .await
                .unwrap()
        );
        let fetched = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::Processing);
        assert!(fetched.updated_at >= task.updated_at);

        assert!(
            !store
                .update_task_status("does-not-exist", TaskStatus::Processing)
                .await
                .unwrap(),
            "updating an unknown task should report no change"
        );
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
    async fn mark_failed_records_error() {
        let store = store().await;
        let key = key_id(&store).await;
        let task = store.create_task(&key, &request()).await.unwrap();

        assert!(
            store
                .mark_task_failed(&task.id, "aggregation timed out")
                .await
                .unwrap()
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
            .update_task_status(&processing.id, TaskStatus::Processing)
            .await
            .unwrap();
        store.mark_task_ready(&ready.id, "0x00").await.unwrap();
        store.mark_task_failed(&failed.id, "nope").await.unwrap();

        let incomplete = store.incomplete_tasks().await.unwrap();
        let ids: Vec<&str> = incomplete.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            incomplete.len(),
            2,
            "only queued and processing are in flight"
        );
        assert!(ids.contains(&queued.id.as_str()));
        assert!(ids.contains(&processing.id.as_str()));
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
            .update_task_status(&first.task.id, TaskStatus::Processing)
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
}

//! Periodic task-TTL sweep.
//!
//! Every task pins a `block_height`, and that reference goes stale as the chain advances: past a
//! point, no amount of aggregation can turn the task into a payload `verifyAndUpdate` will accept.
//! Ingress validation catches a request that is *already* too old to admit; it cannot catch the
//! task that was fine on arrival and then waited too long because the queue drained slowly. This
//! sweep is that second check — it settles lapsed tasks as `expired` so the queue slot, the
//! deduplication slot for their transition index, and the next aggregation round all go to work
//! that can still land, and so a client polling the task learns to re-request instead of waiting
//! on a round that would revert.
//!
//! The sweep is deliberately blind to `processing`: see [`ExpiryStage`].

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, error, info};

use crate::metrics::MetricsCollector;
use crate::store::{ExpiryStage, SqliteStore};

/// How often the sweep runs. Deliberately coarse relative to the task TTL (`TASK_TTL_SECONDS`),
/// since the TTL is a backstop rather than a deadline the client can observe to the second — a
/// task lingers at most one interval past its TTL before being settled.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Runs the TTL sweep every [`SWEEP_INTERVAL`] for as long as the router lives.
///
/// Sweeps before its first sleep so a restart clears the backlog a previous life left behind
/// rather than re-aggregating tasks whose block references went stale while the pod was down.
/// A store error aborts only the pass it happened in: the loop keeps its cadence, because a
/// transient database failure must not silently stop expiry for the rest of the process's life.
pub async fn run_expiry_sweeper(store: SqliteStore, metrics: Arc<MetricsCollector>, ttl: Duration) {
    info!(
        ttl_secs = ttl.as_secs(),
        interval_secs = SWEEP_INTERVAL.as_secs(),
        "task expiry sweep started"
    );
    loop {
        sweep_expired_tasks(&store, Some(&metrics), ttl).await;
        tokio::time::sleep(SWEEP_INTERVAL).await;
    }
}

/// Runs one sweep pass across every [`ExpiryStage`], returning how many tasks it expired.
///
/// Each stage is swept independently so a failure on one still lets the other settle its rows.
pub async fn sweep_expired_tasks(
    store: &SqliteStore,
    metrics: Option<&MetricsCollector>,
    ttl: Duration,
) -> usize {
    let mut total = 0;
    for stage in ExpiryStage::ALL {
        let expired = match store.expire_stale_tasks(stage, ttl).await {
            Ok(ids) => ids,
            Err(e) => {
                error!(?stage, error = %e, "task expiry sweep failed for stage");
                continue;
            }
        };
        if expired.is_empty() {
            continue;
        }

        for task_id in &expired {
            info!(
                task_id = %task_id,
                ?stage,
                reason = stage.code(),
                ttl_secs = ttl.as_secs(),
                "task expired past its TTL"
            );
        }
        if let Some(m) = metrics {
            m.tasks_expired.inc_by(expired.len() as u64);
        }
        total += expired.len();
    }

    if total == 0 {
        debug!(ttl_secs = ttl.as_secs(), "task expiry sweep found nothing");
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingress::GasKillerTaskRequestBody;
    use crate::store::{Task, TaskStatus};
    use alloy_primitives::{Address, U256};

    const TTL: Duration = Duration::from_secs(300);

    async fn store() -> SqliteStore {
        SqliteStore::connect_in_memory()
            .await
            .expect("in-memory store should open and migrate")
    }

    fn request() -> GasKillerTaskRequestBody {
        GasKillerTaskRequestBody {
            target_address: Address::from([0x11; 20]),
            call_data: vec![0xab, 0xcd, 0xef, 0x01],
            transition_index: Some(3),
            from_address: Address::from([0x22; 20]),
            value: U256::ZERO,
            block_height: 21_000_000,
        }
    }

    /// Creates a queued task owned by a fresh API key, satisfying the tasks table's foreign key.
    async fn queued_task(store: &SqliteStore) -> Task {
        let key = store
            .create_api_key(None, None)
            .await
            .expect("key creation should succeed");
        store
            .create_task(&key.id, &request())
            .await
            .expect("task creation should succeed")
    }

    /// Backdates a task's timestamps by `secs`, standing in for the wall-clock wait the sweep
    /// measures. Both columns move together so the row looks uniformly old; tests that need the
    /// two clocks to disagree set them apart afterwards.
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

    async fn status_of(store: &SqliteStore, id: &str) -> TaskStatus {
        store
            .get_task(id)
            .await
            .expect("task read should succeed")
            .expect("task should exist")
            .status
    }

    #[tokio::test]
    async fn sweep_expires_queued_task_past_ttl() {
        let store = store().await;
        let metrics = MetricsCollector::new();
        let task = queued_task(&store).await;
        age_task(&store, &task.id, 301).await;

        assert_eq!(sweep_expired_tasks(&store, Some(&metrics), TTL).await, 1);

        let expired = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(expired.status, TaskStatus::Expired);
        let error = expired.error.expect("an expired task carries its reason");
        assert!(
            error.starts_with("QUEUE_TTL_EXCEEDED"),
            "reason should name the TTL breach: {error}"
        );
        assert!(
            metrics
                .encode()
                .contains("gas_killer_tasks_expired_total 1"),
            "the sweep should count what it expired"
        );
    }

    #[tokio::test]
    async fn sweep_leaves_tasks_inside_the_ttl() {
        let store = store().await;
        let task = queued_task(&store).await;
        age_task(&store, &task.id, 299).await;

        assert_eq!(sweep_expired_tasks(&store, None, TTL).await, 0);
        assert_eq!(status_of(&store, &task.id).await, TaskStatus::Queued);
    }

    /// A round already has its aggregation height assigned and must resolve either way, so
    /// cancelling the row mid-round would only lose the attribution of the result still coming.
    #[tokio::test]
    async fn sweep_never_cancels_a_processing_task() {
        let store = store().await;
        let task = queued_task(&store).await;
        assert!(store.claim_task_for_processing(&task.id).await.unwrap());
        age_task(&store, &task.id, 10_000).await;

        assert_eq!(sweep_expired_tasks(&store, None, TTL).await, 0);
        assert_eq!(status_of(&store, &task.id).await, TaskStatus::Processing);
    }

    #[tokio::test]
    async fn sweep_expires_uncollected_ready_payload() {
        let store = store().await;
        let task = queued_task(&store).await;
        store
            .mark_task_ready_with_bundle(&task.id, "{}", "{}")
            .await
            .unwrap();
        age_task(&store, &task.id, 301).await;

        assert_eq!(sweep_expired_tasks(&store, None, TTL).await, 1);

        let expired = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(expired.status, TaskStatus::Expired);
        assert!(
            expired
                .error
                .unwrap_or_default()
                .starts_with("READY_TTL_EXCEEDED"),
            "a swept ready payload is distinguishable from a swept queue entry"
        );
    }

    /// A `ready` task ages from the moment aggregation recorded its payload, not from submission:
    /// a task that queued for a long time and only just completed has a full TTL to be collected.
    #[tokio::test]
    async fn sweep_ages_a_ready_task_from_when_it_became_ready() {
        let store = store().await;
        let task = queued_task(&store).await;
        age_task(&store, &task.id, 10_000).await;
        store
            .mark_task_ready_with_bundle(&task.id, "{}", "{}")
            .await
            .unwrap();

        assert_eq!(sweep_expired_tasks(&store, None, TTL).await, 0);
        assert_eq!(status_of(&store, &task.id).await, TaskStatus::Ready);
    }

    /// Terminal rows are already settled; re-expiring one would overwrite the reason it carries.
    #[tokio::test]
    async fn sweep_leaves_terminal_tasks_alone() {
        let store = store().await;
        let failed = queued_task(&store).await;
        store
            .mark_task_failed(&failed.id, "aggregation height skipped by quorum")
            .await
            .unwrap();
        age_task(&store, &failed.id, 10_000).await;

        assert_eq!(sweep_expired_tasks(&store, None, TTL).await, 0);
        let fetched = store.get_task(&failed.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::Failed);
        assert_eq!(
            fetched.error.as_deref(),
            Some("aggregation height skipped by quorum")
        );
    }

    #[tokio::test]
    async fn sweep_counts_every_stage_it_expires() {
        let store = store().await;
        let metrics = MetricsCollector::new();
        let queued = queued_task(&store).await;
        let ready = queued_task(&store).await;
        store
            .mark_task_ready_with_bundle(&ready.id, "{}", "{}")
            .await
            .unwrap();
        age_task(&store, &queued.id, 301).await;
        age_task(&store, &ready.id, 301).await;

        assert_eq!(sweep_expired_tasks(&store, Some(&metrics), TTL).await, 2);
        assert!(
            metrics
                .encode()
                .contains("gas_killer_tasks_expired_total 2")
        );
        assert_eq!(status_of(&store, &queued.id).await, TaskStatus::Expired);
        assert_eq!(status_of(&store, &ready.id).await, TaskStatus::Expired);
    }
}

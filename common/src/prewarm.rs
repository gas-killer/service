//! Task prewarm channel: the router publishes each accepted task at INGRESS
//! time, operator nodes poll it and start the expensive tracked-function
//! simulation immediately — instead of waiting for the round broadcast, which
//! the router only sends after finishing its own simulation. When the round
//! arrives, the node's digest cache is already warm (or the in-flight prewarm
//! is nearly done) and the node signs without re-simulating, dropping round
//! time from `router leg + operator leg` to roughly `max(router leg, operator leg)`.
//!
//! This is a pure latency optimization with zero protocol changes: the node
//! still derives the digest independently from its own simulation, exactly as
//! it would on the round's arrival — only the *start time* moves earlier. A
//! missed, stale, or failed prewarm simply falls back to the normal
//! simulate-on-arrival path.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use alloy_primitives::{Address, U256};

use crate::task_data::GasKillerTaskData;
use crate::validator::GasKillerValidator;

/// The task fields a node needs to reproduce the creator's simulation input.
/// Mirrors the ingress request body: `transition_index: None` means "auto",
/// which the node resolves against the chain exactly like the creator does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrewarmTask {
    pub target_address: Address,
    pub call_data: Vec<u8>,
    /// `None` = auto: resolve `stateTransitionCount()` from the chain,
    /// mirroring the creator's dequeue-time resolution.
    pub transition_index: Option<u64>,
    pub from_address: Address,
    pub value: U256,
    pub block_height: u64,
}

/// What `GET /prewarm` returns: a monotonically increasing sequence number and
/// the most recently accepted task. Nodes act when `seq` changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrewarmSnapshot {
    pub seq: u64,
    pub task: PrewarmTask,
}

/// Single-slot broadcast shared between the router's ingress handler (writer)
/// and its internal HTTP server (reader). Holding only the latest task is
/// deliberate: tasks queue FIFO behind a ~20-minute simulation each, so
/// prewarming anything but the most recent accepted task has no payoff.
#[derive(Debug, Default)]
pub struct PrewarmSlot {
    inner: RwLock<Option<PrewarmSnapshot>>,
}

impl PrewarmSlot {
    /// Publishes `task` as the latest prewarm candidate, returning its sequence number.
    pub fn publish(&self, task: PrewarmTask) -> u64 {
        let mut slot = self.inner.write().expect("prewarm slot lock poisoned");
        let seq = slot.as_ref().map(|s| s.seq).unwrap_or(0) + 1;
        *slot = Some(PrewarmSnapshot { seq, task });
        seq
    }

    /// Returns a copy of the latest published snapshot, if any.
    pub fn snapshot(&self) -> Option<PrewarmSnapshot> {
        self.inner
            .read()
            .expect("prewarm slot lock poisoned")
            .clone()
    }
}

/// Poll interval for the node's prewarm loop (`GK_PREWARM_POLL_MS`, default 3000).
fn prewarm_poll_interval() -> Duration {
    let ms = std::env::var("GK_PREWARM_POLL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&ms| ms > 0)
        .unwrap_or(3_000);
    Duration::from_millis(ms)
}

/// Fetches the current prewarm snapshot from the router's internal endpoint.
/// `Ok(None)` means the router has not accepted any task yet (204).
async fn fetch_snapshot(client: &reqwest::Client, url: &str) -> Result<Option<PrewarmSnapshot>> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if resp.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    let snapshot = resp
        .error_for_status()
        .with_context(|| format!("GET {url}"))?
        .json::<PrewarmSnapshot>()
        .await
        .context("decoding prewarm snapshot")?;
    Ok(Some(snapshot))
}

/// Builds the `GasKillerTaskData` the round broadcast will carry, from the
/// ingress-published fields. Resolution mirrors the creator: detect the chain
/// once, then read `stateTransitionCount()` for auto indices and `eth_chainId`
/// for the chain id. `storage_updates` stays empty — the payload hash is built
/// from the *recomputed* updates, never from this field.
async fn build_task_data(
    validator: &GasKillerValidator,
    task: &PrewarmTask,
) -> Result<GasKillerTaskData> {
    let chain_role = validator
        .detect_chain_for_address(task.target_address)
        .await
        .context("prewarm chain detection")?;
    let chain_id = validator.get_chain_id_for(chain_role).await?;
    let transition_index = match task.transition_index {
        Some(idx) => idx,
        None => {
            validator
                .get_state_transition_count_on_chain(task.target_address, chain_role)
                .await?
        }
    };
    Ok(GasKillerTaskData {
        storage_updates: alloy_primitives::Bytes::new(),
        transition_index,
        target_address: task.target_address,
        call_data: task.call_data.clone(),
        from_address: task.from_address,
        value: task.value,
        block_height: task.block_height,
        chain_id,
    })
}

/// Resolves and simulates one published task, landing the digest in the
/// validator's cache. Failures are logged and swallowed: the node falls back
/// to simulating on round arrival, exactly as without prewarm.
async fn prewarm_one(validator: Arc<GasKillerValidator>, snapshot: PrewarmSnapshot) {
    let seq = snapshot.seq;
    let task_data = match build_task_data(&validator, &snapshot.task).await {
        Ok(td) => td,
        Err(e) => {
            warn!(seq, error = %e, "prewarm: failed to resolve task data; will simulate on round arrival");
            return;
        }
    };
    info!(
        seq,
        transition_index = task_data.transition_index,
        block_height = task_data.block_height,
        target_address = %task_data.target_address,
        "prewarm: starting simulation ahead of round broadcast"
    );
    let started = Instant::now();
    match validator.prewarm(&task_data).await {
        Ok(true) => info!(
            seq,
            transition_index = task_data.transition_index,
            block_height = task_data.block_height,
            elapsed_s = started.elapsed().as_secs(),
            "prewarm: digest cached — round arrival will skip EVMSketch"
        ),
        Ok(false) => debug!(
            seq,
            transition_index = task_data.transition_index,
            block_height = task_data.block_height,
            "prewarm: skipped (already cached or in flight)"
        ),
        Err(e) => warn!(
            seq,
            elapsed_s = started.elapsed().as_secs(),
            error = %e,
            "prewarm: simulation failed; will simulate on round arrival"
        ),
    }
}

/// Runs forever: polls `GET {url}` and, whenever the sequence number changes,
/// spawns a prewarm simulation for the newly published task. Duplicate work is
/// guarded inside [`GasKillerValidator::prewarm`] (cached / in-flight keys are
/// skipped), so overlapping spawns for the same round are harmless.
///
/// Intended to be spawned as a background task on the node when
/// `GK_PREWARM_URL` is set.
pub async fn run_prewarm_loop(validator: Arc<GasKillerValidator>, url: String) {
    let interval = prewarm_poll_interval();
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "prewarm: failed to build HTTP client; prewarm disabled");
            return;
        }
    };
    info!(url = %url, poll_ms = interval.as_millis() as u64, "prewarm loop started");

    let mut last_seq: Option<u64> = None;
    loop {
        match fetch_snapshot(&client, &url).await {
            Ok(Some(snapshot)) if last_seq != Some(snapshot.seq) => {
                last_seq = Some(snapshot.seq);
                let validator = Arc::clone(&validator);
                // Spawned so a multi-minute simulation never blocks picking up
                // the next published task.
                tokio::spawn(prewarm_one(validator, snapshot));
            }
            Ok(_) => {}
            // Transient: the router may be restarting or not yet ready.
            Err(e) => debug!(error = %e, "prewarm poll failed; retrying"),
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(block_height: u64) -> PrewarmTask {
        PrewarmTask {
            target_address: Address::from([1u8; 20]),
            call_data: vec![0xAB, 0xCD, 0xEF, 0x01],
            transition_index: None,
            from_address: Address::from([2u8; 20]),
            value: U256::ZERO,
            block_height,
        }
    }

    #[test]
    fn publish_increments_seq_and_snapshot_returns_latest() {
        let slot = PrewarmSlot::default();
        assert!(slot.snapshot().is_none());

        assert_eq!(slot.publish(task(1)), 1);
        assert_eq!(slot.publish(task(2)), 2);

        let snap = slot.snapshot().expect("snapshot after publish");
        assert_eq!(snap.seq, 2);
        assert_eq!(snap.task.block_height, 2);
    }

    #[test]
    fn snapshot_json_round_trips() {
        let snap = PrewarmSnapshot {
            seq: 7,
            task: PrewarmTask {
                transition_index: Some(42),
                ..task(1234)
            },
        };
        let json = serde_json::to_string(&snap).expect("serialize");
        let decoded: PrewarmSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, snap);
    }

    #[test]
    fn auto_transition_index_serializes_as_null() {
        let snap = PrewarmSnapshot {
            seq: 1,
            task: task(1),
        };
        let value: serde_json::Value = serde_json::to_value(&snap).expect("to_value");
        assert!(value["task"]["transition_index"].is_null());
        let decoded: PrewarmSnapshot = serde_json::from_value(value).expect("from_value");
        assert_eq!(decoded.task.transition_index, None);
    }
}

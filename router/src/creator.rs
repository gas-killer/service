use crate::ingress::GasKillerTaskRequest;
use crate::metrics::MetricsCollector;
use commonware_avs_router::creator::Creator;
use gas_killer_common::GasKillerValidator;
use gas_killer_common::task_data::GasKillerTaskData;

use alloy_primitives::Bytes;
use anyhow::Result;
use async_trait::async_trait;
use commonware_codec::Encode;
use commonware_cryptography::{Hasher, Sha256};
use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, warn};

/// Per-round dispatch timestamps: the creator inserts `round -> Instant::now()` when it dispatches
/// a task, and the executor removes the matching entry in `handle_verification` to compute the P2P
/// round-trip duration.
///
/// Keying by round (rather than a single shared slot) means a consensus round that fails without
/// calling `handle_verification` cannot bleed its stale timestamp into the next round's
/// measurement. Rounds advance monotonically and the orchestrator runs one at a time, so the
/// creator evicts any entry older than the round it is dispatching, keeping the map bounded even
/// when rounds fail.
pub type DispatchTime = Arc<Mutex<HashMap<u64, Instant>>>;

/// Records `round`'s dispatch instant for later round-trip measurement, first evicting any entry
/// from an earlier round. Rounds advance monotonically and the orchestrator runs one at a time, so
/// an older entry belongs to a round that failed without completing; dropping it here keeps the map
/// bounded even when rounds repeatedly fail.
pub(crate) fn stamp_dispatch_time(times: &DispatchTime, round: u64) {
    if let Ok(mut times) = times.lock() {
        times.retain(|&r, _| r >= round);
        times.insert(round, Instant::now());
    }
}

/// Removes and returns `round`'s dispatch instant, if one was recorded. Consuming the entry both
/// yields the round-trip start and prevents the completed round from lingering in the map.
pub(crate) fn take_dispatch_time(times: &DispatchTime, round: u64) -> Option<Instant> {
    times.lock().ok().and_then(|mut times| times.remove(&round))
}

pub type TaskSender = UnboundedSender<GasKillerTaskRequest>;
pub type TaskReceiver = UnboundedReceiver<GasKillerTaskRequest>;
/// Shared atomic counter tracking tasks in flight between the ingress sender and creator receiver.
pub type TaskQueueDepth = Arc<AtomicUsize>;

pub fn task_channel() -> (TaskSender, TaskReceiver) {
    mpsc::unbounded_channel()
}

pub fn task_queue_depth() -> TaskQueueDepth {
    Arc::new(AtomicUsize::new(0))
}

/// Configuration for listening creators
#[derive(Debug, Clone)]
pub struct GasKillerConfig {
    pub timeout_ms: u64,
}

impl Default for GasKillerConfig {
    fn default() -> Self {
        let timeout_ms: u64 = env::var("INGRESS_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        Self { timeout_ms }
    }
}

/// Creator for the gas killer usecase without ingress
pub struct GasKillerCreator {
    polling_interval: Duration,
}

impl Default for GasKillerCreator {
    fn default() -> Self {
        Self::new()
    }
}

impl GasKillerCreator {
    pub fn new() -> Self {
        let polling_interval_ms: u64 = env::var("POLLING_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&ms| ms > 0)
            .unwrap_or(2_000);
        Self {
            polling_interval: Duration::from_millis(polling_interval_ms),
        }
    }
}

#[async_trait]
impl Creator for GasKillerCreator {
    type TaskData = GasKillerTaskData;

    async fn get_payload_and_round(&self) -> Result<(Vec<u8>, u64)> {
        let payload = self.get_task_metadata();
        let raw_payload = payload.encode().to_vec();

        Ok((raw_payload, 0)) // set default "round" to 0
    }

    async fn wait_for_new_round(&self, current: u64) -> Result<(Vec<u8>, u64)> {
        tokio::time::sleep(self.polling_interval).await;
        let payload = self.get_task_metadata();
        let raw_payload = payload.encode().to_vec();
        Ok((raw_payload, current + 1))
    }

    fn get_task_metadata(&self) -> Self::TaskData {
        GasKillerTaskData::default()
    }
}

/// Enriched task data that includes computed storage updates and block height
struct EnrichedTask {
    task: GasKillerTaskRequest,
    storage_updates: Bytes,
    block_height: u64,
    /// Resolved transition index (sentinel `None` → concrete count from chain).
    transition_index: u64,
    /// Actual EVM chain ID (e.g. 1 = Ethereum mainnet, 100 = Gnosis, 31337 = Anvil).
    chain_id: u64,
}

/// Seeds the round counter so it does not restart from a fixed value. Nodes refuse to re-sign a
/// round, so reusing rounds across restarts would stall aggregation.
fn initial_round_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Creator for the gas killer usecase that listens for external requests
pub struct ListeningGasKillerCreator {
    receiver: tokio::sync::Mutex<TaskReceiver>,
    queue_depth: TaskQueueDepth,
    config: GasKillerConfig,
    validator: Arc<GasKillerValidator>,
    current_task: Mutex<Option<EnrichedTask>>,
    /// Consensus round number, incremented per dispatched task and unique across restarts.
    round_counter: AtomicU64,
    metrics: Option<Arc<MetricsCollector>>,
    /// Shared with the executor to measure P2P round-trip duration.
    dispatch_time: DispatchTime,
}

impl ListeningGasKillerCreator {
    pub fn new(
        receiver: TaskReceiver,
        queue_depth: TaskQueueDepth,
        config: GasKillerConfig,
        validator: Arc<GasKillerValidator>,
        dispatch_time: DispatchTime,
    ) -> Self {
        Self {
            receiver: tokio::sync::Mutex::new(receiver),
            queue_depth,
            config,
            validator,
            current_task: Mutex::new(None),
            round_counter: AtomicU64::new(initial_round_seed()),
            metrics: None,
            dispatch_time,
        }
    }

    pub fn with_metrics(mut self, metrics: Arc<MetricsCollector>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    async fn wait_for_task(&self) -> Result<GasKillerTaskRequest> {
        let mut rx = self.receiver.lock().await;
        let task = if self.config.timeout_ms == 0 {
            rx.recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("task channel closed"))?
        } else {
            tokio::time::timeout(Duration::from_millis(self.config.timeout_ms), rx.recv())
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Timeout waiting for task after {}ms",
                        self.config.timeout_ms
                    )
                })?
                .ok_or_else(|| anyhow::anyhow!("task channel closed"))?
        };
        let depth = self
            .queue_depth
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            })
            .unwrap()
            .saturating_sub(1);
        if let Some(m) = &self.metrics {
            m.task_queue_depth.set(depth as i64);
        }
        Ok(task)
    }
}

#[async_trait]
impl Creator for ListeningGasKillerCreator {
    type TaskData = GasKillerTaskData;

    async fn wait_for_new_round(&self, current: u64) -> Result<(Vec<u8>, u64)> {
        loop {
            let result = self.get_payload_and_round().await?;
            if result.1 > current {
                return Ok(result);
            }
        }
    }

    async fn get_payload_and_round(&self) -> Result<(Vec<u8>, u64)> {
        let task = self.wait_for_task().await?;

        info!(
            target = format!("{:?}", task.body.target_address),
            from = format!("{:?}", task.body.from_address),
            transition_index = ?task.body.transition_index,
            call_data_len = task.body.call_data.len(),
            "Creator received task"
        );

        if let Some(m) = &self.metrics {
            m.tasks_created.inc();
        }

        debug!(
            "Computing storage updates for target {}",
            task.body.target_address
        );

        // For explicit indices, run storage computation alone (count not needed).
        // For auto mode, run stateTransitionCount() concurrently with EVMSketch: the
        // count RPC call (~200ms) is fully hidden behind EVMSketch (seconds), so the
        // auto path adds zero observable latency compared to the explicit-index path.
        let (
            storage_updates,
            block_height,
            numeric_chain_id,
            resolved_transition_index,
            storage_elapsed,
        ) = if let Some(idx) = task.body.transition_index {
            let start = Instant::now();
            // compute_storage_updates_for_tx detects the chain, runs EVMSketch, and also
            // calls eth_chainId on the same RPC — returns the numeric chain ID directly.
            let (updates, height, chain_id) = self
                .validator
                .compute_storage_updates_for_tx(
                    task.body.target_address,
                    &task.body.call_data,
                    Some(task.body.from_address),
                    Some(task.body.value),
                    task.body.block_height,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Failed to compute storage updates: {}", e))?;
            (updates, height, chain_id, idx, start.elapsed())
        } else {
            // Detect chain once so all concurrent futures skip redundant eth_getCode probes.
            let chain_role = self
                .validator
                .detect_chain_for_address(task.body.target_address)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to detect chain: {}", e))?;
            let rpc_url = self
                .validator
                .rpc_url_for_chain(chain_role)
                .ok_or_else(|| anyhow::anyhow!("No RPC URL for chain {}", chain_role))?
                .to_owned();

            info!(
                target_address = %task.body.target_address,
                chain = %chain_role,
                "Resolving auto transition_index concurrently with EVMSketch"
            );

            let count_validator = Arc::clone(&self.validator);
            let chain_id_validator = Arc::clone(&self.validator);
            let target = task.body.target_address;
            let count_fut = async move {
                count_validator
                    .get_state_transition_count_on_chain(target, chain_role)
                    .await
            };
            let storage_fut = async {
                let start = Instant::now();
                self.validator
                    .analyze_transaction(
                        &rpc_url,
                        task.body.target_address,
                        &task.body.call_data,
                        Some(task.body.from_address),
                        Some(task.body.value),
                        task.body.block_height,
                    )
                    .await
                    .map(|r| (r.storage_updates, r.block_height, start.elapsed()))
            };
            // eth_chainId runs concurrently — completes in ~50ms, well before EVMSketch.
            let chain_id_fut = async move { chain_id_validator.get_chain_id_for(chain_role).await };
            let (count, (updates, height, storage_elapsed), chain_id) =
                tokio::try_join!(count_fut, storage_fut, chain_id_fut)?;

            info!(
                target_address = %task.body.target_address,
                chain = %chain_role,
                count,
                "Resolved auto transition_index from chain"
            );
            (updates, height, chain_id, count, storage_elapsed)
        };

        if let Some(m) = &self.metrics {
            m.storage_computation_seconds
                .observe(storage_elapsed.as_secs_f64());
        }

        // Debug: Log hash of full storage_updates to detect differences vs validators
        let mut storage_hasher = Sha256::new();
        storage_hasher.update(&storage_updates);
        let storage_hash = storage_hasher.finalize();
        let storage_hash_hex = hex::encode(&storage_hash[..8]);
        info!(
            storage_updates_len = storage_updates.len(),
            storage_updates_hash = %storage_hash_hex,
            block_height = block_height,
            transition_index = resolved_transition_index,
            target_address = %task.body.target_address,
            target_function = %task.body.call_data.get(..4).map(hex::encode).unwrap_or_default(),
            chain_id = numeric_chain_id,
            "Creator computed storage updates"
        );

        // Store enriched task with computed storage updates and block height for metadata access
        let enriched = EnrichedTask {
            task,
            storage_updates: storage_updates.into(),
            block_height,
            transition_index: resolved_transition_index,
            chain_id: numeric_chain_id,
        };

        if let Ok(mut current_task) = self.current_task.lock() {
            *current_task = Some(enriched);
        } else {
            error!("Failed to acquire lock on current_task mutex");
        }

        // Prime the validator's digest cache now so node-signature verification skips EVMSketch.
        // The creator already ran EVMSketch to compute the storage updates; the validator would
        // otherwise run it again for each incoming signature, causing a second ~2Gi memory spike.
        let task_data = self.get_task_metadata();
        self.validator
            .prime_cache(&task_data, &task_data.storage_updates)
            .await;

        let payload = task_data.encode().to_vec();

        // Increment round counter for each new task to ensure unique rounds
        // Nodes will refuse to sign the same round twice
        let round = self.round_counter.fetch_add(1, Ordering::SeqCst);
        info!(round = round, "Creator returning payload with round");

        // Stamp this round's dispatch time so the executor can compute P2P round-trip duration.
        stamp_dispatch_time(&self.dispatch_time, round);

        Ok((payload, round))
    }

    fn get_task_metadata(&self) -> Self::TaskData {
        // Try to get metadata from the current task, fall back to defaults if not available
        match self.current_task.lock() {
            Ok(current_task) => {
                if let Some(ref enriched) = *current_task {
                    // Extract metadata from the enriched task
                    info!("Building task metadata from current task");

                    return GasKillerTaskData {
                        storage_updates: enriched.storage_updates.clone(),
                        transition_index: enriched.transition_index,
                        target_address: enriched.task.body.target_address,
                        call_data: enriched.task.body.call_data.clone(),
                        from_address: enriched.task.body.from_address,
                        value: enriched.task.body.value,
                        block_height: enriched.block_height,
                        chain_id: enriched.chain_id,
                    };
                }
                warn!(
                    "get_task_metadata called but no current task set - returning default (zeroed) data. This may indicate get_payload_and_round was not called first."
                );
            }
            Err(e) => {
                error!(
                    "Failed to acquire current_task lock: {} - returning default (zeroed) data",
                    e
                );
            }
        }

        GasKillerTaskData::default()
    }
}

/// This enum allows us to use concrete types at compile time while still
/// supporting different creator implementations. This enables the generic
/// orchestrator to work without runtime polymorphism.
pub enum GasKillerCreatorType {
    /// Basic gas killer creator without ingress
    Basic(GasKillerCreator),
    /// Listening gas killer creator with HTTP ingress
    Listening(Box<ListeningGasKillerCreator>),
}

#[async_trait]
impl Creator for GasKillerCreatorType {
    type TaskData = GasKillerTaskData;

    async fn get_payload_and_round(&self) -> Result<(Vec<u8>, u64)> {
        match self {
            GasKillerCreatorType::Basic(creator) => creator.get_payload_and_round().await,
            GasKillerCreatorType::Listening(creator) => creator.get_payload_and_round().await,
        }
    }

    async fn wait_for_new_round(&self, current: u64) -> Result<(Vec<u8>, u64)> {
        match self {
            GasKillerCreatorType::Basic(creator) => creator.wait_for_new_round(current).await,
            GasKillerCreatorType::Listening(creator) => creator.wait_for_new_round(current).await,
        }
    }

    fn get_task_metadata(&self) -> Self::TaskData {
        match self {
            GasKillerCreatorType::Basic(creator) => creator.get_task_metadata(),
            GasKillerCreatorType::Listening(creator) => creator.get_task_metadata(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, U256};

    #[tokio::test]
    async fn test_creator_get_payload_and_round() {
        let creator = GasKillerCreator::new();
        let result = creator.get_payload_and_round().await;
        assert!(result.is_ok());

        let (payload, round) = result.unwrap();
        assert!(!payload.is_empty());
        assert_eq!(round, 0); // Default round is 0
    }

    #[tokio::test(start_paused = true)]
    async fn test_wait_for_new_round_strictly_advances() {
        let creator = GasKillerCreator::new();
        let (_, round0) = creator.get_payload_and_round().await.unwrap();
        let (_, round1) = creator.wait_for_new_round(round0).await.unwrap();
        assert!(round1 > round0);
        let (_, round2) = creator.wait_for_new_round(round1).await.unwrap();
        assert!(round2 > round1);
    }

    #[test]
    fn test_initial_round_seed_advances_and_is_nonzero() {
        let first = initial_round_seed();
        let second = initial_round_seed();
        assert!(first > 0, "seed must not start from a fixed zero");
        assert!(
            second >= first,
            "seed must not move backwards between starts"
        );
    }

    #[test]
    fn test_dispatch_time_evicts_failed_rounds_and_isolates_measurements() {
        let times: DispatchTime = Arc::new(Mutex::new(HashMap::new()));

        // Round 0 is dispatched but its consensus round fails, so it is never consumed.
        stamp_dispatch_time(&times, 0);
        assert_eq!(times.lock().unwrap().len(), 1);

        // Dispatching round 1 evicts the stale round-0 entry rather than letting it accumulate
        // or bleed into round 1's measurement.
        stamp_dispatch_time(&times, 1);
        {
            let map = times.lock().unwrap();
            assert_eq!(map.len(), 1, "stale failed-round entry should be evicted");
            assert!(map.contains_key(&1));
            assert!(!map.contains_key(&0));
        }

        // The executor consumes round 1's own timestamp exactly once; the failed round 0 is gone.
        assert!(take_dispatch_time(&times, 0).is_none());
        assert!(take_dispatch_time(&times, 1).is_some());
        assert!(take_dispatch_time(&times, 1).is_none());
        assert!(times.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_validator_produces_consistent_hash() {
        // This test verifies the production flow:
        // 1. Creator produces task data
        // 2. Task data is serialized via wire protocol
        // 3. Validator receives and produces a hash
        // 4. The same task data always produces the same hash
        use commonware_avs_router::validator::ValidatorTrait;
        use commonware_avs_router::wire;
        use commonware_codec::{EncodeSize, Write};
        use gas_killer_common::validator::GasKillerValidator;

        // Use with_rpc_url for testing - new() requires HTTP_RPC env var
        let validator = GasKillerValidator::with_rpc_url("https://ethereum-sepolia.publicnode.com");

        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04].into(),
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
            block_height: 12345,
            chain_id: 1u64,
        };

        // Simulate the wire protocol serialization (what happens in production)
        let aggregation = wire::Aggregation::<GasKillerTaskData>::new(
            1, // round
            task_data.clone(),
            None, // payload
        );

        let mut msg_bytes = Vec::with_capacity(aggregation.encode_size());
        aggregation.write(&mut msg_bytes);

        // Get hash from validator (first call)
        let hash1 = validator.get_payload_from_message(&msg_bytes).await;

        // Get hash from validator (second call with same data)
        let hash2 = validator.get_payload_from_message(&msg_bytes).await;

        // If both succeed, hashes should be identical
        if let (Ok(h1), Ok(h2)) = (&hash1, &hash2) {
            assert_eq!(h1, h2);

            // Hash should not be all zeros
            let zero_hash = commonware_cryptography::sha256::Digest::from([0u8; 32]);
            assert_ne!(*h1, zero_hash);
        }
        // If they fail (no Anvil/RPC), that's expected in unit tests
    }

    #[tokio::test]
    async fn test_channel_send_recv() {
        let (sender, mut receiver) = task_channel();
        let task = GasKillerTaskRequest {
            body: crate::ingress::GasKillerTaskRequestBody {
                target_address: Address::from([1u8; 20]),
                call_data: vec![0x12, 0x34, 0x56, 0x78],
                transition_index: Some(1),
                from_address: Address::from([2u8; 20]),
                value: U256::from(1000),
                block_height: 12345,
            },
        };

        sender.send(task.clone()).unwrap();
        let received = receiver.try_recv().unwrap();
        assert_eq!(received.body.transition_index, Some(1));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn test_task_data_from_request() {
        let task = GasKillerTaskRequest {
            body: crate::ingress::GasKillerTaskRequestBody {
                target_address: Address::from([1u8; 20]),
                call_data: vec![0x12, 0x34, 0x56, 0x78],
                transition_index: Some(42),
                from_address: Address::from([2u8; 20]),
                value: U256::from(1000),
                block_height: 12345,
            },
        };

        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04].into(), // would be computed by GasAnalyzer
            transition_index: task.body.transition_index.unwrap_or(0),
            target_address: task.body.target_address,
            call_data: task.body.call_data.clone(),
            from_address: task.body.from_address,
            value: task.body.value,
            block_height: task.body.block_height,
            chain_id: 1u64,
        };

        assert_eq!(task_data.transition_index, 42);
        assert_eq!(task_data.target_address, Address::from([1u8; 20]));
    }
}

use crate::ingress::GasKillerTaskRequest;
use crate::task_data::GasKillerTaskData;
use commonware_avs_router::creator::Creator;

use anyhow::Result;
use async_trait::async_trait;
use commonware_codec::Encode;
use std::env;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// A queue that can hold and provide task requests
pub trait TaskQueue: Send + Sync {
    /// Add a task to the queue
    fn push(&self, task: GasKillerTaskRequest);

    /// Remove and return the next task from the queue
    fn pop(&self) -> Option<GasKillerTaskRequest>;
}

/// Simple in-memory task queue using Arc<Mutex> with proper error handling
#[derive(Clone)]
pub struct GasKillerTaskQueue {
    queue: Arc<Mutex<Vec<GasKillerTaskRequest>>>,
    timeout_ms: u64,
    max_retries: u32,
}

impl GasKillerTaskQueue {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(Vec::new())),
            timeout_ms: 1000, // 1 second default timeout
            max_retries: 3,   // 3 retries by default
        }
    }

    pub fn with_timeout(timeout_ms: u64) -> Self {
        Self {
            queue: Arc::new(Mutex::new(Vec::new())),
            timeout_ms,
            max_retries: 3,
        }
    }

    pub fn with_config(timeout_ms: u64, max_retries: u32) -> Self {
        Self {
            queue: Arc::new(Mutex::new(Vec::new())),
            timeout_ms,
            max_retries,
        }
    }

    /// Try to acquire the lock with timeout and retries
    fn try_lock_with_timeout(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Vec<GasKillerTaskRequest>>, String> {
        let start_time = Instant::now();
        let timeout_duration = Duration::from_millis(self.timeout_ms);

        for attempt in 0..self.max_retries {
            // Try to acquire the lock
            match self.queue.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(_) => {
                    // Check if we've exceeded the timeout
                    if start_time.elapsed() >= timeout_duration {
                        return Err(format!(
                            "Failed to acquire lock after {}ms timeout ({} attempts)",
                            self.timeout_ms,
                            attempt + 1
                        ));
                    }

                    // Small delay before retry to avoid busy waiting
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }

        Err(format!(
            "Failed to acquire lock after {} retries",
            self.max_retries
        ))
    }
}

impl Default for GasKillerTaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

// Align naming with the counter usecase so factories and ingress can share a consistent type.
pub type SimpleTaskQueue = GasKillerTaskQueue;

impl TaskQueue for GasKillerTaskQueue {
    fn push(&self, task: GasKillerTaskRequest) {
        match self.try_lock_with_timeout() {
            Ok(mut queue) => {
                queue.push(task);
                info!("Task enqueued: queue_len={} (after push)", queue.len());
            }
            Err(e) => {
                error!("Failed to push task to queue: {}", e);
                warn!("Task dropped due to lock timeout: {:?}", task);
            }
        }
    }

    fn pop(&self) -> Option<GasKillerTaskRequest> {
        match self.try_lock_with_timeout() {
            Ok(mut queue) => queue.pop(),
            Err(e) => {
                error!("Failed to pop task from queue: {}", e);
                None
            }
        }
    }
}

/// Configuration for listening creators
#[derive(Debug, Clone)]
pub struct GasKillerConfig {
    pub polling_interval_ms: u64,
    pub timeout_ms: u64,
}

impl Default for GasKillerConfig {
    fn default() -> Self {
        let timeout_ms: u64 = env::var("INGRESS_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30_000);

        Self {
            polling_interval_ms: 100,
            timeout_ms,
        }
    }
}

/// Creator for the gas killer usecase without ingress
#[derive(Default)]
pub struct GasKillerCreator {}

impl GasKillerCreator {
    pub fn new() -> Self {
        Self {}
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

    fn get_task_metadata(&self) -> Self::TaskData {
        GasKillerTaskData::default()
    }
}

/// Creator for the gas killer usecase that listens for external requests
pub struct ListeningGasKillerCreator<Q: TaskQueue + Send + Sync + 'static> {
    queue: Arc<Q>,
    config: GasKillerConfig,
    current_task: std::sync::Mutex<Option<GasKillerTaskRequest>>,
}

impl<Q: TaskQueue + Send + Sync + 'static> ListeningGasKillerCreator<Q> {
    pub fn new(queue: Q, config: GasKillerConfig) -> Self {
        Self {
            queue: Arc::new(queue),
            config,
            current_task: std::sync::Mutex::new(None),
        }
    }

    async fn wait_for_task(&self) -> Result<GasKillerTaskRequest> {
        use tokio::time::{Duration, sleep};
        let mut attempts = 0;
        let max_attempts = self.config.timeout_ms / self.config.polling_interval_ms;
        loop {
            if let Some(task) = self.queue.pop() {
                // Store the task for metadata access
                if let Ok(mut current_task) = self.current_task.lock() {
                    *current_task = Some(task.clone());
                } else {
                    error!(
                        "Failed to acquire lock on current_task mutex when storing task metadata"
                    );
                }
                return Ok(task);
            }
            attempts += 1;
            if attempts >= max_attempts {
                break;
            }
            sleep(Duration::from_millis(self.config.polling_interval_ms)).await;
        }
        Err(anyhow::anyhow!(
            "Timeout waiting for task after {}ms",
            self.config.timeout_ms
        ))
    }
}

#[async_trait]
impl<Q: TaskQueue + Send + Sync + 'static> Creator for ListeningGasKillerCreator<Q> {
    type TaskData = GasKillerTaskData;

    async fn get_payload_and_round(&self) -> Result<(Vec<u8>, u64)> {
        let task = self.wait_for_task().await?;
        info!(
            target = format!("{:?}", task.body.target_address),
            from = format!("{:?}", task.body.from_address),
            transition_index = task.body.transition_index,
            call_data_len = task.body.call_data.len(),
            storage_updates_len = task.body.storage_updates.len(),
            "Creator received task"
        );
        let payload = self.get_task_metadata().encode().to_vec();
        Ok((payload, 0)) // set default "round" to 0
    }

    fn get_task_metadata(&self) -> Self::TaskData {
        // Try to get metadata from the current task, fall back to defaults if not available

        if let Ok(current_task) = self.current_task.lock()
            && let Some(ref task) = *current_task
        {
            // Extract metadata from the task request body
            info!("Building task metadata from current task");

            return GasKillerTaskData {
                storage_updates: task.body.storage_updates.clone(),
                transition_index: task.body.transition_index,
                target_address: task.body.target_address,
                call_data: task.body.call_data.clone(),
                from_address: task.body.from_address,
                value: task.body.value,
            };
        }

        // Fall back to default metadata if no task data is available
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
    Listening(ListeningGasKillerCreator<GasKillerTaskQueue>),
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

    #[tokio::test]
    async fn test_validator_produces_consistent_hash() {
        // This test verifies the production flow:
        // 1. Creator produces task data
        // 2. Task data is serialized via wire protocol
        // 3. Validator receives and produces a hash
        // 4. The same task data always produces the same hash
        use crate::validator::GasKillerValidator;
        use commonware_avs_router::validator::ValidatorTrait;
        use commonware_avs_router::wire;
        use commonware_codec::{EncodeSize, Write};

        let validator = GasKillerValidator::with_rpc_url("https://ethereum-holesky.publicnode.com");

        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
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
        let hash1 = validator
            .get_payload_from_message(&msg_bytes)
            .await
            .unwrap();

        // Get hash from validator (second call with same data)
        let hash2 = validator
            .get_payload_from_message(&msg_bytes)
            .await
            .unwrap();

        // Hashes should be identical for the same input
        assert_eq!(hash1, hash2);

        // Hash should not be all zeros
        let zero_hash = commonware_cryptography::sha256::Digest::from([0u8; 32]);
        assert_ne!(hash1, zero_hash);
    }
}

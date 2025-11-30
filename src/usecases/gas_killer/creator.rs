#![allow(dead_code)]
use crate::creator::core::Creator;
use crate::services::GasAnalyzer;
use crate::usecases::gas_killer::ingress::GasKillerTaskRequest;
use crate::usecases::gas_killer::task_data::GasKillerTaskData;
use alloy::sol_types::SolValue;

use anyhow::Result;
use async_trait::async_trait;
use commonware_codec::Encode;
use std::env;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// Internal representation of a task with computed storage updates
#[derive(Clone)]
struct EnrichedTask {
    request: GasKillerTaskRequest,
    computed_storage_updates: Vec<u8>,
}

/// A queue that can hold and provide task requests
pub trait TaskQueue: Send + Sync {
    /// Add a task to the queue
    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub fn with_timeout(timeout_ms: u64) -> Self {
        Self {
            queue: Arc::new(Mutex::new(Vec::new())),
            timeout_ms,
            max_retries: 3,
        }
    }

    #[allow(dead_code)]
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
    /// Creates payload bytes from the task data
    ///
    /// The payload is deterministically encoded from the task data to ensure
    /// that the same analysis produces the same payload hash for consensus.
    /// This must match the on-chain expectedHash in GasKillerSDK.verifyAndUpdate:
    /// sha256(abi.encode(transitionIndex, address(this), targetFunction, storageUpdates))
    #[allow(dead_code)]
    pub fn create_payload_from_task_data(task_data: &GasKillerTaskData) -> Result<Vec<u8>> {
        use alloy::primitives::U256;

        // This must match GasKillerValidator::reconstruct_payload_hash exactly
        let selector = task_data.function_selector();

        // Build flattened ABI encoding matching abi.encode(transitionIndex, address(this), selector, storageUpdates)
        // Heads (32 bytes each)
        let head_transition = U256::from(task_data.transition_index).abi_encode();
        let head_address = task_data.target_address.abi_encode();
        let head_selector = selector.abi_encode();
        // Offset to the dynamic bytes tail: 4 words (3 static + 1 offset) = 0x80
        let head_offset = U256::from(32u64 * 4u64).abi_encode();

        // Tail for dynamic bytes: length (u256) + data + padding
        let data_bytes: &[u8] = &task_data.storage_updates;
        let mut tail = Vec::with_capacity(32 + data_bytes.len() + 31);
        tail.extend_from_slice(&U256::from(data_bytes.len()).abi_encode());
        tail.extend_from_slice(data_bytes);
        let pad_len = (32 - (data_bytes.len() % 32)) % 32;
        if pad_len > 0 {
            tail.extend(std::iter::repeat_n(0u8, pad_len));
        }

        // Concatenate head and tail into final payload
        let mut payload = Vec::with_capacity(32 * 4 + tail.len());
        payload.extend_from_slice(&head_transition);
        payload.extend_from_slice(&head_address);
        payload.extend_from_slice(&head_selector);
        payload.extend_from_slice(&head_offset);
        payload.extend_from_slice(&tail);

        Ok(payload)
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
    gas_analyzer: GasAnalyzer,
    current_task: Mutex<Option<EnrichedTask>>,
}

impl<Q: TaskQueue + Send + Sync + 'static> ListeningGasKillerCreator<Q> {
    pub fn new(queue: Q, config: GasKillerConfig, gas_analyzer: GasAnalyzer) -> Self {
        Self {
            queue: Arc::new(queue),
            config,
            gas_analyzer,
            current_task: Mutex::new(None),
        }
    }

    async fn wait_for_task(&self) -> Result<GasKillerTaskRequest> {
        use tokio::time::{Duration, sleep};
        let mut attempts = 0;
        let max_attempts = self.config.timeout_ms / self.config.polling_interval_ms;
        loop {
            if let Some(task) = self.queue.pop() {
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

    /// Computes storage updates for a task using the gas analyzer
    async fn compute_storage_updates(&self, task: &GasKillerTaskRequest) -> Result<Vec<u8>> {
        let result = self
            .gas_analyzer
            .analyze_transaction(
                task.body.target_address,
                &task.body.call_data,
                Some(task.body.from_address),
                Some(task.body.value),
            )
            .await?;
        Ok(result.storage_updates)
    }
}

#[async_trait]
impl<Q: TaskQueue + Send + Sync + 'static> Creator for ListeningGasKillerCreator<Q> {
    type TaskData = GasKillerTaskData;

    async fn get_payload_and_round(&self) -> Result<(Vec<u8>, u64)> {
        let task = self.wait_for_task().await?;

        // Compute storage_updates using gas analyzer
        let storage_updates = self
            .compute_storage_updates(&task)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to compute storage updates: {}", e))?;

        info!(
            target = format!("{:?}", task.body.target_address),
            from = format!("{:?}", task.body.from_address),
            transition_index = task.body.transition_index,
            call_data_len = task.body.call_data.len(),
            storage_updates_len = storage_updates.len(),
            "Creator computed storage updates for task"
        );

        // Store enriched task for metadata access
        if let Ok(mut current_task) = self.current_task.lock() {
            *current_task = Some(EnrichedTask {
                request: task,
                computed_storage_updates: storage_updates,
            });
        } else {
            error!("Failed to acquire lock on current_task mutex");
        }

        let payload = self.get_task_metadata().encode().to_vec();
        Ok((payload, 0)) // set default "round" to 0
    }

    fn get_task_metadata(&self) -> Self::TaskData {
        // Try to get metadata from the current task, fall back to defaults if not available
        if let Ok(current_task) = self.current_task.lock()
            && let Some(ref enriched) = *current_task
        {
            // Extract metadata from the enriched task
            info!("Building task metadata from current task");

            return GasKillerTaskData {
                storage_updates: enriched.computed_storage_updates.clone(),
                transition_index: enriched.request.body.transition_index,
                target_address: enriched.request.body.target_address,
                call_data: enriched.request.body.call_data.clone(),
                from_address: enriched.request.body.from_address,
                value: enriched.request.body.value,
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
    async fn test_create_payload_from_task_data() {
        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
        };

        let result = GasKillerCreator::create_payload_from_task_data(&task_data);
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_creator_validator_hash_consistency() {
        use crate::usecases::gas_killer::validator::GasKillerValidator;

        let validator = GasKillerValidator::new();

        let task_data = GasKillerTaskData {
            storage_updates: vec![0x01, 0x02, 0x03, 0x04],
            transition_index: 1,
            target_address: Address::from([1u8; 20]),
            call_data: vec![0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01],
            from_address: Address::from([2u8; 20]),
            value: U256::from(1000),
        };

        // Create payload using creator
        let creator_payload = GasKillerCreator::create_payload_from_task_data(&task_data);
        assert!(creator_payload.is_ok());

        // Create a test message to validate with validator
        use crate::validator::interface::ValidatorTrait;
        use crate::wire;
        use commonware_codec::{EncodeSize, Write};

        let aggregation = wire::Aggregation::<GasKillerTaskData>::new(
            1, // round
            task_data.clone(),
            None, // payload
        );

        let mut msg_bytes = Vec::with_capacity(aggregation.encode_size());
        aggregation.write(&mut msg_bytes);

        // Get payload hash using validator
        let validator_payload_hash = validator
            .get_payload_from_message(&msg_bytes)
            .await
            .unwrap();

        // Hash the creator payload to compare
        use commonware_cryptography::{Hasher, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&creator_payload.unwrap());
        let creator_payload_hash = hasher.finalize();

        // Both should produce the same hash
        assert_eq!(creator_payload_hash, validator_payload_hash);
    }
}

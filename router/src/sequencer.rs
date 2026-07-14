//! Task source: pulls tasks off the ingress queue and enriches them (EVMSketch)
//! into [`GasKillerTaskData`] ready for aggregation.
//!
//! Height assignment, directive broadcast/rebroadcast, and resolution tracking are
//! generic and live in [`commonware_avs_router::sequencer`]; this module supplies
//! only the [`commonware_avs_router::sequencer::TaskSource`] implementation that
//! feeds it.

use crate::ingress::GasKillerTaskRequest;
use crate::metrics::MetricsCollector;
use commonware_avs_router::sequencer::{SequencedTask, TaskSource};
use gas_killer_common::GasKillerValidator;
use gas_killer_common::task_data::GasKillerTaskData;

use alloy_primitives::Bytes;
use anyhow::Result;
use commonware_cryptography::{Hasher, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info};

pub type TaskSender = UnboundedSender<GasKillerTaskRequest>;
pub type TaskReceiver = UnboundedReceiver<GasKillerTaskRequest>;
/// Shared atomic counter tracking tasks in flight between the ingress sender and
/// the task source's receiver.
pub type TaskQueueDepth = Arc<AtomicUsize>;

pub fn task_channel() -> (TaskSender, TaskReceiver) {
    mpsc::unbounded_channel()
}

pub fn task_queue_depth() -> TaskQueueDepth {
    Arc::new(AtomicUsize::new(0))
}

/// Enriched task data that includes computed storage updates and block height.
struct EnrichedTask {
    task: GasKillerTaskRequest,
    storage_updates: Bytes,
    block_height: u64,
    /// Resolved transition index (sentinel `None` → concrete count from chain).
    transition_index: u64,
    /// Actual EVM chain ID (e.g. 1 = Ethereum mainnet, 100 = Gnosis, 31337 = Anvil).
    chain_id: u64,
}

impl EnrichedTask {
    fn into_task_data(self) -> GasKillerTaskData {
        GasKillerTaskData {
            storage_updates: self.storage_updates,
            transition_index: self.transition_index,
            target_address: self.task.body.target_address,
            call_data: self.task.body.call_data,
            from_address: self.task.body.from_address,
            value: self.task.body.value,
            block_height: self.block_height,
            chain_id: self.chain_id,
        }
    }
}

/// Pulls tasks from the ingress queue and enriches them into [`GasKillerTaskData`]
/// for the aggregation [`commonware_avs_router::sequencer::Sequencer`].
pub struct GasKillerTaskSource {
    receiver: TaskReceiver,
    queue_depth: TaskQueueDepth,
    validator: Arc<GasKillerValidator>,
    metrics: Option<Arc<MetricsCollector>>,
}

impl GasKillerTaskSource {
    pub fn new(
        receiver: TaskReceiver,
        queue_depth: TaskQueueDepth,
        validator: Arc<GasKillerValidator>,
        metrics: Option<Arc<MetricsCollector>>,
    ) -> Self {
        Self {
            receiver,
            queue_depth,
            validator,
            metrics,
        }
    }

    /// Blocks until a task arrives, maintaining the queue-depth metric.
    ///
    /// Returns `None` when the ingress side of the channel closed.
    async fn wait_for_task(&mut self) -> Option<GasKillerTaskRequest> {
        let task = self.receiver.recv().await?;
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
        Some(task)
    }

    /// Computes storage updates (EVMSketch) and resolves the transition index for a
    /// dequeued task. Lifted nearly verbatim from the old creator.
    async fn enrich(&self, task: GasKillerTaskRequest) -> Result<EnrichedTask> {
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
            "Sequencer computed storage updates"
        );

        Ok(EnrichedTask {
            task,
            storage_updates: storage_updates.into(),
            block_height,
            transition_index: resolved_transition_index,
            chain_id: numeric_chain_id,
        })
    }
}

#[async_trait::async_trait]
impl TaskSource<GasKillerTaskData> for GasKillerTaskSource {
    /// Dequeues the next ingress task and enriches it. Enrichment failures are
    /// logged and dropped; the loop keeps waiting for the next task rather than
    /// shutting the sequencer down.
    async fn next_task(&mut self) -> Option<SequencedTask<GasKillerTaskData>> {
        loop {
            let task = self.wait_for_task().await?;
            let enriched = match self.enrich(task).await {
                Ok(enriched) => enriched,
                Err(e) => {
                    error!(error = %e, "failed to enrich task, dropping request");
                    continue;
                }
            };
            let task_data = enriched.into_task_data();

            // The wire codec asserts the combined calldata + storage-updates size
            // (ingress only bounds the calldata; EVMSketch produces the updates),
            // so reject gracefully here instead of panicking mid-broadcast.
            if let Err(e) = task_data.validate() {
                error!(
                    error = %e,
                    target = %task_data.target_address,
                    "enriched task exceeds wire limits, dropping request"
                );
                continue;
            }

            // Prime the validator's digest cache so the router automaton (and any
            // digest re-derivation) skips EVMSketch: this source already ran it.
            self.validator
                .prime_cache(&task_data, &task_data.storage_updates)
                .await;
            let digest = task_data.build_payload_hash(&task_data.storage_updates);

            // Announce WITHOUT the router's computed storage_updates: nodes
            // independently recompute them with EVMSketch (that is the whole trust
            // model — see GasKillerValidator::expected_digest_for_task), so shipping
            // them is both a validation-bypass smell and dead weight. Dropping the
            // ~700-byte diff shrinks the Announce ~4x, which is the difference between
            // it reliably fitting a single unreliable p2p frame and being dropped in
            // favor of the tiny Skip. The digest is unaffected (the node builds it
            // from its own recomputed updates).
            let announce = GasKillerTaskData {
                storage_updates: Bytes::new(),
                ..task_data.clone()
            };

            return Some(SequencedTask {
                task: task_data,
                announce,
                digest,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, U256};

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

        let enriched = EnrichedTask {
            task,
            storage_updates: vec![0x01, 0x02, 0x03, 0x04].into(), // computed by GasAnalyzer
            block_height: 12345,
            transition_index: 42,
            chain_id: 1u64,
        };
        let task_data = enriched.into_task_data();

        assert_eq!(task_data.transition_index, 42);
        assert_eq!(task_data.target_address, Address::from([1u8; 20]));
        assert_eq!(task_data.chain_id, 1);
    }
}

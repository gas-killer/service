use super::types::GasKillerTask;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Mock Creator for Gas Killer tasks
/// This component manages the task queue and provides tasks to the orchestrator
pub struct GasKillerCreator {
    task_queue: Arc<Mutex<VecDeque<GasKillerTask>>>,
    current_round: Arc<Mutex<u64>>,
}

impl GasKillerCreator {
    pub fn new() -> Self {
        Self {
            task_queue: Arc::new(Mutex::new(VecDeque::new())),
            current_round: Arc::new(Mutex::new(0)),
        }
    }

    /// Add a new task to the queue
    pub fn add_task(&self, task: GasKillerTask) -> Result<(), String> {
        let mut queue = self.task_queue.lock().unwrap();
        queue.push_back(task);
        println!(
            "Added new Gas Killer task to queue, queue size: {}",
            queue.len()
        );
        Ok(())
    }

    /// Get the next task from the queue
    pub fn get_next_task(&self) -> Option<GasKillerTask> {
        let mut queue = self.task_queue.lock().unwrap();
        queue.pop_front()
    }

    /// Get current queue size
    pub fn queue_size(&self) -> usize {
        self.task_queue.lock().unwrap().len()
    }

    /// Get payload and round for orchestration
    pub async fn get_payload_and_round(&self) -> Result<(Vec<u8>, u64), String> {
        // Wait for a task to be available
        let task = loop {
            if let Some(task) = self.get_next_task() {
                break task;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        };

        // Increment round
        let mut round = self.current_round.lock().unwrap();
        *round += 1;
        let current_round = *round;

        // Create payload from task
        let payload = task.to_bytes();

        println!(
            "Created payload for Gas Killer task: {:02x?}..., round: {}, payload size: {} bytes",
            &task.task_id[..8],
            current_round,
            payload.len()
        );

        Ok((payload, current_round))
    }

    /// Get metadata of the current task (mock implementation)
    pub fn get_task_metadata(&self) -> GasKillerTask {
        self.task_queue
            .lock()
            .unwrap()
            .front()
            .cloned()
            .unwrap_or(GasKillerTask {
                task_id: [0u8; 32],
                chain_id: 1,
                target_contract: [0u8; 20],
                calldata: Vec::new(),
                priority: 0,
                timestamp: 0,
            })
    }
}

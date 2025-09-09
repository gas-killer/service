use super::task::{GasKillerTaskData, QueueMessage, Task, TaskEvent};
use crate::creator::core::Creator;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc};
use tokio::time::{interval, sleep};
use tracing::{error, info, warn};

const MAX_RETRIES: u32 = 3;
const RETRY_DELAY_MS: u64 = 1000;
const QUEUE_POLL_INTERVAL_MS: u64 = 100;

pub struct GasKillerCreator {
    queue_receiver: Arc<RwLock<mpsc::UnboundedReceiver<QueueMessage>>>,
    orchestrator_sender: mpsc::UnboundedSender<TaskEvent>,
    current_task: Arc<RwLock<Option<Task>>>,
    task_buffer: Arc<RwLock<Vec<Task>>>,
    dead_letter_queue: Arc<RwLock<Vec<QueueMessage>>>,
}

impl GasKillerCreator {
    pub fn new(
        queue_receiver: mpsc::UnboundedReceiver<QueueMessage>,
        orchestrator_sender: mpsc::UnboundedSender<TaskEvent>,
    ) -> Self {
        Self {
            queue_receiver: Arc::new(RwLock::new(queue_receiver)),
            orchestrator_sender,
            current_task: Arc::new(RwLock::new(None)),
            task_buffer: Arc::new(RwLock::new(Vec::new())),
            dead_letter_queue: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn run(&self) -> Result<()> {
        info!("Starting GasKillerCreator");

        let mut poll_interval = interval(Duration::from_millis(QUEUE_POLL_INTERVAL_MS));

        loop {
            poll_interval.tick().await;

            if let Err(e) = self.process_queue_messages().await {
                error!("Error processing queue messages: {}", e);
            }
        }
    }

    pub async fn process_queue_messages(&self) -> Result<()> {
        let mut receiver = self.queue_receiver.write().await;

        while let Ok(message) = receiver.try_recv() {
            match self.handle_message(message.clone()).await {
                Ok(task) => {
                    info!("Successfully created task: {}", task.id);
                    self.store_task(task).await;
                }
                Err(e) => {
                    error!("Failed to handle message: {}", e);
                    self.add_to_dlq(message).await;
                }
            }
        }

        Ok(())
    }

    async fn handle_message(&self, message: QueueMessage) -> Result<Task> {
        message.validate()?;

        let task = Task::new(
            message.request_id,
            message.target_contract,
            message.target_method,
            message.target_chain_id,
            message.params,
            message.caller,
            message.metadata,
        );

        self.send_to_orchestrator_with_retry(&task).await?;

        Ok(task)
    }

    async fn send_to_orchestrator_with_retry(&self, task: &Task) -> Result<()> {
        let event = task.to_event();
        let mut retries = 0;

        loop {
            match self.orchestrator_sender.send(event.clone()) {
                Ok(_) => {
                    info!("Task event sent to orchestrator: {}", task.id);
                    return Ok(());
                }
                Err(e) => {
                    retries += 1;
                    if retries >= MAX_RETRIES {
                        return Err(anyhow!(
                            "Failed to send to orchestrator after {} retries: {}",
                            MAX_RETRIES,
                            e
                        ));
                    }
                    warn!(
                        "Failed to send to orchestrator (attempt {}/{}): {}",
                        retries, MAX_RETRIES, e
                    );
                    sleep(Duration::from_millis(RETRY_DELAY_MS * retries as u64)).await;
                }
            }
        }
    }

    pub async fn store_task(&self, task: Task) {
        let mut current = self.current_task.write().await;
        *current = Some(task.clone());

        let mut buffer = self.task_buffer.write().await;
        buffer.push(task);

        if buffer.len() > 1000 {
            buffer.drain(0..500);
        }
    }

    pub async fn add_to_dlq(&self, message: QueueMessage) {
        let mut dlq = self.dead_letter_queue.write().await;
        dlq.push(message);

        if dlq.len() > 100 {
            warn!("DLQ size exceeds 100 messages, removing oldest 50");
            dlq.drain(0..50);
        }
    }

    pub async fn get_current_task(&self) -> Option<Task> {
        self.current_task.read().await.clone()
    }

    pub async fn get_dlq_size(&self) -> usize {
        self.dead_letter_queue.read().await.len()
    }

    pub async fn get_task_buffer_size(&self) -> usize {
        self.task_buffer.read().await.len()
    }
}

#[async_trait]
impl Creator for GasKillerCreator {
    type TaskData = GasKillerTaskData;

    async fn get_payload_and_round(&self) -> Result<(Vec<u8>, u64)> {
        let current_task = self.current_task.read().await;

        match &*current_task {
            Some(task) => {
                let mut payload = Vec::new();
                payload.extend_from_slice(task.target_contract.as_slice());
                payload.extend_from_slice(task.target_method.as_bytes());
                payload.extend_from_slice(&task.params);

                let round = task.created_at;

                Ok((payload, round))
            }
            None => Err(anyhow!("No current task available")),
        }
    }

    fn get_task_metadata(&self) -> Self::TaskData {
        // Note: This is a synchronous method, so we use try_read to avoid blocking
        // In production, the task should be set before this is called
        let current_task = self
            .current_task
            .try_read()
            .ok()
            .and_then(|guard| guard.clone());

        match current_task {
            Some(task) => GasKillerTaskData {
                task_id: task.id,
                chain_id: task.target_chain_id,
                target_contract: task.target_contract,
                target_method: task.target_method,
                params: task.params.to_vec(),
            },
            None => GasKillerTaskData {
                task_id: uuid::Uuid::nil(),
                chain_id: 0,
                target_contract: alloy_primitives::Address::ZERO,
                target_method: String::new(),
                params: Vec::new(),
            },
        }
    }
}

pub struct QueueSender {
    sender: mpsc::UnboundedSender<QueueMessage>,
}

impl QueueSender {
    pub fn new(sender: mpsc::UnboundedSender<QueueMessage>) -> Self {
        Self { sender }
    }

    pub fn send(&self, message: QueueMessage) -> Result<()> {
        self.sender
            .send(message)
            .map_err(|e| anyhow!("Failed to send message to queue: {}", e))
    }
}

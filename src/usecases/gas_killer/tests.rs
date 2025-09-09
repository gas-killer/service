use super::creator::{GasKillerCreator, QueueSender};
use super::task::{QueueMessage, Task, TaskEvent, TaskStatus};
use crate::creator::core::Creator;
use alloy_primitives::{Address, Bytes};
use std::collections::HashMap;
use std::str::FromStr;
use tokio::sync::mpsc;
use uuid::Uuid;

#[tokio::test]
async fn test_task_creation() {
    let request_id = Uuid::new_v4();
    let target_contract = Address::from_str("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7").unwrap();
    let target_method = "transfer".to_string();
    let target_chain_id = 1u64;
    let params = Bytes::from(vec![1, 2, 3, 4]);
    let caller = Address::from_str("0x5aAeb6053f3E94C9b9A09f33669435E7Ef1BeAed").unwrap();
    let mut metadata = HashMap::new();
    metadata.insert("gas_price".to_string(), "100000000000".to_string());

    let task = Task::new(
        request_id,
        target_contract,
        target_method.clone(),
        target_chain_id,
        params.clone(),
        caller,
        metadata.clone(),
    );

    assert_eq!(task.request_id, request_id);
    assert_eq!(task.target_contract, target_contract);
    assert_eq!(task.target_method, target_method);
    assert_eq!(task.target_chain_id, target_chain_id);
    assert_eq!(task.params, params);
    assert_eq!(task.caller, caller);
    assert_eq!(task.status, TaskStatus::Pending);
    assert!(task.priority > 50);
}

#[tokio::test]
async fn test_priority_calculation() {
    let mut metadata = HashMap::new();

    let priority_low = Task::calculate_priority(&metadata, 100);
    assert_eq!(priority_low, 50);

    metadata.insert("gas_price".to_string(), "150000000000".to_string());
    let priority_high_gas = Task::calculate_priority(&metadata, 100);
    assert_eq!(priority_high_gas, 70);

    let priority_mainnet = Task::calculate_priority(&metadata, 1);
    assert_eq!(priority_mainnet, 100);

    metadata.insert("urgent".to_string(), "true".to_string());
    let priority_urgent = Task::calculate_priority(&metadata, 100);
    assert_eq!(priority_urgent, 95);
}

#[tokio::test]
async fn test_queue_message_validation() {
    let valid_message = QueueMessage {
        request_id: Uuid::new_v4(),
        target_contract: Address::from_str("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7").unwrap(),
        target_method: "transfer".to_string(),
        target_chain_id: 1,
        params: Bytes::from(vec![1, 2, 3]),
        caller: Address::from_str("0x5aAeb6053f3E94C9b9A09f33669435E7Ef1BeAed").unwrap(),
        metadata: HashMap::new(),
    };

    assert!(valid_message.validate().is_ok());

    let invalid_contract = QueueMessage {
        target_contract: Address::ZERO,
        ..valid_message.clone()
    };
    assert!(invalid_contract.validate().is_err());

    let invalid_method = QueueMessage {
        target_method: "".to_string(),
        ..valid_message.clone()
    };
    assert!(invalid_method.validate().is_err());

    let invalid_chain = QueueMessage {
        target_chain_id: 0,
        ..valid_message.clone()
    };
    assert!(invalid_chain.validate().is_err());

    let invalid_caller = QueueMessage {
        caller: Address::ZERO,
        ..valid_message.clone()
    };
    assert!(invalid_caller.validate().is_err());
}

#[tokio::test]
async fn test_gas_killer_creator_message_processing() {
    let (queue_tx, queue_rx) = mpsc::unbounded_channel::<QueueMessage>();
    let (orchestrator_tx, mut orchestrator_rx) = mpsc::unbounded_channel::<TaskEvent>();

    let creator = GasKillerCreator::new(queue_rx, orchestrator_tx);

    let message = QueueMessage {
        request_id: Uuid::new_v4(),
        target_contract: Address::from_str("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7").unwrap(),
        target_method: "transfer".to_string(),
        target_chain_id: 1,
        params: Bytes::from(vec![1, 2, 3]),
        caller: Address::from_str("0x5aAeb6053f3E94C9b9A09f33669435E7Ef1BeAed").unwrap(),
        metadata: HashMap::new(),
    };

    queue_tx.send(message.clone()).unwrap();

    tokio::spawn(async move {
        let _ = creator.process_queue_messages().await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    if let Ok(event) = orchestrator_rx.try_recv() {
        assert_eq!(event.event_type, "task_created");
        assert_eq!(event.chain_id, 1);
    }
}

#[tokio::test]
async fn test_creator_trait_implementation() {
    let (_queue_tx, queue_rx) = mpsc::unbounded_channel::<QueueMessage>();
    let (orchestrator_tx, _orchestrator_rx) = mpsc::unbounded_channel::<TaskEvent>();

    let creator = GasKillerCreator::new(queue_rx, orchestrator_tx);

    let task = Task::new(
        Uuid::new_v4(),
        Address::from_str("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7").unwrap(),
        "transfer".to_string(),
        1,
        Bytes::from(vec![1, 2, 3]),
        Address::from_str("0x5aAeb6053f3E94C9b9A09f33669435E7Ef1BeAed").unwrap(),
        HashMap::new(),
    );

    creator.store_task(task.clone()).await;

    let (payload, round) = creator.get_payload_and_round().await.unwrap();
    assert!(!payload.is_empty());
    assert_eq!(round, task.created_at);

    let metadata = creator.get_task_metadata();
    assert_eq!(metadata.task_id, task.id);
    assert_eq!(metadata.chain_id, task.target_chain_id);
}

#[tokio::test]
async fn test_dlq_handling() {
    let (_queue_tx, queue_rx) = mpsc::unbounded_channel::<QueueMessage>();
    let (orchestrator_tx, _orchestrator_rx) = mpsc::unbounded_channel::<TaskEvent>();

    let creator = GasKillerCreator::new(queue_rx, orchestrator_tx);

    let invalid_message = QueueMessage {
        request_id: Uuid::new_v4(),
        target_contract: Address::ZERO,
        target_method: "transfer".to_string(),
        target_chain_id: 1,
        params: Bytes::from(vec![1, 2, 3]),
        caller: Address::from_str("0x5aAeb6053f3E94C9b9A09f33669435E7Ef1BeAed").unwrap(),
        metadata: HashMap::new(),
    };

    creator.add_to_dlq(invalid_message).await;

    let dlq_size = creator.get_dlq_size().await;
    assert_eq!(dlq_size, 1);
}

#[tokio::test]
async fn test_task_buffer_management() {
    let (_queue_tx, queue_rx) = mpsc::unbounded_channel::<QueueMessage>();
    let (orchestrator_tx, _orchestrator_rx) = mpsc::unbounded_channel::<TaskEvent>();

    let creator = GasKillerCreator::new(queue_rx, orchestrator_tx);

    for i in 0..5 {
        let task = Task::new(
            Uuid::new_v4(),
            Address::from_str("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7").unwrap(),
            format!("method_{}", i),
            1,
            Bytes::from(vec![i as u8]),
            Address::from_str("0x5aAeb6053f3E94C9b9A09f33669435E7Ef1BeAed").unwrap(),
            HashMap::new(),
        );
        creator.store_task(task).await;
    }

    let buffer_size = creator.get_task_buffer_size().await;
    assert_eq!(buffer_size, 5);

    let current_task = creator.get_current_task().await;
    assert!(current_task.is_some());
    assert_eq!(current_task.unwrap().target_method, "method_4");
}

#[tokio::test]
async fn test_queue_sender() {
    let (tx, mut rx) = mpsc::unbounded_channel::<QueueMessage>();
    let sender = QueueSender::new(tx);

    let message = QueueMessage {
        request_id: Uuid::new_v4(),
        target_contract: Address::from_str("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7").unwrap(),
        target_method: "transfer".to_string(),
        target_chain_id: 1,
        params: Bytes::from(vec![1, 2, 3]),
        caller: Address::from_str("0x5aAeb6053f3E94C9b9A09f33669435E7Ef1BeAed").unwrap(),
        metadata: HashMap::new(),
    };

    assert!(sender.send(message.clone()).is_ok());

    let received = rx.recv().await.unwrap();
    assert_eq!(received.request_id, message.request_id);
    assert_eq!(received.target_contract, message.target_contract);
}

#[tokio::test]
async fn test_task_event_creation() {
    let task = Task::new(
        Uuid::new_v4(),
        Address::from_str("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7").unwrap(),
        "approve".to_string(),
        137,
        Bytes::from(vec![5, 6, 7]),
        Address::from_str("0x5aAeb6053f3E94C9b9A09f33669435E7Ef1BeAed").unwrap(),
        HashMap::new(),
    );

    let event = task.to_event();

    assert_eq!(event.task_id, task.id);
    assert_eq!(event.priority, task.priority);
    assert_eq!(event.chain_id, 137);
    assert_eq!(event.event_type, "task_created");
    assert_eq!(event.timestamp, task.created_at);
    assert_eq!(event.target_contract, task.target_contract);
    assert_eq!(event.target_method, "approve");
}

use super::*;
use tokio;

#[test]
fn test_gas_killer_task_serialization() {
    let task = GasKillerTask {
        task_id: [1u8; 32],
        chain_id: 1,
        target_contract: [2u8; 20],
        calldata: vec![3, 4, 5],
        priority: 1,
        timestamp: 1000,
    };

    // Test serialization
    let bytes = task.to_bytes();
    assert!(bytes.len() > 65);

    // Test deserialization
    let decoded = GasKillerTask::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.task_id, task.task_id);
    assert_eq!(decoded.chain_id, task.chain_id);
    assert_eq!(decoded.target_contract, task.target_contract);
    assert_eq!(decoded.calldata, task.calldata);
    assert_eq!(decoded.priority, task.priority);
    assert_eq!(decoded.timestamp, task.timestamp);
}

#[test]
fn test_operator_registry() {
    let mut registry = OperatorRegistry::new();
    let operator1 = [1u8; 64];
    let operator2 = [2u8; 64];

    // Add operators
    registry.operators.insert(1, vec![operator1, operator2]);
    registry.statuses.insert(operator1, OperatorStatus::Active);
    registry
        .statuses
        .insert(operator2, OperatorStatus::Inactive);
    registry.stakes.insert(operator1, 1000);
    registry.stakes.insert(operator2, 500);

    // Test active operators
    let active = registry.get_active_operators(1);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0], operator1);

    // Test stake requirements
    assert!(registry.check_stake_requirement(&operator1, 500));
    assert!(!registry.check_stake_requirement(&operator1, 2000));
    assert!(!registry.check_stake_requirement(&operator2, 1000));
}

#[test]
fn test_optimization_types() {
    // Test all optimization types are distinct
    assert_ne!(
        OptimizationType::StoragePacking as i32,
        OptimizationType::BatchedUpdates as i32
    );
    assert_ne!(
        OptimizationType::BatchedUpdates as i32,
        OptimizationType::ColdToWarmSlot as i32
    );
    assert_ne!(
        OptimizationType::ColdToWarmSlot as i32,
        OptimizationType::ZeroToNonZero as i32
    );
    assert_ne!(
        OptimizationType::ZeroToNonZero as i32,
        OptimizationType::NonZeroToZero as i32
    );
}

#[tokio::test]
async fn test_gas_killer_creator() {
    let creator = GasKillerCreator::new();

    // Add a task
    let task = GasKillerTask {
        task_id: [1u8; 32],
        chain_id: 1,
        target_contract: [2u8; 20],
        calldata: vec![3, 4, 5],
        priority: 1,
        timestamp: 1000,
    };

    creator.add_task(task.clone()).unwrap();
    assert_eq!(creator.queue_size(), 1);

    // Get next task
    let retrieved = creator.get_next_task().unwrap();
    assert_eq!(retrieved.task_id, task.task_id);
    assert_eq!(creator.queue_size(), 0);
}

#[tokio::test]
async fn test_gas_killer_validator() {
    let validator = GasKillerValidator::new();

    let task = GasKillerTask {
        task_id: [1u8; 32],
        chain_id: 1,
        target_contract: [2u8; 20],
        calldata: vec![3, 4, 5],
        priority: 1,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    };

    // Validate task
    let hash = validator
        .validate_and_return_expected_hash(&task)
        .await
        .unwrap();
    assert_eq!(hash.len(), 32);

    // Check state updates are cached
    let state_updates = validator.get_state_updates(&task.task_id).await;
    assert!(state_updates.is_some());
}

#[tokio::test]
async fn test_gas_killer_executor() {
    let executor = GasKillerExecutor::new();

    let package = ExecutionPackage {
        task_id: [1u8; 32],
        validated_data: GasKillerTask {
            task_id: [1u8; 32],
            chain_id: 1,
            target_contract: [2u8; 20],
            calldata: vec![],
            priority: 1,
            timestamp: 1000,
        },
        state_updates: vec![StateUpdate {
            storage_slot: [1u8; 32],
            old_value: [0u8; 32],
            new_value: [1u8; 32],
            gas_saved: 1000,
        }],
        aggregated_signature: vec![1, 2, 3],
        signers: vec![[1u8; 64]],
        validation_timestamp: 1000,
    };

    // Execute verification
    let result = executor
        .execute_verification(package.clone())
        .await
        .unwrap();
    assert!(result.success);
    assert!(!result.transaction_hash.is_empty());

    // Check execution was recorded
    let count = executor.get_execution_count().await;
    assert_eq!(count, 1);

    // Check executed task
    let executed = executor.get_executed_task(&package.task_id).await;
    assert!(executed.is_some());
}

#[tokio::test]
async fn test_orchestrator_config() {
    let config = GasKillerOrchestratorConfig::default();

    assert_eq!(
        config.aggregation_frequency,
        std::time::Duration::from_secs(10)
    );
    assert_eq!(config.quorum_threshold, 0.67);
    assert_eq!(config.min_operators, 3);
    assert_eq!(config.max_retries, 3);
}

#[tokio::test]
async fn test_validation_request() {
    let task = GasKillerTask {
        task_id: [1u8; 32],
        chain_id: 1,
        target_contract: [2u8; 20],
        calldata: vec![],
        priority: 1,
        timestamp: 1000,
    };

    let request = ValidationRequest {
        task_id: task.task_id,
        task_data: task.clone(),
        validation_deadline: 2000,
        quorum_threshold: 67,
        operator_set: vec![[1u8; 64], [2u8; 64]],
    };

    assert_eq!(request.task_id, task.task_id);
    assert_eq!(request.operator_set.len(), 2);
    assert_eq!(request.quorum_threshold, 67);
}

#[tokio::test]
async fn test_execution_package() {
    let package = ExecutionPackage {
        task_id: [1u8; 32],
        validated_data: GasKillerTask {
            task_id: [1u8; 32],
            chain_id: 1,
            target_contract: [2u8; 20],
            calldata: vec![],
            priority: 1,
            timestamp: 1000,
        },
        state_updates: vec![StateUpdate {
            storage_slot: [1u8; 32],
            old_value: [0u8; 32],
            new_value: [1u8; 32],
            gas_saved: 1000,
        }],
        aggregated_signature: vec![1, 2, 3],
        signers: vec![[1u8; 64]],
        validation_timestamp: 1000,
    };

    assert_eq!(package.task_id, [1u8; 32]);
    assert_eq!(package.state_updates.len(), 1);
    assert_eq!(package.signers.len(), 1);
    assert!(!package.aggregated_signature.is_empty());
}

#[tokio::test]
async fn test_gas_analysis_caching() {
    let validator = GasKillerValidator::new();

    let task = GasKillerTask {
        task_id: [10u8; 32],
        chain_id: 1,
        target_contract: [20u8; 20],
        calldata: vec![0, 1, 2],
        priority: 3,
        timestamp: 2000,
    };

    // First validation should perform analysis
    let hash1 = validator
        .validate_and_return_expected_hash(&task)
        .await
        .unwrap();

    // Second validation should use cache
    let hash2 = validator
        .validate_and_return_expected_hash(&task)
        .await
        .unwrap();

    assert_eq!(hash1, hash2);

    // State updates should be cached
    let state_updates = validator.get_state_updates(&task.task_id).await;
    assert!(state_updates.is_some());
    assert!(!state_updates.unwrap().is_empty());
}

#[tokio::test]
async fn test_optimization_type_detection() {
    let validator = GasKillerValidator::new();

    for i in 0..5 {
        let task = GasKillerTask {
            task_id: [i; 32],
            chain_id: 1,
            target_contract: [0u8; 20],
            calldata: vec![i],
            priority: 1,
            timestamp: 1000,
        };

        let _ = validator
            .validate_and_return_expected_hash(&task)
            .await
            .unwrap();

        let state_updates = validator.get_state_updates(&task.task_id).await.unwrap();
        assert!(!state_updates.is_empty());

        // BatchedUpdates should have 3 state updates, others should have 1
        if i % 5 == 1 {
            assert_eq!(state_updates.len(), 3);
        } else {
            assert_eq!(state_updates.len(), 1);
        }
    }
}

#[tokio::test]
async fn test_quorum_calculation() {
    let operators = &[[1u8; 64], [2u8; 64], [3u8; 64]];
    let quorum_threshold = 0.67;

    let required = ((operators.len() as f64) * quorum_threshold).ceil() as usize;
    assert_eq!(required, 3);

    let operators = &[[1u8; 64], [2u8; 64], [3u8; 64], [4u8; 64]];
    let required = ((operators.len() as f64) * quorum_threshold).ceil() as usize;
    assert_eq!(required, 3);

    let operators = &[[1u8; 64], [2u8; 64], [3u8; 64], [4u8; 64], [5u8; 64]];
    let required = ((operators.len() as f64) * quorum_threshold).ceil() as usize;
    assert_eq!(required, 4);
}

#[tokio::test]
async fn test_orchestrator_initialization() {
    let config = GasKillerOrchestratorConfig::default();
    let orchestrator = GasKillerOrchestrator::new(config);

    // Add a task
    let task = GasKillerTask {
        task_id: [100u8; 32],
        chain_id: 1,
        target_contract: [50u8; 20],
        calldata: vec![1, 2, 3],
        priority: 5,
        timestamp: 5000,
    };

    orchestrator.add_task(task).unwrap();

    // Verify Byzantine operator handling
    let byzantine_operators = vec![[99u8; 64], [98u8; 64]];
    orchestrator
        .handle_byzantine_operators(byzantine_operators)
        .await;
}

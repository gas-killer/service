use crate::validator::gas_killer::*;
use crate::validator::interface::ValidatorTrait;
use alloy_primitives::{Address, Bytes, U256};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[tokio::test]
async fn test_validation_task_creation() {
    let task = ValidationTask {
        task_id: Uuid::new_v4(),
        target_contract: Address::ZERO,
        target_method: "transfer".to_string(),
        target_chain_id: 1,
        params: Bytes::default(),
        caller: Address::ZERO,
        block_number: Some(12345),
    };
    
    assert_eq!(task.target_method, "transfer");
    assert_eq!(task.target_chain_id, 1);
    assert_eq!(task.block_number, Some(12345));
}

#[tokio::test]
async fn test_state_updates_creation() {
    let mut accessed_addresses = HashSet::new();
    accessed_addresses.insert(Address::ZERO);
    
    let state_updates = StateUpdates {
        storage_slots: vec![],
        account_changes: vec![],
        accessed_addresses,
        accessed_storage_keys: HashSet::new(),
        gas_used: U256::from(21000),
        gas_saved: U256::from(4200),
    };
    
    assert_eq!(state_updates.gas_used, U256::from(21000));
    assert_eq!(state_updates.gas_saved, U256::from(4200));
    assert!(state_updates.accessed_addresses.contains(&Address::ZERO));
}

#[tokio::test]
async fn test_gas_metrics_calculation() {
    let metrics = GasMetrics {
        original_gas: U256::from(100000),
        optimized_gas: U256::from(80000),
        savings_percentage: 20.0,
    };
    
    assert_eq!(metrics.original_gas, U256::from(100000));
    assert_eq!(metrics.optimized_gas, U256::from(80000));
    assert_eq!(metrics.savings_percentage, 20.0);
}

#[tokio::test]
async fn test_validator_creation_with_endpoints() {
    let mut endpoints = HashMap::new();
    endpoints.insert(1, "http://localhost:8545".to_string());
    endpoints.insert(10, "http://localhost:8546".to_string());
    
    let _validator = GasKillerValidator::new(endpoints.clone());
    
    assert_eq!(endpoints.len(), 2);
}

#[tokio::test]
async fn test_validator_with_custom_retry_config() {
    let mut endpoints = HashMap::new();
    endpoints.insert(1, "http://localhost:8545".to_string());
    
    let _validator = GasKillerValidator::new(endpoints)
        .with_retry_config(5, std::time::Duration::from_secs(3));
    
    // Validator created successfully with custom retry config
}

#[tokio::test]
async fn test_validation_response_serialization() {
    let response = ValidationResponse {
        task_id: Uuid::new_v4(),
        validated: true,
        state_updates: None,
        simulation_block: 12345,
        gas_metrics: Some(GasMetrics {
            original_gas: U256::from(100000),
            optimized_gas: U256::from(80000),
            savings_percentage: 20.0,
        }),
        error_message: None,
    };
    
    let serialized = serde_json::to_string(&response).unwrap();
    let deserialized: ValidationResponse = serde_json::from_str(&serialized).unwrap();
    
    assert_eq!(response.task_id, deserialized.task_id);
    assert_eq!(response.validated, deserialized.validated);
    assert_eq!(response.simulation_block, deserialized.simulation_block);
}

#[tokio::test]
async fn test_account_diff_serialization() {
    let diff = AccountDiff {
        address: Address::ZERO,
        balance_before: U256::from(1000),
        balance_after: U256::from(900),
        nonce_before: 5,
        nonce_after: 6,
    };
    
    let serialized = serde_json::to_string(&diff).unwrap();
    let deserialized: AccountDiff = serde_json::from_str(&serialized).unwrap();
    
    assert_eq!(diff.address, deserialized.address);
    assert_eq!(diff.balance_before, deserialized.balance_before);
    assert_eq!(diff.balance_after, deserialized.balance_after);
    assert_eq!(diff.nonce_before, deserialized.nonce_before);
    assert_eq!(diff.nonce_after, deserialized.nonce_after);
}

#[tokio::test]
async fn test_validator_trait_implementation() {
    let mut endpoints = HashMap::new();
    endpoints.insert(1, "http://localhost:8545".to_string());
    
    let validator = GasKillerValidator::new(endpoints);
    
    let msg = b"test message";
    let hash = validator.get_payload_from_message(msg).await;
    
    assert!(hash.is_ok());
}

#[tokio::test]
async fn test_port_picker() {
    use crate::validator::gas_killer::pick_unused_port;
    let port = pick_unused_port();
    assert!(port.is_some());
    
    if let Some(p) = port {
        assert!((8545..9000).contains(&p));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    
    #[tokio::test]
    #[ignore]
    async fn test_validation_with_real_anvil() {
        let mut endpoints = HashMap::new();
        endpoints.insert(1, std::env::var("ETH_RPC_URL").unwrap_or_else(|_| "http://localhost:8545".to_string()));
        
        let validator = GasKillerValidator::new(endpoints);
        
        let task = ValidationTask {
            task_id: Uuid::new_v4(),
            target_contract: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".parse().unwrap(),
            target_method: "balanceOf".to_string(),
            target_chain_id: 1,
            params: Bytes::from(hex::decode("70a08231000000000000000000000000742d35cc6634c0532925a3b844bc9e7595f0beb9").unwrap()),
            caller: Address::ZERO,
            block_number: None,
        };
        
        let response = validator.validate_task(task).await;
        
        assert!(response.is_ok());
        if let Ok(resp) = response {
            println!("Validation response: {resp:?}");
        }
    }
}
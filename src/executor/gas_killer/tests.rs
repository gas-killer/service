#[cfg(test)]
mod gas_killer_executor_tests {
    use crate::executor::gas_killer::handlers::{MockGasKillerHandler, MockGasPriceOracle};
    use crate::executor::gas_killer::traits::{GasKillerContractHandler, GasPriceOracle};
    use crate::executor::gas_killer::{
        ExecutionPackage, ExecutionStatus, GasKillerConfig, GasKillerExecutionResult, StateUpdate,
        types::GasPriceConfig,
    };
    use alloy_primitives::{Address, Bytes, FixedBytes, U256};

    #[allow(dead_code)]
    fn create_test_config() -> GasKillerConfig {
        GasKillerConfig {
            rpc_url: "http://localhost:8545".to_string(),
            private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                .to_string(),
            gk_interface_id: FixedBytes::from([0x01, 0x02, 0x03, 0x04]),
            gas_buffer_percent: 110,
            max_retries: 3,
            confirmation_timeout: 60,
        }
    }

    fn create_test_package() -> ExecutionPackage {
        ExecutionPackage {
            task_id: "test_task_001".to_string(),
            target_contract: Address::from([0x11; 20]),
            target_method: "executeOptimized".to_string(),
            params: Bytes::from(vec![0x01, 0x02, 0x03]),
            state_updates: vec![
                StateUpdate {
                    contract: Address::from([0x22; 20]),
                    slot: U256::from(1),
                    value: U256::from(100),
                },
                StateUpdate {
                    contract: Address::from([0x33; 20]),
                    slot: U256::from(2),
                    value: U256::from(200),
                },
            ],
            // Using a placeholder signature - would need proper construction in real usage
            aggregated_signature: unsafe { std::mem::zeroed() },
            operator_set: vec![Address::from([0x44; 20]), Address::from([0x55; 20])],
            validation_timestamp: 1234567890,
            chain_id: 1,
        }
    }

    #[test]
    fn test_execution_package_creation() {
        let package = create_test_package();
        assert_eq!(package.task_id, "test_task_001");
        assert_eq!(package.state_updates.len(), 2);
        assert_eq!(package.operator_set.len(), 2);
    }

    #[test]
    fn test_state_update_encoding() {
        let update = StateUpdate {
            contract: Address::from([0xaa; 20]),
            slot: U256::from(42),
            value: U256::from(1337),
        };

        assert_eq!(update.slot, U256::from(42));
        assert_eq!(update.value, U256::from(1337));
    }

    #[test]
    fn test_gas_price_calculation() {
        let config = GasPriceConfig {
            base_fee: U256::from(30_000_000_000u64),         // 30 gwei
            max_priority_fee: U256::from(2_000_000_000u64),  // 2 gwei
            max_fee_per_gas: U256::from(100_000_000_000u64), // 100 gwei
            priority: 0,
        };

        let optimal = config.calculate_optimal_price();
        // Should be base_fee + max_priority_fee = 32 gwei
        assert_eq!(optimal, U256::from(32_000_000_000u64));
    }

    #[test]
    fn test_gas_price_with_priority() {
        let config = GasPriceConfig {
            base_fee: U256::from(30_000_000_000u64),         // 30 gwei
            max_priority_fee: U256::from(2_000_000_000u64),  // 2 gwei
            max_fee_per_gas: U256::from(100_000_000_000u64), // 100 gwei
            priority: 2,                                     // High priority
        };

        let optimal = config.calculate_optimal_price();
        // Should be (base_fee + max_priority_fee) * 1.25 = 40 gwei
        assert_eq!(optimal, U256::from(40_000_000_000u64));
    }

    #[test]
    fn test_gas_price_capped_at_max() {
        let config = GasPriceConfig {
            base_fee: U256::from(90_000_000_000u64),         // 90 gwei
            max_priority_fee: U256::from(20_000_000_000u64), // 20 gwei
            max_fee_per_gas: U256::from(100_000_000_000u64), // 100 gwei cap
            priority: 2,                                     // High priority would push it over
        };

        let optimal = config.calculate_optimal_price();
        // Should be capped at max_fee_per_gas
        assert_eq!(optimal, U256::from(100_000_000_000u64));
    }

    #[test]
    fn test_execution_status_serialization() {
        let status = ExecutionStatus::Confirmed;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"Confirmed\"");

        let deserialized: ExecutionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ExecutionStatus::Confirmed);
    }

    #[tokio::test]
    async fn test_mock_gas_price_oracle() {
        let oracle = MockGasPriceOracle;

        let base_fee = oracle.get_base_fee().await.unwrap();
        assert_eq!(base_fee, U256::from(30_000_000_000u64));

        let priority_fee = oracle.get_priority_fee(1).await.unwrap();
        assert_eq!(priority_fee, U256::from(2_000_000_000u64));

        let max_fee = oracle.get_max_fee_per_gas().await.unwrap();
        assert_eq!(max_fee, U256::from(100_000_000_000u64));

        let predicted = oracle.predict_gas_price(5).await.unwrap();
        assert_eq!(predicted, U256::from(35_000_000_000u64));
    }

    #[tokio::test]
    async fn test_mock_contract_handler() {
        let handler = MockGasKillerHandler;

        let supports = handler
            .supports_interface(
                Address::from([0x11; 20]),
                FixedBytes::from([0x01, 0x02, 0x03, 0x04]),
            )
            .await
            .unwrap();
        assert!(supports);

        let valid = handler
            .verify_aggregated_signature(
                Address::from([0x11; 20]),
                FixedBytes::from([0xaa; 32]),
                Bytes::from(vec![0x01, 0x02]),
                vec![Address::from([0x22; 20])],
            )
            .await
            .unwrap();
        assert!(valid);
    }

    #[test]
    fn test_execution_result_creation() {
        let result = GasKillerExecutionResult {
            task_id: "test_001".to_string(),
            tx_hash: FixedBytes::from([0xab; 32]),
            block_number: 12345678,
            gas_used: U256::from(100_000),
            gas_saved: U256::from(20_000),
            status: ExecutionStatus::Confirmed,
            error_message: None,
        };

        assert_eq!(result.task_id, "test_001");
        assert_eq!(result.block_number, 12345678);
        assert_eq!(result.gas_used, U256::from(100_000));
        assert_eq!(result.gas_saved, U256::from(20_000));
        assert_eq!(result.status, ExecutionStatus::Confirmed);
        assert!(result.error_message.is_none());
    }

    #[test]
    fn test_failed_execution_result() {
        let result = GasKillerExecutionResult {
            task_id: "test_002".to_string(),
            tx_hash: FixedBytes::from([0xcd; 32]),
            block_number: 0,
            gas_used: U256::ZERO,
            gas_saved: U256::ZERO,
            status: ExecutionStatus::Failed,
            error_message: Some("Transaction reverted".to_string()),
        };

        assert_eq!(result.status, ExecutionStatus::Failed);
        assert_eq!(
            result.error_message,
            Some("Transaction reverted".to_string())
        );
        assert_eq!(result.gas_used, U256::ZERO);
    }
}

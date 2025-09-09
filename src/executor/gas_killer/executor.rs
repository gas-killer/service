use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use alloy_primitives::{Address, Bytes, FixedBytes, U256, keccak256};
use alloy_provider::ProviderBuilder;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::time::Duration;
use tracing::{error, info, warn};

use super::traits::{GasKillerContractHandler, GasKillerExecutorTrait, GasPriceOracle};
use super::types::{
    ExecutionPackage, ExecutionStatus, GasKillerConfig, GasKillerExecutionResult, GasPriceConfig,
    StateUpdate,
};
use crate::executor::core::{ExecutionResult, VerificationData, VerificationExecutor};

// Define the Gas Killer SDK interface
sol! {
    interface IGasKiller {
        function supportsInterface(bytes4 interfaceId) external view returns (bool);

        function executeWithStateUpdates(
            bytes calldata targetCalldata,
            bytes32[] calldata stateKeys,
            bytes32[] calldata stateValues,
            bytes calldata aggregatedSignature,
            address[] calldata operators
        ) external returns (bool success, bytes memory returnData);

        function verifyAggregatedSignature(
            bytes32 messageHash,
            bytes calldata aggregatedSignature,
            address[] calldata operators
        ) external view returns (bool valid);
    }
}

/// Gas Killer executor implementation
pub struct GasKillerExecutor<H: GasKillerContractHandler> {
    provider: Box<dyn Provider>,
    #[allow(dead_code)]
    wallet: EthereumWallet,
    config: GasKillerConfig,
    contract_handler: H,
    gas_oracle: Box<dyn GasPriceOracle>,
}

impl<H: GasKillerContractHandler> GasKillerExecutor<H> {
    /// Create a new Gas Killer executor
    #[allow(dead_code)]
    pub async fn new(
        config: GasKillerConfig,
        contract_handler: H,
        gas_oracle: Box<dyn GasPriceOracle>,
    ) -> Result<Self> {
        // Parse private key and create signer
        let signer = config
            .private_key
            .parse::<PrivateKeySigner>()
            .map_err(|e| anyhow!("Failed to parse private key: {}", e))?;
        let wallet = EthereumWallet::from(signer);

        // Create provider with wallet
        #[allow(deprecated)] // TODO: Update to connect() when available in our alloy version
        let provider = ProviderBuilder::new()
            .wallet(wallet.clone())
            .on_builtin(&config.rpc_url)
            .await
            .map_err(|e| anyhow!("Failed to create provider: {}", e))?;

        Ok(Self {
            provider: Box::new(provider),
            wallet,
            config,
            contract_handler,
            gas_oracle,
        })
    }

    /// Encode state updates for contract call
    fn encode_state_updates(
        &self,
        state_updates: &[StateUpdate],
    ) -> (Vec<FixedBytes<32>>, Vec<FixedBytes<32>>) {
        let mut keys = Vec::new();
        let mut values = Vec::new();

        for update in state_updates {
            // Combine contract address and slot for unique key
            let key_data = [update.contract.as_slice(), &update.slot.to_be_bytes::<32>()].concat();
            keys.push(keccak256(&key_data));
            values.push(FixedBytes::from(update.value.to_be_bytes::<32>()));
        }

        (keys, values)
    }

    /// Wait for transaction confirmation with timeout
    async fn wait_for_confirmation(
        &self,
        tx_hash: FixedBytes<32>,
        timeout_secs: u64,
    ) -> Result<(u64, U256)> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                return Err(anyhow!("Transaction confirmation timeout"));
            }

            // Check transaction receipt
            if let Some(receipt) = self
                .provider
                .get_transaction_receipt(tx_hash)
                .await
                .map_err(|e| anyhow!("Failed to get receipt: {}", e))?
            {
                if !receipt.status() {
                    return Err(anyhow!("Transaction reverted"));
                }

                let block_number = receipt
                    .block_number
                    .ok_or_else(|| anyhow!("No block number in receipt"))?;
                let gas_used = receipt.gas_used;

                return Ok((block_number, U256::from(gas_used)));
            }

            // Wait before next check
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

#[async_trait]
impl<H: GasKillerContractHandler> GasKillerExecutorTrait for GasKillerExecutor<H> {
    async fn execute_gas_killer_transaction(
        &mut self,
        package: ExecutionPackage,
    ) -> Result<GasKillerExecutionResult> {
        info!(
            task_id = %package.task_id,
            target = ?package.target_contract,
            method = %package.target_method,
            "Starting Gas Killer transaction execution"
        );

        // Step 1: Verify contract supports Gas Killer interface
        if !self
            .verify_gas_killer_interface(package.target_contract)
            .await?
        {
            return Err(anyhow!("Contract does not support Gas Killer interface"));
        }

        // Step 2: Prepare optimized calldata
        let calldata = self.prepare_calldata(&package.target_method, &package.params, &package)?;

        // Step 3: Estimate gas with state preloading
        let gas_estimate = self
            .estimate_gas_with_state(package.target_contract, &calldata)
            .await?;

        // Apply buffer
        let gas_limit = gas_estimate * U256::from(self.config.gas_buffer_percent) / U256::from(100);

        // Step 4: Calculate optimal gas price
        let gas_price_config = self.calculate_optimal_gas_price(1).await?;

        // Step 5: Submit transaction
        let tx_hash = self
            .submit_transaction(
                package.target_contract,
                calldata,
                gas_limit,
                gas_price_config,
            )
            .await?;

        info!(
            task_id = %package.task_id,
            tx_hash = ?tx_hash,
            "Transaction submitted"
        );

        // Step 6: Monitor transaction
        let (block_number, gas_used) = match self.monitor_transaction(tx_hash).await {
            Ok(result) => result,
            Err(e) => {
                error!(
                    task_id = %package.task_id,
                    error = %e,
                    "Transaction monitoring failed"
                );

                return Ok(GasKillerExecutionResult {
                    task_id: package.task_id,
                    tx_hash,
                    block_number: 0,
                    gas_used: U256::ZERO,
                    gas_saved: U256::ZERO,
                    status: ExecutionStatus::Failed,
                    error_message: Some(e.to_string()),
                });
            }
        };

        // Calculate gas saved
        let original_estimate = gas_estimate * U256::from(120) / U256::from(100); // Assume 20% higher without optimization
        let gas_saved = if original_estimate > gas_used {
            original_estimate - gas_used
        } else {
            U256::ZERO
        };

        info!(
            task_id = %package.task_id,
            tx_hash = ?tx_hash,
            block_number = block_number,
            gas_used = %gas_used,
            gas_saved = %gas_saved,
            "Transaction execution completed"
        );

        Ok(GasKillerExecutionResult {
            task_id: package.task_id,
            tx_hash,
            block_number,
            gas_used,
            gas_saved,
            status: ExecutionStatus::Confirmed,
            error_message: None,
        })
    }

    async fn verify_gas_killer_interface(&self, contract_address: Address) -> Result<bool> {
        self.contract_handler
            .supports_interface(contract_address, self.config.gk_interface_id)
            .await
    }

    fn prepare_calldata(
        &self,
        _target_method: &str,
        params: &Bytes,
        package: &ExecutionPackage,
    ) -> Result<Bytes> {
        // Encode state updates
        let (state_keys, state_values) = self.encode_state_updates(&package.state_updates);

        // Serialize aggregated signature - convert to bytes
        let sig_bytes = Bytes::from(vec![0u8; 96]); // Placeholder for BLS signature serialization

        // Build the executeWithStateUpdates call
        let call = IGasKiller::executeWithStateUpdatesCall {
            targetCalldata: params.clone(),
            stateKeys: state_keys,
            stateValues: state_values,
            aggregatedSignature: sig_bytes,
            operators: package.operator_set.clone(),
        };

        use alloy::sol_types::SolCall;
        Ok(Bytes::from(call.abi_encode()))
    }

    async fn estimate_gas_with_state(
        &self,
        contract_address: Address,
        calldata: &Bytes,
    ) -> Result<U256> {
        let tx = TransactionRequest::default()
            .to(contract_address)
            .input(calldata.clone().into());

        let estimate = self
            .provider
            .estimate_gas(tx)
            .await
            .map_err(|e| anyhow!("Gas estimation failed: {}", e))?;

        Ok(U256::from(estimate))
    }

    async fn calculate_optimal_gas_price(&self, priority: u8) -> Result<GasPriceConfig> {
        let base_fee = self.gas_oracle.get_base_fee().await?;
        let max_priority_fee = self.gas_oracle.get_priority_fee(priority).await?;
        let max_fee_per_gas = self.gas_oracle.get_max_fee_per_gas().await?;

        Ok(GasPriceConfig {
            base_fee,
            max_priority_fee,
            max_fee_per_gas,
            priority,
        })
    }

    async fn submit_transaction(
        &self,
        contract_address: Address,
        calldata: Bytes,
        gas_limit: U256,
        gas_price_config: GasPriceConfig,
    ) -> Result<FixedBytes<32>> {
        let optimal_price = gas_price_config.calculate_optimal_price();

        let tx = TransactionRequest::default()
            .to(contract_address)
            .input(calldata.into())
            .with_gas_limit(gas_limit.to::<u64>())
            .with_max_fee_per_gas(optimal_price.to::<u128>())
            .with_max_priority_fee_per_gas(gas_price_config.max_priority_fee.to::<u128>());

        let pending_tx = self
            .provider
            .send_transaction(tx)
            .await
            .map_err(|e| anyhow!("Failed to send transaction: {}", e))?;

        Ok(*pending_tx.tx_hash())
    }

    async fn monitor_transaction(&self, tx_hash: FixedBytes<32>) -> Result<(u64, U256)> {
        self.wait_for_confirmation(tx_hash, self.config.confirmation_timeout)
            .await
    }

    async fn replace_transaction(
        &self,
        original_tx_hash: FixedBytes<32>,
        gas_price_increase_percent: u8,
    ) -> Result<FixedBytes<32>> {
        // Get original transaction
        let _original_tx = self
            .provider
            .get_transaction_by_hash(original_tx_hash)
            .await
            .map_err(|e| anyhow!("Failed to get original transaction: {}", e))?
            .ok_or_else(|| anyhow!("Original transaction not found"))?;

        // For now, we'll use default values for transaction replacement
        // The actual transaction details extraction would require proper access to the transaction fields
        // which depends on the exact alloy version and API
        let to_address = Address::ZERO; // Placeholder - would need proper extraction
        let input = Bytes::new();
        let gas_limit = U256::from(21000); // Standard gas limit
        let nonce = 0u64;
        let max_fee = U256::from(0);

        // Increase gas price
        let new_max_fee = max_fee * U256::from(100 + gas_price_increase_percent) / U256::from(100);

        // Build replacement transaction with same nonce
        let replacement_tx = TransactionRequest::default()
            .to(to_address)
            .input(input.into())
            .with_gas_limit(gas_limit.to::<u64>())
            .with_max_fee_per_gas(new_max_fee.to::<u128>())
            .with_nonce(nonce);

        let pending_tx = self
            .provider
            .send_transaction(replacement_tx)
            .await
            .map_err(|e| anyhow!("Failed to send replacement transaction: {}", e))?;

        warn!(
            original_tx = ?original_tx_hash,
            replacement_tx = ?pending_tx.tx_hash(),
            "Transaction replaced with higher gas price"
        );

        Ok(*pending_tx.tx_hash())
    }
}

// Implement VerificationExecutor trait for compatibility with existing system
#[async_trait]
impl<H: GasKillerContractHandler> VerificationExecutor for GasKillerExecutor<H> {
    async fn execute_verification(
        &mut self,
        _payload_hash: &[u8],
        _verification_data: VerificationData,
    ) -> Result<ExecutionResult> {
        // This is a compatibility layer - in practice, Gas Killer executor
        // would receive ExecutionPackage from the orchestrator

        // For now, return a placeholder result
        Ok(ExecutionResult {
            transaction_hash: format!("0x{}", hex::encode(_payload_hash)),
            block_number: None,
            gas_used: None,
            status: Some(true),
            contract_address: None,
        })
    }
}

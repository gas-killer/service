use alloy::hex;
use alloy::node_bindings::{Anvil, AnvilInstance};
use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::{TransactionRequest, BlockNumberOrTag};
use alloy::sol_types::SolValue;
use alloy_provider::{
    RootProvider,
    fillers::{BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller},
};
use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use commonware_cryptography::sha256::Digest;
use commonware_cryptography::{Hasher, Sha256};

type AnvilProvider = FillProvider<
    JoinFill<
        alloy_provider::Identity,
        JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
    >,
    RootProvider,
>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRequest {
    pub target_address: String,
    pub function_selector: String,
    pub function_params: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub success: bool,
    pub return_data: Vec<u8>,
    pub gas_used: Option<u64>,
    pub payload_hash: Digest,
}

pub struct ExecutionValidator {
    anvil: Option<AnvilInstance>,
    provider: Option<AnvilProvider>,
}

impl ExecutionValidator {
    pub fn new() -> Self {
        Self {
            anvil: None,
            provider: None,
        }
    }

    pub async fn initialize_anvil(&mut self) -> Result<()> {
        let anvil = Anvil::new()
            .fork("https://eth.llamarpc.com")
            .fork_block_number(21000000)
            .spawn();

        let endpoint = anvil.endpoint();
        let provider = ProviderBuilder::new()
            .on_http(url::Url::parse(&endpoint)?);

        self.anvil = Some(anvil);
        self.provider = Some(provider);

        Ok(())
    }

    pub async fn validate_execution(
        &mut self,
        request: ExecutionRequest,
    ) -> Result<ExecutionResult> {
        if self.provider.is_none() {
            self.initialize_anvil().await?;
        }

        let provider = self.provider.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Provider not initialized"))?;

        let target_address = Address::from_str(&request.target_address)
            .context("Invalid target address")?;

        let selector_bytes = hex::decode(&request.function_selector)
            .context("Invalid function selector hex")?;
        
        if selector_bytes.len() != 4 {
            return Err(anyhow::anyhow!("Function selector must be 4 bytes"));
        }

        let mut calldata = Vec::with_capacity(4 + request.function_params.len());
        calldata.extend_from_slice(&selector_bytes);
        calldata.extend_from_slice(&request.function_params);

        let tx_request = TransactionRequest::default()
            .to(target_address)
            .input(calldata.clone().into());

        let result = provider
            .call(&tx_request)
            .block(BlockNumberOrTag::Latest)
            .await;

        let (success, return_data, gas_used) = match result {
            Ok(data) => {
                let gas = provider
                    .estimate_gas(&tx_request)
                    .await
                    .ok()
                    .map(|g| g as u64);
                
                (true, data.to_vec(), gas)
            }
            Err(e) => {
                let error_data = e.to_string().as_bytes().to_vec();
                (false, error_data, None)
            }
        };

        let payload_hash = self.compute_payload_hash(&calldata, &return_data)?;

        Ok(ExecutionResult {
            success,
            return_data,
            gas_used,
            payload_hash,
        })
    }

    pub async fn simulate_transaction(
        &mut self,
        target_address: String,
        function_selector: String,
        function_params: Vec<u8>,
    ) -> Result<ExecutionResult> {
        let request = ExecutionRequest {
            target_address,
            function_selector,
            function_params,
        };

        self.validate_execution(request).await
    }

    pub async fn validate_and_return_expected_hash(
        &mut self,
        msg: &[u8],
    ) -> Result<Digest> {
        let request: ExecutionRequest = serde_json::from_slice(msg)
            .context("Failed to deserialize execution request")?;

        let result = self.validate_execution(request).await?;
        
        if !result.success {
            return Err(anyhow::anyhow!(
                "Transaction simulation failed: {:?}",
                String::from_utf8_lossy(&result.return_data)
            ));
        }

        Ok(result.payload_hash)
    }

    fn compute_payload_hash(&self, calldata: &[u8], return_data: &[u8]) -> Result<Digest> {
        let mut hasher = Sha256::new();
        hasher.update(calldata);
        hasher.update(return_data);
        let hash = hasher.finalize();
        Ok(hash)
    }

    pub async fn verify_contract_state(
        &mut self,
        contract_address: String,
        storage_slot: U256,
    ) -> Result<U256> {
        if self.provider.is_none() {
            self.initialize_anvil().await?;
        }

        let provider = self.provider.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Provider not initialized"))?;

        let address = Address::from_str(&contract_address)
            .context("Invalid contract address")?;

        let value = provider
            .get_storage_at(address, storage_slot)
            .await
            .context("Failed to get storage value")?;

        Ok(value)
    }

    pub fn shutdown(&mut self) {
        self.provider = None;
        self.anvil = None;
    }
}

impl Drop for ExecutionValidator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initialize_anvil() {
        let mut validator = ExecutionValidator::new();
        let result = validator.initialize_anvil().await;
        assert!(result.is_ok());
        validator.shutdown();
    }

    #[tokio::test]
    async fn test_simulate_transaction() {
        let mut validator = ExecutionValidator::new();
        
        let target_address = "0x0000000000000000000000000000000000000000".to_string();
        let function_selector = "70a08231".to_string();
        let function_params = vec![0u8; 32];
        
        let result = validator
            .simulate_transaction(target_address, function_selector, function_params)
            .await;
        
        assert!(result.is_ok());
        validator.shutdown();
    }
}
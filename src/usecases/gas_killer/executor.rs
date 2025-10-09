use crate::bindings::WalletProvider;
use crate::bindings::blssigcheckoperatorstateretriever::BLSSigCheckOperatorStateRetriever::getNonSignerStakesAndSignatureReturn;
use crate::bindings::gaskillersdk::{BN254, GasKillerSDK, IBLSSignatureCheckerTypes};
use crate::executor::bls::{BlsSignatureVerificationHandler, convert_non_signer_data};
use crate::executor::core::ExecutionResult;
use crate::usecases::gas_killer::task_data::GasKillerTaskData;
use alloy_primitives::{Bytes, FixedBytes, U256};
use anyhow::Result;
use async_trait::async_trait;

/// Handler for executing verifyAndUpdate transactions
#[allow(dead_code)]
pub struct GasKillerHandler {
    provider: WalletProvider,
}

#[allow(dead_code)]
impl GasKillerHandler {
    pub fn new(provider: WalletProvider) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl BlsSignatureVerificationHandler for GasKillerHandler {
    type TaskData = GasKillerTaskData;
    async fn handle_verification(
        &mut self,
        msg_hash: FixedBytes<32>,
        quorum_numbers: Bytes,
        current_block_number: u32,
        non_signer_data: getNonSignerStakesAndSignatureReturn,
        task_data: Option<&Self::TaskData>,
    ) -> Result<ExecutionResult> {
        // Convert the non-signer data to the format expected by the GasKillerSDK
        let converted_data = convert_non_signer_data(non_signer_data);
        let non_signer_struct_data = IBLSSignatureCheckerTypes::NonSignerStakesAndSignature {
            nonSignerQuorumBitmapIndices: converted_data.nonSignerQuorumBitmapIndices,
            nonSignerPubkeys: converted_data
                .nonSignerPubkeys
                .into_iter()
                .map(|p| BN254::G1Point { X: p.X, Y: p.Y })
                .collect(),
            quorumApks: converted_data
                .quorumApks
                .into_iter()
                .map(|p| BN254::G1Point { X: p.X, Y: p.Y })
                .collect(),
            apkG2: BN254::G2Point {
                X: converted_data.apkG2.X,
                Y: converted_data.apkG2.Y,
            },
            sigma: BN254::G1Point {
                X: converted_data.sigma.X,
                Y: converted_data.sigma.Y,
            },
            quorumApkIndices: converted_data.quorumApkIndices,
            totalStakeIndices: converted_data.totalStakeIndices,
            nonSignerStakeIndices: converted_data.nonSignerStakeIndices,
        };

        // Validate that task data is provided
        let task_data = task_data
            .ok_or_else(|| anyhow::anyhow!("Task data is required for gas killer verification"))?;

        // Extract task data parameters
        let storage_updates = Bytes::from(task_data.storage_updates.clone());
        let transition_index = U256::from(task_data.transition_index);
        let target_function = task_data.function_selector();
        let target_addr = task_data.target_address;

        // Create GasKillerSDK instance dynamically using target_address from task data
        let gas_killer_sdk = GasKillerSDK::new(target_addr, self.provider.clone());

        // Ensure contract implements the GasKiller interface via ERC-165 check
        let interface_id = FixedBytes::<4>::from([0x93, 0xde, 0x45, 0x31]);
        let supports = gas_killer_sdk
            .supportsInterface(interface_id)
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("supportsInterface call failed: {}", e))?;
        if !supports._0 {
            return Err(anyhow::anyhow!(
                "Target contract does not support GasKiller interface (0x93de4531)"
            ));
        }

        // Execute the gas killer verifyAndUpdate
        let call_return = gas_killer_sdk
            .verifyAndUpdate(
                msg_hash,
                quorum_numbers,
                current_block_number,
                storage_updates,
                transition_index,
                target_function,
                non_signer_struct_data,
            )
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send verifyAndUpdate transaction: {}", e))?;

        let receipt = call_return
            .get_receipt()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get transaction receipt: {}", e))?;

        Ok(ExecutionResult {
            transaction_hash: format!("{:?}", receipt.transaction_hash),
            block_number: receipt.block_number,
            gas_used: Some(receipt.gas_used),
            status: Some(receipt.status()),
            contract_address: receipt.contract_address.map(|addr| format!("{:?}", addr)),
        })
    }
}

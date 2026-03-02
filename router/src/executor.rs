use gas_killer_common::WalletProvider;
use gas_killer_common::bindings::gaskillersdk::{BN254, GasKillerSDK, IBLSSignatureCheckerTypes as GasKillerIBLSTypes};
use gas_killer_common::ChainId;
use commonware_avs_router::bindings::blssigcheckoperatorstateretriever::BLSSigCheckOperatorStateRetriever::getNonSignerStakesAndSignatureReturn;
use commonware_avs_router::executor::bls::BlsSignatureVerificationHandler;
use commonware_avs_router::executor::ExecutionResult;
use crate::task_data::GasKillerTaskData;
use alloy_primitives::{Address, Bytes, FixedBytes, U256};
use alloy_provider::Provider;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Handler for executing verifyAndUpdate transactions with multi-chain support
pub struct GasKillerHandler {
    /// Wallet providers keyed by chain ID
    providers: HashMap<ChainId, WalletProvider>,
}

impl GasKillerHandler {
    /// Creates a new handler with a single provider (for backwards compatibility)
    pub fn new(provider: WalletProvider) -> Self {
        let mut providers = HashMap::new();
        providers.insert(ChainId::Sepolia, provider);
        Self { providers }
    }

    /// Creates a new handler with providers for multiple chains
    pub fn with_providers(providers: HashMap<ChainId, WalletProvider>) -> Self {
        Self { providers }
    }

    /// Adds a provider for a specific chain
    pub fn add_provider(&mut self, chain_id: ChainId, provider: WalletProvider) {
        self.providers.insert(chain_id, provider);
    }

    /// Gets the provider for a specific chain
    fn get_provider(&self, chain_id: ChainId) -> Option<&WalletProvider> {
        self.providers.get(&chain_id)
    }

    /// Detects which chain has code deployed at the given address
    async fn detect_chain_for_address(&self, address: Address) -> Result<ChainId> {
        debug!(
            address = %address,
            "Detecting chain for address in executor"
        );

        // Check Sepolia first (primary chain), then Gnosis
        for chain_id in [ChainId::Sepolia, ChainId::Gnosis] {
            if let Some(provider) = self.get_provider(chain_id) {
                match provider.get_code_at(address).await {
                    Ok(code) => {
                        if !code.is_empty() {
                            debug!(
                                chain = %chain_id,
                                address = %address,
                                code_len = code.len(),
                                "Found contract code on chain"
                            );
                            return Ok(chain_id);
                        }
                    }
                    Err(e) => {
                        debug!(
                            chain = %chain_id,
                            error = %e,
                            "Failed to check code on chain"
                        );
                    }
                }
            }
        }

        Err(anyhow::anyhow!(
            "No contract code found at address {} on any supported chain",
            address
        ))
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
        // Unwrap the return type to get the actual data
        let data = non_signer_data._0;

        // Convert the non-signer data to the format expected by the GasKillerSDK
        let non_signer_struct_data = GasKillerIBLSTypes::NonSignerStakesAndSignature {
            nonSignerQuorumBitmapIndices: data.nonSignerQuorumBitmapIndices,
            nonSignerPubkeys: data
                .nonSignerPubkeys
                .into_iter()
                .map(|p| BN254::G1Point { X: p.X, Y: p.Y })
                .collect(),
            quorumApks: data
                .quorumApks
                .into_iter()
                .map(|p| BN254::G1Point { X: p.X, Y: p.Y })
                .collect(),
            apkG2: BN254::G2Point {
                X: data.apkG2.X,
                Y: data.apkG2.Y,
            },
            sigma: BN254::G1Point {
                X: data.sigma.X,
                Y: data.sigma.Y,
            },
            quorumApkIndices: data.quorumApkIndices,
            totalStakeIndices: data.totalStakeIndices,
            nonSignerStakeIndices: data.nonSignerStakeIndices,
        };

        // Validate that task data is provided
        let task_data = task_data
            .ok_or_else(|| anyhow::anyhow!("Task data is required for gas killer verification"))?;

        // Detect which chain the contract is on
        let chain_id = self
            .detect_chain_for_address(task_data.target_address)
            .await?;

        // Get the chain-specific provider
        let provider = self
            .get_provider(chain_id)
            .ok_or_else(|| anyhow::anyhow!("No provider configured for chain: {}", chain_id))?;

        info!(
            storage_updates_len = task_data.storage_updates.len(),
            chain = %chain_id,
            "Using storage updates from task data on detected chain"
        );

        // Extract task data parameters - use pre-computed storage_updates from task data
        let storage_updates = Bytes::from(task_data.storage_updates.clone());
        let transition_index = U256::from(task_data.transition_index);
        let target_function = task_data.function_selector();
        let target_addr = task_data.target_address;

        // Debug: Log exact inputs for hash comparison
        debug!(
            transition_index = %transition_index,
            target_address = %target_addr,
            target_function = %target_function,
            storage_updates_len = storage_updates.len(),
            storage_updates_first_32 = %hex::encode(&task_data.storage_updates[..std::cmp::min(32, task_data.storage_updates.len())]),
            detected_chain = %chain_id,
            "Executor getMessageHash inputs"
        );

        let gas_killer_sdk = GasKillerSDK::new(target_addr, provider.clone());
        // Query the contract's getMessageHash and compare with the provided msg_hash
        match gas_killer_sdk
            .getMessageHash(transition_index, target_function, storage_updates.clone())
            .call()
            .await
        {
            Ok(expected_hash) => {
                if expected_hash != msg_hash {
                    warn!(
                        offchain_msg_hash = %msg_hash,
                        onchain_expected_hash = %expected_hash,
                        transition_index = %transition_index,
                        target_address = %target_addr,
                        target_function = %target_function,
                        storage_updates_len = storage_updates.len(),
                        "Message hash mismatch between offchain and onchain"
                    );
                    return Err(anyhow::anyhow!(
                        "Message hash mismatch: offchain {} != onchain {}",
                        msg_hash,
                        expected_hash
                    ));
                } else {
                    info!("Message hash match confirmed");
                }
            }
            Err(e) => {
                warn!("getMessageHash call failed: {}", e);
            }
        }

        // Ensure contract implements the GasKiller interface via ERC-165 check
        let interface_id = FixedBytes::<4>::from([0x93, 0xde, 0x45, 0x31]);
        match gas_killer_sdk.supportsInterface(interface_id).call().await {
            Ok(supported) => {
                if !supported {
                    warn!("Target contract does not support GasKiller interface (0x93de4531)");
                    return Err(anyhow::anyhow!(
                        "Target contract does not support GasKiller interface (0x93de4531)"
                    ));
                }
            }
            Err(e) => {
                warn!("supportsInterface call failed: {}", e);
                return Err(anyhow::anyhow!("supportsInterface call failed: {}", e));
            }
        };

        // Execute the gas killer verifyAndUpdate
        info!("Sending verifyAndUpdate transaction");
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
        info!(
            tx = %receipt.transaction_hash,
            block = receipt.block_number,
            status = ?receipt.status(),
            gas_used = ?receipt.gas_used,
            "verifyAndUpdate receipt"
        );

        Ok(ExecutionResult {
            transaction_hash: format!("{:?}", receipt.transaction_hash),
            block_number: receipt.block_number,
            gas_used: Some(receipt.gas_used),
            status: Some(receipt.status()),
            contract_address: receipt.contract_address.map(|addr| format!("{:?}", addr)),
        })
    }
}

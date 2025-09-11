use crate::bindings::blsapkregistry::BLSApkRegistry;
use crate::bindings::blssigcheckoperatorstateretriever::BLSSigCheckOperatorStateRetriever;
use crate::executor::bls::BlsEigenlayerExecutor;
use crate::usecases::gas_killer::{GasKillerHandler, GasKillerTaskData};
use alloy_provider::ProviderBuilder;
use alloy_signer_local::PrivateKeySigner;
use anyhow::Result;
use commonware_eigenlayer::config::AvsDeployment;
use std::{env, str::FromStr};

/// Creates a new BlsEigenlayerExecutor configured for Gas Killer operations
#[allow(dead_code)]
pub async fn create_gas_killer_executor()
-> Result<BlsEigenlayerExecutor<GasKillerHandler, GasKillerTaskData>> {
    let http_rpc = env::var("HTTP_RPC").expect("HTTP_RPC must be set");
    let view_only_provider = ProviderBuilder::new().on_http(url::Url::parse(&http_rpc).unwrap());

    let deployment =
        AvsDeployment::load().map_err(|e| anyhow::anyhow!("Failed to load deployment: {}", e))?;

    let bls_apk_registry_address = deployment
        .bls_apk_registry_address()
        .map_err(|e| anyhow::anyhow!("Failed to get BLS APK registry address: {}", e))?;
    let registry_coordinator_address = deployment
        .registry_coordinator_address()
        .map_err(|e| anyhow::anyhow!("Failed to get registry coordinator address: {}", e))?;

    let ecdsa_signer =
        PrivateKeySigner::from_str(&env::var("PRIVATE_KEY").expect("PRIVATE_KEY must be set"))
            .map_err(|e| anyhow::anyhow!("Failed to parse private key: {}", e))?;
    let bls_operator_state_retriever_address = deployment
        .bls_sig_check_operator_state_retriever_address()
        .map_err(|e| {
            anyhow::anyhow!("Failed to get BLS operator state retriever address: {}", e)
        })?;

    let write_provider = ProviderBuilder::new()
        .wallet(ecdsa_signer)
        .connect(&http_rpc)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect write provider: {}", e))?;

    let bls_apk_registry =
        BLSApkRegistry::new(bls_apk_registry_address, view_only_provider.clone());
    let bls_operator_state_retriever = BLSSigCheckOperatorStateRetriever::new(
        bls_operator_state_retriever_address,
        view_only_provider.clone(),
    );

    let gas_killer_handler = GasKillerHandler::new(write_provider);

    Ok(BlsEigenlayerExecutor::new(
        view_only_provider,
        bls_apk_registry,
        bls_operator_state_retriever,
        registry_coordinator_address,
        gas_killer_handler,
    ))
}

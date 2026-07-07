//! On-chain reads for the pre-committed-nonce mode: operator key points from the
//! `SchnorrStakeRegistry` and batch coverage from the `SchnorrNonceRegistry` — the
//! "nodes read the registries like they read stakes" piece of the plan (§5, §7.2).
//!
//! These run once at startup (single static epoch, like the rest of the stack); an
//! epoch-rotating deployment would re-run them per epoch via the engine's provider.

use super::PublicKey as SchnorrPublicKey;
use alloy::providers::Provider;
use alloy::sol;
use alloy_primitives::Address;

sol! {
    #[sol(rpc)]
    interface ISchnorrStakeRegistryView {
        function operators(address operatorId)
            external
            view
            returns (uint256 x, uint256 y, uint256 weight, bool registered);
    }

    #[sol(rpc)]
    interface ISchnorrNonceRegistryView {
        function batchCount(address operatorId) external view returns (uint256);
        function batches(address operatorId, uint256 index)
            external
            view
            returns (bytes32 root, uint64 startSlot, uint64 count);
        function coverage(address operatorId) external view returns (uint64);
    }
}

/// One registered batch's on-chain metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchMeta {
    pub batch_index: u64,
    pub root: [u8; 32],
    pub start_slot: u64,
    pub count: u64,
}

/// Loads the Schnorr key point for every operator identity address from the stake
/// registry, verifying each point hashes back to its identity. Errors on unregistered
/// operators — the caller should treat that as "register on-chain before starting".
pub async fn load_operator_keys<P: Provider>(
    provider: P,
    stake_registry: Address,
    operators: &[Address],
) -> Result<Vec<SchnorrPublicKey>, String> {
    let registry = ISchnorrStakeRegistryView::new(stake_registry, provider);
    let mut keys = Vec::with_capacity(operators.len());
    for operator in operators {
        let record = registry
            .operators(*operator)
            .call()
            .await
            .map_err(|e| format!("stake registry read failed for {operator}: {e}"))?;
        if !record.registered {
            return Err(format!(
                "operator {operator} is not registered in the SchnorrStakeRegistry"
            ));
        }
        let key = SchnorrPublicKey::from_coordinates(
            &record.x.to_be_bytes::<32>(),
            &record.y.to_be_bytes::<32>(),
        )
        .ok_or_else(|| format!("registered key for {operator} is not on the curve"))?;
        if key.eth_address() != *operator {
            return Err(format!(
                "registered key for {operator} hashes to {} (registry corruption?)",
                key.eth_address()
            ));
        }
        keys.push(key);
    }
    Ok(keys)
}

/// Loads every registered batch's metadata for one operator from the nonce registry.
pub async fn load_batches<P: Provider>(
    provider: P,
    nonce_registry: Address,
    operator: Address,
) -> Result<Vec<BatchMeta>, String> {
    let registry = ISchnorrNonceRegistryView::new(nonce_registry, provider);
    let count = registry
        .batchCount(operator)
        .call()
        .await
        .map_err(|e| format!("nonce registry batchCount failed for {operator}: {e}"))?;
    let count = u64::try_from(count).map_err(|_| "absurd batch count".to_string())?;
    let mut batches = Vec::with_capacity(count as usize);
    for index in 0..count {
        let batch = registry
            .batches(operator, alloy_primitives::U256::from(index))
            .call()
            .await
            .map_err(|e| format!("nonce registry batches({operator}, {index}) failed: {e}"))?;
        batches.push(BatchMeta {
            batch_index: index,
            root: batch.root.0,
            start_slot: batch.startSlot,
            count: batch.count,
        });
    }
    Ok(batches)
}

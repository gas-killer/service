//! Pins the sol!-generated selectors to the deployed solidity-sdk ABI.
use alloy::sol_types::SolCall;
use gas_killer_common::shard::{
    argmaxRangeCall, forwardRangeCall, fulfilCall, fulfilResumedCall, seg35, settledRootsCall,
};

#[test]
fn selectors_match_solidity_sdk() {
    assert_eq!(hex::encode(forwardRangeCall::SELECTOR), "568f9e26");
    assert_eq!(hex::encode(argmaxRangeCall::SELECTOR), "cfa1c545");
    assert_eq!(hex::encode(fulfilCall::SELECTOR), "9c98c06e");
}

/// Prefix-resume settlement ABI — `GasKillerChatSharded`/`GasKillerChat35Sharded`
/// consumer commit 2a2071c. Identical signature/selector on both families.
#[test]
fn resume_selectors_match_solidity_sdk() {
    assert_eq!(hex::encode(fulfilResumedCall::SELECTOR), "6c4d43bc");
    assert_eq!(hex::encode(settledRootsCall::SELECTOR), "56408a4f");
}

/// The engine-v3 (Qwen3.5-35B-A3B) segment ABI — `Qwen35SegEngine` commit
/// 916ad17. `packedConfig` is `bytes32[4]` and the `Call` carries `stateIn`
/// (unified full-attention KV + DeltaNet snapshots) instead of `kvIn`, so the
/// selectors differ from the 0.6B family above.
#[test]
fn qwen35_selectors_match_solidity_sdk() {
    assert_eq!(hex::encode(seg35::forwardRangeCall::SELECTOR), "4faab046");
    assert_eq!(hex::encode(seg35::argmaxRangeCall::SELECTOR), "18d6ba7d");
}

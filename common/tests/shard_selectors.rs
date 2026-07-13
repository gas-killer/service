//! Pins the sol!-generated selectors to the deployed solidity-sdk ABI.
use alloy::sol_types::SolCall;
use gas_killer_common::shard::{argmaxRangeCall, forwardRangeCall, fulfilCall};

#[test]
fn selectors_match_solidity_sdk() {
    assert_eq!(hex::encode(forwardRangeCall::SELECTOR), "568f9e26");
    assert_eq!(hex::encode(argmaxRangeCall::SELECTOR), "cfa1c545");
    assert_eq!(hex::encode(fulfilCall::SELECTOR), "9c98c06e");
}

#![allow(clippy::too_many_arguments)]
use alloy::sol;

// ArraySummation contract bindings generated at compile time from ABI
//
// IMPORTANT: You need to provide the ABI file at:
//   scripts/bindings/abis/ArraySummation.json
//
// This can be obtained from:
//   - Foundry: out/ArraySummation.sol/ArraySummation.json
//   - Hardhat: artifacts/contracts/ArraySummation.sol/ArraySummation.json
sol! {
    #[sol(rpc, ignore_unlinked)]
    ArraySummation,
    "bindings/abis/ArraySummation.json"
}

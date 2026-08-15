#![allow(clippy::too_many_arguments)]
use alloy::sol;

// ArraySummation contract bindings generated at compile time from the vendored ABI at
// scripts/src/bindings/abis/ArraySummation.json.
//
// To refresh it, copy the Foundry artifact for the contract:
//   out/ArraySummation.sol/ArraySummation.json
sol! {
    #[sol(rpc, ignore_unlinked)]
    ArraySummation,
    "src/bindings/abis/ArraySummation.json"
}

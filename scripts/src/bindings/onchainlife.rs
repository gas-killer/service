#![allow(clippy::too_many_arguments)]
use alloy::sol;

// OnchainLife contract bindings generated at compile time from the vendored ABI at
// scripts/src/bindings/abis/OnchainLife.json.
//
// To refresh it, copy the Foundry artifact for the contract:
//   out/OnchainLife.sol/OnchainLife.json
sol! {
    #[sol(rpc, ignore_unlinked)]
    OnchainLife,
    "src/bindings/abis/OnchainLife.json"
}

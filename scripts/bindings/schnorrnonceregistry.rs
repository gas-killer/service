#![allow(clippy::too_many_arguments)]
use alloy::sol;

// SchnorrNonceRegistry contract bindings generated at compile time from ABI
// (artifact includes bytecode so the deploy binary can `::deploy` it)
sol! {
    #[sol(rpc, ignore_unlinked)]
    SchnorrNonceRegistry,
    "bindings/abis/SchnorrNonceRegistry.json"
}

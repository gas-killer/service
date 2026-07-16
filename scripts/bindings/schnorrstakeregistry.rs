#![allow(clippy::too_many_arguments)]
use alloy::sol;

// SchnorrStakeRegistry contract bindings generated at compile time from ABI
// (artifact includes bytecode so the deploy binary can `::deploy` it)
sol! {
    #[sol(rpc, ignore_unlinked)]
    SchnorrStakeRegistry,
    "bindings/abis/SchnorrStakeRegistry.json"
}

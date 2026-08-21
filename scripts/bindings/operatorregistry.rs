#![allow(clippy::too_many_arguments)]
use alloy::sol;

// OperatorRegistry contract bindings generated at compile time from ABI
sol! {
    #[sol(rpc, ignore_unlinked)]
    OperatorRegistry,
    "bindings/abis/OperatorRegistry.json"
}

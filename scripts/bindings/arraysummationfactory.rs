#![allow(clippy::too_many_arguments)]
use alloy::sol;

// ArraySummationFactory contract bindings generated at compile time from ABI
sol! {
    #[sol(rpc, ignore_unlinked)]
    ArraySummationFactory,
    "bindings/abis/ArraySummationFactory.json"
}

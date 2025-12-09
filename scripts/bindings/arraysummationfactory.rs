#![allow(clippy::too_many_arguments)]
use alloy::sol;

// ArraySummation contract bindings generated at compile time from ABI
sol! {
    #[sol(rpc)]
    ArraySummationFactory,
    "bindings/abis/ArraySummationFactory.json"
}

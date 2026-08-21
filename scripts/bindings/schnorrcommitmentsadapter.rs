#![allow(clippy::too_many_arguments)]
use alloy::sol;

// SchnorrCommitmentsAdapter contract bindings generated at compile time from ABI
sol! {
    #[sol(rpc, ignore_unlinked)]
    SchnorrCommitmentsAdapter,
    "bindings/abis/SchnorrCommitmentsAdapter.json"
}

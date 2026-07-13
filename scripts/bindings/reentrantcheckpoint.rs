#![allow(clippy::too_many_arguments)]
use alloy::sol;

// ReentrantCheckpoint bindings generated at compile time from ABI.
// Used by send_request to build the `advance()` task call_data and poll `counter()`.
sol! {
    #[sol(rpc, ignore_unlinked)]
    ReentrantCheckpoint,
    "bindings/abis/ReentrantCheckpoint.json"
}

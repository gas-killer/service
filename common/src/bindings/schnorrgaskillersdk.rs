#![allow(clippy::too_many_arguments)]
use alloy::sol;

// SchnorrGasKillerSDK contract bindings generated at compile time from ABI
sol! {
    #[sol(rpc, ignore_unlinked)]
    SchnorrGasKillerSDK,
    "src/bindings/abis/SchnorrGasKillerSDK.json"
}

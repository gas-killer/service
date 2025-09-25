#![allow(clippy::too_many_arguments)]
use alloy::sol;

// GasKillerSDK contract bindings generated at compile time from ABI
sol! {
    #[sol(rpc)]
    GasKillerSDK,
    "src/bindings/abis/GasKillerSDK.json"
}

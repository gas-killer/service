#![allow(clippy::too_many_arguments)]
use alloy::sol;

// BLSApkRegistry contract bindings generated at compile time from ABI
sol! {
    #[sol(rpc)]
    BLSApkRegistry,
    "src/bindings/abis/BLSApkRegistry.json"
}

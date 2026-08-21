#![allow(clippy::too_many_arguments)]
use alloy::sol;

// MintableERC20 contract bindings generated at compile time from ABI
sol! {
    #[sol(rpc, ignore_unlinked)]
    MintableERC20,
    "bindings/abis/MintableERC20.json"
}

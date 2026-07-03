use alloy::sol;

// EigenLayer ECDSAStakeRegistry bindings generated at compile time from the
// Foundry artifact (contracts/out/ECDSAStakeRegistry.sol/ECDSAStakeRegistry.json,
// built from lib/eigenlayer-middleware). Includes deploy bytecode.
sol! {
    #[sol(rpc, ignore_unlinked)]
    ECDSAStakeRegistry,
    "bindings/abis/ECDSAStakeRegistry.json"
}

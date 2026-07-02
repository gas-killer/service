use alloy::sol;

// BLSApkRegistry contract bindings generated at compile time from ABI.
// The router resolves operator addresses via pubkeyHashToOperator when
// assembling on-chain NonSignerStakesAndSignature submissions.
sol! {
    #[sol(rpc, ignore_unlinked)]
    BLSApkRegistry,
    "src/bindings/abis/BLSApkRegistry.json"
}

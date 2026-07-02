use alloy::sol;

// BLSSigCheckOperatorStateRetriever contract bindings generated at compile time
// from ABI. The router calls getNonSignerStakesAndSignature to construct the
// verifyAndUpdate calldata for a certified (height, digest, signer bitmap).
sol! {
    #[sol(rpc, ignore_unlinked)]
    BLSSigCheckOperatorStateRetriever,
    "src/bindings/abis/BLSSigCheckOperatorStateRetriever.json"
}

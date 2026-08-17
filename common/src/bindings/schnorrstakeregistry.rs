use alloy::sol;

// The router reads a single view function off `SchnorrStakeRegistry`, so it declares just that
// function rather than vendoring the contract's ABI artifact a second time (the deploy tooling in
// `scripts` already carries the full artifact, bytecode included, because it deploys the registry).
// `scripts::bindings` pins this declaration against that artifact so an upstream rename fails a
// test rather than silently degrading into a warning at runtime.
sol! {
    #[sol(rpc)]
    interface ISchnorrStakeRegistry {
        /// The earliest block at which the operator set can change, or `type(uint256).max` when no
        /// change is scheduled. A settlement is safe from set-mutation invalidation while the block
        /// it lands in is below this value.
        function nextPossibleMutationBlock() external view returns (uint256);
    }
}

use alloy::sol;

// The one view an SDK that binds `msg.value` into its signed digest adds. Declared on its own
// rather than read from the vendored ABI, so callers can probe any target for it whatever SDK
// version that ABI was refreshed from.
sol! {
    #[sol(rpc)]
    interface IValueBoundDigest {
        function getMessageHashWithValue(
            uint256 transitionIndex,
            bytes4 targetFunction,
            bytes storageUpdates,
            uint256 value
        ) external view returns (bytes32);
    }
}

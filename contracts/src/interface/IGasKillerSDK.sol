// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.0;

import {IERC165} from "./IERC165.sol";

/// @title IGasKillerSDK
/// @notice Interface for GasKillerSDK contracts
/// @dev Defines the core functionality that GasKillerSDK implementations must provide.
///      State updates are approved by an ECDSA quorum: registered operators sign the
///      task digest with their secp256k1 keys, and the contract recovers each signer
///      with `ecrecover` and checks the quorum threshold.
interface IGasKillerSDK is IERC165 {
    // Custom errors

    /// @notice Thrown when `transitionIndex + 1` does not equal the current `stateTransitionCount`
    error InvalidTransitionIndex();

    /// @notice Thrown when the reconstructed message hash does not match `msgHash`
    error InvalidSignature();

    /// @notice Thrown when the provided storage updates cannot be decoded or applied
    error InvalidStorageUpdates();

    /// @notice Thrown when an unrecognised state update operation type is encountered
    error InvalidOperation();

    /// @notice Thrown when fewer than `QUORUM_THRESHOLD`% of registered operators signed
    error InsufficientQuorumThreshold();

    /// @notice Thrown when recovered signer addresses are not in strictly ascending order
    ///         (ascending order is required so duplicate signatures cannot inflate the count)
    error UnorderedSignatures();

    /// @notice Thrown when a recovered signer is not a registered operator
    /// @param signer The recovered address that is not registered
    error NotRegisteredOperator(address signer);

    /// @notice Thrown when an operator-registry mutation is attempted by anyone but the operator admin
    error NotOperatorAdmin();

    /// @notice Thrown when registering an operator that is already registered, registering
    ///         the zero address, or deregistering an operator that is not registered
    error InvalidOperator(address operator);

    /// @notice Verify the operators' ECDSA quorum signatures and apply the encoded state updates
    /// @param msgHash The hash of the message to verify (sha256 of the encoded task)
    /// @param storageUpdates The storage updates to verify and apply
    /// @param transitionIndex The transition index
    /// @param targetFunction The target function selector
    /// @param signatures 65-byte `r || s || v` ECDSA signatures over `msgHash`, ordered by
    ///        strictly ascending signer address
    function verifyAndUpdate(
        bytes32 msgHash,
        bytes calldata storageUpdates,
        uint256 transitionIndex,
        bytes4 targetFunction,
        bytes[] calldata signatures
    ) external;
}

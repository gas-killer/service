// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.0;

import {IERC165} from "./interface/IERC165.sol";
import {IGasKillerSDK} from "./interface/IGasKillerSDK.sol";
import {StateTracker} from "./StateTracker.sol";
import {StateChangeHandlerLib, StateUpdateType} from "./StateChangeHandlerLib.sol";
import {ECDSALib} from "./ECDSALib.sol";

/// @title GasKillerSDK
/// @notice Base SDK for implementing Gas Killer functionality in contracts
/// @dev Inherit from this contract to add Gas Killer capabilities to your contract.
///
///      State updates are authorised by an ECDSA multisig: the contract keeps a
///      registry of operator addresses (the operators' EigenLayer ECDSA keys) and
///      `verifyAndUpdate` accepts a list of 65-byte `r || s || v` secp256k1 signatures
///      over the task digest, ordered by strictly ascending signer address. Each
///      signer is recovered with `ecrecover` and must be a registered operator;
///      at least `QUORUM_THRESHOLD`% of the registered operators must have signed.
abstract contract GasKillerSDK is StateTracker, IGasKillerSDK {
    /// @custom:storage-location erc7201:gaskiller.GasKillerSDKECDSA.storage
    struct GasKillerSDKStorage {
        /// @notice Namespace derived from the AVS address; used to scope this contract within the AVS
        bytes namespace;
        /// @notice The AVS service manager address
        address avsAddress;
        /// @notice The address allowed to mutate the operator registry
        address operatorAdmin;
        /// @notice Number of registered operators
        uint256 operatorCount;
        /// @notice Registered operator ECDSA addresses
        mapping(address => bool) operators;
    }

    // keccak256(abi.encode(uint256(keccak256("gaskiller.GasKillerSDKECDSA.storage")) - 1)) & ~bytes32(uint256(0xff));
    bytes32 private constant GAS_KILLER_SDK_STORAGE_LOCATION =
        0x6056deb87cab365bf76a6725b8b096dec334581845ea9d3c2627f8b0efdde700;

    /// @notice Denominator used when evaluating operator-count percentage thresholds (representing 100%)
    uint8 public constant THRESHOLD_DENOMINATOR = 100;

    /// @notice Minimum percentage of registered operators that must have signed to approve a state update
    ///         (QUORUM_THRESHOLD/THRESHOLD_DENOMINATOR)
    uint8 public constant QUORUM_THRESHOLD = 66;

    /// @notice Emitted when an operator is registered
    /// @param operator The registered operator address
    event OperatorRegistered(address indexed operator);

    /// @notice Emitted when an operator is deregistered
    /// @param operator The deregistered operator address
    event OperatorDeregistered(address indexed operator);

    /// @notice Verify the operators' ECDSA quorum signatures and apply the encoded state updates
    /// @param msgHash The hash of the message to verify
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
    ) external trackState {
        GasKillerSDKStorage storage $ = _getGasKillerSDKStorage();

        // Verify transition index and message hash
        require(transitionIndex + 1 == stateTransitionCount(), InvalidTransitionIndex());
        bytes32 expectedHash = sha256(abi.encode(transitionIndex, address(this), targetFunction, storageUpdates));
        require(expectedHash == msgHash, InvalidSignature());

        // Recover every signer, enforce strictly ascending order (no duplicates), and
        // require each to be a registered operator
        address lastSigner = address(0);
        for (uint256 i = 0; i < signatures.length; i++) {
            address signer = ECDSALib.recover(msgHash, signatures[i]);
            require(signer > lastSigner, UnorderedSignatures());
            require($.operators[signer], NotRegisteredOperator(signer));
            lastSigner = signer;
        }

        // Check that signatories are at least 66% of the registered operator set
        require(
            signatures.length * THRESHOLD_DENOMINATOR >= $.operatorCount * QUORUM_THRESHOLD,
            InsufficientQuorumThreshold()
        );

        // Apply the state changes
        _stateChangeHandler(storageUpdates);
    }

    /// @notice Query if a contract implements an interface
    /// @dev Supports ERC-165 and IGasKillerSDK interface detection
    /// @param interfaceId The interface identifier, as specified in ERC-165
    /// @return `true` if the contract implements `interfaceId` and `false` otherwise
    function supportsInterface(bytes4 interfaceId) public view virtual override returns (bool) {
        return interfaceId == type(IERC165).interfaceId || interfaceId == type(IGasKillerSDK).interfaceId;
    }

    /// @notice Compute the expected message hash for a given transition, function, and storage updates
    /// @param transitionIndex The transition index
    /// @param targetFunction The target function selector
    /// @param storageUpdates The ABI-encoded storage updates
    /// @return The expected SHA-256 hash
    function getMessageHash(uint256 transitionIndex, bytes4 targetFunction, bytes calldata storageUpdates)
        external
        view
        returns (bytes32)
    {
        return sha256(abi.encode(transitionIndex, address(this), targetFunction, storageUpdates));
    }

    /// @notice Return the configured AVS service manager address
    /// @return The AVS address
    function avsAddress() external view returns (address) {
        return _getGasKillerSDKStorage().avsAddress;
    }

    /// @notice Return the namespace bytes derived from the AVS address
    /// @return The namespace
    function namespace() external view returns (bytes memory) {
        return _getGasKillerSDKStorage().namespace;
    }

    /// @notice Return the address allowed to mutate the operator registry
    /// @return The operator admin address
    function operatorAdmin() external view returns (address) {
        return _getGasKillerSDKStorage().operatorAdmin;
    }

    /// @notice Return whether `operator` is a registered operator
    /// @param operator The address to check
    /// @return Whether the address is registered
    function isOperator(address operator) external view returns (bool) {
        return _getGasKillerSDKStorage().operators[operator];
    }

    /// @notice Return the number of registered operators
    /// @return The registered operator count
    function operatorCount() external view returns (uint256) {
        return _getGasKillerSDKStorage().operatorCount;
    }

    /// @notice Register an operator's ECDSA address
    /// @param operator The operator address to register
    function registerOperator(address operator) external {
        require(msg.sender == _getGasKillerSDKStorage().operatorAdmin, NotOperatorAdmin());
        _registerOperator(operator);
    }

    /// @notice Deregister an operator's ECDSA address
    /// @param operator The operator address to deregister
    function deregisterOperator(address operator) external {
        GasKillerSDKStorage storage $ = _getGasKillerSDKStorage();
        require(msg.sender == $.operatorAdmin, NotOperatorAdmin());
        require($.operators[operator], InvalidOperator(operator));
        $.operators[operator] = false;
        $.operatorCount -= 1;
        emit OperatorDeregistered(operator);
    }

    /// @notice Decode and execute ABI-encoded storage updates
    /// @param storageUpdates ABI-encoded `(StateUpdateType[], bytes[])` pair
    function _stateChangeHandler(bytes calldata storageUpdates) internal {
        (StateUpdateType[] memory types, bytes[] memory args) = abi.decode(storageUpdates, (StateUpdateType[], bytes[]));
        StateChangeHandlerLib._runStateUpdates(types, args);
    }

    /// @notice Set the AVS address and derive the namespace from it
    /// @dev The namespace is `abi.encodePacked(avsAddress, "gaskiller")`
    /// @param _avsAddress The new AVS service manager address
    function _setAvsAddress(address _avsAddress) internal {
        GasKillerSDKStorage storage $ = _getGasKillerSDKStorage();
        $.avsAddress = _avsAddress;
        $.namespace = abi.encodePacked($.avsAddress, "gaskiller");
    }

    /// @notice Set the address allowed to mutate the operator registry
    /// @param _operatorAdmin The new operator admin address
    function _setOperatorAdmin(address _operatorAdmin) internal {
        _getGasKillerSDKStorage().operatorAdmin = _operatorAdmin;
    }

    /// @notice Register a single operator address
    /// @param operator The operator address to register
    function _registerOperator(address operator) internal {
        GasKillerSDKStorage storage $ = _getGasKillerSDKStorage();
        require(operator != address(0) && !$.operators[operator], InvalidOperator(operator));
        $.operators[operator] = true;
        $.operatorCount += 1;
        emit OperatorRegistered(operator);
    }

    /// @notice Register the initial operator set (constructor helper)
    /// @param operators The operator addresses to register
    function _registerOperators(address[] memory operators) internal {
        for (uint256 i = 0; i < operators.length; i++) {
            _registerOperator(operators[i]);
        }
    }

    /// @notice Load the ERC-7201 storage struct for GasKillerSDK
    /// @return $ The GasKillerSDK storage struct
    function _getGasKillerSDKStorage() private pure returns (GasKillerSDKStorage storage $) {
        assembly {
            $.slot := GAS_KILLER_SDK_STORAGE_LOCATION
        }
    }
}

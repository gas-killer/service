// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {IGasKillerSlasher} from "./interface/IGasKillerSlasher.sol";
import {ISP1Verifier} from "./interface/ISP1Verifier.sol";
import {IHeliosLightClient} from "./interface/IHeliosLightClient.sol";
import {IERC1271Upgradeable} from
    "@openzeppelin-upgrades/contracts/interfaces/IERC1271Upgradeable.sol";
import {
    IAllocationManager,
    IAllocationManagerTypes
} from "eigenlayer-contracts/src/contracts/interfaces/IAllocationManager.sol";
import {IStrategy} from "eigenlayer-contracts/src/contracts/interfaces/IStrategy.sol";
import {OperatorSet} from "eigenlayer-contracts/src/contracts/libraries/OperatorSetLib.sol";
import {OwnableUpgradeable} from "@openzeppelin-upgrades/contracts/access/OwnableUpgradeable.sol";

/// @title GasKillerSlasher (ECDSA quorum variant)
/// @notice Detects fraudulent Gas Killer commitments and slashes the operators who ECDSA-signed
///         them via EigenLayer's `AllocationManager`.
/// @dev A commitment is fraudulent when the aggregate network signed storage updates that differ
///      from the ones produced by actually executing the committed call. A challenger proves the
///      correct execution with the Gas Killer challenger SP1 program (re-executing
///      `contractCalldata` from `callerAddress` against `contractAddress` at the state anchored by
///      `anchorHash`) and commits the resulting canonical storage updates.
///
///      `slash()`:
///      1. checks the operators actually ECDSA-signed the commitment for a valid quorum, using the
///         same `ECDSAStakeRegistry.isValidSignature` the AVS's `verifyAndUpdate` trusts — so the
///         attributed signer set is exactly the one that authorized the (fraudulent) update;
///      2. verifies the SP1 proof and the anchor block hash;
///      3. requires the proven storage updates to differ from the signed ones (fraud);
///      4. slashes every signing operator through `AllocationManager.slashOperator`.
///
///      This contract must be authorized (via the AVS's PermissionController appointee) to call
///      `AllocationManager.slashOperator` on behalf of `AVS`, and the operators must be registered
///      to the AVS's operator set with allocated magnitude for the slash to reduce stake.
contract GasKillerSlasher is IGasKillerSlasher, OwnableUpgradeable {
    // ============ Constants ============

    /// @notice Wad amount for full slash (100%)
    uint256 public constant FULL_SLASH_WAD = 1e18;

    /// @notice `AnchorType.BlockHash` as committed by the challenger program
    uint8 public constant ANCHOR_TYPE_BLOCK_HASH = 0;

    // ============ Immutables ============

    /// @notice The SP1 verifier contract (Groth16 or PLONK gateway)
    ISP1Verifier public immutable SP1_VERIFIER;

    /// @notice The Helios light client used to verify anchor block hashes
    IHeliosLightClient public immutable HELIOS;

    /// @notice The AVS's ECDSA stake registry (ERC-1271 quorum verifier)
    IERC1271Upgradeable public immutable ECDSA_STAKE_REGISTRY;

    /// @notice The EigenLayer AllocationManager
    IAllocationManager public immutable ALLOCATION_MANAGER;

    /// @notice The AVS address (Gas Killer service manager)
    address public immutable AVS;

    /// @notice The operator set ID whose allocated stake is slashed
    uint32 public immutable OPERATOR_SET_ID;

    /// @notice The SP1 verification key of the Gas Killer challenger program
    bytes32 public immutable PROGRAM_V_KEY;

    // ============ Storage ============

    /// @notice Chain config hashes (chain id + active hardfork) accepted for proofs
    mapping(bytes32 => bool) public acceptedChainConfigHash;

    /// @notice Mapping of commitment hash to slashed status
    mapping(bytes32 => bool) private _slashed;

    // ============ Constructor ============

    constructor(
        address _sp1Verifier,
        address _helios,
        address _ecdsaStakeRegistry,
        address _allocationManager,
        address _avs,
        uint32 _operatorSetId,
        bytes32 _programVKey,
        bytes32 _chainConfigHash,
        address _owner
    ) {
        SP1_VERIFIER = ISP1Verifier(_sp1Verifier);
        HELIOS = IHeliosLightClient(_helios);
        ECDSA_STAKE_REGISTRY = IERC1271Upgradeable(_ecdsaStakeRegistry);
        ALLOCATION_MANAGER = IAllocationManager(_allocationManager);
        AVS = _avs;
        OPERATOR_SET_ID = _operatorSetId;
        PROGRAM_V_KEY = _programVKey;
        _transferOwnership(_owner);
        acceptedChainConfigHash[_chainConfigHash] = true;
        emit ChainConfigHashSet(_chainConfigHash, true);
    }

    // ============ External Functions ============

    /// @inheritdoc IGasKillerSlasher
    function slash(
        SignedCommitment calldata commitment,
        uint32 referenceBlockNumber,
        address[] calldata operators,
        bytes[] calldata signatures,
        bytes calldata sp1Proof,
        bytes calldata sp1PublicValues
    ) external {
        require(operators.length != 0, NoOperators());

        bytes32 commitmentHash = computeCommitmentHash(commitment);
        require(!_slashed[commitmentHash], AlreadySlashed());

        // 1. Verify the operators actually ECDSA-signed the commitment for a valid quorum, using
        //    the same ERC-1271 path the AVS's verifyAndUpdate trusts. This proves the attributed
        //    signer set is exactly the quorum that authorized the update. Reverts on an invalid
        //    signature / insufficient stake; the magic-value guard catches a misconfigured registry.
        bytes4 magicValue = ECDSA_STAKE_REGISTRY.isValidSignature(
            commitmentHash, abi.encode(operators, signatures, referenceBlockNumber)
        );
        require(magicValue == IERC1271Upgradeable.isValidSignature.selector, InvalidQuorumSignature());

        // 2. Verify the SP1 proof of the correct execution.
        _verifyProof(sp1Proof, sp1PublicValues);

        // 3. Compare the proven execution with the signed commitment.
        GasKillerPublicValues memory proven = abi.decode(sp1PublicValues, (GasKillerPublicValues));
        _checkInputs(commitment, proven);

        // 4. Verify the anchor block hash is a real block on this chain.
        _verifyAnchorHash(proven.anchorHash);

        // 5. Fraud iff the proven storage updates differ from the signed ones.
        require(
            keccak256(proven.storageUpdates) != keccak256(commitment.storageUpdates), NoFraudDetected()
        );

        _slashed[commitmentHash] = true;

        // 6. Slash every operator that signed the fraudulent commitment.
        _executeSlashing(operators, commitmentHash);

        emit SlashingExecuted(commitmentHash, msg.sender, operators, FULL_SLASH_WAD);
    }

    /// @inheritdoc IGasKillerSlasher
    function setChainConfigHashAccepted(bytes32 chainConfigHash, bool accepted) external onlyOwner {
        acceptedChainConfigHash[chainConfigHash] = accepted;
        emit ChainConfigHashSet(chainConfigHash, accepted);
    }

    /// @inheritdoc IGasKillerSlasher
    function isSlashed(bytes32 commitmentHash) external view returns (bool) {
        return _slashed[commitmentHash];
    }

    /// @inheritdoc IGasKillerSlasher
    function programVKey() external view returns (bytes32) {
        return PROGRAM_V_KEY;
    }

    /// @inheritdoc IGasKillerSlasher
    function computeCommitmentHash(SignedCommitment calldata commitment) public pure returns (bytes32) {
        return sha256(
            abi.encode(
                commitment.transitionIndex,
                commitment.contractAddress,
                commitment.anchorHash,
                commitment.callerAddress,
                commitment.contractCalldata,
                commitment.storageUpdates
            )
        );
    }

    // ============ Internal Functions ============

    function _verifyProof(bytes calldata proofBytes, bytes calldata publicValues) internal view {
        try SP1_VERIFIER.verifyProof(PROGRAM_V_KEY, publicValues, proofBytes) {}
        catch {
            revert InvalidProof();
        }
    }

    function _checkInputs(SignedCommitment calldata commitment, GasKillerPublicValues memory proven)
        internal
        view
    {
        require(acceptedChainConfigHash[proven.chainConfigHash], InvalidChainConfig());
        require(proven.anchorType == ANCHOR_TYPE_BLOCK_HASH, InputMismatch());
        require(proven.anchorHash == commitment.anchorHash, InputMismatch());
        require(proven.callerAddress == commitment.callerAddress, InputMismatch());
        require(proven.contractAddress == commitment.contractAddress, InputMismatch());
        require(
            keccak256(proven.contractCalldata) == keccak256(commitment.contractCalldata), InputMismatch()
        );
    }

    function _verifyAnchorHash(bytes32 anchorHash) internal view {
        if (address(HELIOS) != address(0) && HELIOS.isBlockHashValid(anchorHash)) {
            return;
        }
        revert UnverifiedBlock();
    }

    /// @notice Slash each operator via EigenLayer's AllocationManager
    /// @dev The signer list is exactly the quorum the ECDSA registry validated, so every entry is
    ///      an operator that authorized the fraudulent commitment. `getStrategiesInOperatorSet`
    ///      returns the strategies whose allocated magnitude backs the operator set, in insertion
    ///      order; `slashOperator` requires them ascending, so they are sorted first.
    function _executeSlashing(address[] calldata operators, bytes32 commitmentHash) internal {
        OperatorSet memory operatorSet = OperatorSet({avs: AVS, id: OPERATOR_SET_ID});
        IStrategy[] memory strategies = ALLOCATION_MANAGER.getStrategiesInOperatorSet(operatorSet);
        _sortAscending(strategies);

        uint256[] memory wadsToSlash = new uint256[](strategies.length);
        for (uint256 i = 0; i < strategies.length; i++) {
            wadsToSlash[i] = FULL_SLASH_WAD;
        }

        string memory description =
            string(abi.encodePacked("Gas Killer fraud: ", _bytes32ToHexString(commitmentHash)));

        for (uint256 i = 0; i < operators.length; i++) {
            if (ALLOCATION_MANAGER.isOperatorSlashable(operators[i], operatorSet)) {
                IAllocationManagerTypes.SlashingParams memory params = IAllocationManagerTypes.SlashingParams({
                    operator: operators[i],
                    operatorSetId: OPERATOR_SET_ID,
                    strategies: strategies,
                    wadsToSlash: wadsToSlash,
                    description: description
                });
                ALLOCATION_MANAGER.slashOperator(AVS, params);
            }
        }
    }

    /// @dev Insertion sort by strategy address; operator sets hold a handful of strategies.
    function _sortAscending(IStrategy[] memory strategies) internal pure {
        for (uint256 i = 1; i < strategies.length; i++) {
            IStrategy current = strategies[i];
            uint256 j = i;
            while (j > 0 && address(strategies[j - 1]) > address(current)) {
                strategies[j] = strategies[j - 1];
                j--;
            }
            strategies[j] = current;
        }
    }

    function _bytes32ToHexString(bytes32 value) internal pure returns (string memory) {
        bytes memory alphabet = "0123456789abcdef";
        bytes memory str = new bytes(66);
        str[0] = "0";
        str[1] = "x";
        for (uint256 i = 0; i < 32; i++) {
            str[2 + i * 2] = alphabet[uint8(value[i] >> 4)];
            str[3 + i * 2] = alphabet[uint8(value[i] & 0x0f)];
        }
        return string(str);
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {IGasKillerSlasher} from "./interface/IGasKillerSlasher.sol";
import {ISP1Verifier} from "./interface/ISP1Verifier.sol";
import {IHeliosLightClient} from "./interface/IHeliosLightClient.sol";
import {IERC1271Upgradeable} from "@openzeppelin-upgrades/contracts/interfaces/IERC1271Upgradeable.sol";
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
///         attributed signer set is exactly the one that authorized the (fraudulent) update; the
///         reference block must be within `MAX_REFERENCE_BLOCK_AGE` (the challenge window);
///      2. verifies the SP1 proof and the anchor block hash;
///      3. requires the proven storage updates to differ from the signed ones (fraud);
///      4. slashes every signing operator through `AllocationManager.slashOperator`, tracked per
///         (commitment, operator) so a partial (quorum-subset) challenge never immunizes the
///         commitment's remaining signers.
///
///      This contract must be authorized (via the AVS's PermissionController appointee) to call
///      `AllocationManager.slashOperator` on behalf of `AVS`. `GasKillerServiceManager`'s
///      registrar hook guarantees operator-set members registered with an ECDSA signing key and
///      allocated magnitude, so an attributed signer has stake for the slash to burn.
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

    /// @notice Oldest accepted `referenceBlockNumber` age, in blocks
    /// @dev Bounds two windows at once: how long after a fraudulent commitment a challenge can
    ///      still attribute its signatures (pick a value >= the AllocationManager's
    ///      DEALLOCATION_DELAY so a deregistering fraudster stays challengeable for as long as
    ///      its stake stays burnable), and how long a rotated-away ECDSA signing key can still
    ///      produce a slashable attribution if it later leaks (an unbounded lookback would make
    ///      every historical key slash-capable forever).
    uint32 public immutable MAX_REFERENCE_BLOCK_AGE;

    // ============ Storage ============

    /// @notice Chain config hashes (chain id + active hardfork) accepted for proofs
    mapping(bytes32 => bool) public acceptedChainConfigHash;

    /// @notice Mapping of commitment hash to slashed status (any signer burned)
    mapping(bytes32 => bool) private _slashed;

    /// @notice Per-operator slash bookkeeping, so a challenge attributing only a quorum subset
    ///         cannot immunize the commitment's remaining signers
    mapping(bytes32 => mapping(address => bool)) private _operatorSlashed;

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
        address _owner,
        uint32 _maxReferenceBlockAge
    ) {
        SP1_VERIFIER = ISP1Verifier(_sp1Verifier);
        HELIOS = IHeliosLightClient(_helios);
        ECDSA_STAKE_REGISTRY = IERC1271Upgradeable(_ecdsaStakeRegistry);
        ALLOCATION_MANAGER = IAllocationManager(_allocationManager);
        AVS = _avs;
        OPERATOR_SET_ID = _operatorSetId;
        PROGRAM_V_KEY = _programVKey;
        // A zero age would reject every reference block (the registry also requires it strictly
        // in the past), permanently bricking slash().
        require(_maxReferenceBlockAge != 0, InvalidMaxReferenceBlockAge());
        MAX_REFERENCE_BLOCK_AGE = _maxReferenceBlockAge;
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
        // Bound the lookback: an arbitrarily old reference block would let leaked, long-rotated
        // signing keys (checkpointed at that height) attribute a fresh fraudulent commitment.
        require(uint256(referenceBlockNumber) + MAX_REFERENCE_BLOCK_AGE >= block.number, StaleReferenceBlock());

        bytes32 commitmentHash = computeCommitmentHash(commitment);
        require(_hasFreshOperator(commitmentHash, operators), AlreadySlashed());

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
        require(keccak256(proven.storageUpdates) != keccak256(commitment.storageUpdates), NoFraudDetected());

        // 6. Slash every not-yet-slashed operator that signed the fraudulent commitment.
        address[] memory freshOperators = _executeSlashing(operators, commitmentHash);

        // Mark the commitment slashed once at least one signer has actually been burned.
        if (freshOperators.length != 0) {
            _slashed[commitmentHash] = true;
        }

        emit SlashingExecuted(commitmentHash, msg.sender, freshOperators, FULL_SLASH_WAD);
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
    function isOperatorSlashed(bytes32 commitmentHash, address operator) external view returns (bool) {
        return _operatorSlashed[commitmentHash][operator];
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

    function _checkInputs(SignedCommitment calldata commitment, GasKillerPublicValues memory proven) internal view {
        require(acceptedChainConfigHash[proven.chainConfigHash], InvalidChainConfig());
        require(proven.anchorType == ANCHOR_TYPE_BLOCK_HASH, InputMismatch());
        require(proven.anchorHash == commitment.anchorHash, InputMismatch());
        require(proven.callerAddress == commitment.callerAddress, InputMismatch());
        require(proven.contractAddress == commitment.contractAddress, InputMismatch());
        require(keccak256(proven.contractCalldata) == keccak256(commitment.contractCalldata), InputMismatch());
    }

    function _verifyAnchorHash(bytes32 anchorHash) internal view {
        if (address(HELIOS) != address(0) && HELIOS.isBlockHashValid(anchorHash)) {
            return;
        }
        revert UnverifiedBlock();
    }

    /// @notice Return true if at least one of `operators` has not yet been burned for this
    ///         commitment, so the challenge can still slash a fresh signer.
    /// @dev A call naming only already-slashed operators is a redundant no-op and reverts
    ///      `AlreadySlashed`; per-operator tracking lets a later challenge burn a signer a
    ///      previous quorum-subset challenge left untouched.
    function _hasFreshOperator(bytes32 commitmentHash, address[] calldata operators) internal view returns (bool) {
        for (uint256 i = 0; i < operators.length; i++) {
            if (!_operatorSlashed[commitmentHash][operators[i]]) {
                return true;
            }
        }
        return false;
    }

    /// @notice Slash each not-yet-burned operator via EigenLayer's AllocationManager
    /// @dev The signer list is exactly the quorum the ECDSA registry validated, so every entry is
    ///      an operator that authorized the fraudulent commitment. `getStrategiesInOperatorSet`
    ///      returns the strategies whose allocated magnitude backs the operator set, in insertion
    ///      order; `slashOperator` requires them ascending, so they are sorted first. Operators
    ///      already burned for this commitment (a prior partial challenge) are skipped; the
    ///      per-operator flag is set only after a burn that actually reduced stake. Setting it
    ///      after the call is reentrancy-safe: `slashOperator` only updates burn accounting
    ///      (`increaseBurnOrRedistributableShares`) — no token transfer or strategy callout
    ///      happens until the separate permissionless `clearBurnOrRedistributableShares`.
    /// @return freshOperators The operators newly slashed by this call.
    function _executeSlashing(address[] calldata operators, bytes32 commitmentHash)
        internal
        returns (address[] memory freshOperators)
    {
        OperatorSet memory operatorSet = OperatorSet({avs: AVS, id: OPERATOR_SET_ID});
        IStrategy[] memory strategies = ALLOCATION_MANAGER.getStrategiesInOperatorSet(operatorSet);
        _sortAscending(strategies);

        uint256[] memory wadsToSlash = new uint256[](strategies.length);
        for (uint256 i = 0; i < strategies.length; i++) {
            wadsToSlash[i] = FULL_SLASH_WAD;
        }

        string memory description = string(abi.encodePacked("Gas Killer fraud: ", _bytes32ToHexString(commitmentHash)));

        address[] memory slashedBuf = new address[](operators.length);
        uint256 count = 0;
        for (uint256 i = 0; i < operators.length; i++) {
            address operator = operators[i];
            // Skip signers already burned for this commitment, and signers with no slashable
            // magnitude (e.g. fully deallocated past the window).
            if (_operatorSlashed[commitmentHash][operator]) {
                continue;
            }
            if (!ALLOCATION_MANAGER.isOperatorSlashable(operator, operatorSet)) {
                continue;
            }

            IAllocationManagerTypes.SlashingParams memory params = IAllocationManagerTypes.SlashingParams({
                operator: operator,
                operatorSetId: OPERATOR_SET_ID,
                strategies: strategies,
                wadsToSlash: wadsToSlash,
                description: description
            });
            (, uint256[] memory shares) = ALLOCATION_MANAGER.slashOperator(AVS, params);

            // Only record the operator as slashed if stake was actually burned. An operator
            // registered but with zero current magnitude (a pending/deallocated allocation)
            // is `isOperatorSlashable` yet burns nothing — marking it here would immunize it
            // against a later challenge once its magnitude becomes effective.
            if (_anyNonzero(shares)) {
                _operatorSlashed[commitmentHash][operator] = true;
                slashedBuf[count++] = operator;
            }
        }

        freshOperators = new address[](count);
        for (uint256 i = 0; i < count; i++) {
            freshOperators[i] = slashedBuf[i];
        }
    }

    /// @dev True iff any element is nonzero.
    function _anyNonzero(uint256[] memory values) internal pure returns (bool) {
        for (uint256 i = 0; i < values.length; i++) {
            if (values[i] != 0) {
                return true;
            }
        }
        return false;
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

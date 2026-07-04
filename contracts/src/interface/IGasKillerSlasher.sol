// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

/// @title IGasKillerSlasher
/// @notice Interface for the Gas Killer slashing contract (ECDSA quorum variant)
/// @dev Detects fraudulent Gas Killer commitments and slashes the operators who ECDSA-signed
///      them, using the same `ECDSAStakeRegistry` quorum the AVS's `verifyAndUpdate` trusts for
///      attribution, an SP1 execution proof for correctness, and EigenLayer's `AllocationManager`
///      for the slash itself.
interface IGasKillerSlasher {
    // ============ Structs ============

    /// @notice A commitment the aggregate network ECDSA-signed
    /// @dev `sha256(abi.encode(transitionIndex, contractAddress, anchorHash, callerAddress,
    ///      contractCalldata, storageUpdates))` is the message hash operators sign and the hash
    ///      `GasKillerSDK.verifyAndUpdate` verifies
    /// @param transitionIndex Sequential counter for state transitions
    /// @param contractAddress The target contract address
    /// @param anchorHash Hash of the block the execution is anchored to
    /// @param callerAddress The caller address (msg.sender for the original call)
    /// @param contractCalldata Full calldata with arguments
    /// @param storageUpdates Claimed storage changes, encoded as `abi.encode(StateUpdateType[], bytes[])`
    struct SignedCommitment {
        uint256 transitionIndex;
        address contractAddress;
        bytes32 anchorHash;
        address callerAddress;
        bytes contractCalldata;
        bytes storageUpdates;
    }

    /// @notice Public values committed by the Gas Killer challenger SP1 program
    /// @param id Anchor id (block number for BlockHash anchors)
    /// @param anchorHash Hash of the block the execution was anchored to
    /// @param anchorType Type of anchor (0 = BlockHash, 1 = Timestamp, 2 = Slot)
    /// @param chainConfigHash Hash of the chain configuration (chain id + active hardfork)
    /// @param callerAddress The caller address used in the proven execution
    /// @param contractAddress The contract address used in the proven execution
    /// @param contractCalldata The calldata used in the proven execution
    /// @param contractOutput The return data of the proven execution
    /// @param storageUpdates The storage updates produced by the proven execution, encoded
    ///        exactly as an honest operator would sign them
    /// @param opcodeHash keccak256 of the state-modifying opcodes executed
    struct GasKillerPublicValues {
        uint256 id;
        bytes32 anchorHash;
        uint8 anchorType;
        bytes32 chainConfigHash;
        address callerAddress;
        address contractAddress;
        bytes contractCalldata;
        bytes contractOutput;
        bytes storageUpdates;
        bytes32 opcodeHash;
    }

    // ============ Events ============

    /// @notice Emitted when slashing is executed
    /// @param commitmentHash Hash of the slashed commitment
    /// @param challenger Address of the challenger who submitted the proof
    /// @param slashedOperators Operators who were slashed
    /// @param slashWad Slash proportion per strategy, in WAD (1e18 = 100%)
    event SlashingExecuted(
        bytes32 indexed commitmentHash, address indexed challenger, address[] slashedOperators, uint256 slashWad
    );

    /// @notice Emitted when a chain config hash is accepted or revoked
    event ChainConfigHashSet(bytes32 indexed chainConfigHash, bool accepted);

    // ============ Errors ============

    error InvalidProof();
    error UnverifiedBlock();
    error InputMismatch();
    error InvalidChainConfig();
    error NoFraudDetected();
    error AlreadySlashed();
    error InvalidQuorumSignature();
    error NoOperators();

    // ============ External Functions ============

    /// @notice Submit a fraud proof for a signed commitment and slash the operators who signed it
    /// @dev Verifies (1) the operators actually ECDSA-signed the commitment for a valid quorum
    ///      (via the AVS's `ECDSAStakeRegistry.isValidSignature`), (2) the SP1 proof of the correct
    ///      execution, (3) the anchor block hash, and (4) that the proven storage updates differ
    ///      from the signed ones. On success, each signing operator is slashed through EigenLayer's
    ///      `AllocationManager`.
    /// @param commitment The signed commitment being challenged
    /// @param referenceBlockNumber The reference block the quorum signature was produced against
    /// @param operators The operators that signed, in strictly ascending address order
    /// @param signatures 65-byte `r || s || v` ECDSA signatures, index-aligned with `operators`
    /// @param sp1Proof The SP1 proof bytes (Groth16 or PLONK)
    /// @param sp1PublicValues The ABI-encoded `GasKillerPublicValues`
    function slash(
        SignedCommitment calldata commitment,
        uint32 referenceBlockNumber,
        address[] calldata operators,
        bytes[] calldata signatures,
        bytes calldata sp1Proof,
        bytes calldata sp1PublicValues
    ) external;

    /// @notice Accept or revoke a chain config hash for challenger proofs (owner-only)
    function setChainConfigHashAccepted(bytes32 chainConfigHash, bool accepted) external;

    /// @notice Whether proofs carrying `chainConfigHash` are accepted
    function acceptedChainConfigHash(bytes32 chainConfigHash) external view returns (bool);

    /// @notice Check if a commitment has been slashed
    function isSlashed(bytes32 commitmentHash) external view returns (bool);

    /// @notice Get the SP1 program verification key of the challenger program
    function programVKey() external view returns (bytes32);

    /// @notice Compute the commitment hash operators sign
    function computeCommitmentHash(SignedCommitment calldata commitment) external pure returns (bytes32);
}

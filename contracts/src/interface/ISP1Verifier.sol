// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.0;

/**
 * @title ISP1Verifier
 * @notice Interface for SP1 proof verification (Groth16 or PLONK)
 * @dev This interface wraps the SP1 verifier contract from Succinct. The Gas Killer slasher
 *      wires in the Groth16 verifier, but the interface is proof-system-agnostic.
 */
interface ISP1Verifier {
    /**
     * @notice Verify an SP1 proof (Groth16 or PLONK)
     * @param programVKey The verification key for the SP1 program
     * @param publicValues The public values from the proof
     * @param proofBytes The proof bytes
     */
    function verifyProof(
        bytes32 programVKey,
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external view;
}

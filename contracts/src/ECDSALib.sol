// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.0;

/// @title ECDSALib
/// @notice Minimal secp256k1 signature recovery for 65-byte `r || s || v` signatures
/// @dev Enforces canonical (EIP-2 low-s) signatures so a signature cannot be malleated
///      into a second "distinct" signature by the same signer.
library ECDSALib {
    /// @notice Thrown when a signature is not exactly 65 bytes
    error InvalidSignatureLength();

    /// @notice Thrown when `s` is in the upper half of the curve order (malleable form)
    error InvalidSignatureS();

    /// @notice Thrown when `v` is not 27 or 28 (0/1 inputs are normalised first)
    error InvalidSignatureV();

    /// @notice Thrown when `ecrecover` returns the zero address
    error InvalidSigner();

    /// @dev secp256k1 group order / 2: signatures with `s` above this are rejected (EIP-2)
    uint256 private constant HALF_CURVE_ORDER =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    /// @notice Recover the signer of `digest` from a 65-byte `r || s || v` signature
    /// @param digest The 32-byte digest that was signed (used as-is, no EIP-191 prefix)
    /// @param signature The 65-byte signature; `v` may be 0/1 or 27/28
    /// @return The recovered signer address (never the zero address)
    function recover(bytes32 digest, bytes memory signature) internal pure returns (address) {
        require(signature.length == 65, InvalidSignatureLength());

        bytes32 r;
        bytes32 s;
        uint8 v;
        assembly {
            r := mload(add(signature, 0x20))
            s := mload(add(signature, 0x40))
            v := byte(0, mload(add(signature, 0x60)))
        }

        if (v < 27) {
            v += 27;
        }
        require(v == 27 || v == 28, InvalidSignatureV());
        require(uint256(s) <= HALF_CURVE_ORDER, InvalidSignatureS());

        address signer = ecrecover(digest, v, r, s);
        require(signer != address(0), InvalidSigner());
        return signer;
    }
}

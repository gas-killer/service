// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.0;

/// @title IERC165
/// @notice Standard interface detection (EIP-165)
interface IERC165 {
    /// @notice Query if a contract implements an interface
    /// @param interfaceID The interface identifier, as specified in ERC-165
    /// @return `true` if the contract implements `interfaceID` and
    ///  `interfaceID` is not 0xffffffff, `false` otherwise
    function supportsInterface(bytes4 interfaceID) external view returns (bool);
}

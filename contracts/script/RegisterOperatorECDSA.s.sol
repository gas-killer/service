// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {Script} from "forge-std/Script.sol";

import {ECDSAStakeRegistry} from "@eigenlayer-middleware/unaudited/ECDSAStakeRegistry.sol";
import {IAVSDirectory} from "eigenlayer-contracts/src/contracts/interfaces/IAVSDirectory.sol";
import {ISignatureUtilsMixinTypes} from
    "eigenlayer-contracts/src/contracts/interfaces/ISignatureUtilsMixin.sol";

/// @title RegisterOperatorECDSA
/// @notice Registers one operator with the Gas Killer `ECDSAStakeRegistry`:
///         signs the AVSDirectory registration digest with the operator key and
///         calls `registerOperatorWithSignature` as the operator, with signing
///         key = operator address (what the nodes sign task digests with).
/// @dev Run once per operator by the eigenlayer setup container, mirroring the
///      BLS `RegisterOperator.s.sol` flow. The salt is derived from the current
///      block so re-running against the same chain never reuses a spent salt.
///
///      Required env: OPERATOR_PRIVATE_KEY, ECDSA_STAKE_REGISTRY_ADDRESS,
///      GAS_KILLER_SERVICE_MANAGER_ADDRESS, AVS_DIRECTORY_ADDRESS.
contract RegisterOperatorECDSA is Script {
    function run() external {
        uint256 operatorKey = vm.envUint("OPERATOR_PRIVATE_KEY");
        address operator = vm.addr(operatorKey);
        ECDSAStakeRegistry registry =
            ECDSAStakeRegistry(vm.envAddress("ECDSA_STAKE_REGISTRY_ADDRESS"));
        address serviceManager = vm.envAddress("GAS_KILLER_SERVICE_MANAGER_ADDRESS");
        IAVSDirectory avsDirectory = IAVSDirectory(vm.envAddress("AVS_DIRECTORY_ADDRESS"));

        // Unique per run: AVSDirectory permanently spends (operator, salt) pairs.
        bytes32 salt = keccak256(
            abi.encodePacked("gas-killer-ecdsa", operator, block.number, block.timestamp)
        );
        uint256 expiry = block.timestamp + 1 days;

        bytes32 digest = avsDirectory.calculateOperatorAVSRegistrationDigestHash(
            operator, serviceManager, salt, expiry
        );
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(operatorKey, digest);

        vm.startBroadcast(operatorKey);
        registry.registerOperatorWithSignature(
            ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry({
                signature: abi.encodePacked(r, s, v),
                salt: salt,
                expiry: expiry
            }),
            operator
        );
        vm.stopBroadcast();
    }
}

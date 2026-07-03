// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {Script} from "forge-std/Script.sol";
import {console2} from "forge-std/console2.sol";

import {ECDSAStakeRegistry} from "@eigenlayer-middleware/unaudited/ECDSAStakeRegistry.sol";

/// @title FinalizeECDSAStack
/// @notice Sets the `ECDSAStakeRegistry` stake threshold to
///         `ceil(totalWeight * QUORUM_THRESHOLD / THRESHOLD_DENOMINATOR)` once
///         all operators are registered (the same quorum fraction the AVS
///         service manager wrapper advertises).
/// @dev Run by the eigenlayer setup container after the per-operator
///      `RegisterOperatorECDSA` runs. Must be broadcast by the registry owner
///      (the `DeployECDSAStack` deployer).
///
///      Required env: PRIVATE_KEY, ECDSA_STAKE_REGISTRY_ADDRESS,
///      QUORUM_THRESHOLD, THRESHOLD_DENOMINATOR.
contract FinalizeECDSAStack is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        ECDSAStakeRegistry registry =
            ECDSAStakeRegistry(vm.envAddress("ECDSA_STAKE_REGISTRY_ADDRESS"));
        uint256 quorumThreshold = vm.envUint("QUORUM_THRESHOLD");
        uint256 thresholdDenominator = vm.envUint("THRESHOLD_DENOMINATOR");
        require(
            quorumThreshold > 0 && thresholdDenominator >= quorumThreshold,
            "invalid quorum threshold fraction"
        );

        uint256 totalWeight = registry.getLastCheckpointTotalWeight();
        require(totalWeight > 0, "no registered operator weight");

        uint256 threshold =
            (totalWeight * quorumThreshold + thresholdDenominator - 1) / thresholdDenominator;

        vm.startBroadcast(deployerKey);
        registry.updateStakeThreshold(threshold);
        vm.stopBroadcast();

        console2.log("ECDSA stake threshold set:", threshold);
        console2.log("Total registered weight:", totalWeight);
    }
}

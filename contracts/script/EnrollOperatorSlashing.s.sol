// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {Script} from "forge-std/Script.sol";
import {console2} from "forge-std/console2.sol";

import {
    IAllocationManager,
    IAllocationManagerTypes
} from "eigenlayer-contracts/src/contracts/interfaces/IAllocationManager.sol";
import {OperatorSet} from "eigenlayer-contracts/src/contracts/libraries/OperatorSetLib.sol";
import {IStrategy} from "eigenlayer-contracts/src/contracts/interfaces/IStrategy.sol";

/// @title EnrollOperatorSlashing
/// @notice Makes one registered operator slashable: allocates its magnitude to the
///         Gas Killer operator set and registers it for the set. After this, a
///         fraudulent commitment signed by the operator lets the `GasKillerSlasher`
///         burn the allocated stake.
/// @dev Run once per operator, after `RegisterOperatorECDSA` (the service
///      manager's registrar hook rejects operators without an ECDSA signing key,
///      and rejects operators without an allocation — which is why this script
///      allocates BEFORE registering) and after the operator's allocation delay
///      has become effective — the AllocationManager activates an operator's
///      (first) allocation delay `ALLOCATION_CONFIGURATION_DELAY + 1` blocks after
///      `DelegationManager.registerAsOperator` set it. On a local anvil devnet,
///      mine those blocks with `cast rpc anvil_mine <n>`; this script reverts with
///      a clear message until then. Safe to re-run: already-completed steps are
///      skipped.
///
///      Required env: OPERATOR_PRIVATE_KEY, GAS_KILLER_SERVICE_MANAGER_ADDRESS,
///      ALLOCATION_MANAGER_ADDRESS, LST_STRATEGY_ADDRESS.
///      Optional env: OPERATOR_SET_ID (defaults to 0), ALLOCATION_MAGNITUDE
///      (WAD-scale slashable fraction of the operator's stake; defaults to 1e18 =
///      the operator's entire magnitude).
contract EnrollOperatorSlashing is Script {
    uint256 internal constant WAD = 1e18;

    function run() external {
        uint256 operatorKey = vm.envUint("OPERATOR_PRIVATE_KEY");
        address operator = vm.addr(operatorKey);
        address serviceManager = vm.envAddress("GAS_KILLER_SERVICE_MANAGER_ADDRESS");
        IAllocationManager allocationManager = IAllocationManager(vm.envAddress("ALLOCATION_MANAGER_ADDRESS"));
        IStrategy strategy = IStrategy(vm.envAddress("LST_STRATEGY_ADDRESS"));
        uint32 operatorSetId = uint32(vm.envOr("OPERATOR_SET_ID", uint256(0)));
        uint256 rawMagnitude = vm.envOr("ALLOCATION_MAGNITUDE", WAD);
        // AllocateParams.newMagnitudes is uint64; an unchecked cast would silently
        // wrap >= 2^64 values into tiny (under-slashable) allocations.
        require(rawMagnitude > 0 && rawMagnitude <= WAD, "ALLOCATION_MAGNITUDE must be in (0, 1e18]");
        uint64 magnitude = uint64(rawMagnitude);

        OperatorSet memory operatorSet = OperatorSet({avs: serviceManager, id: operatorSetId});

        (bool delayEffective,) = allocationManager.getAllocationDelay(operator);
        require(
            delayEffective,
            "operator allocation delay not effective yet: wait ALLOCATION_CONFIGURATION_DELAY + 1 "
            "blocks after DelegationManager.registerAsOperator (local devnet: cast rpc anvil_mine <n>)"
        );

        IAllocationManagerTypes.Allocation memory current =
            allocationManager.getAllocation(operator, operatorSet, strategy);
        // Treat an in-flight allocation to the same target as done: modifyAllocations reverts
        // ModificationAlreadyPending while a change is pending, so a re-run after a partially
        // applied first run (allocation landed, registration failed) must not re-allocate.
        int256 effectiveMagnitude = int256(uint256(current.currentMagnitude)) + int256(current.pendingDiff);
        bool alreadyAllocated = effectiveMagnitude == int256(uint256(magnitude));
        bool alreadyRegistered = allocationManager.isMemberOfOperatorSet(operator, operatorSet);

        vm.startBroadcast(operatorKey);

        if (alreadyAllocated) {
            console2.log("Allocation already at target magnitude; skipping modifyAllocations");
        } else {
            IStrategy[] memory strategies = new IStrategy[](1);
            strategies[0] = strategy;
            uint64[] memory magnitudes = new uint64[](1);
            magnitudes[0] = magnitude;

            IAllocationManagerTypes.AllocateParams[] memory allocParams =
                new IAllocationManagerTypes.AllocateParams[](1);
            allocParams[0] = IAllocationManagerTypes.AllocateParams({
                operatorSet: operatorSet, strategies: strategies, newMagnitudes: magnitudes
            });
            allocationManager.modifyAllocations(operator, allocParams);
        }

        if (alreadyRegistered) {
            console2.log("Operator already registered for the operator set; skipping registration");
        } else {
            // The service manager's registrar hook requires EFFECTIVE (current) magnitude, not a
            // still-pending increase. For a delay-0 operator the allocation above is effective in
            // this same block; a delay-N operator must re-run this script N blocks later. Surface
            // that plainly instead of the raw OperatorNotAllocated revert.
            require(
                allocationManager.getAllocation(operator, operatorSet, strategy).currentMagnitude > 0,
                "allocation still pending (operator allocation delay > 0): re-run this script once it "
                "matures (local devnet: cast rpc anvil_mine <delay>)"
            );
            uint32[] memory operatorSetIds = new uint32[](1);
            operatorSetIds[0] = operatorSetId;
            allocationManager.registerForOperatorSets(
                operator,
                IAllocationManagerTypes.RegisterParams({avs: serviceManager, operatorSetIds: operatorSetIds, data: ""})
            );
        }

        vm.stopBroadcast();

        console2.log("Operator enrolled for slashing:", operator);
        console2.log("Allocated magnitude (WAD):", magnitude);
    }
}

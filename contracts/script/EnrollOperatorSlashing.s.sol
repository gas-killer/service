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
///      manager's registrar hook rejects operators without an ECDSA signing key)
///      and after the operator's allocation delay has become effective — the
///      AllocationManager activates an operator's (first) allocation delay
///      `ALLOCATION_CONFIGURATION_DELAY + 1` blocks after
///      `DelegationManager.registerAsOperator` set it. On a local anvil devnet,
///      mine those blocks with `cast rpc anvil_mine <n>`; this script reverts with
///      a clear message until then.
///
///      Required env: OPERATOR_PRIVATE_KEY, GAS_KILLER_SERVICE_MANAGER_ADDRESS,
///      ALLOCATION_MANAGER_ADDRESS, LST_STRATEGY_ADDRESS.
///      Optional env: OPERATOR_SET_ID (defaults to 0), ALLOCATION_MAGNITUDE
///      (WAD-scale slashable fraction of the operator's stake; defaults to 1e18 =
///      the operator's entire magnitude).
contract EnrollOperatorSlashing is Script {
    function run() external {
        uint256 operatorKey = vm.envUint("OPERATOR_PRIVATE_KEY");
        address operator = vm.addr(operatorKey);
        address serviceManager = vm.envAddress("GAS_KILLER_SERVICE_MANAGER_ADDRESS");
        IAllocationManager allocationManager =
            IAllocationManager(vm.envAddress("ALLOCATION_MANAGER_ADDRESS"));
        IStrategy strategy = IStrategy(vm.envAddress("LST_STRATEGY_ADDRESS"));
        uint32 operatorSetId = uint32(vm.envOr("OPERATOR_SET_ID", uint256(0)));
        uint64 magnitude = uint64(vm.envOr("ALLOCATION_MAGNITUDE", uint256(1e18)));

        (bool delayEffective,) = allocationManager.getAllocationDelay(operator);
        require(
            delayEffective,
            "operator allocation delay not effective yet: wait ALLOCATION_CONFIGURATION_DELAY + 1 "
            "blocks after DelegationManager.registerAsOperator (local devnet: cast rpc anvil_mine <n>)"
        );

        IStrategy[] memory strategies = new IStrategy[](1);
        strategies[0] = strategy;
        uint64[] memory magnitudes = new uint64[](1);
        magnitudes[0] = magnitude;

        IAllocationManagerTypes.AllocateParams[] memory allocParams =
            new IAllocationManagerTypes.AllocateParams[](1);
        allocParams[0] = IAllocationManagerTypes.AllocateParams({
            operatorSet: OperatorSet({avs: serviceManager, id: operatorSetId}),
            strategies: strategies,
            newMagnitudes: magnitudes
        });

        uint32[] memory operatorSetIds = new uint32[](1);
        operatorSetIds[0] = operatorSetId;

        vm.startBroadcast(operatorKey);
        allocationManager.modifyAllocations(operator, allocParams);
        allocationManager.registerForOperatorSets(
            operator,
            IAllocationManagerTypes.RegisterParams({
                avs: serviceManager,
                operatorSetIds: operatorSetIds,
                data: ""
            })
        );
        vm.stopBroadcast();

        console2.log("Operator enrolled for slashing:", operator);
        console2.log("Allocated magnitude (WAD):", magnitude);
    }
}

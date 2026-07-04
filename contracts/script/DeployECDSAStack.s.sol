// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {Script} from "forge-std/Script.sol";

import {ECDSAStakeRegistry} from "@eigenlayer-middleware/unaudited/ECDSAStakeRegistry.sol";
import {IECDSAStakeRegistryTypes} from "@eigenlayer-middleware/interfaces/IECDSAStakeRegistry.sol";
import {IDelegationManager} from
    "eigenlayer-contracts/src/contracts/interfaces/IDelegationManager.sol";
import {IStrategy} from "eigenlayer-contracts/src/contracts/interfaces/IStrategy.sol";

import {GasKillerServiceManager} from "../src/GasKillerServiceManager.sol";

/// @title DeployECDSAStack
/// @notice Deploys the Gas Killer ECDSA AVS stack: the EigenLayer
///         `ECDSAStakeRegistry` plus the `GasKillerServiceManager` wired to it,
///         initialised with a single-strategy quorum (10_000 BPS on the LST
///         strategy the operators deposited into). When
///         `ALLOCATION_MANAGER_ADDRESS` is provided, it also registers the AVS's
///         metadata with the AllocationManager and creates the slashable operator
///         set (id `OPERATOR_SET_ID`, backed by the same LST strategy) that
///         `EnrollOperatorSlashing` registers operators into and the
///         `GasKillerSlasher` burns stake from.
/// @dev Run by the eigenlayer setup container (BreadchainCoop/eigenlayer-bls-local
///      `main.sh`) after the middleware deployment. The stake threshold is
///      initialised fail-closed (`type(uint224).max`) and lowered by
///      `FinalizeECDSAStack` once all operators are registered and the total
///      weight is known.
///
///      Required env: PRIVATE_KEY, DELEGATION_MANAGER_ADDRESS,
///      AVS_DIRECTORY_ADDRESS, LST_STRATEGY_ADDRESS.
///      Optional env: REWARDS_COORDINATOR_ADDRESS, ALLOCATION_MANAGER_ADDRESS,
///      AVS_METADATA_URI.
///
///      Writes `script/deployments/ecdsa-stack/<chainid>.json` with an
///      `addresses` object, mergeable into `avs_deploy.json` via `jq -s '.[0] * .[1]'`.
contract DeployECDSAStack is Script {
    /// @notice The id of the slashable operator set (mirrored by the router's
    ///         `AVS_OPSET_ID` metadata and the `GasKillerSlasher`'s constructor arg).
    uint32 public constant OPERATOR_SET_ID = 0;

    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        address deployer = vm.addr(deployerKey);
        address delegationManager = vm.envAddress("DELEGATION_MANAGER_ADDRESS");
        address avsDirectory = vm.envAddress("AVS_DIRECTORY_ADDRESS");
        address strategy = vm.envAddress("LST_STRATEGY_ADDRESS");
        address rewardsCoordinator = vm.envOr("REWARDS_COORDINATOR_ADDRESS", address(0));
        address allocationManager = vm.envOr("ALLOCATION_MANAGER_ADDRESS", address(0));

        vm.startBroadcast(deployerKey);

        ECDSAStakeRegistry registry =
            new ECDSAStakeRegistry(IDelegationManager(delegationManager));
        GasKillerServiceManager serviceManager = new GasKillerServiceManager(
            avsDirectory,
            address(registry),
            rewardsCoordinator,
            delegationManager,
            allocationManager,
            deployer
        );

        IECDSAStakeRegistryTypes.Quorum memory quorum;
        quorum.strategies = new IECDSAStakeRegistryTypes.StrategyParams[](1);
        quorum.strategies[0] = IECDSAStakeRegistryTypes.StrategyParams({
            strategy: IStrategy(strategy),
            multiplier: 10_000
        });
        // Initialize FAIL-CLOSED: the threshold is checkpointed per block and the
        // real value cannot be computed until operators register (total weight is
        // 0 here), so `FinalizeECDSAStack` sets it afterward. Starting at 0 would
        // instead be fail-OPEN — `ECDSAStakeRegistry` reads the threshold at the
        // caller-chosen reference block, and `GasKillerSDK.verifyAndUpdate` accepts
        // reference blocks up to `blockStaleMeasure` (~300) old, so a reference
        // block in the [firstRegistration, finalize) window would let a single
        // operator satisfy the quorum. `type(uint224).max` (the registry checkpoints weights as uint224) makes every such block
        // reject (InsufficientSignedStake) until finalize installs the real
        // threshold; legitimate submissions always reference a recent block.
        registry.initialize(address(serviceManager), type(uint224).max, quorum);

        // Operator-set (slashing) bootstrap: register the AVS with the
        // AllocationManager and create the operator set whose allocated stake the
        // GasKillerSlasher burns on fraud. The service manager doubles as the
        // AVS's IAVSRegistrar, so operators can immediately register for the set
        // (via EnrollOperatorSlashing) once they hold an ECDSA signing key.
        if (allocationManager != address(0)) {
            serviceManager.updateAllocationManagerMetadataURI(
                vm.envOr("AVS_METADATA_URI", string("gas-killer-avs"))
            );
            IStrategy[] memory operatorSetStrategies = new IStrategy[](1);
            operatorSetStrategies[0] = IStrategy(strategy);
            serviceManager.createOperatorSet(OPERATOR_SET_ID, operatorSetStrategies);
        }

        vm.stopBroadcast();

        string memory inner =
            vm.serializeAddress("addresses", "ecdsaStakeRegistry", address(registry));
        inner = vm.serializeAddress(
            "addresses", "gasKillerServiceManager", address(serviceManager)
        );
        string memory output = vm.serializeString("root", "addresses", inner);
        string memory path = string.concat(
            vm.projectRoot(),
            "/script/deployments/ecdsa-stack/",
            vm.toString(block.chainid),
            ".json"
        );
        vm.writeJson(output, path);
    }
}

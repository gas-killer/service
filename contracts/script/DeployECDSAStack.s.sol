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
///         strategy the operators deposited into).
/// @dev Run by the eigenlayer setup container (BreadchainCoop/eigenlayer-bls-local
///      `main.sh`) after the middleware deployment. The stake threshold is
///      initialised to 0 and raised by `FinalizeECDSAStack` once all operators
///      are registered and the total weight is known.
///
///      Required env: PRIVATE_KEY, DELEGATION_MANAGER_ADDRESS,
///      AVS_DIRECTORY_ADDRESS, LST_STRATEGY_ADDRESS.
///      Optional env: REWARDS_COORDINATOR_ADDRESS, ALLOCATION_MANAGER_ADDRESS.
///
///      Writes `script/deployments/ecdsa-stack/<chainid>.json` with an
///      `addresses` object, mergeable into `avs_deploy.json` via `jq -s '.[0] * .[1]'`.
contract DeployECDSAStack is Script {
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
        // Threshold starts at 0; FinalizeECDSAStack raises it to the configured
        // fraction of total weight after operator registration.
        registry.initialize(address(serviceManager), 0, quorum);

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

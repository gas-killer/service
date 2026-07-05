// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {Script} from "forge-std/Script.sol";
import {console2} from "forge-std/console2.sol";

import {IAllocationManager} from "eigenlayer-contracts/src/contracts/interfaces/IAllocationManager.sol";
import {OperatorSet} from "eigenlayer-contracts/src/contracts/libraries/OperatorSetLib.sol";

import {GasKillerServiceManager} from "../src/GasKillerServiceManager.sol";
import {GasKillerSlasher} from "../src/GasKillerSlasher.sol";
import {SP1Verifier} from "../src/vendor/sp1/SP1VerifierGroth16.sol";

/// @title DeployGasKillerSlasher
/// @notice Deploys the `GasKillerSlasher` and appoints it (via the service
///         manager's PermissionController forward) to call
///         `AllocationManager.slashOperator` on the AVS's behalf — after this,
///         anyone holding a valid fraud proof can burn the signing operators'
///         allocated stake.
/// @dev Run after `DeployECDSAStack` (which creates the operator set). Must be
///      broadcast by the `GasKillerServiceManager` owner (the stack deployer).
///      When re-deploying (e.g. a new challenger vkey), revoke the retired
///      slasher first: `serviceManager.removeAppointee(oldSlasher,
///      allocationManager, IAllocationManager.slashOperator.selector)` — the
///      appointment does not expire on its own.
///
///      Required env: PRIVATE_KEY, GAS_KILLER_SERVICE_MANAGER_ADDRESS,
///      ECDSA_STAKE_REGISTRY_ADDRESS, ALLOCATION_MANAGER_ADDRESS,
///      HELIOS_ADDRESS, PROGRAM_VKEY (the challenger SP1 program's verification
///      key), CHAIN_CONFIG_HASH (the accepted chain-id + hardfork hash the
///      challenger program commits).
///      Optional env: SP1_VERIFIER_ADDRESS (defaults to deploying the vendored
///      SP1 Groth16 verifier), OPERATOR_SET_ID (defaults to 0), SLASHER_OWNER
///      (defaults to the deployer; may update the accepted chain-config hashes
///      after hardforks), MAX_REFERENCE_BLOCK_AGE (oldest accepted quorum
///      reference block, in blocks).
///
///      Writes `script/deployments/ecdsa-stack/<chainid>.slasher.json` with an
///      `addresses` object, mergeable into `avs_deploy.json` via `jq -s '.[0] * .[1]'`.
contract DeployGasKillerSlasher is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        address deployer = vm.addr(deployerKey);
        GasKillerServiceManager serviceManager =
            GasKillerServiceManager(vm.envAddress("GAS_KILLER_SERVICE_MANAGER_ADDRESS"));
        address ecdsaStakeRegistry = vm.envAddress("ECDSA_STAKE_REGISTRY_ADDRESS");
        address allocationManager = vm.envAddress("ALLOCATION_MANAGER_ADDRESS");
        address helios = vm.envAddress("HELIOS_ADDRESS");
        bytes32 programVKey = vm.envBytes32("PROGRAM_VKEY");
        bytes32 chainConfigHash = vm.envBytes32("CHAIN_CONFIG_HASH");
        address sp1Verifier = vm.envOr("SP1_VERIFIER_ADDRESS", address(0));
        uint32 operatorSetId = uint32(vm.envOr("OPERATOR_SET_ID", uint256(0)));
        address slasherOwner = vm.envOr("SLASHER_OWNER", deployer);
        // Oldest accepted reference block, in blocks. Default ~30 days at 12s/block; keep it
        // >= the AllocationManager's DEALLOCATION_DELAY so a deregistering fraudster stays
        // challengeable for as long as its stake remains burnable.
        uint32 maxReferenceBlockAge = uint32(vm.envOr("MAX_REFERENCE_BLOCK_AGE", uint256(216_000)));

        // Fail fast if DeployECDSAStack hasn't created the operator set (or the id
        // doesn't match): a slasher pointed at a nonexistent set could never burn stake.
        require(
            IAllocationManager(allocationManager)
                .isOperatorSet(OperatorSet({avs: address(serviceManager), id: operatorSetId})),
            "operator set does not exist for this AVS; run DeployECDSAStack first / check OPERATOR_SET_ID"
        );

        vm.startBroadcast(deployerKey);

        if (sp1Verifier == address(0)) {
            sp1Verifier = address(new SP1Verifier());
        }

        GasKillerSlasher slasher = new GasKillerSlasher(
            sp1Verifier,
            helios,
            ecdsaStakeRegistry,
            allocationManager,
            address(serviceManager),
            operatorSetId,
            programVKey,
            chainConfigHash,
            slasherOwner,
            maxReferenceBlockAge
        );

        // Grant the slasher `AllocationManager.slashOperator` on the AVS's
        // authority; the service manager is its own default PermissionController
        // admin, so its owner can appoint directly.
        serviceManager.setAppointee(address(slasher), allocationManager, IAllocationManager.slashOperator.selector);

        vm.stopBroadcast();

        console2.log("GasKillerSlasher deployed:", address(slasher));
        console2.log("SP1 verifier:", sp1Verifier);

        string memory inner = vm.serializeAddress("addresses", "gasKillerSlasher", address(slasher));
        inner = vm.serializeAddress("addresses", "sp1Verifier", sp1Verifier);
        string memory output = vm.serializeString("root", "addresses", inner);
        string memory path = string.concat(
            vm.projectRoot(), "/script/deployments/ecdsa-stack/", vm.toString(block.chainid), ".slasher.json"
        );
        vm.writeJson(output, path);
    }
}

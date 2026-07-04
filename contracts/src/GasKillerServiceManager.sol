// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {ECDSAServiceManagerBase} from "@eigenlayer-middleware/unaudited/ECDSAServiceManagerBase.sol";
import {ECDSAStakeRegistry} from "@eigenlayer-middleware/unaudited/ECDSAStakeRegistry.sol";
import {
    IAllocationManager,
    IAllocationManagerTypes
} from "eigenlayer-contracts/src/contracts/interfaces/IAllocationManager.sol";
import {IAVSRegistrar} from "eigenlayer-contracts/src/contracts/interfaces/IAVSRegistrar.sol";
import {IPermissionController} from
    "eigenlayer-contracts/src/contracts/interfaces/IPermissionController.sol";
import {IStrategy} from "eigenlayer-contracts/src/contracts/interfaces/IStrategy.sol";
import {PermissionControllerMixin} from
    "eigenlayer-contracts/src/contracts/mixins/PermissionControllerMixin.sol";

/// @title GasKillerServiceManager
/// @notice Concrete ECDSA service manager for the Gas Killer AVS, integrated with
///         EigenLayer operator sets so fraudulent commitments are slashable.
/// @dev Sits between the `ECDSAStakeRegistry` and EigenLayer core: the stake
///      registry calls `registerOperatorToAVS` / `deregisterOperatorFromAVS` here,
///      which forward to the `AVSDirectory`. Ownership is set in the constructor
///      (rather than an `initialize` call) so the contract is usable without a
///      proxy — `ECDSAServiceManagerBase`'s constructor disables initializers.
///
///      Slashing integration (all AllocationManager interactions use this contract's
///      address as the AVS identity):
///      - The owner creates the slashable operator set via `createOperatorSet` and
///        appoints the `GasKillerSlasher` for `AllocationManager.slashOperator` via
///        `setAppointee`, using this contract's default-admin rights on the
///        EigenLayer `PermissionController`.
///      - This contract is also the AVS's `IAVSRegistrar` (the AllocationManager
///        defaults the registrar to the AVS address): operators may only join the
///        slashable operator set after registering their signing key with the
///        `ECDSAStakeRegistry`, so the slashable set never diverges from the set
///        whose signatures `verifyAndUpdate` accepts.
contract GasKillerServiceManager is ECDSAServiceManagerBase, IAVSRegistrar {
    /// @notice Registration attempted for an AVS other than this contract.
    error UnsupportedAVS();
    /// @notice Operator-set registrar callbacks may only come from the AllocationManager.
    error OnlyAllocationManager();
    /// @notice Operator must register with the ECDSAStakeRegistry before joining the
    ///         slashable operator set.
    error OperatorNotRegisteredWithStakeRegistry();

    constructor(
        address _avsDirectory,
        address _stakeRegistry,
        address _rewardsCoordinator,
        address _delegationManager,
        address _allocationManager,
        address _initialOwner
    )
        ECDSAServiceManagerBase(
            _avsDirectory,
            _stakeRegistry,
            _rewardsCoordinator,
            _delegationManager,
            _allocationManager
        )
    {
        _transferOwnership(_initialOwner);
        _setRewardsInitiator(_initialOwner);
    }

    // ============ Operator-set lifecycle (owner) ============

    /// @notice Registers this AVS's metadata with the AllocationManager. Must be
    ///         called once before `createOperatorSet` (the AllocationManager rejects
    ///         operator-set creation for AVSs without registered metadata).
    /// @dev Distinct from `updateAVSMetadataURI`, which targets the legacy AVSDirectory.
    function updateAllocationManagerMetadataURI(string calldata metadataURI) external onlyOwner {
        IAllocationManager(allocationManager).updateAVSMetadataURI(address(this), metadataURI);
    }

    /// @notice Creates the operator set whose allocated stake backs Gas Killer
    ///         commitments (and is burned by the `GasKillerSlasher` on fraud).
    function createOperatorSet(uint32 operatorSetId, IStrategy[] calldata strategies)
        external
        onlyOwner
    {
        IAllocationManagerTypes.CreateSetParams[] memory params =
            new IAllocationManagerTypes.CreateSetParams[](1);
        params[0] = IAllocationManagerTypes.CreateSetParams({
            operatorSetId: operatorSetId,
            strategies: strategies
        });
        IAllocationManager(allocationManager).createOperatorSets(address(this), params);
    }

    /// @notice Ejects an operator from operator sets on the AVS's authority (operators
    ///         can always deregister themselves directly on the AllocationManager).
    /// @dev A deregistered operator stays slashable for the AllocationManager's
    ///      `DEALLOCATION_DELAY`, so ejection cannot be used to dodge a pending slash.
    function deregisterOperatorFromOperatorSets(address operator, uint32[] memory operatorSetIds)
        external
        onlyOwner
    {
        IAllocationManager(allocationManager).deregisterFromOperatorSets(
            IAllocationManagerTypes.DeregisterParams({
                operator: operator,
                avs: address(this),
                operatorSetIds: operatorSetIds
            })
        );
    }

    // ============ IAVSRegistrar ============

    /// @inheritdoc IAVSRegistrar
    /// @dev Called by the AllocationManager when an operator registers for this AVS's
    ///      operator sets. Only operators already registered with the ECDSAStakeRegistry
    ///      (i.e. with an attributable signing key) may become slashable members.
    function registerOperator(
        address operator,
        address avs,
        uint32[] calldata,
        bytes calldata
    ) external view {
        require(msg.sender == allocationManager, OnlyAllocationManager());
        require(avs == address(this), UnsupportedAVS());
        require(
            ECDSAStakeRegistry(stakeRegistry).operatorRegistered(operator),
            OperatorNotRegisteredWithStakeRegistry()
        );
    }

    /// @inheritdoc IAVSRegistrar
    function deregisterOperator(address, address avs, uint32[] calldata) external view {
        require(msg.sender == allocationManager, OnlyAllocationManager());
        require(avs == address(this), UnsupportedAVS());
    }

    /// @inheritdoc IAVSRegistrar
    function supportsAVS(address avs) external view returns (bool) {
        return avs == address(this);
    }

    // ============ Permission management (UAM) ============
    // This contract is its own default admin on the EigenLayer PermissionController;
    // the owner drives that authority through these forwards. `setAppointee` is how
    // the GasKillerSlasher is granted `AllocationManager.slashOperator`.

    function addPendingAdmin(address admin) external onlyOwner {
        _permissionController().addPendingAdmin(address(this), admin);
    }

    function removePendingAdmin(address pendingAdmin) external onlyOwner {
        _permissionController().removePendingAdmin(address(this), pendingAdmin);
    }

    function removeAdmin(address admin) external onlyOwner {
        _permissionController().removeAdmin(address(this), admin);
    }

    function setAppointee(address appointee, address target, bytes4 selector) external onlyOwner {
        _permissionController().setAppointee(address(this), appointee, target, selector);
    }

    function removeAppointee(address appointee, address target, bytes4 selector)
        external
        onlyOwner
    {
        _permissionController().removeAppointee(address(this), appointee, target, selector);
    }

    /// @dev The PermissionController is the one the AllocationManager checks
    ///      (`PermissionControllerMixin.permissionController`), read at call time so
    ///      the constructor keeps its pre-slashing signature.
    function _permissionController() internal view returns (IPermissionController) {
        return PermissionControllerMixin(allocationManager).permissionController();
    }
}

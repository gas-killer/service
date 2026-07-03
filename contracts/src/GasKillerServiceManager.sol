// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {ECDSAServiceManagerBase} from "@eigenlayer-middleware/unaudited/ECDSAServiceManagerBase.sol";

/// @title GasKillerServiceManager
/// @notice Minimal concrete ECDSA service manager for the Gas Killer AVS.
/// @dev Sits between the `ECDSAStakeRegistry` and EigenLayer core: the stake
///      registry calls `registerOperatorToAVS` / `deregisterOperatorFromAVS` here,
///      which forward to the `AVSDirectory`. Ownership is set in the constructor
///      (rather than an `initialize` call) so the contract is usable without a
///      proxy — `ECDSAServiceManagerBase`'s constructor disables initializers.
contract GasKillerServiceManager is ECDSAServiceManagerBase {
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

    // The permission-controller surface of IServiceManager is not used by this
    // AVS; the UAM (user access management) hooks are intentionally no-ops.

    function addPendingAdmin(address admin) external onlyOwner {}

    function removePendingAdmin(address pendingAdmin) external onlyOwner {}

    function removeAdmin(address admin) external onlyOwner {}

    function setAppointee(address appointee, address target, bytes4 selector) external onlyOwner {}

    function removeAppointee(address appointee, address target, bytes4 selector) external onlyOwner {}

    function deregisterOperatorFromOperatorSets(
        address operator,
        uint32[] memory operatorSetIds
    ) external {}
}

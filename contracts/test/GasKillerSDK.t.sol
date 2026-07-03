// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.13;

import {ArraySummation} from "../src/examples/array-summation/ArraySummation.sol";
import {ArraySummationFactory} from "../src/examples/array-summation/ArraySummationFactory.sol";
import {ECDSALib} from "../src/ECDSALib.sol";
import {IGasKillerSDK} from "../src/interface/IGasKillerSDK.sol";
import {IERC165} from "../src/interface/IERC165.sol";
import {StateUpdateType} from "../src/StateChangeHandlerLib.sol";

/// @notice Minimal subset of the Foundry cheatcode interface (keeps the project free of
///         a forge-std dependency; the cheatcode address is Foundry's well-known constant).
interface Vm {
    function sign(uint256 privateKey, bytes32 digest) external pure returns (uint8 v, bytes32 r, bytes32 s);
    function addr(uint256 privateKey) external pure returns (address);
    function prank(address sender) external;
    function expectRevert(bytes4 revertData) external;
    function expectRevert(bytes calldata revertData) external;
}

contract GasKillerSDKTest {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));

    /// @dev secp256k1 group order, for constructing malleable (high-s) signatures
    uint256 internal constant CURVE_ORDER =
        0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;

    address internal constant AVS = address(0xA11CE);
    address internal constant ADMIN = address(0xAD1119);

    ArraySummation internal target;

    /// @dev Operator private keys, index-aligned with `operators` (sorted by address ascending)
    uint256[] internal operatorKeys;
    address[] internal operators;

    function setUp() public {
        // Three operators, sorted by address so tests can build ordered signature lists.
        uint256[] memory keys = new uint256[](3);
        keys[0] = 0xA0;
        keys[1] = 0xB0;
        keys[2] = 0xC0;
        // Insertion sort by derived address.
        for (uint256 i = 1; i < keys.length; i++) {
            uint256 k = keys[i];
            uint256 j = i;
            while (j > 0 && vm.addr(keys[j - 1]) > vm.addr(k)) {
                keys[j] = keys[j - 1];
                j--;
            }
            keys[j] = k;
        }
        for (uint256 i = 0; i < keys.length; i++) {
            operatorKeys.push(keys[i]);
            operators.push(vm.addr(keys[i]));
        }

        target = new ArraySummation(AVS, ADMIN, operators, 10, 1000, 42);
    }

    // ---------------------------------------------------------------- helpers

    /// @dev ABI-encodes a single STORE update for `slot` = `value`
    function storeUpdate(bytes32 slot, bytes32 value) internal pure returns (bytes memory) {
        StateUpdateType[] memory types = new StateUpdateType[](1);
        types[0] = StateUpdateType.STORE;
        bytes[] memory args = new bytes[](1);
        args[0] = abi.encode(slot, value);
        return abi.encode(types, args);
    }

    function messageHash(uint256 transitionIndex, bytes4 targetFunction, bytes memory storageUpdates)
        internal
        view
        returns (bytes32)
    {
        return sha256(abi.encode(transitionIndex, address(target), targetFunction, storageUpdates));
    }

    /// @dev Signs `digest` with the first `count` operators (already address-ascending)
    function signQuorum(bytes32 digest, uint256 count) internal view returns (bytes[] memory sigs) {
        sigs = new bytes[](count);
        for (uint256 i = 0; i < count; i++) {
            (uint8 v, bytes32 r, bytes32 s) = vm.sign(operatorKeys[i], digest);
            sigs[i] = abi.encodePacked(r, s, v);
        }
    }

    /// @dev A STORE payload targeting `currentSum` (slot 0 of ArraySummation) plus its digest
    function currentSumPayload(uint256 newSum, uint256 transitionIndex)
        internal
        view
        returns (bytes memory updates, bytes32 digest)
    {
        updates = storeUpdate(bytes32(uint256(0)), bytes32(newSum));
        digest = messageHash(transitionIndex, ArraySummation.sum.selector, updates);
    }

    // ------------------------------------------------------------------ tests

    function test_verifyAndUpdate_appliesStorageUpdates_fullQuorum() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(1352, 0);
        bytes[] memory sigs = signQuorum(digest, 3);

        target.verifyAndUpdate(digest, updates, 0, ArraySummation.sum.selector, sigs);

        require(target.currentSum() == 1352, "currentSum not updated");
        require(target.stateTransitionCount() == 1, "transition count not incremented");
    }

    function test_verifyAndUpdate_twoOfThreeMeetsThreshold() public {
        // 2 * 100 >= 3 * 66 — two signers clear the 66% threshold with three operators.
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        bytes[] memory sigs = signQuorum(digest, 2);

        target.verifyAndUpdate(digest, updates, 0, ArraySummation.sum.selector, sigs);
        require(target.currentSum() == 7, "currentSum not updated");
    }

    function test_verifyAndUpdate_oneOfThreeBelowThreshold() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        bytes[] memory sigs = signQuorum(digest, 1);

        vm.expectRevert(IGasKillerSDK.InsufficientQuorumThreshold.selector);
        target.verifyAndUpdate(digest, updates, 0, ArraySummation.sum.selector, sigs);
    }

    function test_verifyAndUpdate_rejectsDuplicateSigner() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        bytes[] memory sigs = signQuorum(digest, 2);
        sigs[1] = sigs[0];

        vm.expectRevert(IGasKillerSDK.UnorderedSignatures.selector);
        target.verifyAndUpdate(digest, updates, 0, ArraySummation.sum.selector, sigs);
    }

    function test_verifyAndUpdate_rejectsDescendingOrder() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        bytes[] memory ordered = signQuorum(digest, 2);
        bytes[] memory sigs = new bytes[](2);
        sigs[0] = ordered[1];
        sigs[1] = ordered[0];

        vm.expectRevert(IGasKillerSDK.UnorderedSignatures.selector);
        target.verifyAndUpdate(digest, updates, 0, ArraySummation.sum.selector, sigs);
    }

    function test_verifyAndUpdate_rejectsNonOperatorSigner() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        uint256 strangerKey = 0xDEAD;
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(strangerKey, digest);
        bytes[] memory sigs = new bytes[](2);
        // Keep ascending order regardless of how the stranger's address compares.
        bytes memory strangerSig = abi.encodePacked(r, s, v);
        bytes[] memory opSig = signQuorum(digest, 1);
        if (vm.addr(strangerKey) < operators[0]) {
            sigs[0] = strangerSig;
            sigs[1] = opSig[0];
        } else {
            sigs[0] = opSig[0];
            sigs[1] = strangerSig;
        }

        vm.expectRevert(
            abi.encodeWithSelector(IGasKillerSDK.NotRegisteredOperator.selector, vm.addr(strangerKey))
        );
        target.verifyAndUpdate(digest, updates, 0, ArraySummation.sum.selector, sigs);
    }

    function test_verifyAndUpdate_rejectsWrongTransitionIndex() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 5);
        bytes[] memory sigs = signQuorum(digest, 3);

        vm.expectRevert(IGasKillerSDK.InvalidTransitionIndex.selector);
        target.verifyAndUpdate(digest, updates, 5, ArraySummation.sum.selector, sigs);
    }

    function test_verifyAndUpdate_rejectsWrongMsgHash() public {
        (bytes memory updates,) = currentSumPayload(7, 0);
        bytes32 wrong = keccak256("not the digest");
        bytes[] memory sigs = signQuorum(wrong, 3);

        vm.expectRevert(IGasKillerSDK.InvalidSignature.selector);
        target.verifyAndUpdate(wrong, updates, 0, ArraySummation.sum.selector, sigs);
    }

    function test_verifyAndUpdate_rejectsHighSMalleatedSignature() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        bytes[] memory sigs = signQuorum(digest, 3);

        // Malleate the first signature into its high-s twin (same recovered address
        // under a naive ecrecover, but non-canonical under EIP-2).
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(operatorKeys[0], digest);
        uint8 flippedV = v == 27 ? 28 : 27;
        bytes32 highS = bytes32(CURVE_ORDER - uint256(s));
        sigs[0] = abi.encodePacked(r, highS, flippedV);

        vm.expectRevert(ECDSALib.InvalidSignatureS.selector);
        target.verifyAndUpdate(digest, updates, 0, ArraySummation.sum.selector, sigs);
    }

    function test_verifyAndUpdate_rejectsBadSignatureLength() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        bytes[] memory sigs = new bytes[](1);
        sigs[0] = hex"deadbeef";

        vm.expectRevert(ECDSALib.InvalidSignatureLength.selector);
        target.verifyAndUpdate(digest, updates, 0, ArraySummation.sum.selector, sigs);
    }

    function test_verifyAndUpdate_sequentialTransitions() public {
        for (uint256 i = 0; i < 3; i++) {
            (bytes memory updates, bytes32 digest) = currentSumPayload(100 + i, i);
            target.verifyAndUpdate(digest, updates, i, ArraySummation.sum.selector, signQuorum(digest, 3));
            require(target.currentSum() == 100 + i, "currentSum not updated");
        }
        require(target.stateTransitionCount() == 3, "transition count mismatch");
    }

    function test_operatorRegistry_adminGating() public {
        address newOperator = address(0xBEEF);

        vm.expectRevert(IGasKillerSDK.NotOperatorAdmin.selector);
        target.registerOperator(newOperator);

        vm.prank(ADMIN);
        target.registerOperator(newOperator);
        require(target.isOperator(newOperator), "operator not registered");
        require(target.operatorCount() == 4, "count not incremented");

        vm.prank(ADMIN);
        vm.expectRevert(abi.encodeWithSelector(IGasKillerSDK.InvalidOperator.selector, newOperator));
        target.registerOperator(newOperator);

        vm.expectRevert(IGasKillerSDK.NotOperatorAdmin.selector);
        target.deregisterOperator(newOperator);

        vm.prank(ADMIN);
        target.deregisterOperator(newOperator);
        require(!target.isOperator(newOperator), "operator not deregistered");
        require(target.operatorCount() == 3, "count not decremented");
    }

    function test_thresholdTracksRegistrySize() public {
        // Register a 4th operator: 2-of-4 (200 < 264) must fail, 3-of-4 (300 >= 264) passes.
        vm.prank(ADMIN);
        target.registerOperator(address(0xBEEF));

        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);

        bytes[] memory two = signQuorum(digest, 2);
        vm.expectRevert(IGasKillerSDK.InsufficientQuorumThreshold.selector);
        target.verifyAndUpdate(digest, updates, 0, ArraySummation.sum.selector, two);

        bytes[] memory three = signQuorum(digest, 3);
        target.verifyAndUpdate(digest, updates, 0, ArraySummation.sum.selector, three);
        require(target.currentSum() == 7, "currentSum not updated");
    }

    /// @dev CROSS-LANGUAGE PARITY ANCHOR. The signature below was produced by the
    ///      Rust signer (`common/src/ecdsa`, see `common/examples/parity_fixture.rs`)
    ///      for private key 0x...01 over sha256("gas-killer cross-language parity").
    ///      If this test breaks, Rust-signed certificates will no longer verify in
    ///      `verifyAndUpdate`. Regenerate the fixture with:
    ///      `cargo run -p gas-killer-common --example parity_fixture`
    function test_rustSignatureParity() public pure {
        bytes32 digest = 0xa6908bd8562fc968b4956ad73f3f5c2cdb5bccd639df156fb03336754b5cda37;
        bytes memory rustSignature =
            hex"296567bffde6bc687fedf35041dce68d8acd271af425e3b2c947c858937156901a74e3045c236731752bf6e55437febb540014ba1d850592c0755f62949f16fd1c";
        address expected = 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf; // address of key 0x...01

        require(sha256("gas-killer cross-language parity") == digest, "digest preimage mismatch");
        require(ECDSALib.recover(digest, rustSignature) == expected, "Rust signature must ecrecover");
    }

    function test_supportsInterface() public view {
        require(target.supportsInterface(type(IERC165).interfaceId), "IERC165 not supported");
        require(target.supportsInterface(type(IGasKillerSDK).interfaceId), "IGasKillerSDK not supported");
        require(!target.supportsInterface(0xffffffff), "0xffffffff must be unsupported");
    }

    function test_getMessageHash_parity() public view {
        bytes memory updates = storeUpdate(bytes32(uint256(0)), bytes32(uint256(99)));
        bytes32 expected = sha256(abi.encode(uint256(0), address(target), ArraySummation.sum.selector, updates));
        require(
            target.getMessageHash(0, ArraySummation.sum.selector, updates) == expected,
            "getMessageHash parity broken"
        );
    }

    function test_factory_deploysAndTracks() public {
        ArraySummationFactory factory = new ArraySummationFactory();
        address deployed = factory.deployArraySummation(AVS, ADMIN, operators, 5, 100, 1);

        require(factory.getDeployedContractCount() == 1, "count mismatch");
        require(factory.isContractDeployedByFactory(deployed), "membership missing");
        require(factory.deployedContracts(0) == deployed, "list mismatch");

        ArraySummation instance = ArraySummation(deployed);
        require(instance.operatorCount() == 3, "operators not registered");
        require(instance.isOperator(operators[0]), "operator missing");
        require(instance.avsAddress() == AVS, "avs mismatch");
        require(instance.operatorAdmin() == ADMIN, "admin mismatch");
    }
}

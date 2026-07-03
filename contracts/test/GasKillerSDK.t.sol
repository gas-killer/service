// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {ArraySummation} from "../src/examples/array-summation/ArraySummation.sol";
import {ArraySummationFactory} from "../src/examples/array-summation/ArraySummationFactory.sol";
import {GasKillerServiceManager} from "../src/GasKillerServiceManager.sol";
import {IGasKillerSDK} from "../src/interface/IGasKillerSDK.sol";
import {IERC165} from "../src/interface/IERC165.sol";
import {StateUpdateType} from "../src/StateChangeHandlerLib.sol";

import {ECDSAStakeRegistry} from "@eigenlayer-middleware/unaudited/ECDSAStakeRegistry.sol";
import {
    IECDSAStakeRegistryErrors,
    IECDSAStakeRegistryTypes
} from "@eigenlayer-middleware/interfaces/IECDSAStakeRegistry.sol";
import {IStrategy} from "eigenlayer-contracts/src/contracts/interfaces/IStrategy.sol";
import {IDelegationManager} from
    "eigenlayer-contracts/src/contracts/interfaces/IDelegationManager.sol";
import {ISignatureUtilsMixinTypes} from
    "eigenlayer-contracts/src/contracts/interfaces/ISignatureUtilsMixin.sol";

/// @notice Minimal subset of the Foundry cheatcode interface (keeps the project free of
///         a forge-std dependency; the cheatcode address is Foundry's well-known constant).
interface Vm {
    function sign(uint256 privateKey, bytes32 digest) external pure returns (uint8 v, bytes32 r, bytes32 s);
    function addr(uint256 privateKey) external pure returns (address);
    function prank(address sender) external;
    function roll(uint256 newHeight) external;
    function expectRevert(bytes4 revertData) external;
    function expectRevert(bytes calldata revertData) external;
}

/// @notice DelegationManager stand-in: only `getOperatorShares` is consulted by
///         `ECDSAStakeRegistry.getOperatorWeight`.
contract MockDelegationManager {
    mapping(address => uint256) public shares;

    function setShares(address operator, uint256 value) external {
        shares[operator] = value;
    }

    function getOperatorShares(
        address operator,
        IStrategy[] memory strategies
    ) external view returns (uint256[] memory) {
        uint256[] memory result = new uint256[](strategies.length);
        for (uint256 i = 0; i < strategies.length; i++) {
            result[i] = shares[operator];
        }
        return result;
    }
}

/// @notice AVSDirectory stand-in: accepts any operator registration (the real
///         directory validates the operator's EIP-712 registration signature).
contract MockAVSDirectory {
    function registerOperatorToAVS(
        address,
        ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry memory
    ) external {}

    function deregisterOperatorFromAVS(address) external {}
}

contract GasKillerSDKTest {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));

    /// @dev secp256k1 group order, for constructing malleable (high-s) signatures
    uint256 internal constant CURVE_ORDER =
        0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;

    uint256 internal constant OPERATOR_SHARES = 100 ether;
    /// @dev 2-of-3 equal-weight quorum: threshold equals two operators' weight
    uint256 internal constant THRESHOLD_WEIGHT = 200 ether;

    MockDelegationManager internal delegation;
    MockAVSDirectory internal avsDirectory;
    ECDSAStakeRegistry internal registry;
    GasKillerServiceManager internal serviceManager;
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

        // EigenLayer stack: mocked core (delegation manager + AVS directory),
        // real ECDSAStakeRegistry, and the concrete Gas Killer service manager.
        delegation = new MockDelegationManager();
        avsDirectory = new MockAVSDirectory();
        registry = new ECDSAStakeRegistry(IDelegationManager(address(delegation)));
        serviceManager = new GasKillerServiceManager(
            address(avsDirectory),
            address(registry),
            address(0),
            address(delegation),
            address(0),
            address(this)
        );
        registry.initialize(address(serviceManager), THRESHOLD_WEIGHT, singleStrategyQuorum());

        // Register the operators with signing key == operator address (the same
        // setup the e2e deploy performs against the real AVSDirectory).
        for (uint256 i = 0; i < operators.length; i++) {
            delegation.setShares(operators[i], OPERATOR_SHARES);
            vm.prank(operators[i]);
            registry.registerOperatorWithSignature(emptyOperatorSignature(), operators[i]);
        }

        target = new ArraySummation(address(serviceManager), address(registry), 10, 1000, 42);

        // Reference blocks must be strictly below block.number and at/after the
        // registration checkpoints.
        vm.roll(block.number + 10);
    }

    // ---------------------------------------------------------------- helpers

    function singleStrategyQuorum() internal pure returns (IECDSAStakeRegistryTypes.Quorum memory q) {
        q.strategies = new IECDSAStakeRegistryTypes.StrategyParams[](1);
        q.strategies[0] = IECDSAStakeRegistryTypes.StrategyParams({
            strategy: IStrategy(address(0x1)),
            multiplier: 10_000
        });
    }

    function emptyOperatorSignature()
        internal
        view
        returns (ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry memory)
    {
        // The mock AVS directory accepts any registration payload.
        return ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry({
            signature: "",
            salt: bytes32(0),
            expiry: block.timestamp + 1 days
        });
    }

    function referenceBlock() internal view returns (uint32) {
        return uint32(block.number - 1);
    }

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
    function signQuorum(bytes32 digest, uint256 count)
        internal
        view
        returns (address[] memory signers, bytes[] memory sigs)
    {
        signers = new address[](count);
        sigs = new bytes[](count);
        for (uint256 i = 0; i < count; i++) {
            (uint8 v, bytes32 r, bytes32 s) = vm.sign(operatorKeys[i], digest);
            signers[i] = operators[i];
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
        (address[] memory signers, bytes[] memory sigs) = signQuorum(digest, 3);

        target.verifyAndUpdate(
            digest, referenceBlock(), updates, 0, ArraySummation.sum.selector, signers, sigs
        );

        require(target.currentSum() == 1352, "currentSum not updated");
        require(target.stateTransitionCount() == 1, "transition count not incremented");
    }

    function test_verifyAndUpdate_twoOfThreeMeetsThreshold() public {
        // 200 ether signed weight == 200 ether threshold.
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        (address[] memory signers, bytes[] memory sigs) = signQuorum(digest, 2);

        target.verifyAndUpdate(
            digest, referenceBlock(), updates, 0, ArraySummation.sum.selector, signers, sigs
        );
        require(target.currentSum() == 7, "currentSum not updated");
    }

    function test_verifyAndUpdate_oneOfThreeBelowThreshold() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        (address[] memory signers, bytes[] memory sigs) = signQuorum(digest, 1);

        vm.expectRevert(IECDSAStakeRegistryErrors.InsufficientSignedStake.selector);
        target.verifyAndUpdate(
            digest, referenceBlock(), updates, 0, ArraySummation.sum.selector, signers, sigs
        );
    }

    function test_verifyAndUpdate_rejectsDuplicateSigner() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        (address[] memory signers, bytes[] memory sigs) = signQuorum(digest, 2);
        signers[1] = signers[0];
        sigs[1] = sigs[0];

        vm.expectRevert(IECDSAStakeRegistryErrors.NotSorted.selector);
        target.verifyAndUpdate(
            digest, referenceBlock(), updates, 0, ArraySummation.sum.selector, signers, sigs
        );
    }

    function test_verifyAndUpdate_rejectsDescendingOrder() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        (address[] memory ordered, bytes[] memory orderedSigs) = signQuorum(digest, 2);
        address[] memory signers = new address[](2);
        bytes[] memory sigs = new bytes[](2);
        signers[0] = ordered[1];
        signers[1] = ordered[0];
        sigs[0] = orderedSigs[1];
        sigs[1] = orderedSigs[0];

        vm.expectRevert(IECDSAStakeRegistryErrors.NotSorted.selector);
        target.verifyAndUpdate(
            digest, referenceBlock(), updates, 0, ArraySummation.sum.selector, signers, sigs
        );
    }

    function test_verifyAndUpdate_rejectsNonOperatorSigner() public {
        // A signer that never registered has no signing-key checkpoint, so the
        // registry resolves its signing key to address(0) and rejects the
        // signature.
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        uint256 strangerKey = 0xDEAD;
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(strangerKey, digest);

        address[] memory signers = new address[](2);
        bytes[] memory sigs = new bytes[](2);
        (address[] memory opSigners, bytes[] memory opSigs) = signQuorum(digest, 1);
        if (vm.addr(strangerKey) < opSigners[0]) {
            signers[0] = vm.addr(strangerKey);
            sigs[0] = abi.encodePacked(r, s, v);
            signers[1] = opSigners[0];
            sigs[1] = opSigs[0];
        } else {
            signers[0] = opSigners[0];
            sigs[0] = opSigs[0];
            signers[1] = vm.addr(strangerKey);
            sigs[1] = abi.encodePacked(r, s, v);
        }

        vm.expectRevert(IECDSAStakeRegistryErrors.InvalidSignature.selector);
        target.verifyAndUpdate(
            digest, referenceBlock(), updates, 0, ArraySummation.sum.selector, signers, sigs
        );
    }

    function test_verifyAndUpdate_rejectsWrongTransitionIndex() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 5);
        (address[] memory signers, bytes[] memory sigs) = signQuorum(digest, 3);

        vm.expectRevert(IGasKillerSDK.InvalidTransitionIndex.selector);
        target.verifyAndUpdate(
            digest, referenceBlock(), updates, 5, ArraySummation.sum.selector, signers, sigs
        );
    }

    function test_verifyAndUpdate_rejectsWrongMsgHash() public {
        (bytes memory updates,) = currentSumPayload(7, 0);
        bytes32 wrong = keccak256("not the digest");
        (address[] memory signers, bytes[] memory sigs) = signQuorum(wrong, 3);

        vm.expectRevert(IGasKillerSDK.InvalidSignature.selector);
        target.verifyAndUpdate(
            wrong, referenceBlock(), updates, 0, ArraySummation.sum.selector, signers, sigs
        );
    }

    function test_verifyAndUpdate_rejectsHighSMalleatedSignature() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        (address[] memory signers, bytes[] memory sigs) = signQuorum(digest, 3);

        // Malleate the first signature into its high-s twin: OZ's ECDSA rejects
        // it, so the registry reports an invalid signature.
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(operatorKeys[0], digest);
        uint8 flippedV = v == 27 ? 28 : 27;
        bytes32 highS = bytes32(CURVE_ORDER - uint256(s));
        sigs[0] = abi.encodePacked(r, highS, flippedV);

        vm.expectRevert(IECDSAStakeRegistryErrors.InvalidSignature.selector);
        target.verifyAndUpdate(
            digest, referenceBlock(), updates, 0, ArraySummation.sum.selector, signers, sigs
        );
    }

    function test_verifyAndUpdate_rejectsFutureReferenceBlock() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        (address[] memory signers, bytes[] memory sigs) = signQuorum(digest, 3);

        vm.expectRevert(IGasKillerSDK.FutureBlockNumber.selector);
        target.verifyAndUpdate(
            digest, uint32(block.number), updates, 0, ArraySummation.sum.selector, signers, sigs
        );
    }

    function test_verifyAndUpdate_rejectsStaleReferenceBlock() public {
        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        (address[] memory signers, bytes[] memory sigs) = signQuorum(digest, 3);
        uint32 stale = referenceBlock();

        vm.roll(block.number + target.blockStaleMeasure() + 1);

        vm.expectRevert(IGasKillerSDK.StaleBlockNumber.selector);
        target.verifyAndUpdate(
            digest, stale, updates, 0, ArraySummation.sum.selector, signers, sigs
        );
    }

    function test_verifyAndUpdate_sequentialTransitions() public {
        for (uint256 i = 0; i < 3; i++) {
            (bytes memory updates, bytes32 digest) = currentSumPayload(100 + i, i);
            (address[] memory signers, bytes[] memory sigs) = signQuorum(digest, 3);
            target.verifyAndUpdate(
                digest, referenceBlock(), updates, i, ArraySummation.sum.selector, signers, sigs
            );
            require(target.currentSum() == 100 + i, "currentSum not updated");
        }
        require(target.stateTransitionCount() == 3, "transition count mismatch");
    }

    function test_thresholdTracksStakeRegistry() public {
        // Raising the registry threshold above two operators' combined weight
        // makes the previously sufficient 2-of-3 quorum fail.
        registry.updateStakeThreshold(THRESHOLD_WEIGHT + 1);
        vm.roll(block.number + 1);

        (bytes memory updates, bytes32 digest) = currentSumPayload(7, 0);
        (address[] memory signers, bytes[] memory sigs) = signQuorum(digest, 2);

        vm.expectRevert(IECDSAStakeRegistryErrors.InsufficientSignedStake.selector);
        target.verifyAndUpdate(
            digest, referenceBlock(), updates, 0, ArraySummation.sum.selector, signers, sigs
        );

        (address[] memory allSigners, bytes[] memory allSigs) = signQuorum(digest, 3);
        target.verifyAndUpdate(
            digest, referenceBlock(), updates, 0, ArraySummation.sum.selector, allSigners, allSigs
        );
        require(target.currentSum() == 7, "currentSum not updated");
    }

    /// @dev CROSS-LANGUAGE PARITY ANCHOR. The signature below was produced by the
    ///      Rust signer (`common/src/ecdsa`, see `common/examples/parity_fixture.rs`)
    ///      for private key 0x...01 over sha256("gas-killer cross-language parity").
    ///      It must verify through the REAL EigenLayer ECDSAStakeRegistry path
    ///      (OZ SignatureChecker). If this test breaks, Rust-signed certificates
    ///      will no longer verify on-chain. Regenerate the fixture with:
    ///      `cargo run -p gas-killer-common --example parity_fixture`
    function test_rustSignatureParity_throughStakeRegistry() public {
        bytes32 digest = 0xa6908bd8562fc968b4956ad73f3f5c2cdb5bccd639df156fb03336754b5cda37;
        bytes memory rustSignature =
            hex"296567bffde6bc687fedf35041dce68d8acd271af425e3b2c947c858937156901a74e3045c236731752bf6e55437febb540014ba1d850592c0755f62949f16fd1c";
        address rustSigner = 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf; // address of key 0x...01
        require(sha256("gas-killer cross-language parity") == digest, "digest preimage mismatch");

        // Register the Rust signer as the sole operator of a fresh registry.
        ECDSAStakeRegistry parityRegistry =
            new ECDSAStakeRegistry(IDelegationManager(address(delegation)));
        GasKillerServiceManager paritySm = new GasKillerServiceManager(
            address(avsDirectory),
            address(parityRegistry),
            address(0),
            address(delegation),
            address(0),
            address(this)
        );
        parityRegistry.initialize(address(paritySm), OPERATOR_SHARES, singleStrategyQuorum());
        delegation.setShares(rustSigner, OPERATOR_SHARES);
        vm.prank(rustSigner);
        parityRegistry.registerOperatorWithSignature(emptyOperatorSignature(), rustSigner);
        vm.roll(block.number + 1);

        address[] memory signers = new address[](1);
        signers[0] = rustSigner;
        bytes[] memory sigs = new bytes[](1);
        sigs[0] = rustSignature;

        bytes4 magic = parityRegistry.isValidSignature(
            digest, abi.encode(signers, sigs, uint32(block.number - 1))
        );
        require(magic == bytes4(0x1626ba7e), "Rust signature must verify via ECDSAStakeRegistry");
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
        address deployed =
            factory.deployArraySummation(address(serviceManager), address(registry), 5, 100, 1);

        require(factory.getDeployedContractCount() == 1, "count mismatch");
        require(factory.isContractDeployedByFactory(deployed), "membership missing");
        require(factory.deployedContracts(0) == deployed, "list mismatch");

        ArraySummation instance = ArraySummation(deployed);
        require(instance.ecdsaStakeRegistry() == address(registry), "registry mismatch");
        require(instance.avsAddress() == address(serviceManager), "avs mismatch");
    }
}

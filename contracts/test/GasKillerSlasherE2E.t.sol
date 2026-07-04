// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

import {Test} from "forge-std/Test.sol";
import {stdJson} from "forge-std/StdJson.sol";

import {GasKillerSlasher} from "../src/GasKillerSlasher.sol";
import {IGasKillerSlasher} from "../src/interface/IGasKillerSlasher.sol";
import {IHeliosLightClient} from "../src/interface/IHeliosLightClient.sol";
import {GasKillerServiceManager} from "../src/GasKillerServiceManager.sol";
import {SP1Verifier} from "../src/vendor/sp1/SP1VerifierGroth16.sol";

import {ECDSAStakeRegistry} from "@eigenlayer-middleware/unaudited/ECDSAStakeRegistry.sol";
import {IECDSAStakeRegistryTypes} from "@eigenlayer-middleware/interfaces/IECDSAStakeRegistry.sol";
import {IStrategy} from "eigenlayer-contracts/src/contracts/interfaces/IStrategy.sol";
import {IDelegationManager} from
    "eigenlayer-contracts/src/contracts/interfaces/IDelegationManager.sol";
import {IAllocationManager} from
    "eigenlayer-contracts/src/contracts/interfaces/IAllocationManager.sol";
import {OperatorSet} from "eigenlayer-contracts/src/contracts/libraries/OperatorSetLib.sol";
import {ISignatureUtilsMixinTypes} from
    "eigenlayer-contracts/src/contracts/interfaces/ISignatureUtilsMixin.sol";

// ============ Test doubles ============
// The proof, its verification, the ECDSA quorum, and the fraud detection are all REAL. Only the
// off-chain-anchored Helios light client and the AllocationManager stake-burn are doubled: the
// former needs a synced light client, the latter is EigenLayer's own audited primitive. The
// AllocationManager double implements the exact `slashOperator` interface the slasher calls and
// records every call so the test can assert the slasher attributes the slash correctly.

/// @notice DelegationManager stand-in: only `getOperatorShares` is consulted by the registry.
contract MockDelegationManager {
    mapping(address => uint256) public shares;

    function setShares(address operator, uint256 value) external {
        shares[operator] = value;
    }

    function getOperatorShares(address operator, IStrategy[] memory strategies)
        external
        view
        returns (uint256[] memory)
    {
        uint256[] memory result = new uint256[](strategies.length);
        for (uint256 i = 0; i < strategies.length; i++) {
            result[i] = shares[operator];
        }
        return result;
    }
}

/// @notice AVS directory stand-in: accepts any operator registration.
contract MockAVSDirectory {
    function registerOperatorToAVS(address, ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry memory)
        external
    {}

    function deregisterOperatorFromAVS(address) external {}
}

/// @notice Placeholder strategy; only its address is passed around.
contract MockStrategy {}

/// @notice Helios stand-in: validates explicitly whitelisted block hashes.
contract MockHeliosLightClient is IHeliosLightClient {
    mapping(bytes32 => bool) public valid;

    function setValid(bytes32 blockHash, bool v) external {
        valid[blockHash] = v;
    }

    function isBlockHashValid(bytes32 blockHash) external view returns (bool) {
        return valid[blockHash];
    }

    function getBlockHash(uint256) external pure returns (bytes32) {
        return bytes32(0);
    }
}

/// @notice Recording AllocationManager: implements the exact methods the slasher calls and records
///         every `slashOperator` call so the test asserts the attributed operators and params.
contract RecordingAllocationManager {
    IStrategy[] internal _strategies;
    address[] public slashedOperators;
    uint256[] public slashedWads;
    uint32 public lastOperatorSetId;
    address public lastAvs;

    constructor(IStrategy strategy) {
        _strategies.push(strategy);
    }

    function getStrategiesInOperatorSet(OperatorSet memory) external view returns (IStrategy[] memory) {
        return _strategies;
    }

    function isOperatorSlashable(address, OperatorSet memory) external pure returns (bool) {
        return true;
    }

    function slashOperator(address avs, IAllocationManager.SlashingParams calldata params)
        external
        returns (uint256, uint256[] memory)
    {
        lastAvs = avs;
        lastOperatorSetId = params.operatorSetId;
        slashedOperators.push(params.operator);
        require(params.wadsToSlash.length == _strategies.length, "wads/strategies length");
        slashedWads.push(params.wadsToSlash[0]);
        uint256[] memory shares = new uint256[](params.strategies.length);
        return (slashedOperators.length, shares);
    }

    function slashCount() external view returns (uint256) {
        return slashedOperators.length;
    }
}

/// @title GasKillerSlasherE2ETest
/// @notice End-to-end slashing test on the ECDSA path: a real 3-operator ECDSA quorum signs a
///         FRAUDULENT commitment, a challenger submits a REAL SP1 Groth16 proof of the correct
///         execution, and the slasher — verifying the quorum via the same `ECDSAStakeRegistry`
///         the AVS's `verifyAndUpdate` trusts, then the proof, then the storage mismatch — slashes
///         every signing operator through the AllocationManager.
/// @dev Requires a proof fixture at `test/fixtures/gas-killer-fixture.json` produced by the Gas
///      Killer challenger host. When absent, all tests skip so the suite stays green.
contract GasKillerSlasherE2ETest is Test {
    using stdJson for string;

    struct Fixture {
        bytes32 anchorHash;
        bytes32 chainConfigHash;
        address callerAddress;
        address contractAddress;
        bytes contractCalldata;
        bytes storageUpdates;
        bytes32 vkey;
        bytes publicValues;
        bytes proof;
    }

    /// @dev Path to the proof fixture; overridden by subclasses to run the same end-to-end
    ///      slashing flow against a different challenger proof (e.g. the ArraySummation demo).
    function _fixturePath() internal pure virtual returns (string memory) {
        return "test/fixtures/gas-killer-fixture.json";
    }

    uint256 internal constant OPERATOR_SHARES = 100 ether;
    /// @dev 2-of-3 equal-weight quorum threshold
    uint256 internal constant THRESHOLD_WEIGHT = 200 ether;
    uint32 internal constant OPERATOR_SET_ID = 0;

    Fixture internal fixture;

    MockDelegationManager internal delegation;
    MockAVSDirectory internal avsDirectory;
    ECDSAStakeRegistry internal registry;
    GasKillerServiceManager internal serviceManager;
    SP1Verifier internal sp1Verifier;
    MockHeliosLightClient internal helios;
    RecordingAllocationManager internal allocationManager;
    GasKillerSlasher internal slasher;

    uint256[] internal operatorKeys;
    address[] internal operators;

    address internal challenger = makeAddr("challenger");

    function setUp() public {
        string memory json;
        try vm.readFile(_fixturePath()) returns (string memory contents) {
            json = contents;
        } catch {
            vm.skip(true, "missing test/fixtures/gas-killer-fixture.json (or fs_permissions denies it)");
            return;
        }

        fixture.anchorHash = json.readBytes32(".anchorHash");
        fixture.chainConfigHash = json.readBytes32(".chainConfigHash");
        fixture.callerAddress = json.readAddress(".callerAddress");
        fixture.contractAddress = json.readAddress(".contractAddress");
        fixture.contractCalldata = json.readBytes(".contractCalldata");
        fixture.storageUpdates = json.readBytes(".storageUpdates");
        fixture.vkey = json.readBytes32(".vkey");
        fixture.publicValues = json.readBytes(".publicValues");
        fixture.proof = json.readBytes(".proof");

        // Three operators, sorted by address so the quorum signature list is ascending.
        uint256[] memory keys = new uint256[](3);
        keys[0] = 0xA0;
        keys[1] = 0xB0;
        keys[2] = 0xC0;
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

        // Real EigenLayer-ECDSA quorum: mocked core, real ECDSAStakeRegistry + service manager.
        delegation = new MockDelegationManager();
        avsDirectory = new MockAVSDirectory();
        registry = new ECDSAStakeRegistry(IDelegationManager(address(delegation)));
        serviceManager = new GasKillerServiceManager(
            address(avsDirectory), address(registry), address(0), address(delegation), address(0), address(this)
        );
        registry.initialize(address(serviceManager), THRESHOLD_WEIGHT, _singleStrategyQuorum());
        for (uint256 i = 0; i < operators.length; i++) {
            delegation.setShares(operators[i], OPERATOR_SHARES);
            vm.prank(operators[i]);
            registry.registerOperatorWithSignature(_emptyOperatorSignature(), operators[i]);
        }

        // Real SP1 Groth16 verifier + real proof; doubled Helios + AllocationManager.
        sp1Verifier = new SP1Verifier();
        helios = new MockHeliosLightClient();
        helios.setValid(fixture.anchorHash, true);
        allocationManager = new RecordingAllocationManager(IStrategy(address(new MockStrategy())));

        slasher = new GasKillerSlasher(
            address(sp1Verifier),
            address(helios),
            address(registry),
            address(allocationManager),
            address(serviceManager),
            OPERATOR_SET_ID,
            fixture.vkey,
            fixture.chainConfigHash,
            address(this)
        );

        vm.roll(block.number + 10);
    }

    // ============ Tests ============

    /// @notice A fraudulent commitment (storage updates ≠ the proven ones) that the quorum really
    ///         ECDSA-signed is slashed: the real proof verifies and all three signers are slashed.
    function test_e2e_fraudSlashesSigningOperators() public {
        // The operators sign a commitment with tampered storage updates — everything else matches
        // the fixture's proven execution, so the challenge is valid and the storage mismatch is fraud.
        IGasKillerSlasher.SignedCommitment memory commitment = _fraudCommitment();
        bytes32 commitmentHash = slasher.computeCommitmentHash(commitment);
        (address[] memory signers, bytes[] memory sigs) = _signQuorum(commitmentHash, 3);

        vm.prank(challenger);
        slasher.slash(commitment, _referenceBlock(), signers, sigs, fixture.proof, fixture.publicValues);

        assertTrue(slasher.isSlashed(commitmentHash), "commitment not marked slashed");
        assertEq(allocationManager.slashCount(), 3, "expected all three signers slashed");
        assertEq(allocationManager.slashedOperators(0), operators[0]);
        assertEq(allocationManager.slashedOperators(1), operators[1]);
        assertEq(allocationManager.slashedOperators(2), operators[2]);
        assertEq(allocationManager.slashedWads(0), slasher.FULL_SLASH_WAD());
        assertEq(allocationManager.lastAvs(), address(serviceManager), "slashed on behalf of the AVS");
        assertEq(allocationManager.lastOperatorSetId(), OPERATOR_SET_ID);
    }

    /// @notice An honest commitment (storage updates == the proven ones) is not fraud.
    function test_e2e_honestCommitmentNotSlashed() public {
        IGasKillerSlasher.SignedCommitment memory commitment = _fraudCommitment();
        commitment.storageUpdates = fixture.storageUpdates; // matches the proof → no fraud
        bytes32 commitmentHash = slasher.computeCommitmentHash(commitment);
        (address[] memory signers, bytes[] memory sigs) = _signQuorum(commitmentHash, 3);

        vm.prank(challenger);
        vm.expectRevert(IGasKillerSlasher.NoFraudDetected.selector);
        slasher.slash(commitment, _referenceBlock(), signers, sigs, fixture.proof, fixture.publicValues);
    }

    /// @notice A commitment the quorum never signed cannot be slashed, even if fraudulent: the
    ///         ECDSA registry rejects a below-threshold signer set.
    function test_e2e_belowQuorumRejected() public {
        IGasKillerSlasher.SignedCommitment memory commitment = _fraudCommitment();
        bytes32 commitmentHash = slasher.computeCommitmentHash(commitment);
        (address[] memory signers, bytes[] memory sigs) = _signQuorum(commitmentHash, 1); // 100 < 200

        vm.prank(challenger);
        vm.expectRevert(); // ECDSAStakeRegistry: InsufficientSignedStake
        slasher.slash(commitment, _referenceBlock(), signers, sigs, fixture.proof, fixture.publicValues);
    }

    /// @notice A corrupted Groth16 proof is rejected by the real verifier.
    function test_e2e_invalidProofRejected() public {
        IGasKillerSlasher.SignedCommitment memory commitment = _fraudCommitment();
        bytes32 commitmentHash = slasher.computeCommitmentHash(commitment);
        (address[] memory signers, bytes[] memory sigs) = _signQuorum(commitmentHash, 3);

        bytes memory badProof = bytes.concat(fixture.proof);
        badProof[badProof.length - 1] ^= 0x01;

        vm.prank(challenger);
        vm.expectRevert(IGasKillerSlasher.InvalidProof.selector);
        slasher.slash(commitment, _referenceBlock(), signers, sigs, badProof, fixture.publicValues);
    }

    /// @notice A commitment whose calldata differs from the proven execution is not challengeable.
    function test_e2e_calldataMismatchRejected() public {
        IGasKillerSlasher.SignedCommitment memory commitment = _fraudCommitment();
        commitment.contractCalldata = bytes.concat(fixture.contractCalldata, hex"ff");
        bytes32 commitmentHash = slasher.computeCommitmentHash(commitment);
        (address[] memory signers, bytes[] memory sigs) = _signQuorum(commitmentHash, 3);

        vm.prank(challenger);
        vm.expectRevert(IGasKillerSlasher.InputMismatch.selector);
        slasher.slash(commitment, _referenceBlock(), signers, sigs, fixture.proof, fixture.publicValues);
    }

    /// @notice The same commitment cannot be slashed twice.
    function test_e2e_alreadySlashed() public {
        IGasKillerSlasher.SignedCommitment memory commitment = _fraudCommitment();
        bytes32 commitmentHash = slasher.computeCommitmentHash(commitment);
        (address[] memory signers, bytes[] memory sigs) = _signQuorum(commitmentHash, 3);

        vm.prank(challenger);
        slasher.slash(commitment, _referenceBlock(), signers, sigs, fixture.proof, fixture.publicValues);

        vm.prank(challenger);
        vm.expectRevert(IGasKillerSlasher.AlreadySlashed.selector);
        slasher.slash(commitment, _referenceBlock(), signers, sigs, fixture.proof, fixture.publicValues);
    }

    // ============ Helpers ============

    /// @dev A fraudulent commitment: the fixture's exact execution context, but storage updates
    ///      tampered so they diverge from the proven ones.
    function _fraudCommitment() internal view returns (IGasKillerSlasher.SignedCommitment memory) {
        return IGasKillerSlasher.SignedCommitment({
            transitionIndex: 0,
            contractAddress: fixture.contractAddress,
            anchorHash: fixture.anchorHash,
            callerAddress: fixture.callerAddress,
            contractCalldata: fixture.contractCalldata,
            storageUpdates: _tamper(fixture.storageUpdates)
        });
    }

    /// @dev Flip the last byte (or seed one) so the signed updates differ from the proven ones.
    function _tamper(bytes memory data) internal pure returns (bytes memory) {
        bytes memory out = bytes.concat(data);
        if (out.length == 0) {
            return hex"01";
        }
        out[out.length - 1] = out[out.length - 1] ^ 0x01;
        return out;
    }

    function _referenceBlock() internal view returns (uint32) {
        return uint32(vm.getBlockNumber() - 1);
    }

    function _signQuorum(bytes32 digest, uint256 count)
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

    function _singleStrategyQuorum() internal pure returns (IECDSAStakeRegistryTypes.Quorum memory q) {
        q.strategies = new IECDSAStakeRegistryTypes.StrategyParams[](1);
        q.strategies[0] =
            IECDSAStakeRegistryTypes.StrategyParams({strategy: IStrategy(address(0x1)), multiplier: 10_000});
    }

    function _emptyOperatorSignature()
        internal
        view
        returns (ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry memory)
    {
        return ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry({
            signature: "",
            salt: bytes32(0),
            expiry: block.timestamp + 1 days
        });
    }
}

/// @title GasKillerSlasherArraySummationE2ETest
/// @notice Runs the exact same end-to-end slashing flow, but against a REAL SP1 proof of the
///         actual `ArraySummation` demo contract's `sum([])` execution — deployed on a local
///         anvil chain and proved with the challenger host's `--dev-genesis` mode. This is the
///         Gas Killer demo app end-to-end: a fraudulent `sum` state transition, a real proof of
///         the correct one, and the signing operators slashed.
contract GasKillerSlasherArraySummationE2ETest is GasKillerSlasherE2ETest {
    function _fixturePath() internal pure override returns (string memory) {
        return "test/fixtures/arraysummation-fixture.json";
    }
}

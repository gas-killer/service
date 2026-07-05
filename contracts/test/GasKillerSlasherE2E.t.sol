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
import {
    IECDSAStakeRegistryErrors,
    IECDSAStakeRegistryTypes
} from "@eigenlayer-middleware/interfaces/IECDSAStakeRegistry.sol";
import {CoreDeployLib} from "@eigenlayer-middleware-test/utils/CoreDeployLib.sol";

import {IStrategy} from "eigenlayer-contracts/src/contracts/interfaces/IStrategy.sol";
import {IStrategyManager} from "eigenlayer-contracts/src/contracts/interfaces/IStrategyManager.sol";
import {IDelegationManager} from "eigenlayer-contracts/src/contracts/interfaces/IDelegationManager.sol";
import {
    IAllocationManager,
    IAllocationManagerTypes
} from "eigenlayer-contracts/src/contracts/interfaces/IAllocationManager.sol";
import {IAVSDirectory} from "eigenlayer-contracts/src/contracts/interfaces/IAVSDirectory.sol";
import {OperatorSet} from "eigenlayer-contracts/src/contracts/libraries/OperatorSetLib.sol";
import {ISignatureUtilsMixinTypes} from "eigenlayer-contracts/src/contracts/interfaces/ISignatureUtilsMixin.sol";
import {StrategyFactory} from "eigenlayer-contracts/src/contracts/strategies/StrategyFactory.sol";

import {ProxyAdmin} from "@openzeppelin/contracts/proxy/transparent/ProxyAdmin.sol";
import {ERC20Mock} from "@openzeppelin/contracts/mocks/ERC20Mock.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

// ============ Test doubles ============
// Everything EigenLayer is REAL: the full core (AllocationManager, DelegationManager,
// StrategyManager, AVSDirectory, PermissionController) is deployed via the middleware's
// CoreDeployLib, operators deposit real (mock-token) stake, allocate real magnitude to the
// operator set, and the slash burns that stake for real. The proof, its verification, the ECDSA
// quorum, and the fraud detection are also all REAL. The only double left is the Helios light
// client, which needs an off-chain sync committee to anchor block hashes.

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

/// @notice Exposes the slasher's internal strategy sort (the `slashOperator` ascending-order
///         requirement) for direct unit coverage; the E2E flow only exercises one strategy.
contract SlasherSortHarness is GasKillerSlasher {
    constructor()
        GasKillerSlasher(
            address(1), address(1), address(1), address(1), address(1), 0, bytes32(0), bytes32(0), address(1), 300
        )
    {}

    function exposedSortAscending(IStrategy[] memory strategies) external pure returns (IStrategy[] memory) {
        _sortAscending(strategies);
        return strategies;
    }
}

/// @title GasKillerSlasherE2ETest
/// @notice End-to-end slashing test on the ECDSA path against a REAL EigenLayer deployment: a
///         real 3-operator ECDSA quorum with real deposited-and-allocated stake signs a
///         FRAUDULENT commitment, a challenger submits a REAL SP1 Groth16 proof of the correct
///         execution, and the slasher — verifying the quorum via the same `ECDSAStakeRegistry`
///         the AVS's `verifyAndUpdate` trusts, then the proof, then the storage mismatch —
///         slashes every signing operator through the real `AllocationManager`, burning their
///         stake down to zero.
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

    uint256 internal constant OPERATOR_STAKE = 100 ether;
    /// @dev 2-of-3 equal-weight quorum threshold
    uint256 internal constant THRESHOLD_WEIGHT = 200 ether;
    uint32 internal constant OPERATOR_SET_ID = 0;
    /// @dev Full allocation of the operator's magnitude to the Gas Killer operator set
    uint64 internal constant FULL_MAGNITUDE = 1e18;
    /// @dev Small delays so the test doesn't roll through mainnet-scale block ranges
    uint32 internal constant ALLOCATION_CONFIGURATION_DELAY = 25;
    uint32 internal constant DEALLOCATION_DELAY = 50;
    /// @dev Oldest reference block the slasher accepts (the challenge window); the tests
    ///      challenge with a reference block one behind head (>= DEALLOCATION_DELAY in production).
    uint32 internal constant MAX_REFERENCE_BLOCK_AGE = 300;
    /// @dev Where the AllocationManager sends slashed stake of non-redistributing operator sets
    address internal constant EIGENLAYER_BURN_ADDRESS = 0x00000000000000000000000000000000000E16E4;

    Fixture internal fixture;

    CoreDeployLib.DeploymentData internal core;
    ProxyAdmin internal proxyAdmin;
    ERC20Mock internal token;
    IStrategy internal strategy;
    ECDSAStakeRegistry internal registry;
    GasKillerServiceManager internal serviceManager;
    SP1Verifier internal sp1Verifier;
    MockHeliosLightClient internal helios;
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

        _deployEigenLayerCore();
        _deployAvsAndRegisterOperators();
        _enrollOperatorsInOperatorSet();
        _deploySlasher();

        vm.roll(block.number + 10);
    }

    // ============ Setup phases ============

    /// @dev Real EigenLayer core behind transparent proxies, plus a real strategy for a mock LST.
    function _deployEigenLayerCore() internal {
        // Start beyond the withdrawal-delay horizon: DelegationManager.slashOperatorShares looks
        // back MIN_WITHDRAWAL_DELAY_BLOCKS + 1 blocks for still-slashable queued withdrawals and
        // would underflow on a chain younger than that.
        vm.roll(block.number + DEALLOCATION_DELAY + 1);

        proxyAdmin = new ProxyAdmin();

        CoreDeployLib.DeploymentConfigData memory configData;
        configData.strategyManager.initialOwner = address(this);
        configData.strategyManager.initialStrategyWhitelister = address(this);
        configData.delegationManager.minWithdrawalDelayBlocks = DEALLOCATION_DELAY;
        configData.eigenPodManager.initialOwner = address(this);
        configData.allocationManager.deallocationDelay = DEALLOCATION_DELAY;
        configData.allocationManager.allocationConfigurationDelay = ALLOCATION_CONFIGURATION_DELAY;
        configData.strategyFactory.initialOwner = address(this);
        configData.avsDirectory.initialOwner = address(this);
        configData.rewardsCoordinator.initialOwner = address(this);
        configData.rewardsCoordinator.rewardsUpdater = address(this);
        configData.rewardsCoordinator.activationDelay = 0;
        configData.rewardsCoordinator.defaultSplitBips = 1000;
        configData.rewardsCoordinator.calculationIntervalSeconds = 86400;
        configData.rewardsCoordinator.maxRewardsDuration = 864000;
        configData.rewardsCoordinator.maxRetroactiveLength = 86400;
        configData.rewardsCoordinator.maxFutureLength = 86400;
        configData.rewardsCoordinator.genesisRewardsTimestamp = 1672531200;
        configData.ethPOSDeposit.ethPOSDepositAddress = address(0x123);

        core = CoreDeployLib.deployContracts(address(proxyAdmin), configData);

        IStrategyManager(core.strategyManager).setStrategyWhitelister(core.strategyFactory);
        token = new ERC20Mock();
        strategy = StrategyFactory(core.strategyFactory).deployNewStrategy(IERC20(address(token)));
    }

    /// @dev Real ECDSAStakeRegistry + service manager; each operator registers with EigenLayer,
    ///      deposits real stake into the strategy, and registers its signing key with a real
    ///      AVSDirectory registration signature.
    function _deployAvsAndRegisterOperators() internal {
        registry = new ECDSAStakeRegistry(IDelegationManager(core.delegationManager));
        serviceManager = new GasKillerServiceManager(
            core.avsDirectory,
            address(registry),
            core.rewardsCoordinator,
            core.delegationManager,
            core.allocationManager,
            address(this)
        );
        registry.initialize(address(serviceManager), THRESHOLD_WEIGHT, _singleStrategyQuorum());

        for (uint256 i = 0; i < operators.length; i++) {
            address operator = operators[i];
            token.mint(operator, OPERATOR_STAKE);

            vm.startPrank(operator);
            // allocationDelay 0: allocations become slashable as soon as the configuration
            // delay for the operator's first delay-set has elapsed.
            IDelegationManager(core.delegationManager).registerAsOperator(address(0), 0, "");
            token.approve(core.strategyManager, OPERATOR_STAKE);
            IStrategyManager(core.strategyManager).depositIntoStrategy(strategy, IERC20(address(token)), OPERATOR_STAKE);
            registry.registerOperatorWithSignature(_operatorSignature(i), operator);
            vm.stopPrank();
        }
    }

    /// @dev The AVS creates the slashable operator set; every operator allocates its full
    ///      magnitude and registers for the set (via the service manager's IAVSRegistrar hook).
    function _enrollOperatorsInOperatorSet() internal {
        IStrategy[] memory strategies = new IStrategy[](1);
        strategies[0] = strategy;

        serviceManager.updateAllocationManagerMetadataURI("gas-killer-avs");
        serviceManager.createOperatorSet(OPERATOR_SET_ID, strategies);

        // The operators' allocation delay (0, set at registerAsOperator) becomes effective
        // ALLOCATION_CONFIGURATION_DELAY + 1 blocks after it was set.
        vm.roll(block.number + ALLOCATION_CONFIGURATION_DELAY + 1);

        uint32[] memory operatorSetIds = new uint32[](1);
        operatorSetIds[0] = OPERATOR_SET_ID;
        uint64[] memory magnitudes = new uint64[](1);
        magnitudes[0] = FULL_MAGNITUDE;

        IAllocationManagerTypes.AllocateParams[] memory allocParams = new IAllocationManagerTypes.AllocateParams[](1);
        allocParams[0] = IAllocationManagerTypes.AllocateParams({
            operatorSet: OperatorSet({avs: address(serviceManager), id: OPERATOR_SET_ID}),
            strategies: strategies,
            newMagnitudes: magnitudes
        });

        for (uint256 i = 0; i < operators.length; i++) {
            vm.startPrank(operators[i]);
            IAllocationManager(core.allocationManager).modifyAllocations(operators[i], allocParams);
            IAllocationManager(core.allocationManager)
                .registerForOperatorSets(
                    operators[i],
                    IAllocationManagerTypes.RegisterParams({
                    avs: address(serviceManager), operatorSetIds: operatorSetIds, data: ""
                })
                );
            vm.stopPrank();
        }
        vm.roll(block.number + 1);
    }

    /// @dev Real SP1 Groth16 verifier; the slasher is appointed for
    ///      `AllocationManager.slashOperator` on the AVS's authority.
    function _deploySlasher() internal {
        sp1Verifier = new SP1Verifier();
        helios = new MockHeliosLightClient();
        helios.setValid(fixture.anchorHash, true);

        slasher = new GasKillerSlasher(
            address(sp1Verifier),
            address(helios),
            address(registry),
            core.allocationManager,
            address(serviceManager),
            OPERATOR_SET_ID,
            fixture.vkey,
            fixture.chainConfigHash,
            address(this),
            MAX_REFERENCE_BLOCK_AGE
        );

        serviceManager.setAppointee(address(slasher), core.allocationManager, IAllocationManager.slashOperator.selector);
    }

    // ============ Tests ============

    /// @notice A fraudulent commitment (storage updates ≠ the proven ones) that the quorum really
    ///         ECDSA-signed is slashed: the real proof verifies and all three signers lose their
    ///         entire allocated stake through the real AllocationManager.
    function test_e2e_fraudSlashesSigningOperators() public {
        IStrategy[] memory strategies = new IStrategy[](1);
        strategies[0] = strategy;
        for (uint256 i = 0; i < operators.length; i++) {
            assertEq(
                IAllocationManager(core.allocationManager).getMaxMagnitude(operators[i], strategy),
                FULL_MAGNITUDE,
                "expected full magnitude before slash"
            );
            assertEq(
                IDelegationManager(core.delegationManager).getOperatorShares(operators[i], strategies)[0],
                OPERATOR_STAKE,
                "expected full delegated stake before slash"
            );
            assertEq(registry.getOperatorWeight(operators[i]), OPERATOR_STAKE);
        }

        // The operators sign a commitment with tampered storage updates — everything else matches
        // the fixture's proven execution, so the challenge is valid and the storage mismatch is fraud.
        IGasKillerSlasher.SignedCommitment memory commitment = _fraudCommitment();
        bytes32 commitmentHash = slasher.computeCommitmentHash(commitment);
        (address[] memory signers, bytes[] memory sigs) = _signQuorum(commitmentHash, 3);

        vm.prank(challenger);
        slasher.slash(commitment, _referenceBlock(), signers, sigs, fixture.proof, fixture.publicValues);

        assertTrue(slasher.isSlashed(commitmentHash), "commitment not marked slashed");

        // Real slashing effects: magnitude and delegated stake burned to zero, and the operators'
        // live quorum weight collapses with them.
        for (uint256 i = 0; i < operators.length; i++) {
            assertEq(
                IAllocationManager(core.allocationManager).getMaxMagnitude(operators[i], strategy),
                0,
                "operator magnitude not fully slashed"
            );
            assertEq(
                IDelegationManager(core.delegationManager).getOperatorShares(operators[i], strategies)[0],
                0,
                "operator stake not fully slashed"
            );
            assertEq(registry.getOperatorWeight(operators[i]), 0, "operator weight survived slash");
        }

        // The slashed stake is actually burned: sweeping the per-slash accounting (slash ids are
        // sequential per operator set, one per slashed operator) sends every staked token to
        // EigenLayer's burn address.
        OperatorSet memory operatorSet = OperatorSet({avs: address(serviceManager), id: OPERATOR_SET_ID});
        for (uint256 slashId = 1; slashId <= operators.length; slashId++) {
            IStrategyManager(core.strategyManager).clearBurnOrRedistributableShares(operatorSet, slashId);
        }
        assertEq(
            token.balanceOf(EIGENLAYER_BURN_ADDRESS), OPERATOR_STAKE * operators.length, "slashed stake not burned"
        );
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
        vm.expectRevert(IECDSAStakeRegistryErrors.InsufficientSignedStake.selector);
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

    /// @notice Operators without an ECDSAStakeRegistry signing key are rejected from the
    ///         slashable operator set by the service manager's registrar hook.
    function test_e2e_operatorSetRequiresStakeRegistryRegistration() public {
        (address stranger,) = makeAddrAndKey("stranger");

        vm.startPrank(stranger);
        IDelegationManager(core.delegationManager).registerAsOperator(address(0), 0, "");

        uint32[] memory operatorSetIds = new uint32[](1);
        operatorSetIds[0] = OPERATOR_SET_ID;
        vm.expectRevert(GasKillerServiceManager.OperatorNotRegisteredWithStakeRegistry.selector);
        IAllocationManager(core.allocationManager)
            .registerForOperatorSets(
                stranger,
                IAllocationManagerTypes.RegisterParams({
                avs: address(serviceManager), operatorSetIds: operatorSetIds, data: ""
            })
            );
        vm.stopPrank();
    }

    /// @notice Operators with a signing key but no allocated magnitude are rejected from the
    ///         slashable operator set: their quorum weight (delegated shares) would otherwise
    ///         back fraudulent signatures with nothing at stake to burn.
    function test_e2e_operatorSetRequiresAllocation() public {
        (address stranger, uint256 strangerKey) = makeAddrAndKey("stranger");

        vm.startPrank(stranger);
        IDelegationManager(core.delegationManager).registerAsOperator(address(0), 0, "");
        registry.registerOperatorWithSignature(_avsDirectorySignature(stranger, strangerKey), stranger);

        // Past the allocation-configuration delay, so a real allocation WOULD be possible —
        // the operator simply never makes one.
        vm.roll(block.number + ALLOCATION_CONFIGURATION_DELAY + 1);

        uint32[] memory operatorSetIds = new uint32[](1);
        operatorSetIds[0] = OPERATOR_SET_ID;
        vm.expectRevert(GasKillerServiceManager.OperatorNotAllocated.selector);
        IAllocationManager(core.allocationManager)
            .registerForOperatorSets(
                stranger,
                IAllocationManagerTypes.RegisterParams({
                avs: address(serviceManager), operatorSetIds: operatorSetIds, data: ""
            })
            );
        vm.stopPrank();
    }

    /// @notice An operator with only a PENDING (not-yet-effective) allocation is rejected from
    ///         the operator set: a long allocation delay must not let it sign fraud with stake
    ///         that isn't slashable yet.
    function test_e2e_operatorSetRejectsPendingAllocation() public {
        (address late, uint256 lateKey) = makeAddrAndKey("late-allocator");
        // An allocation delay far larger than the slasher's challenge window.
        uint32 longDelay = MAX_REFERENCE_BLOCK_AGE + 100;

        token.mint(late, OPERATOR_STAKE);
        vm.startPrank(late);
        IDelegationManager(core.delegationManager).registerAsOperator(address(0), longDelay, "");
        token.approve(core.strategyManager, OPERATOR_STAKE);
        IStrategyManager(core.strategyManager).depositIntoStrategy(strategy, IERC20(address(token)), OPERATOR_STAKE);
        registry.registerOperatorWithSignature(_avsDirectorySignature(late, lateKey), late);
        vm.stopPrank();

        // Wait for the allocation-delay configuration to activate, then allocate — the allocation
        // itself is now pending for `longDelay` blocks (currentMagnitude stays 0).
        vm.roll(block.number + ALLOCATION_CONFIGURATION_DELAY + 1);

        IStrategy[] memory strategies = new IStrategy[](1);
        strategies[0] = strategy;
        uint64[] memory magnitudes = new uint64[](1);
        magnitudes[0] = FULL_MAGNITUDE;
        IAllocationManagerTypes.AllocateParams[] memory allocParams = new IAllocationManagerTypes.AllocateParams[](1);
        allocParams[0] = IAllocationManagerTypes.AllocateParams({
            operatorSet: OperatorSet({avs: address(serviceManager), id: OPERATOR_SET_ID}),
            strategies: strategies,
            newMagnitudes: magnitudes
        });

        uint32[] memory operatorSetIds = new uint32[](1);
        operatorSetIds[0] = OPERATOR_SET_ID;

        vm.startPrank(late);
        IAllocationManager(core.allocationManager).modifyAllocations(late, allocParams);
        // Magnitude is pending, not effective → the registrar hook rejects the operator.
        assertEq(IAllocationManager(core.allocationManager).getMaxMagnitude(late, strategy), FULL_MAGNITUDE);
        assertEq(
            IAllocationManager(core.allocationManager)
            .getAllocation(late, OperatorSet({avs: address(serviceManager), id: OPERATOR_SET_ID}), strategy)
            .currentMagnitude,
            0,
            "allocation should still be pending"
        );
        vm.expectRevert(GasKillerServiceManager.OperatorNotAllocated.selector);
        IAllocationManager(core.allocationManager)
            .registerForOperatorSets(
                late,
                IAllocationManagerTypes.RegisterParams({
                avs: address(serviceManager), operatorSetIds: operatorSetIds, data: ""
            })
            );
        vm.stopPrank();
    }

    /// @notice A challenge that attributes only a quorum subset slashes exactly those signers —
    ///         and does NOT immunize the remaining signer: a later challenge with its signature
    ///         still burns it.
    function test_e2e_partialSignerSlashLeavesCoSignersSlashable() public {
        IStrategy[] memory strategies = new IStrategy[](1);
        strategies[0] = strategy;

        IGasKillerSlasher.SignedCommitment memory commitment = _fraudCommitment();
        bytes32 commitmentHash = slasher.computeCommitmentHash(commitment);

        // First challenge: only the first two signers (weight 200 >= threshold 200).
        (address[] memory subset, bytes[] memory subsetSigs) = _signQuorum(commitmentHash, 2);
        vm.prank(challenger);
        slasher.slash(commitment, _referenceBlock(), subset, subsetSigs, fixture.proof, fixture.publicValues);

        // Exactly the attributed signers are burned; the third operator is untouched.
        for (uint256 i = 0; i < 2; i++) {
            assertTrue(slasher.isOperatorSlashed(commitmentHash, operators[i]));
            assertEq(IAllocationManager(core.allocationManager).getMaxMagnitude(operators[i], strategy), 0);
        }
        assertFalse(slasher.isOperatorSlashed(commitmentHash, operators[2]));
        assertEq(
            IAllocationManager(core.allocationManager).getMaxMagnitude(operators[2], strategy),
            FULL_MAGNITUDE,
            "unattributed operator must keep its stake"
        );
        assertEq(
            IDelegationManager(core.delegationManager).getOperatorShares(operators[2], strategies)[0], OPERATOR_STAKE
        );

        // Second challenge with all three signatures burns the remaining signer.
        (address[] memory all, bytes[] memory allSigs) = _signQuorum(commitmentHash, 3);
        vm.prank(challenger);
        slasher.slash(commitment, _referenceBlock(), all, allSigs, fixture.proof, fixture.publicValues);

        assertTrue(slasher.isOperatorSlashed(commitmentHash, operators[2]));
        assertEq(IAllocationManager(core.allocationManager).getMaxMagnitude(operators[2], strategy), 0);

        // With every signer burned, a further challenge is a redundant no-op.
        vm.prank(challenger);
        vm.expectRevert(IGasKillerSlasher.AlreadySlashed.selector);
        slasher.slash(commitment, _referenceBlock(), all, allSigs, fixture.proof, fixture.publicValues);

        // All three slashes (ids 1..3) burn the full 300 ether of stake.
        OperatorSet memory operatorSet = OperatorSet({avs: address(serviceManager), id: OPERATOR_SET_ID});
        for (uint256 slashId = 1; slashId <= operators.length; slashId++) {
            IStrategyManager(core.strategyManager).clearBurnOrRedistributableShares(operatorSet, slashId);
        }
        assertEq(token.balanceOf(EIGENLAYER_BURN_ADDRESS), OPERATOR_STAKE * operators.length);
    }

    /// @notice An anchor hash the light client does not recognize is rejected — without it, a
    ///         challenger could "prove" an execution against a fabricated pre-state.
    function test_e2e_unverifiedAnchorRejected() public {
        helios.setValid(fixture.anchorHash, false);

        IGasKillerSlasher.SignedCommitment memory commitment = _fraudCommitment();
        bytes32 commitmentHash = slasher.computeCommitmentHash(commitment);
        (address[] memory signers, bytes[] memory sigs) = _signQuorum(commitmentHash, 3);

        vm.prank(challenger);
        vm.expectRevert(IGasKillerSlasher.UnverifiedBlock.selector);
        slasher.slash(commitment, _referenceBlock(), signers, sigs, fixture.proof, fixture.publicValues);
    }

    /// @notice A reference block older than the challenge window is rejected, bounding the
    ///         slash exposure of rotated-away signing keys.
    function test_e2e_staleReferenceBlockRejected() public {
        IGasKillerSlasher.SignedCommitment memory commitment = _fraudCommitment();
        bytes32 commitmentHash = slasher.computeCommitmentHash(commitment);
        (address[] memory signers, bytes[] memory sigs) = _signQuorum(commitmentHash, 3);
        uint32 referenceBlock = _referenceBlock();

        vm.roll(block.number + MAX_REFERENCE_BLOCK_AGE + 1);

        vm.prank(challenger);
        vm.expectRevert(IGasKillerSlasher.StaleReferenceBlock.selector);
        slasher.slash(commitment, referenceBlock, signers, sigs, fixture.proof, fixture.publicValues);
    }

    /// @notice The slasher sorts operator-set strategies into the ascending order
    ///         `AllocationManager.slashOperator` requires, whatever order the set returns them in.
    function test_sortAscendingOrdersStrategies() public {
        SlasherSortHarness harness = new SlasherSortHarness();

        IStrategy[] memory shuffled = new IStrategy[](4);
        shuffled[0] = IStrategy(address(0xCC));
        shuffled[1] = IStrategy(address(0x11));
        shuffled[2] = IStrategy(address(0xEE));
        shuffled[3] = IStrategy(address(0x22));

        IStrategy[] memory sorted = harness.exposedSortAscending(shuffled);
        for (uint256 i = 1; i < sorted.length; i++) {
            assertLt(uint160(address(sorted[i - 1])), uint160(address(sorted[i])), "strategies not ascending");
        }
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

    function _singleStrategyQuorum() internal view returns (IECDSAStakeRegistryTypes.Quorum memory q) {
        q.strategies = new IECDSAStakeRegistryTypes.StrategyParams[](1);
        q.strategies[0] = IECDSAStakeRegistryTypes.StrategyParams({strategy: strategy, multiplier: 10_000});
    }

    /// @dev A real AVSDirectory registration signature: the operator signs the directory's
    ///      registration digest for this AVS (the service manager).
    function _operatorSignature(uint256 operatorIndex)
        internal
        view
        returns (ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry memory)
    {
        return _avsDirectorySignature(operators[operatorIndex], operatorKeys[operatorIndex]);
    }

    function _avsDirectorySignature(address operator, uint256 privateKey)
        internal
        view
        returns (ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry memory)
    {
        bytes32 salt = keccak256(abi.encodePacked("gas-killer-e2e", operator));
        uint256 expiry = block.timestamp + 1 days;
        bytes32 digest = IAVSDirectory(core.avsDirectory)
            .calculateOperatorAVSRegistrationDigestHash(operator, address(serviceManager), salt, expiry);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(privateKey, digest);
        return ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry({
            signature: abi.encodePacked(r, s, v), salt: salt, expiry: expiry
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

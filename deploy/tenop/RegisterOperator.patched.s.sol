// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.12;

import {Test, console} from "forge-std/Test.sol";
import {StdCheats} from "forge-std/StdCheats.sol";
import {Script} from "forge-std/Script.sol";
import {
    ISignatureUtilsMixin,
    ISignatureUtilsMixinTypes
} from "@eigenlayer/contracts/interfaces/ISignatureUtilsMixin.sol";
import {IBLSApkRegistryTypes} from "@eigenlayer-middleware/src/interfaces/IBLSApkRegistry.sol";
import {BN254} from "@eigenlayer-middleware/src/libraries/BN254.sol";
import {IRegistryCoordinator} from "@eigenlayer-middleware/src/interfaces/IRegistryCoordinator.sol";
import {RegistryCoordinator} from "@eigenlayer-middleware/src/RegistryCoordinator.sol";
import {SlashingRegistryCoordinator} from "@eigenlayer-middleware/src/SlashingRegistryCoordinator.sol";
import {BN256G2} from "../src/libraries/BN256G2.sol";
import {IAVSDirectory} from "@eigenlayer/contracts/interfaces/IAVSDirectory.sol";
import {stdJson} from "forge-std/StdJson.sol";
import {
    ISlashingRegistryCoordinator,
    ISlashingRegistryCoordinatorTypes
} from "@eigenlayer-middleware/src/interfaces/ISlashingRegistryCoordinator.sol";
import {IAllocationManager, IAllocationManagerTypes} from "@eigenlayer/contracts/interfaces/IAllocationManager.sol";
import {IStrategy} from "@eigenlayer/contracts/interfaces/IStrategy.sol";
import {OperatorSet} from "@eigenlayer/contracts/libraries/OperatorSetLib.sol";

// Mainnet
// DELEGATION_MANAGER_ADDRESS=0x39053D51B77DC0d36036Fc1fCc8Cb819df8Ef37A
// Holesky
// DELEGATION_MANAGER_ADDRESS=0xA44151489861Fe9e3055d95adC98FbD462B948e7
// Mainnet
// STRATEGY_MANAGER_ADDRESS=0x858646372CC42E1A627fcE94aa7A7033e7CF075A
// Holesky
// STRATEGY_MANAGER_ADDRESS=0xdfB5f6CE42aAA7830E94ECFCcAd411beF4d4D5b6
// Holesky stETH
// LST_CONTRACT_ADDRESS=0x3F1c547b21f65e10480dE3ad8E19fAAC46C95034
// Mainnet stETH
// LST_CONTRACT_ADDRESS=0xae7ab96520DE3A18E5e111B5EaAb095312D7fE84
// Holesky stETH strategy
// LST_STRATEGY_ADDRESS=0x7D704507b76571a51d9caE8AdDAbBFd0ba0e63d3
// Mainnet stETH strategy
// LST_STRATEGY_ADDRESS=0x93c4b944D05dfe6df7645A86cd2206016c51564D

contract RegisterOperator is Script {
    using BN254 for BN254.G1Point;
    using stdJson for string;

    // Core contracts
    address constant DELEGATION_MANAGER_ADDRESS_HOLESKY = 0xA44151489861Fe9e3055d95adC98FbD462B948e7;
    address constant AVS_DIRECTORY_ADDRESS_HOLESKY = 0x055733000064333CaDDbC92763c58BF0192fFeBf;
    address constant STRATEGY_MANAGER_ADDRESS_HOLESKY = 0xdfB5f6CE42aAA7830E94ECFCcAd411beF4d4D5b6;
    // LST contracts
    address constant LST_CONTRACT_ADDRESS_HOLESKY = 0x3F1c547b21f65e10480dE3ad8E19fAAC46C95034;
    address constant LST_STRATEGY_ADDRESS_HOLESKY = 0x7D704507b76571a51d9caE8AdDAbBFd0ba0e63d3;
    // Opacity middleware contracts
    address constant OPACITY_REGISTRY_COORDINATOR_ADDRESS_HOLESKY = 0x3e43AA225b5cB026C5E8a53f62572b10D526a50B;
    address constant OPACTIY_AVS_ADDRESS_HOLESKY = 0xbfc5d26C6eEb46475eB3960F5373edC5341eE535;

    address registryCoordinatorMimicOwner = makeAddr("registryCoordinatorMimicOwner");

    struct Operator {
        address operator;
        uint256 ecdsaPrivateKey;
        uint256 blsPrivateKey;
        BN254.G1Point pk1;
        BN254.G2Point pk2;
    }

    function readIPConfig(string memory operatorId) internal view returns (string memory) {
        string memory root = vm.projectRoot();
        string memory path = string.concat(root, "/docker/eigenlayer/config.json");
        string memory json = vm.readFile(path);

        // Read the operator socket address from config using the operator ID
        return vm.parseJsonString(json, string.concat("$.operators.", operatorId, ".socketAddress"));
    }

    function run() public {
        // Get the operator ID from the environment variable
        string memory operatorId = vm.envString("OPERATOR_ID");
        if (bytes(operatorId).length == 0) {
            revert("OPERATOR_ID environment variable not set");
        }

        string memory ecdsaPrivateKey = vm.readFile("./private.ecdsa.json");
        uint256 ecdsaPrivateKeyUint = ecdsaPrivateKey.readUint(".privateKey");
        vm.startBroadcast(ecdsaPrivateKeyUint);
        address operatorAddress = ecdsaPrivateKey.readAddress(".publicKey");
        string memory blsPrivateKey = vm.readFile("./private.bls.json");
        uint256 blsPrivateKeyUint = blsPrivateKey.readUint(".privateKey");
        BN254.G1Point memory pk1 = BN254.scalar_mul(BN254.generatorG1(), blsPrivateKeyUint);
        BN254.G2Point memory g2 = BN254.generatorG2();
        BN254.G2Point memory pk2;
        (pk2.X[1], pk2.X[0], pk2.Y[1], pk2.Y[0]) =
            BN256G2.ECTwistMul(blsPrivateKeyUint, g2.X[1], g2.X[0], g2.Y[1], g2.Y[0]);

        // ensure correct encoding by checking pairing
        bool result = BN254.pairing(pk1, BN254.negGeneratorG2(), BN254.generatorG1(), pk2);
        require(result, "Pairing check on BLS key generation failed");
        Operator memory operator = Operator({
            operator: operatorAddress,
            ecdsaPrivateKey: ecdsaPrivateKeyUint,
            blsPrivateKey: blsPrivateKeyUint,
            pk1: pk1,
            pk2: pk2
        });
        string memory json = vm.readFile("./avs_deploy.json");
        address registryCoordinator = json.readAddress(".addresses.registryCoordinator");
        address serviceManager = json.readAddress(".addresses.IncredibleSquaringServiceManager");
        registerOperator(IRegistryCoordinator(registryCoordinator), serviceManager, operator, operatorId);
        vm.stopBroadcast();
    }

    function registerOperator(
        IRegistryCoordinator registryCoordinator,
        address avs,
        Operator memory operator,
        string memory operatorId
    ) internal {
        bytes memory quorumNumbers = hex"00";
        // Read the socket address from config for the specific operator
        string memory socket = readIPConfig(operatorId);

        BN254.G1Point memory h = registryCoordinator.pubkeyRegistrationMessageHash(operator.operator);
        BN254.G1Point memory sig = BN254.scalar_mul(h, operator.blsPrivateKey);

        IBLSApkRegistryTypes.PubkeyRegistrationParams memory params = IBLSApkRegistryTypes.PubkeyRegistrationParams({
            pubkeyG1: operator.pk1, pubkeyG2: operator.pk2, pubkeyRegistrationSignature: sig
        });
        ISlashingRegistryCoordinatorTypes.RegistrationType registrationType =
        ISlashingRegistryCoordinatorTypes.RegistrationType.NORMAL;
        bytes memory encodedParams = abi.encode(registrationType, socket, params);
        uint32[] memory operatorSetIds = new uint32[](1);
        operatorSetIds[0] = 0;

        IAllocationManagerTypes.RegisterParams memory registerParams =
            IAllocationManagerTypes.RegisterParams({avs: avs, operatorSetIds: operatorSetIds, data: encodedParams});

        IStrategy[] memory strategies = new IStrategy[](1);
        address lstStrategyAddress = vm.envAddress("LST_STRATEGY_ADDRESS");
        require(lstStrategyAddress != address(0), "LST_STRATEGY_ADDRESS env var not set or invalid");
        strategies[0] = IStrategy(lstStrategyAddress);
        uint64[] memory newMagnitudes = new uint64[](1);
        // Ref: https://github.com/Layr-Labs/eigenlayer-contracts/blob/734f7361884d24fe51961b342e93dde1290961d0/src/contracts/libraries/SlashingLib.sol#L12
        // 1e18 is 100%
        newMagnitudes[0] = 1e18;

        IAllocationManagerTypes.AllocateParams[] memory allocationMods = new IAllocationManagerTypes.AllocateParams[](1);
        allocationMods[0] = IAllocationManagerTypes.AllocateParams({
            operatorSet: OperatorSet({avs: avs, id: 0}), strategies: strategies, newMagnitudes: newMagnitudes
        });
        // Re-runs after a partial setup hit SameMagnitude() here: the allocation
        // already exists at 1e18 from the first pass. SKIP_ALLOCATION=true jumps
        // straight to registerForOperatorSets.
        if (!vm.envOr("SKIP_ALLOCATION", false)) {
            IAllocationManager(registryCoordinator.allocationManager()).modifyAllocations(operator.operator, allocationMods);

            vm.roll(block.number + 1); // Workaround for testnet, txs can't be in the same block
        }

        IAllocationManager(registryCoordinator.allocationManager())
            .registerForOperatorSets(operator.operator, registerParams);
    }

    function _newOperatorRegistrationSignature(Operator memory operator, address avs, bytes32 salt, uint256 expiry)
        internal
        view
        returns (ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry memory)
    {
        bytes32 operatorRegistrationDigestHash = IAVSDirectory(AVS_DIRECTORY_ADDRESS_HOLESKY)
            .calculateOperatorAVSRegistrationDigestHash(operator.operator, avs, salt, expiry);

        (uint8 v, bytes32 r, bytes32 s) = vm.sign(operator.ecdsaPrivateKey, operatorRegistrationDigestHash);
        bytes memory signature = abi.encodePacked(r, s, v);
        return ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry({signature: signature, salt: salt, expiry: expiry});
    }
}

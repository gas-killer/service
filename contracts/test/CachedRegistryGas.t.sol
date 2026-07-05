// SPDX-License-Identifier: AGPL-3.0-only
pragma solidity ^0.8.27;

// GAS EXPERIMENT (not wired into production). Measures the ECDSA quorum-verification
// gas of the stock EigenLayer ECDSAStakeRegistry (per-operator checkpoint binary
// searches) against a snapshot-cached fast path that answers from a flat
// signingKey => weight map when the reference block is at/after the last operator-set
// mutation. Run: forge test --mp test/CachedRegistryGas.t.sol -vv

import {ECDSAStakeRegistry} from "@eigenlayer-middleware/unaudited/ECDSAStakeRegistry.sol";
import {IECDSAStakeRegistryTypes} from "@eigenlayer-middleware/interfaces/IECDSAStakeRegistry.sol";
import {IStrategy} from "eigenlayer-contracts/src/contracts/interfaces/IStrategy.sol";
import {IDelegationManager} from
    "eigenlayer-contracts/src/contracts/interfaces/IDelegationManager.sol";
import {ISignatureUtilsMixinTypes} from
    "eigenlayer-contracts/src/contracts/interfaces/ISignatureUtilsMixin.sol";
import {ECDSAUpgradeable} from
    "@openzeppelin-upgrades/contracts/utils/cryptography/ECDSAUpgradeable.sol";
import {console2} from "forge-std/console2.sol";

interface Vm {
    function sign(uint256 privateKey, bytes32 digest) external pure returns (uint8 v, bytes32 r, bytes32 s);
    function addr(uint256 privateKey) external pure returns (address);
    function prank(address sender) external;
    function roll(uint256 newHeight) external;
}

contract MockDM {
    mapping(address => uint256) public shares;
    function setShares(address o, uint256 v) external { shares[o] = v; }
    function getOperatorShares(address o, IStrategy[] memory s) external view returns (uint256[] memory r) {
        r = new uint256[](s.length);
        for (uint256 i; i < s.length; i++) r[i] = shares[o];
    }
}

contract MockDir {
    function registerOperatorToAVS(address, ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry memory) external {}
    function deregisterOperatorFromAVS(address) external {}
}

/// @notice Prototype: stock registry + a snapshot cache read path.
contract CachedECDSAStakeRegistry is ECDSAStakeRegistry {
    struct Snap {
        uint64 effectiveBlock; // block of the last operator-set mutation
        uint192 totalWeight;
    }

    Snap internal _snap;
    uint256 internal _snapThreshold;
    // signingKey => weight + 1 (0 = not a current signer)
    mapping(address => uint256) internal _snapWeightBySigningKey;
    // signing keys currently populated in the map, so a re-sync can evict prior entries
    address[] internal _snapKeys;

    constructor(IDelegationManager dm) ECDSAStakeRegistry(dm) {}

    /// @dev PROTOTYPE ONLY. Production would maintain this incrementally inside the
    ///      checkpoint-push hooks (register/deregister/updateSigningKey/updateOperators/
    ///      updateStakeThreshold/updateQuorum). Here we rebuild from a supplied list to
    ///      isolate the read-path gas being measured. A full rebuild first evicts the
    ///      prior snapshot's keys — otherwise a re-sync with a shrunk/rotated operator set
    ///      would leave stale signing keys marked registered and the fast path would accept
    ///      non-current signers. This eviction runs off the measured read path (setUp only),
    ///      so it does not affect the reported gas. Production maintains the map
    ///      incrementally, so it has no equivalent full-rebuild step.
    function syncSnapshot(address[] calldata operators) external {
        uint256 stale = _snapKeys.length;
        for (uint256 i; i < stale; i++) {
            delete _snapWeightBySigningKey[_snapKeys[i]];
        }
        delete _snapKeys;

        uint256 total;
        for (uint256 i; i < operators.length; i++) {
            address key = this.getLatestOperatorSigningKey(operators[i]);
            uint256 w = this.getLastCheckpointOperatorWeight(operators[i]);
            _snapWeightBySigningKey[key] = w + 1;
            _snapKeys.push(key);
            total += w;
        }
        _snap = Snap({effectiveBlock: uint64(block.number), totalWeight: uint192(total)});
        _snapThreshold = this.getLastCheckpointThresholdWeight();
    }

    /// @notice Snapshot fast path: valid only when `referenceBlock` is at/after the last
    ///         set mutation (so the current snapshot equals the state at that block).
    ///         Recovers the signing key directly from each signature (no operators[]),
    ///         one SLOAD per signer, no per-operator checkpoint walk.
    function isValidSignatureCached(
        bytes32 digest,
        bytes[] calldata signatures,
        uint32 referenceBlock
    ) external view returns (bytes4) {
        Snap memory s = _snap;
        require(referenceBlock < block.number, "future");
        require(referenceBlock >= s.effectiveBlock, "stale-snapshot"); // caller falls back to base path
        uint256 signed;
        address last;
        for (uint256 i; i < signatures.length; i++) {
            address signer = ECDSAUpgradeable.recover(digest, signatures[i]);
            require(signer > last, "not-sorted");
            uint256 wp1 = _snapWeightBySigningKey[signer];
            require(wp1 != 0, "not-registered");
            unchecked {
                signed += wp1 - 1;
            }
            last = signer;
        }
        require(signed >= _snapThreshold, "insufficient");
        return 0x1626ba7e;
    }
}

contract CachedRegistryGasTest {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));
    uint256 internal constant SHARES = 100 ether;
    uint256 internal constant THRESH = 200 ether;

    MockDM dm;
    MockDir dir;
    CachedECDSAStakeRegistry reg;
    uint256[] keys;
    address[] ops;

    function setUp() public {
        uint256[] memory k = new uint256[](3);
        k[0] = 0xA0; k[1] = 0xB0; k[2] = 0xC0;
        for (uint256 i = 1; i < 3; i++) {
            uint256 kk = k[i]; uint256 j = i;
            while (j > 0 && vm.addr(k[j-1]) > vm.addr(kk)) { k[j] = k[j-1]; j--; }
            k[j] = kk;
        }
        for (uint256 i; i < 3; i++) { keys.push(k[i]); ops.push(vm.addr(k[i])); }

        dm = new MockDM();
        dir = new MockDir();
        reg = new CachedECDSAStakeRegistry(IDelegationManager(address(dm)));

        IECDSAStakeRegistryTypes.Quorum memory q;
        q.strategies = new IECDSAStakeRegistryTypes.StrategyParams[](1);
        q.strategies[0] = IECDSAStakeRegistryTypes.StrategyParams({strategy: IStrategy(address(0x1)), multiplier: 10_000});
        reg.initialize(address(dir), THRESH, q);

        for (uint256 i; i < 3; i++) {
            dm.setShares(ops[i], SHARES);
            vm.prank(ops[i]);
            reg.registerOperatorWithSignature(
                ISignatureUtilsMixinTypes.SignatureWithSaltAndExpiry({signature: "", salt: bytes32(0), expiry: block.timestamp + 1 days}),
                ops[i]
            );
        }
        reg.syncSnapshot(ops);
        vm.roll(block.number + 5);
    }

    function _digest() internal pure returns (bytes32) { return keccak256("gas-experiment-task"); }

    function _sigs(uint256 n) internal view returns (bytes[] memory sigs) {
        sigs = new bytes[](n);
        for (uint256 i; i < n; i++) {
            (uint8 v, bytes32 r, bytes32 s) = vm.sign(keys[i], _digest());
            sigs[i] = abi.encodePacked(r, s, v);
        }
    }

    /// @dev Stock registry path: ERC-1271 isValidSignature(digest, abi.encode(operators, signatures, refBlock)).
    function test_gas_base_isValidSignature_3ops() public view {
        bytes[] memory sigs = _sigs(3);
        bytes memory data = abi.encode(ops, sigs, uint32(block.number - 1));
        uint256 g = gasleft();
        bytes4 mv = reg.isValidSignature(_digest(), data);
        uint256 used = g - gasleft();
        require(mv == 0x1626ba7e, "base failed");
        _log("BASE  isValidSignature(3 ops)", used);
    }

    function test_gas_cached_isValidSignature_3ops() public view {
        bytes[] memory sigs = _sigs(3);
        uint256 g = gasleft();
        bytes4 mv = reg.isValidSignatureCached(_digest(), sigs, uint32(block.number - 1));
        uint256 used = g - gasleft();
        require(mv == 0x1626ba7e, "cached failed");
        _log("CACHED isValidSignatureCached(3 ops)", used);
    }

    function test_gas_base_2ops() public view {
        bytes[] memory sigs = _sigs(2);
        bytes memory data = abi.encode(_take(ops, 2), sigs, uint32(block.number - 1));
        uint256 g = gasleft();
        reg.isValidSignature(_digest(), data);
        _log("BASE  isValidSignature(2 ops)", g - gasleft());
    }

    function test_gas_cached_2ops() public view {
        bytes[] memory sigs = _sigs(2);
        uint256 g = gasleft();
        reg.isValidSignatureCached(_digest(), sigs, uint32(block.number - 1));
        _log("CACHED isValidSignatureCached(2 ops)", g - gasleft());
    }

    function _take(address[] storage src, uint256 n) internal view returns (address[] memory out) {
        out = new address[](n);
        for (uint256 i; i < n; i++) out[i] = src[i];
    }

    function _log(string memory label, uint256 g) internal pure {
        console2.log(label, g);
    }
}

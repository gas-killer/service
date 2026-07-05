# ECDSA stake-registry verification cache (design + measurements)

Status: **designed, prototyped, red-teamed — production fork deferred to a dedicated PR.**
Tracks `gas-killer/solidity-sdk#45` item **E**. Benchmark: `contracts/test/CachedRegistryGas.t.sol`.

## Problem

`GasKillerSDK.verifyAndUpdate` delegates quorum verification to EigenLayer's
`ECDSAStakeRegistry.isValidSignature` (ERC-1271). Per signer, `_checkSignatures` does
**two checkpoint binary searches** — `_operatorSigningKeyHistory[op].getAtBlock(refBlock)`
and `_operatorWeightHistory[op].getAtBlock(refBlock)` — plus two global ones
(`_totalWeightHistory`, `_thresholdWeightHistory`). These re-walk histories that are
**static between operator-set changes**, and the submitter always queries a recent
reference block (`current − 1`).

## Measured cost (forge, mocked DelegationManager — accurate for the verify path,
cross-validated against a live Sepolia-fork trace: base per-operator marginal 15,581 here
vs 15,586 on the fork)

| | base registry | snapshot-cached | saved |
|---|---|---|---|
| `isValidSignature`, 3 ops | 65,027 | **30,825** | −53% |
| `isValidSignature`, 2 ops | 49,446 | 24,125 | −51% |
| **marginal / operator** | **15,581** | **6,700** | **−57%** |

End-to-end `verifyAndUpdate` (N=3) is projected ~90–100k vs 150,656 (~35–40%), and the
BLS crossover moves from N≈7 to past N≈30.

## Two candidate designs

### E-conservative (#45.E): watermark + `latest()` fast read — byte-identical
Add `uint32 _lastStakeUpdateBlock` (consumed `__gap` slot). Invariant: **≥ the block of
the latest checkpoint of every history**. Route all four checkpoint `push` sites through
helpers that bump it, then in the four `_getX` accessors read
`refBlock >= _lastStakeUpdateBlock ? h.latest() : h.getAtBlock(refBlock)`. Result is
byte-identical to base; only the binary search is skipped. Keeps the `operators[]`
interface and ERC-1271 contract-signer support. Saves ~`log2(H)` cold SLOADs per lookup —
a **scaling** win (meaningful at deep histories H≥8 / large signer sets), smaller at
shallow histories.

### E-aggressive (this session's prototype): flat `signingKey → weight` snapshot
Maintain `mapping(address signingKey => uint256 weightPlus1)` + `(effectiveBlock,
totalWeight, threshold)`; a typed `isValidSignatureCached(digest, signatures[], refBlock)`
recovers the signer directly (no `operators[]`), one SLOAD per signer, and lets
`GasKillerSDK` drop its `abi.encode`-into-bytes overhead. Bigger, flat win (the 30,825
above) but narrows to **EOA signing keys** and sorts by signing key.

## Soundness (the crux, unchanged for both)

`effectiveBlock`/`_lastStakeUpdateBlock` = **max** over all histories' last-change block.
The guard `refBlock ≥ watermark` ⇒ `refBlock ≥` every history's last change ⇒ each
history's value *at refBlock* equals its current value ⇒ the cache equals the true state
at refBlock. Taking the max is conservative: it can only force the slow (`getAtBlock`)
path *more* often, never accept a stale snapshot. When the guard fails, fall back to the
base path.

## Red-team result (20 agents, 16 attack scenarios, adversarially verified)

**Zero safety breaks** — nothing where the cache accepts a quorum the base rejects while
the guard passes. Refuted: the permissionless-`updateOperators` grief (the base's
`if (delta == 0) return;` means a no-op update pushes no checkpoint, so bumping only at
real pushes is grief-free) and the global-vs-per-history watermark concern (the *max*
argument). Confirmed issues are all **fail-closed liveness** narrowings:

1. **ERC-1271 contract signing keys** — the aggressive design's `ecrecover` is EOA-only;
   contract-signer operators must use the base path. Fix: `require(signingKey.code.length
   == 0)` at register/rotate + caller fallback. *Non-issue for gas-killer* (EOA keys).
2. **Signing-key uniqueness + sort domain** — the aggressive design keys/sorts by signing
   key; needs a global uniqueness invariant + submitter sorting by signing key. *Non-issue
   for gas-killer* (signing key == operator address). The conservative design avoids this
   entirely (keeps `operators[]`).
3. **`initialize()` seeding** — seed the watermark/snapshot at init (put the maintenance in
   the internal helpers, not just external wrappers).
4. **Upgrade migration (critical for the conservative design):** on upgrading a *deployed*
   registry, an unset watermark defaults to 0 → the fast path would retroactively rewrite
   historical stake. Fix with a `reinitializer` setting it to `uint32(block.number)`.

## Why a dedicated follow-up PR (not bundled here)

It **forks the vendored `ECDSAStakeRegistry`** (marked *NOT AUDITED* upstream) — the exact
"use EigenLayer's verifier" decision the migration made deliberately. Correct instrumentation
of the watermark at the *exact* push sites is where a silent bug would hide, so it needs: the
full fork with `_pushX` helpers, the `reinitializer`, a **fuzz invariant** asserting
`fastPathRead(h, r) == h.getAtBlock(r) == h.latest()` for random histories and random
`r ≥ watermark`, and its own audit. Recommended: ship the **conservative (byte-identical)**
variant with the fuzz invariant; consider the aggressive variant only if the flat per-tx win
justifies the EOA-only narrowing.

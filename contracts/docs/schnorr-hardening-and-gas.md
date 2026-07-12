# Schnorr settlement hardening + measured gas contexts

This branch hardens the aggregate-Schnorr settlement path (`SchnorrGasKillerSDK` →
`SchnorrStakeRegistry`, service #312) against the three gaps deliberately left open there —
re-entrancy, midway-state observation, and multicall — and pins down the gas story in both
execution contexts, because the two differ ~3x and have already been quoted against each
other by mistake.

## 1. The two gas contexts (why published numbers disagreed)

`SchnorrStakeRegistry.isValidSignature` measures very differently depending on the EIP-2929
access-list state:

| context | full participation | marginal / non-signer |
|---|---|---|
| **warm** (registry already touched this tx) | **6,512** | **+4,197** |
| **cold** (first touch — a standalone `verifyAndUpdate` tx) | **17,033** | **+10,194** |

Both are true (registry-attributable cost, harness reads hoisted out of the brackets).
The PR #312 body's `6,749 / +4,162` table is the warm context measured through the shipped
benchmark's brackets; running that same benchmark under `forge test --gas-report` prints
`~19.2k / +12.2k` because **`--gas-report` executes calls in isolation (fresh access lists
per call)** — the cold context, plus the benchmark's own cold reference SLOAD. The
warm→cold delta reconciles exactly: cold account access (+2.5k) + first-touch SLOADs of
`effectiveBlock`/`aggX`/`aggY`/`totalWeight` (+2k each) = ~10.5k, and per non-signer the
operator record's slots going cold. Neither number is stale; they answer different
questions:

- **warm** = the composability price (multicall sub-transitions after the first, or any tx
  that already touched the registry);
- **cold** = what a standalone settlement receipt reflects (e.g. the e2e's measured
  `verifyAndUpdate` receipt of 112,425 vs 157,460 for the same task on the ECDSA path).

`test/SchnorrStakeRegistryGas.t.sol` measures both contexts explicitly, plus linearity in
the non-signer count (k = 1..8, ~4.2k/step warm), so the context can never again be
ambiguous. Measurement hygiene notes live in that file: forge-std asserts/`console2.log`
are cheatcode calls and must stay outside `gasleft()` brackets, and forge resets access
lists between `setUp` and each test (which is what makes first-call-in-test a genuine cold
measurement).

Marginal per non-signer decomposes as ~4.2k compute (one affine point subtraction whose
modular inverse runs through the `modexp` precompile) + the non-signer's record SLOADs.
"Constant gas" is constant **in signers**, linear **in non-signers**: in the cold
(standalone-tx) context even 1–2 non-signers reach the cached-ECDSA cost (30,825 @ 3 ops,
#311; 27,227 measured at k=1), and in the warm context the crossover is ~6 non-signers —
so the scheme's economics assume the healthy-participation regime the coordinator's
suspect-exclusion actively maintains.

## 2. Re-entrancy guard + in-transition latch (`TransitionGuard`)

`StateChangeHandlerLib`'s `CALL` update type forwards all remaining gas to an arbitrary
target **mid-transition** — after `trackState` has bumped the counter, before the
transition's remaining updates land. Two consequences, one mechanism:

- **Re-entrancy:** a `CALL` target holding transition N+1's *valid* quorum signature could
  re-enter `verifyAndUpdate` (the index check passes — the counter already reads N+1) and
  interleave two signed transitions. `guardTransition` on both entrypoints reverts this
  (`ReentrantTransition`), for ~3 transient-storage ops (~300 gas) per settlement. EIP-1153
  transient storage means no dirty slot survives the tx and no storage-refund accounting.
  Requires Cancun+; the repo already targets prague.
- **Midway state:** during a `CALL` update, external readers observe `count = N+1` with
  only a prefix of transition N's writes applied — a state the quorum never signed. The
  same transient flag is exposed as `inTransition()`; integrators reading a Gas Killer
  contract from code that can execute mid-transition should fail closed on it (one warm
  TLOAD, ~100 gas, paid by the reader). `test_latchVisibleMidTransition` demonstrates the
  exact inconsistency the latch guards against.

Deliberately **not** done here:
- **STOREs-before-CALLs reordering** in `_runStateUpdates` — nearly free defense-in-depth,
  but a semantic change (a `CALL` could no longer observe pre-write state) that must be
  coordinated with the off-chain EVMSketch encoder to preserve digest parity.
- **The ECDSA `GasKillerSDK` port** — same hole (shared `StateChangeHandlerLib`), same fix,
  but that file is owned by the ECDSA branches (#309/#311) and patching it here would
  conflict; it needs the identical two-line adoption of `TransitionGuard`.

## 3. Multicall (`verifyAndUpdateBatch`)

Batches N **independently signed** transitions into one transaction. Each applied
submission is checked exactly as a standalone `verifyAndUpdate` (same digest preimage,
same registry call), so nothing changes for the off-chain signing path; transition
indices run consecutively from the current count.

Failure semantics are deliberately asymmetric. A submission whose index is **already
settled is skipped** (not validated, not applied): settlement is permissionless, so a
third party lifting one submission from the mempool and settling it standalone would
otherwise revert the victim's entire batch with one cheap front-run — and since an index
can only ever be consumed by a quorum-signed transition for this contract, a skipped
item's transition has already happened (redelivering a whole batch is likewise an
idempotent no-op). An index **gap** or any failing applied sub-transition still reverts
the whole batch. The guard latch is held across the whole batch — re-entering either
entrypoint from inside a batch reverts.

What batching amortizes, per transition at batch size N: the 21,000 intrinsic (÷N), the
once-per-tx cold warm-up of the registry account + base slots (~10.5k, ÷N), and the SDK's
own config-slot cold accesses. Sub-transitions after the first verify at warm prices
(6,512 vs 17,033). It does **not** amortize per-non-signer costs (each sub-verify pays its
own non-signer subtraction) — collapsing the verify itself to one signature over a batch
digest would, but that changes the signing protocol and is out of scope.

ERC-165: `ISchnorrGasKillerSDK` is untouched (still single-function, its interfaceId still
exactly the `verifyAndUpdate` selector `0x82b35a01` the router preflight probes). The
batch + latch surface is a separate `ISchnorrGasKillerSDKBatch`, additionally reported by
`supportsInterface` — additive, nothing existing re-keys. Router-side batch submission
(accumulating sequential heights into one tx) is the natural follow-up and is off-chain
work only.

## 4. Operator-record packing

`SchnorrStakeRegistry.Operator` packs `weight` (now `uint96`, EigenLayer's stake width)
with `registered` into one slot: 3 slots per record instead of 4. The verification loop
reads a full record per non-signer, so this saves one cold SLOAD (−2.1k) per non-signer in
the standalone-tx context — measured cold marginal dropped 12,162 → 10,194. Warm marginal
is ~+40 (packed-field extraction), the right trade since the cold context is what
standalone receipts pay. `registerOperator`'s ABI is unchanged (`uint256 weight` parameter,
bounds-checked → `WeightOverflow`); the `operators` getter's ABI narrows `weight` to
`uint96` (not read by any Rust binding).

The Rust-embedded deploy artifacts (`scripts/bindings/abis/SchnorrStakeRegistry.json`,
`SchnorrArraySummationFactory.json`, `common/src/bindings/abis/SchnorrGasKillerSDK.json`)
are regenerated from this build — the e2e deploys the registry and the example app from
those JSONs, so they must track the Solidity or the live stack silently deploys stale
bytecode.

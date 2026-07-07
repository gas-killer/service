# Schnorr Nonce-Commitment Registry — non-interactive aggregate signing

Status: **plan** (no implementation yet). Branch: `Rubydusa/schnorr-nonce-registry`,
building on `RonTuretzky/schnorr-aggregate-signatures`.

## 1. Motivation

The current aggregate-Schnorr mode is a two-round interactive MuSig2 protocol over
commonware p2p (channel 2), per `(height, attempt)` session:

```text
router --NonceRequest{h,a}-----------------------------> all operators
node   --NonceCommit{h,a,pubkey,nonce}-----------------> router
router --SignRequest{h,a,digest,signers,aggR1R2,Raddr}-> subset S
node   --PartialSig{h,a,partial}-----------------------> router
```

The first round exists only to distribute fresh nonce pairs — it is **message-independent**
(`common/src/schnorr/musig.rs` already documents that round 1 "can be pre-processed off the
critical path"). Every signing session therefore pays one extra round trip and couples
liveness to a synchronous exchange: a node that misses the nonce round is a non-signer for
the attempt even if it is perfectly able to sign.

This plan removes the online nonce round entirely. Operators pre-commit large batches of
nonce points; a short commitment goes **on-chain** into a new `SchnorrNonceRegistry`
(read by nodes the same way they read stakes from the EigenLayer/stake registries); the
full nonce points travel **p2p only** and are verified against the on-chain commitment.
When a task proliferates, every node can independently determine which nonce slot every
operator uses, reconstruct the aggregate nonce `R`, compute the challenge, and emit its
partial signature immediately — zero coordination rounds in the good case.

What does **not** change: the crypto core (`musig.rs` equations, `b`/`e` derivations), the
final on-chain artifact `(s, Raddr, nonSigners, refBlock)`, `SchnorrVerify`, and
`SchnorrStakeRegistry.isValidSignature`. The chain never sees individual nonces; the new
contract is pure commitment bookkeeping.

## 2. Design at a glance

```text
── provisioning (rare, off critical path) ─────────────────────────────────────
operator: seed → derive N slots of (k1,k2); build Merkle root over (R1,R2) leaves
operator → SchnorrNonceRegistry.registerBatch(root, count, keySig)   [1 tx, ~1 SSTORE]
operator → p2p broadcast of the full batch (N × 66 bytes of points)
peers:    verify batch against on-chain root; persist

── signing (per task, good case: ZERO extra rounds) ───────────────────────────
task for height h proliferates (channel 1, unchanged)
every node:  idx = slot(h, attempt=0); S₀ = operators with coverage at idx
             reconstruct R1agg/R2agg from everyone's committed points at idx
             b, e, partial s_i  → broadcast PartialSig{h,0,s_i}
router:      verify each partial against the sender's committed nonce; assemble; submit

── fallback (some operator down): one directed round, still no nonce round ────
router → SignRequest{h, a≥1, digest, signers=S_a} → S_a
nodes  → PartialSig{h, a, s_i}   (fresh slot idx = slot(h, a) — never reuse)
```

The signing-time shape recovers the ECDSA mode's "node unilaterally signs on task
resolution" latency while keeping constant-gas on-chain verification.

## 3. Cryptography (unchanged core, preprocessed round 1)

Per signer `i` with key `x_i` and slot nonces `(k1, k2)`, `R_{i,j} = k_j·G`
(all conventions exactly as implemented today in `common/src/schnorr/`):

```text
R1agg = Σ_{i∈S} R_{i,1}          R2agg = Σ_{i∈S} R_{i,2}
b = H(NONCE_COEFF_TAG ‖ R1agg_c ‖ R2agg_c ‖ Xagg_c ‖ m) mod n
R = R1agg + b·R2agg              Raddr = eth_address(R)
e = keccak256(Xx ‖ Xparity ‖ m ‖ Raddr) mod n
s_i = k1 + b·k2 − e·x_i          s = Σ s_i     verify: R == s·G + e·Xagg
```

Why this stays secure with nonces committed long before the message is known:

* **MuSig2 was designed for exactly this.** The two-point nonce plus the coefficient `b`
  (which binds the aggregate nonces, aggregate key, *and message*) is what defeats
  Drijvers/ROS/Wagner-style attacks on pre-shared nonces; the MuSig2 security proof
  (Nick–Ruffing–Seurin 2020) explicitly covers preprocessing round 1 before the message
  exists, including fully concurrent sessions. FROST ships the same commit-in-advance
  pattern. A single-point precommitted nonce would be broken; the two-point scheme is the
  load-bearing reason this plan works without new cryptography.
* **Rogue keys** stay defeated by the existing proof-of-possession at
  `SchnorrStakeRegistry.registerOperator` (plain-sum aggregation, `a_i = 1`, unchanged).
* An adversary who registers their nonce batch *after* seeing honest batches learns
  nothing useful: `b` and `e` depend on the message and the full sums, which is precisely
  the adversarial setting the MuSig2 proof already covers (adversary picks its nonces
  after seeing the honest ones).

## 4. Slot assignment: deterministic, monotone, collision-free

Everyone must be able to predict everyone else's slot with **no local discretion** —
otherwise reconstructing `R` requires hearing each signer's choice, which is interactivity
again. Slots are addressed absolutely per operator:

```text
idx(h, a) = h · MAX_ATTEMPTS + a        a ∈ [0, MAX_ATTEMPTS)
```

* `h` is the sequencer height — already globally agreed and strictly increasing; the
  session key `(height, attempt)` maps 1:1 onto a slot, so the participant's existing
  `Session` state machine (issued → signing → signed/refused, fingerprint idempotency)
  transfers wholesale.
* Injective by construction: no collisions, no abstain-on-collision liveness loss.
* Slots for attempts that never run are simply burned — never used is always safe. Worst
  case provisioning is `MAX_ATTEMPTS ×` the height rate; at 66 bytes/slot this is noise.
* The "shared randomness from the request" lives in `b`/`e` (both bind the task digest);
  the *index* deliberately does not depend on request content, so a request-grinding
  adversary cannot steer which slots get consumed.

Rejected alternative — `idx = H(request digest)`: birthday collisions force honest
abstentions (~√N requests in), colliding slots create reuse temptation, and it buys
nothing since slots are interchangeable.

## 5. On-chain: `SchnorrNonceRegistry`

An authenticated bulletin board of batch commitments. The chain **never** opens a
commitment and never sees a nonce point; verification of batch contents is done by peers
off-chain (Merkle paths exist for targeted repair and any future fraud-proof extension).

```solidity
struct Batch { bytes32 root; uint64 startSlot; uint64 count; }

/// batches[operator] is append-only and contiguous:
/// batch k covers [startSlot, startSlot + count) with startSlot = previous end (0 for k=0).
mapping(address operatorId => Batch[]) public batches;

function registerBatch(
    uint256 x, uint256 y,        // operator schnorr key (must be registered in stake registry)
    bytes32 root, uint64 count,
    uint256 sigS, address sigR   // single-key Schnorr sig over the batch message, PoP-style
) external;

function coverage(address operatorId) external view returns (uint64 end);
function batchAt(address operatorId, uint64 slot) external view returns (bytes32 root, uint64 offset);

event NonceBatchRegistered(address indexed operatorId, uint64 indexed batchIndex,
                           uint64 startSlot, uint64 count, bytes32 root);
```

* **Authentication is cryptographic, not `msg.sender`-based**: the registration carries a
  single-key Schnorr signature (verified with the existing `SchnorrVerify`, exactly like
  the PoP path) over
  `batchMsg = keccak256(BATCH_TAG ‖ chainid ‖ address(this) ‖ operatorId ‖ batchIndex ‖ startSlot ‖ count ‖ root)`
  with `BATCH_TAG = "gas-killer/schnorr/nonce-batch/v1"`. Anyone can relay the tx; the
  binding is to the operator key. `chainid`/registry address in the preimage kill
  cross-deployment replay of old batch registrations.
* The contract requires the key to be registered in `SchnorrStakeRegistry` (add a view to
  `ISchnorrStakeRegistry`; `operators` is already a public mapping).
* **Append-only ⇒ no watermark needed**: coverage for already-assigned slots can never be
  mutated, so reads at any block are stable — simpler than the stake registry's
  `effectiveBlock` fail-close (which still governs the operator *set* itself).
* Merkle leaf: `keccak256(LEAF_TAG ‖ operatorId ‖ absoluteSlot ‖ R1_compressed ‖ R2_compressed)`,
  fixed-depth positional tree (pad with a domain-separated empty leaf), keccak inner nodes.
  Binding the absolute slot into the leaf prevents cross-position replay of points.
* Gas: one array push + event per batch; amortized over `count` signatures ⇒ ~0.

## 6. Off-chain changes

### 6.1 Nonce derivation, storage, and the spent watermark (the critical invariant)

```text
k_{slot,j} = scalar(keccak256(NONCE_SEED_TAG ‖ chainid ‖ registry ‖ operatorId ‖ batchSeed ‖ slot ‖ j))
```

* `batchSeed` is fresh OS entropy per batch, stored encrypted-at-rest **in the same
  keystore class as the signing key** — a leaked seed plus one later-published partial
  reveals the private key (`x_i = (k_eff − s_i)/e`), so seed secrecy ≡ key secrecy.
  Deriving (rather than storing 2N scalars) keeps the secret footprint at 32 bytes and
  makes batch generation crash-reproducible.
* **INVARIANT N1 — one partial per slot, ever.** Today the node is safe-by-amnesia:
  sessions are memory-only, so a restart forgets secret nonces and refuses in-flight
  sessions. Deterministic derivation destroys that property — after a restart the node can
  recompute every `k`. Therefore a **persisted spent-slot watermark is mandatory**: append
  `(slot, context fingerprint)` to a journal and **fsync before emitting the partial**
  (write-ahead). On any ask for `slot ≤ watermark` with a different fingerprint: refuse
  loudly. This also finally discharges the "persist a nonce-spent marker" TODO in
  `musig.rs`'s docs, which the interactive mode left to restart-amnesia.
* Signing the *same message* twice under *different* slots (attempt 0 then attempt 1) is
  safe and expected — the invariant is per-nonce, not per-message.

### 6.2 Batch gossip & sync

New channel-2 messages (or a channel 3): `BatchAnnounce` (chunked points for a registered
batch), `BatchRequest` (pull missing ranges from the operator or any peer). Receivers
recompute the Merkle root and accept only if it matches the on-chain registration
(triggered by the `NonceBatchRegistered` event / registry polling — the same
read-the-registry pattern as stakes). Verified batches are persisted locally (they are
public data; loss is repairable by re-fetch). Every operator needs every other operator's
points to compute `b` — batch distribution is a prerequisite of signing, not an
optimization. `PartialSig` can carry `(R1, R2, merkle path)` (~480 bytes at depth 12) so
the coordinator can verify a partial even before holding the sender's full batch.

### 6.3 Wire protocol changes

* **Delete** `NonceRequest` / `NonceCommit` from the signing path.
* **Attempt 0 is implicit**: on resolving height `h`'s digest locally (existing
  TaskBook/DigestResolver path), a node computes `S₀` = operators with on-chain coverage
  at `idx(h,0)` (over the operator set it already tracks), reconstructs the context, and
  unilaterally broadcasts `PartialSig{h, 0, s_i}`. No router message needed beyond the
  existing task announcement.
* **Attempts ≥ 1 keep `SignRequest`** (router picks `S_a` = attempt-`a−1` responders,
  minus attributed-bad partials, as today) — but with committed nonces there is nothing to
  collect first; `agg_nonces`/`r_addr` in the request become a redundant cross-check that
  signers still recompute locally (keep them: they preserve today's "lying coordinator"
  refusal test shape). Partials are bound to `(h, a)` and never mix across attempts
  (different `e`).
* Coordinator: `verify_partial` against the sender's *committed* slot points instead of a
  session `NonceCommit`; `build_context`/`assemble` unchanged.
* Set-consistency note: if nodes briefly disagree on `S₀` (e.g. right after a batch
  registration lands), their partials disagree, attempt 0 fails to assemble, and attempt 1
  with an explicit signer list self-heals — same failure mode and remedy as an offline
  operator. Mismatch between the signing-time set and the `refBlock` the submitter later
  picks fails closed on-chain (`StaleSnapshot`), exactly as today.

## 7. Security analysis

| Threat | Defense |
|---|---|
| Nonce reuse (key extraction) | Invariant N1: derived-slot determinism + fsync'd spent watermark + fingerprint idempotency; slots injective per `(h,a)` by construction |
| ROS/Wagner on pre-shared nonces, concurrent sessions | Two-point nonces + `b` binding (MuSig2 with preprocessing — proven setting) |
| Rogue key | Existing PoP at stake-registry registration (unchanged) |
| Adaptive batch registration (nonces chosen after seeing honest batches) | Covered by MuSig2 adversary model; `b`, `e` bind message + sums |
| Copying another operator's points into own batch | Can't produce a valid partial without `k` ⇒ self-DoS only |
| Same points in two own slots | Self-harm only (leaks own key if both signed); honest derivation never does it |
| Coordinator equivocation (many attempts / conflicting sets for one height) | Each slot signs once (N1); equivocation only burns the victim's slots — bounded by `MAX_ATTEMPTS` per height |
| Restart replay | WAL watermark consulted before every partial; journal fsync precedes send |
| Cross-deployment / cross-chain replay of batches or registrations | `chainid` + registry address in seed derivation, leaf tag, and `batchMsg` |
| Batch withholding / unavailability | Withholder simply can't be included (peers lack its points) ⇒ it is a non-signer; threshold absorbs; no safety impact |
| Stale registry view | Stake-registry `effectiveBlock` fail-close unchanged; nonce registry is append-only so covered slots are immutable |

## 8. Liveness & operations

* **Exhaustion**: an operator whose coverage ends before `idx(h,a)` abstains (deterministic,
  visible to everyone) until it registers the next batch. No interactive fallback — one
  protocol, one code path; the stake threshold absorbs stragglers.
* **Auto re-provisioning**: node registers batch `k+1` when consumption crosses ~50% of
  batch `k` (parameter). With `N = 2^16` slots and `MAX_ATTEMPTS = 4`, one batch covers
  ≥ 16k heights (~22h at 1 height/5s worst case) for a ~4.3 MB one-time gossip.
* **Parameters** (initial proposals): `N = 65536` slots/batch, `MAX_ATTEMPTS = 4`
  (coordinator already deadline-bounds attempts via `ROUND_TIMEOUT`), re-register at 50%.
* **Metrics**: slots remaining per operator, attempt-0 assembly rate, abstention causes
  (no-coverage vs offline vs refused), watermark lag.

## 9. Implementation plan

1. **Contract + parity fixtures.** `SchnorrNonceRegistry.sol` (registration with key-sig
   auth via `SchnorrVerify`, contiguity, views, events) + `ISchnorrStakeRegistry`
   registered-lookup; Foundry tests (auth, replay, contiguity, coverage queries). Rust
   `common/src/schnorr/precommit.rs`: seed→slot derivation, Merkle builder/prover, batch
   message parity fixture against the contract (extend `schnorr_parity_fixture.rs`).
2. **Batch lifecycle.** Node-side batch generation + registration tooling (extend
   `generate_key`/deploy scripts), gossip messages + persistence + root verification on
   both node and router, spent-slot WAL with fsync-before-send and restart tests.
3. **Signing path swap.** Participant: implicit attempt-0 partial on digest resolution;
   slot-derived nonces replace `Session::Issued`; coordinator: collect-first flow for
   attempt 0, committed-nonce `verify_partial`, explicit-set `SignRequest` for attempts ≥ 1;
   delete `NonceRequest`/`NonceCommit`; adapt e2e + Helm flows (keep the interactive mode
   selectable via `SIGNATURE_SCHEME` until e2e parity, then remove).
4. **Hardening.** Chaos tests (restart mid-sign ⇒ WAL refusal; batch withholding;
   equivocating coordinator burns ≤ MAX_ATTEMPTS slots), re-provisioning automation,
   metrics, gas + latency snapshots vs the interactive baseline.

## 10. Alternatives considered

* **FROST / threshold Schnorr** — fixed group key, any t-of-n, no subtraction; rejected:
  requires DKG + resharing on churn, changes the trust model, and the registry's
  non-signer-subtraction design already fits EigenLayer-style dynamic operator sets.
* **Riding the commonware aggregation engine as a custom `certificate::Scheme`** — with
  signing now non-interactive, attempt-0 partials are ack-shaped, but the engine's trait
  contract doesn't fit MuSig2: `Scheme::assemble` must form a certificate from *any*
  threshold-sized subset of attestations, while MuSig2 partials are set-bound (each bakes
  `X_agg(S)`/`R_agg(S)` into `e`/`b`, so only exactly-`S` assembles and one offline
  operator invalidates all collected partials); acks are keyed `(height, digest)` with no
  attempt axis for reduced-set retries; `sign(&self, …)` is stateless while our signing
  consumes watermark-gated nonce state; and partial verification needs committed nonce
  points, not just participant keys. The engine's fixed N3f1 quorum also mismatches the
  stake-weighted threshold. Schnorr mode therefore keeps the custom channel-2 actors
  (engine remains ECDSA-mode-only, sequencer shared — same split as today).
* **Single-point precommitted nonces** — broken (ROS/Wagner); the two-point `b` scheme is
  non-negotiable.
* **Storing nonce points on-chain** — 66 B × operators × slots of calldata/storage;
  defeats the purpose. Excluded by design brief.
* **Hash-of-concatenation commitment (no Merkle)** — no per-slot opening for repair or
  future fraud proofs; Merkle costs nothing extra here.
* **`H(request)`-indexed slots** — birthday collisions ⇒ abstentions/reuse pressure (§4).

## 11. Open questions

1. `MAX_ATTEMPTS` and batch size defaults (gossip size vs re-registration cadence).
2. Should skip heights consume/burn slots implicitly (currently no skip signing session
   exists, so no)? Revisit if skip certificates ever become consumed downstream.
3. Batch retirement: allow an operator to void *unused future* coverage (e.g. suspected
   seed compromise) — an append-only "tombstone from slot X" record; adds a mutation ⇒
   would need an `effectiveBlock`-style watermark after all. Defer unless needed.
4. Fraud proofs for provably-malformed batches (Merkle path vs on-curve check on-chain):
   cheap to add later because leaves already bind `(operator, slot, points)`; not needed
   for safety (malformed ⇒ operator just can't sign).

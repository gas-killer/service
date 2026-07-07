# Schnorr Nonce-Commitment Registry — non-interactive aggregate signing

Status: **phases 1–3 implemented + phase-4 groundwork**; live node/router wiring pending.
Branch: `Rubydusa/schnorr-nonce-registry`, building on
`RonTuretzky/schnorr-aggregate-signatures`.

## Implementation status

Done (all tested; `cargo test --workspace` + `forge test` green):

| Piece | Where |
|---|---|
| `SchnorrNonceRegistry` contract (§5) + interfaces | `contracts/src/SchnorrNonceRegistry.sol`, `interface/ISchnorrNonceRegistry.sol`, `ISchnorrStakeRegistry.operators` |
| Foundry suite w/ in-Solidity Schnorr signer + Rust parity vector | `contracts/test/SchnorrNonceRegistry.t.sol` |
| Slot mapping, seed→nonce derivation, Merkle batches, batch message (§4, §6.1) | `common/src/schnorr/precommit.rs` |
| `SchnorrScheme` (`certificate::Scheme`, two-form certificate) (§7) | `common/src/schnorr/scheme.rs` (incl. engine smoke test + completion-round crypto proof) |
| Durable spend journal (invariant N1, fsync write-ahead, torn-tail recovery) | `common/src/schnorr/journal.rs` |
| Batch gossip + completion wire messages (§6.2–6.3) | `common/src/schnorr/wire.rs` (`PrecommitMsg`) |
| Deploy tooling: registry deploy + per-operator batch-0 commitment | `scripts/deploy_array_summation.rs` (`SCHNORR_NONCE_REGISTRY_ADDRESS`, `SCHNORR_NONCE_BATCH_SLOTS`), `scripts/bindings/schnorrnonceregistry.rs` |
| Batch gossip store (chunk reassembly, self-authenticating verification, serve/re-announce) | `common/src/schnorr/batches.rs` (`BatchStore`) |
| Completion round: context/sign/verify on the scheme + router-side collector | `scheme.rs` (`completion_context`/`sign_completion`), `batches.rs` (`CompletionCollector`) — end-to-end tested through the production API |
| On-chain startup reads (operator points, batch metadata) | `common/src/schnorr/onchain.rs` (`load_operator_keys`, `load_batches`) |

Note: batch seeds derive from the operator key (`precommit::derive_batch_seed`), so the
node recomputes its secrets from the key file + on-chain batch metadata — no second
secret to distribute (seed ≡ key exposure either way, §6.1).

Remaining (phase 4 §10.4 + phase 5) — the actor/binary shell around the tested layer;
all protocol logic already lives in `common` so these are thin I/O adapters:

* Mode wiring (`SIGNATURE_SCHEME=schnorr-precommit`): in `node/src/main.rs` /
  `router/src/main.rs`, build the scheme from `onchain::load_operator_keys` +
  `load_batches` (envs: `SCHNORR_STAKE_REGISTRY_ADDRESS`, `SCHNORR_NONCE_REGISTRY_ADDRESS`;
  chain id via `eth_chainId`), `SeedSecrets` from `derive_batch_seed(key, i)` per on-chain
  batch, `FileSpendJournal` under `STORAGE_DIR`, and instantiate the aggregation engine
  exactly as the ECDSA arm does (`ConstantProvider` + `StaticEpochMonitor` are
  scheme-generic; reporters only touch `certificate.item`, so genericizing them is
  mechanical).
* Precommit actor on channel 2 (both binaries): broadcast own `BatchStore::chunk_batch`
  chunks at startup, `BatchStore::ingest` announces / `serve` requests; node side answers
  `Attested` certificates (tapped from the reporter) with
  `SchnorrScheme::sign_completion` → `CompletionPartial`; router side runs one
  `CompletionCollector` per `Attested` certificate and emits `SchnorrCertified` to the
  existing schnorr submitter (stake-threshold gate before submission). Optional
  hardening: pin gossiped roots against `onchain::load_batches` (closes the
  equivocation-liveness nuisance documented in `batches.rs`).
* Own-batch auto re-registration at the consumption threshold (`registerBatch` from the
  node, batch index `i` seeded by `derive_batch_seed(key, i)`).
* Delete `NonceRequest`/`NonceCommit` + the interactive coordinator/participant actors
  once e2e parity holds; e2e/Helm flow updates; chaos tests (§10.5).

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
             b, e, partial s_i  → the partial IS the node's aggregation-engine
             ack for (h, digest) — TipAck gossip on channel 0, engine-native
everyone:    engine collects/verifies acks; at S₀ complete, `assemble` emits the
             final aggregate certificate (s, Raddr) locally; router submits

── fallback (some operator down): one deterministic completion round ──────────
engine certifies a quorum bitmap S₁ ⊊ S₀ (partial-bundle certificate)
members of S₁: re-sign for exactly S₁ at slot(h, a=1) → gossip channel 2
(no coordinator message needed — S₁ is read from the certificate everyone holds)
```

The signing-time shape recovers the ECDSA mode's "node unilaterally signs on task
resolution" latency while keeping constant-gas on-chain verification — and certificate
formation rides the same commonware aggregation engine PR #299 migrated to (§7), instead
of the custom channel-2 session actors.

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
  `Session` invariants (signed/refused terminality, fingerprint idempotency) transfer
  directly into the `SchnorrScheme` and completion actor (§7).
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
any verifier can check a partial even before holding the sender's full batch.

### 6.3 Wire protocol changes

* **Delete** `NonceRequest` / `NonceCommit` — and with them the entire custom signing
  session protocol. The attempt-0 partial becomes the node's aggregation-engine ack (§7);
  the only remaining channel-2 traffic is batch gossip (§6.2) and the deterministic
  completion round (§7.3).
* Partials are bound to `(h, a)` and never mix across attempts (different `e`).
* Set-consistency note: if nodes briefly disagree on `S₀` (e.g. right after a batch
  registration lands), their attempt-0 partials disagree, no full-set aggregate forms, and
  the completion round over the engine-certified bitmap self-heals — same failure mode and
  remedy as an offline operator. Mismatch between the signing-time set and the `refBlock`
  the submitter later picks fails closed on-chain (`StaleSnapshot`), exactly as today.

## 7. Riding the commonware aggregation engine

The interactive protocol could not use the engine — a nonce round-trip does not fit the
one-shot ack model, which is why the current Schnorr mode ships custom channel-2 actors
that re-implement sessions, retries, pruning, and restart semantics by hand. Pre-committed
nonces remove that blocker: signing becomes a deterministic one-shot per height, i.e.
exactly ack-shaped. This plan therefore brings Schnorr mode back onto
`commonware_consensus::aggregation::Engine` (2026.5.0, scheme-generic), following the
architecture PR #299 established for the BN254→engine migration and the `EcdsaScheme`
precedent (`common/src/ecdsa/scheme.rs`): nodes run signing engine instances on channel 0,
the router runs a verifier-only instance (`me() == None`), the TaskBook/automaton derives
each height's digest locally, and the engine provides TipAck gossip, rebroadcast,
journaling/restart recovery, epochs, tip skip-ahead, and attribution — all of which the
custom actors currently hand-roll.

### 7.1 The trait mismatch, stated precisely

`certificate::Scheme::assemble` must form a certificate from **any** quorum-sized subset
of verified attestations. MuSig2 partials are set-bound: each bakes `X_agg(S₀)` and
`R_agg(S₀)` into `e`/`b`, so only *exactly-S₀* sums to a valid signature. One offline
operator invalidates every collected partial for that item, and acks have no attempt axis.
A naive "partial = attestation, aggregate = certificate" scheme is unsound under the trait
contract.

### 7.2 Resolution: `SchnorrScheme` with a two-form certificate

```rust
type Signature   = SchnorrPartial;          // s_i for context (h, a=0, S₀) — 32 bytes
enum SchnorrCertificate {
    /// subset == S₀: the final aggregate, on-chain-submittable as-is.
    Aggregate { s: Scalar, r_addr: Address, signers: Signers },
    /// quorum ⊊ S₀: attributable partial bundle — certifies WHICH set signed,
    /// pending completion (§7.3). Attestations are individually verifiable.
    Attested  { partials: Vec<(Participant, Scalar)>, signers: Signers },
}
```

* `sign(subject)` derives `idx(h, 0)`, consults the fsync'd watermark (N1), computes the
  partial against the deterministic full committed set `S₀`, persists `(slot, partial)`,
  and returns it. Re-asked for an already-signed item it returns the **cached** partial
  (idempotent — same slot, same context; this is today's participant re-send rule moved
  into the scheme). Verifier-only instances return `None`, as `EcdsaScheme` does.
* `verify_attestation` checks `s_i·G == R1_i + b·R2_i − e·X_i` from the sender's
  *committed* slot points — pure public data held by the scheme (today's
  `Coordinator::verify_partial`, verbatim). Bad partials are attributed and blocked by the
  engine per participant, replacing the coordinator's exclusion bookkeeping.
* `assemble` sums iff the attestation set covers `S₀` exactly (common case: emits the
  final `Aggregate`, self-verified against the on-chain identity); otherwise it emits
  `Attested` at engine quorum. Both are honest certificates for "this set attested to this
  digest"; `verify_certificate` verifies the aggregate identity resp. each partial.
* Scheme instances are built per epoch from on-chain reads (stake-registry operator set +
  nonce-registry coverage) via the engine's provider hook — the same place `EcdsaScheme`
  gets its ordered participant set; participant indices are positions in the ordered
  operator-address set, exactly as in ECDSA mode.

### 7.3 Completion round (only when the certificate is `Attested`)

An `Attested{signers: S₁}` certificate is the **set agreement** the interactive protocol
needed a coordinator for: everyone locally recovers the same `S₁` from the engine. Each
member of `S₁` then emits one follow-up partial for context `(h, a=1, S₁)` at slot
`idx(h,1)` on channel 2 — no request message, no coordinator; a tiny completion actor
collects them (verifying with the same committed-nonce check) and assembles the final
aggregate for the submitter. If an `S₁` member dies between acking and completing (rare —
it just acked), bounded idempotent re-requests, then the height falls to the deadline/skip
path as today. `MAX_ATTEMPTS` bounds the shrink-and-retry (`S₂ ⊂ S₁`, slot `idx(h,2)`, …).

### 7.4 Consequences and integration risks

* **Deleted**: `router/src/schnorr_coordinator.rs` and `node/src/schnorr_participant.rs`
  session machinery (≈700 lines of hand-rolled retry/prune/restart logic) — replaced by
  the engine + `SchnorrScheme` + the small completion actor. The router regains full
  symmetry with ECDSA mode (verifier-only engine, sequencer, submitter).
* **Quorum semantics**: the engine's threshold is its fixed N3f1 participant count (PR
  #299 note); the *stake-weighted* threshold stays authoritative on-chain and is
  re-checked by the submitter before submission — unchanged from today, but now an
  `Attested`/`Aggregate` certificate can exist whose stake weight is insufficient; the
  submitter must treat that as "keep waiting for a bigger set", not submit-and-revert.
* **Journal replay vs the watermark** (verify during implementation): on restart the
  engine replays journaled activities and may re-request signatures. The scheme's
  cached-partial rule makes replay idempotent, and the watermark gate refuses any
  *different* context for a consumed slot — restart must never re-derive a partial for a
  new context on an old slot. This interplay is the one place engine semantics touch
  invariant N1 and needs a dedicated chaos test.
* **Digest disagreement across nodes** (task-validity dispute): conflicting acks for a
  height simply never reach quorum on either digest — no certificate, deadline/skip path,
  same as ECDSA mode. The watermark burns slot `idx(h,0)` on first sign regardless
  (signing a different digest for `h` later is exactly what N1 forbids).

## 8. Security analysis

| Threat | Defense |
|---|---|
| Nonce reuse (key extraction) | Invariant N1: derived-slot determinism + fsync'd spent watermark + fingerprint idempotency; slots injective per `(h,a)` by construction |
| ROS/Wagner on pre-shared nonces, concurrent sessions | Two-point nonces + `b` binding (MuSig2 with preprocessing — proven setting) |
| Rogue key | Existing PoP at stake-registry registration (unchanged) |
| Adaptive batch registration (nonces chosen after seeing honest batches) | Covered by MuSig2 adversary model; `b`, `e` bind message + sums |
| Copying another operator's points into own batch | Can't produce a valid partial without `k` ⇒ self-DoS only |
| Same points in two own slots | Self-harm only (leaks own key if both signed); honest derivation never does it |
| Malicious completion requests (conflicting sets/attempts for one height) | Completion sets derive from the engine certificate, not a request message; each slot signs once (N1) — worst case burns ≤ `MAX_ATTEMPTS` slots per height |
| Restart replay | WAL watermark consulted before every partial; journal fsync precedes send |
| Cross-deployment / cross-chain replay of batches or registrations | `chainid` + registry address in seed derivation, leaf tag, and `batchMsg` |
| Batch withholding / unavailability | Withholder simply can't be included (peers lack its points) ⇒ it is a non-signer; threshold absorbs; no safety impact |
| Stale registry view | Stake-registry `effectiveBlock` fail-close unchanged; nonce registry is append-only so covered slots are immutable |

## 9. Liveness & operations

* **Exhaustion**: an operator whose coverage ends before `idx(h,a)` abstains (deterministic,
  visible to everyone) until it registers the next batch. No interactive fallback — one
  protocol, one code path; the stake threshold absorbs stragglers.
* **Auto re-provisioning**: node registers batch `k+1` when consumption crosses ~50% of
  batch `k` (parameter). With `N = 2^16` slots and `MAX_ATTEMPTS = 4`, one batch covers
  ≥ 16k heights (~22h at 1 height/5s worst case) for a ~4.3 MB one-time gossip.
* **Parameters** (initial proposals): `N = 65536` slots/batch, `MAX_ATTEMPTS = 4`
  (the deadline/skip path bounds attempts via `ROUND_TIMEOUT`), re-register at 50%.
* **Metrics**: slots remaining per operator, attempt-0 assembly rate, abstention causes
  (no-coverage vs offline vs refused), watermark lag.

## 10. Implementation plan

1. **Contract + parity fixtures.** `SchnorrNonceRegistry.sol` (registration with key-sig
   auth via `SchnorrVerify`, contiguity, views, events) + `ISchnorrStakeRegistry`
   registered-lookup; Foundry tests (auth, replay, contiguity, coverage queries). Rust
   `common/src/schnorr/precommit.rs`: seed→slot derivation, Merkle builder/prover, batch
   message parity fixture against the contract (extend `schnorr_parity_fixture.rs`).
2. **Batch lifecycle.** Node-side batch generation + registration tooling (extend
   `generate_key`/deploy scripts), gossip messages + persistence + root verification on
   both node and router, spent-slot WAL with fsync-before-send and restart tests.
3. **`SchnorrScheme` for the aggregation engine** (§7). Implement
   `certificate::Scheme` in `common/src/schnorr/scheme.rs` mirroring `EcdsaScheme`
   (partial = attestation; two-form `Aggregate`/`Attested` certificate; committed-nonce
   `verify_attestation`; cached-partial idempotent `sign` gated by the watermark).
   Unit tests at the scheme level (quorum boundaries, subset ≠ S₀ ⇒ `Attested`, replay
   idempotency), mirroring the `EcdsaScheme` test suite.
4. **Engine wiring + completion actor.** Schnorr mode instantiates the engine on
   channel 0 (nodes signing, router verifier-only) per the PR #299 architecture;
   epoch provider reads stake + nonce registries; completion actor on channel 2 for
   `Attested` certificates; submitter consumes `Aggregate` certificates (stake-threshold
   gate before submission); **delete** `NonceRequest`/`NonceCommit` and the custom
   coordinator/participant actors; adapt e2e + Helm flows (keep the interactive mode
   selectable via `SIGNATURE_SCHEME` until e2e parity, then remove).
5. **Hardening.** Chaos tests (restart mid-sign / journal replay ⇒ WAL refusal, §7.4;
   batch withholding; completion-round death ⇒ shrink-and-retry burns ≤ MAX_ATTEMPTS
   slots), re-provisioning automation, metrics, gas + latency snapshots vs the
   interactive baseline.

## 11. Alternatives considered

* **FROST / threshold Schnorr** — fixed group key, any t-of-n, no subtraction; rejected:
  requires DKG + resharing on churn, changes the trust model, and the registry's
  non-signer-subtraction design already fits EigenLayer-style dynamic operator sets.
* **Keeping the custom channel-2 session actors (engine stays ECDSA-only)** — the
  original shape of this plan, rejected once the trait mismatch was resolved: a naive
  "partial = attestation, aggregate = certificate" scheme is unsound because
  `Scheme::assemble` may receive *any* quorum subset while MuSig2 partials only sum over
  exactly-`S₀`, but the two-form certificate (§7.2) makes every assemble outcome an honest
  certificate and moves reduced-set handling to a deterministic completion round (§7.3).
  Given that, keeping ~700 lines of hand-rolled session/retry/restart machinery instead of
  the engine's journaled, epoch-aware, attributing implementation is strictly worse.
* **Single-point precommitted nonces** — broken (ROS/Wagner); the two-point `b` scheme is
  non-negotiable.
* **Storing nonce points on-chain** — 66 B × operators × slots of calldata/storage;
  defeats the purpose. Excluded by design brief.
* **Hash-of-concatenation commitment (no Merkle)** — no per-slot opening for repair or
  future fraud proofs; Merkle costs nothing extra here.
* **`H(request)`-indexed slots** — birthday collisions ⇒ abstentions/reuse pressure (§4).

## 12. Open questions

1. `MAX_ATTEMPTS` and batch size defaults (gossip size vs re-registration cadence).
2. Should skip heights consume/burn slots implicitly (currently no skip signing session
   exists, so no)? Revisit if skip certificates ever become consumed downstream.
3. Batch retirement: allow an operator to void *unused future* coverage (e.g. suspected
   seed compromise) — an append-only "tombstone from slot X" record; adds a mutation ⇒
   would need an `effectiveBlock`-style watermark after all. Defer unless needed.
4. Fraud proofs for provably-malformed batches (Merkle path vs on-curve check on-chain):
   cheap to add later because leaves already bind `(operator, slot, points)`; not needed
   for safety (malformed ⇒ operator just can't sign).

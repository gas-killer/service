# Non-interactive aggregate Schnorr via half-aggregation

Branch: `Rubydusa/non-interactive-agg-schnorr` (based on `Rubydusa/schnorr-nonce-registry`
to reuse the aggregation-engine wiring: generic reporter tap, mode plumbing,
`CertificateInspect`; the nonce-registry components themselves are **not used** by this
mode). Status: **plan** — nothing implemented yet.

Technique: *half-aggregation* of Schnorr signatures — Chalkias, Garillot, Kondi,
Nikolaenko, "Non-interactive half-aggregation of EdDSA and variants of Schnorr
signatures" (CT-RSA 2021, ePrint 2021/350); also specified for BIP-340 in Blockstream
Research's cross-input-aggregation work ("Half-Aggregation of BIP-340 Signatures").
We use the construction, not the BIP encoding: our instantiation keeps this repo's
keccak/`ecrecover` (Scribe) conventions.

## 1. Why a third Schnorr mode

Both existing Schnorr modes are MuSig2-style: **one shared challenge** `e` computed over
the **aggregate** nonce `R = Σ Rᵢ` and aggregate key. That single design choice is what
buys constant-size on-chain verification — and what causes every operational problem the
modes have:

* the challenge cannot exist until *everyone's* nonce is known ⇒ either an interactive
  nonce round (`schnorr` mode) or the whole precommit apparatus (`schnorr-precommit`
  mode: on-chain registry, batch gossip and its data-availability/partition concerns,
  the S₀-consistency requirement, the fsync spend journal, the completion round when the
  certified set ⊊ S₀);
* the signer set must be fixed **before** signing, but the aggregation engine discovers
  it **after** (eager assembly at quorum) — hence the two-form
  `Aggregate`/`Attested` certificate and the completion round, which at realistic
  operator counts (n ≥ 4, quorum < n) is the *common* path, not the fallback.

Half-aggregation inverts the dependency. Each operator produces an ordinary,
fully independent Schnorr signature with its **own** nonce and its **own** challenge
`eᵢ = H(Xᵢ, m, Rᵢ)` — no shared randomness anywhere. Compression happens *after the
fact*, by anyone, over *any subset* of signatures, with no participation from the
signers. Aggregation becomes a pure post-processing step, so:

* no nonce round, no nonce registry, no batch gossip, no DA/partition surface;
* the signer set is whatever the engine certifies — native fit for eager-quorum
  assembly, no completion round, no `Aggregate`/`Attested` split;
* deterministic per-message nonces (RFC6979/BIP-340 style) are safe for single-signer
  Schnorr, so the spend journal and the one-partial-per-slot invariant disappear —
  restarts are safe *by construction*, statelessly.

The price: the certificate is **O(n)** (one nonce point per signer plus one scalar
instead of `n` scalars — "half" the size of trivial concatenation), and on-chain
verification is **O(n)** work instead of O(non-signers) subtraction + one `ecrecover`.
§5 quantifies this; it is a good trade for small-to-medium operator sets and removes an
entire class of liveness machinery.

| | `schnorr` (interactive) | `schnorr-precommit` | `schnorr-halfagg` (this) |
|---|---|---|---|
| Nonce coordination | 2-round online | on-chain registry + gossip | **none** |
| Nonce DA risk | per-request | batch withholding/partition (healed by relay+pull) | **none** |
| Signer set known | before signing | before signing (S₀) + completion round | **after** — any certified subset |
| Completion round | n/a (own protocol) | required when quorum ⊊ S₀ (common at n ≥ 4) | **never** |
| Nonce-reuse protection | amnesia (in-memory) | fsync spend journal (N1) | **stateless** (deterministic nonces) |
| Signature size | 64 B | 64 B | 32·(n+1) B + bitmap |
| On-chain verify | O(non-signers) + 1 ecrecover | same | O(signers) |
| Rogue-key defense | PoP required | PoP required | **not needed** (no key aggregation) |

## 2. The construction

All operators sign the same 32-byte message `m` (the engine item digest). Repo
conventions (`common/src/schnorr/mod.rs`): challenge
`e = keccak256(Xx ‖ Xp ‖ m ‖ Raddr) mod n`, signing `s = k − e·x`, so `R = s·G + e·X`.

**Sign (per operator, independent):**

```text
kᵢ = keccak256(HALFAGG_NONCE_TAG ‖ xᵢ ‖ Xᵢ ‖ m) mod n     (reject 0; deterministic)
Rᵢ = kᵢ·G
eᵢ = keccak256(Xᵢx ‖ Xᵢp ‖ m ‖ addr(Rᵢ)) mod n
sᵢ = kᵢ − eᵢ·xᵢ mod n
σᵢ = (Rᵢ, sᵢ)                                              (full point, 33 B compressed + 32 B)
```

This is exactly the existing single-signer verification identity — each σᵢ is
individually checkable with one `ecrecover` (Scribe trick) against operator i's
registered key.

**Aggregate (anyone, any subset, ex post).** Order signers ascending by participant
index (canonical). Compute Fiat–Shamir randomizers over the *entire* input list:

```text
ctx = keccak256(HALFAGG_CTX_TAG ‖ m ‖ X₁ ‖ R₁ ‖ … ‖ Xₙ ‖ Rₙ)
z₁  = 1
zᵢ  = keccak256(HALFAGG_Z_TAG ‖ ctx ‖ i) mod n            (i ≥ 2)
s̃   = Σ zᵢ·sᵢ mod n
Σig = (bitmap, [R₁ … Rₙ], s̃)
```

**Verify:**

```text
eᵢ from (Xᵢ, m, addr(Rᵢ)) as above
check  s̃·G  ==  Σ zᵢ·Rᵢ  −  Σ (zᵢ·eᵢ mod n)·Xᵢ
```

The randomizers `zᵢ` are what make this sound: without them an attacker controlling one
key could craft `sⱼ` values that cancel across the sum. With `zᵢ` drawn by Fiat–Shamir
over all `(Xᵢ, Rᵢ, m)`, forging the aggregate reduces to forging an individual Schnorr
signature (ROM; tight in ROM+AGM — Chen & Zhao, ESORICS 2022). `z₁ = 1` is the paper's
standard optimization. Because keys are never added together, there is **no rogue-key
attack surface** and PoP is not cryptographically required (we keep the registry's PoP
check anyway for uniformity).

Deviations from BIP-340 (deliberate, for EVM/`ecrecover` compatibility): keccak256
instead of SHA-256 tagged hashes, full points with explicit parity instead of x-only
even-Y keys, `addr(R)` instead of `Rx` inside the challenge (binds R at 160 bits — the
same binding Scribe's audited design accepts), and `s = k − e·x` sign convention.

## 3. Engine fit: `certificate::Scheme` maps natively

This is the part the precommit mode had to fight for. Half-agg gets it for free:

* `sign(subject)` → the independent σᵢ above. Deterministic; re-signing the same item
  after a restart reproduces the identical signature. No journal, no slots, no attempts.
* `verify_attestation` → one single-signer Schnorr check against participant i's key.
  No shared context — a verifier needs nothing but the registered keys. **No S₀, no
  view-consistency requirement**: two nodes can never produce "incompatible" partials.
* `assemble(attestations)` → run the aggregation step over whatever subset the engine
  collected. Any quorum works; eager assembly at quorum is exactly what we want. One
  certificate form: `HalfAggCertificate { signers: Signers, nonces: Vec<Point>, s̃ }`.
* `verify_certificate` → the MSM check (in Rust: one vartime multi-scalar
  multiplication, faster than n separate verifies).

The reporter tap / completion actor from precommit mode is unnecessary; the router's
`handle_certified` consumes the certificate directly (`needs_completion` is always
false). `CertificateInspect` gains a third impl.

## 4. On-chain verification

Signer-set stake accounting is direct: sum stake over the bitmap (no checkpointed
aggregate key, no non-signer subtraction — those exist only to serve the constant-size
aggregate-key equation). Two verification paths, decided per deployment:

**Path A — per-signer `ecrecover` loop (recommended default on L1).** The certificate's
components are individually verifiable, so the contract can check each σᵢ with the
existing Scribe identity: `ecrecover(−sᵢ·Xᵢx, 27+Xᵢp, Xᵢx, eᵢ·Xᵢx) == addr(Rᵢ)`. This
requires submitting individual `sᵢ` (52 B/signer calldata: `addr(Rᵢ)` 20 B + `sᵢ` 32 B)
and forgoes the compression on-chain — the half-agg form still serves the off-chain
certificate. ≈ 3.4k gas ecrecover + ~1.6k calldata + ~2.1k cold key SLOAD ≈ **~7k
gas/signer**; n = 10 → ~70k, n = 30 → ~210k (vs ~25k constant for the MuSig modes).

**Path B — true half-agg verification (compressed calldata).** Submit
`([Rᵢ compressed 33 B], s̃)` — ~33 B/signer, 37% less calldata than A — and verify the
MSM. The EVM has no secp256k1 mul precompile, so the MSM uses prover-supplied helper
points checked via the `ecrecover` point-multiplication trick
(`ecrecover(0, v, Px, z·Px) == addr(z·P)` verifies a claimed `z·P`), then Jacobian adds.
That costs ~2 ecrecovers + 2 helper points + adds ≈ **~12k gas/signer** — *more* total
gas than A on L1. Path B only wins where **data is the binding cost and execution is
cheap** (data-priced L2s), or if a secp256k1 MSM precompile ever lands. Given the
project is literally about killing gas, both paths get implemented behind one interface
and benchmarked; the deployment picks.

Honest summary: on Ethereum L1 the half-aggregation technique's value in this system is
**off-chain** (the engine certificate, storage, relay bandwidth) plus **eliminating all
nonce-coordination machinery**; on-chain it is linear either way, and the choice between
A and B is a calldata-vs-compute trade.

## 5. Components

* `common/src/schnorr/halfagg.rs` — sign/aggregate/verify + randomizer derivation,
  unit-tested against the exact on-chain identities (both paths). Property tests:
  any-subset aggregation, randomizer soundness (a forged cancellation attempt fails),
  determinism across restarts.
* `common/src/schnorr/halfagg_scheme.rs` — `HalfAggScheme: certificate::Scheme`
  (participants = ecdsa identities, keys index-aligned, **no** directory/journal/secrets
  state). Roughly a third of `SchnorrScheme`'s surface.
* `contracts/src/SchnorrHalfAggVerifier.sol` (+ interface) — paths A and B behind
  `isValidSignature`-compatible entry points; reads keys/stake from the existing
  `SchnorrStakeRegistry`. Foundry tests mirror `SchnorrNonceRegistry.t.sol`'s in-Solidity
  signer approach.
* Mode plumbing — `SignatureScheme::SchnorrHalfAgg` ("schnorr-halfagg"), node/router
  arms (strictly simpler than precommit: no channel-2 actor, no tap), deploy script arm,
  e2e mode. Reuses the precommit branch's generic reporter and mode scaffolding as-is.

## 6. Phases

| Phase | Deliverable | Status |
|---|---|---|
| 1 | `halfagg.rs` core + unit/property tests | not started |
| 2 | `SchnorrHalfAggVerifier.sol` (path A) + Foundry tests | not started |
| 3 | `HalfAggScheme` + engine integration tests | not started |
| 4 | Live wiring (node/router/deploy/e2e mode) | not started |
| 5 | Path B verifier + gas benchmarks A vs B vs MuSig modes | not started |

## 7. Security considerations

* **Aggregate forgery** reduces to single-signer Schnorr forgery via the Fiat–Shamir
  randomizers (CGKN Thm. 3 analog; tight in AGM). The randomizer hash **must** cover the
  full ordered `(Xᵢ, Rᵢ)` list and `m` — covering less re-opens cancellation attacks.
* **Nonce safety**: deterministic `kᵢ = H(xᵢ, Xᵢ, m)` is the single-signer setting where
  determinism is safe (unlike MuSig2, where deterministic nonces are catastrophic —
  which is why the precommit mode needed the journal). Same `m` twice → byte-identical
  signature; different `m` → independent nonce. No cross-signer influence on `kᵢ` exists.
* **Certificate malleability**: signer ordering is fixed (ascending participant index)
  and the bitmap is part of the certificate; two different subsets are two different
  valid certificates for the same item, which the engine already tolerates (same as
  ECDSA mode).
* **Replay/domain separation**: `m` is the engine item digest (height-bound); challenge
  binds the key; randomizer tags are versioned like the repo's other tags.
* **Weight**: quorum is enforced by the engine (N3f1 count) *and* on-chain by summed
  stake over the bitmap, same layering as the ECDSA mode.

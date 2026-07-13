# Sharded inference (Stage A)

One LLM inference, split across the operator set — instead of every operator
replaying the entire multi-billion-gas simulation, each executes only its
committee's share of hash-committed segments, and the answer settles through
the unchanged `verifyAndUpdate` path.

```
user ──POST /shard/infer──▶ router (shard coordinator)
                              │ plans (position × layer-range) segment DAG
                              │ assigns each segment to a k=2-of-N committee
      node-1 ◀──poll /shard/work──┤──▶ node-2 ──▶ node-3
        │ eth_call Qwen3SegEngine.forwardRange / argmaxRange (own sim RPC)
        └──POST /shard/result──▶ router: k results must be byte-identical
                              │ threads xOut/kvAppend between segments,
                              │ merges argmax shards, assembles commit chain
                              ▼
        {answer_ids, pipeline_root}   chain served at /shard/chain/<root>
                              │
user ──/trigger fulfil(promptIds, maxNew, answerIds, pipelineRoot)──▶ normal round
        each node's validator gate refuses to sign unless the chain verifies AND
        the digests of the segments IT executed match its local records
                              ▼
        BLS quorum ──▶ verifyAndUpdate (1 SSTORE + ChatAnswered log, ~100k gas)
```

## Protocol objects (`common/src/shard.rs`)

- **ShardJob** `{infer_id, seg_id, kind, to, data, gas}` — a pre-encoded view
  call; the node is a dumb executor (all planning is router-side).
- **SegmentChk** — forward segments return their commitment as the third ABI
  word (`keccak("gaskiller.seg.v1", posLo, posHi, layerLo, layerHi,
  keccak(tokenIds), keccak(xIn), keccak(kvIn), keccak(xOut), keccak(kvAppend))`,
  computed inside the engine); argmax segments get a synthetic commitment
  binding job calldata to returndata.
- **ShardChain** `{infer_id, consumer, prompt_ids, answer_ids, pipeline_root,
  entries[{seg_id, kind, committee, chk, returndata_hash}]}` —
  `pipeline_root = keccak(concat(chk) in seg_id order)`.

## Endpoints (router internal server, `:8081` — never on the public ingress)

| Endpoint | Who | What |
|---|---|---|
| `POST /shard/infer` | harness/client | run one sharded inference to completion |
| `GET /shard/work?operator=<id>` | nodes | drain pending segment jobs |
| `POST /shard/result` | nodes | return raw returndata for a job |
| `GET /shard/chain/<root>` | nodes (validator gate) | commit chain lookup |

## Configuration

Router: `GK_SHARD_K` (committee size, default 2), `GK_SHARD_OPERATORS`
(default 3), `GK_SHARD_GAS` (segment gas override, default 2^40),
`GK_SHARD_SEGMENT_TIMEOUT_SECS` (default 180).

Node: `GK_SHARD_URL` (router internal base, e.g. `http://router:8081`),
`GK_SHARD_OPERATOR_ID` (0..N), `GK_SHARD_POLL_MS` (default 500; 300 in compose),
and optionally `GK_SHARD_CONSUMER`. Sharding arms when `GK_SHARD_URL` +
`GK_SHARD_OPERATOR_ID` are set — the consumer address is NOT required at
startup: with `GK_SHARD_CONSUMER` unset the validator gate fires on any
`fulfil(...)` round, and with it set the gate is scoped to that one consumer.
Arming at startup (rather than after deploy) is deliberate — it lets the whole
stack come up together so the authenticated p2p mesh forms once and healthy,
instead of being torn by a mid-run node recreation.

## Trust model (MVP — Stage A)

- Execution redundancy is **k=2 per segment** instead of N-plus-router full
  replication; the coordinator aborts on any committee divergence.
- An operator's round signature means *"I verified the commit chain, the
  answer ids, and the digests of every segment I executed myself"* — enforced
  by the validator gate in `validate_and_build_hash` (a gate failure means the
  node never signs, starving quorum).
- What Stage A does NOT yet provide (see the distributed-inference design
  notes in the solidity-sdk repo): VRF sortition (committees are deterministic
  rotation — a malicious router could grind assignments), SP1 fraud proofs per
  segment (the chk chain is sized for one-shot proofs, binding is future work),
  DA custody receipts for boundary blobs, and challenge-window economics.
  Sharded rounds are therefore committee-trust + full-quorum-chain-verification,
  not yet slashing-complete.

## E2E

```
GK_SDK_DIR=<solidity-sdk checkout> bash scripts/run_sharded_llm_e2e_test.sh
```

Deploys the synthetic engine-v2 fixture model + `Qwen3SegEngine` +
`GasKillerChatSharded` on the harness anvil, runs `/shard/infer` (k=2
committees over the 3 compose operators), asserts the answer ids are bit-exact
vs the fixture reference, settles via `fulfil()` through the normal round, and
asserts: committee agreement on every segment, a strict work share per node
(nobody executed everything), and gate verification on all three nodes before
signing.

### Running locally on Apple Silicon

The compose images pin `platform: linux/amd64`; under QEMU emulation on arm64
the commonware authenticated-p2p noise handshake fails
(`HandshakeError(DecryptionFailed)`) and no round reaches quorum — this affects
every consumer, not just sharded ones. Build the router+nodes native by adding a
local (gitignored) `docker-compose.override.yml`:

```yaml
services:
  node-1: { platform: linux/arm64 }
  node-2: { platform: linux/arm64 }
  node-3: { platform: linux/arm64 }
  router: { platform: linux/arm64 }
```

The amd64 infra containers (ethereum/eigenlayer/signer) are reached over HTTP
only, so mixing arches is fine. If the host's `:8080` is taken, set
`ROUTER_PUBLIC_PORT`/`ROUTER_INTERNAL_PORT`. CI runs on native amd64 and needs
neither.

---

# Addendum: weight-sharding + layer affinity (Qwen3.5-35B-A3B)

Stage A above assumed every operator holds the WHOLE model, so any committee can
run any segment. The 35B (`qwen35`) is too big for that: workers hold a **layer
slice** (weight-sharding), so a segment for layers `[lo, hi)` can only go to
workers whose held range covers it. This addendum adds that, generalizes the
0.6B-specific DAG assumptions into a `ModelSpec`, and wires the real
`Qwen35SegEngine` call ABI. **The 0.6B path is unchanged** — every new behavior
is gated on env (`GK_SHARD_MODEL`, worker advertisements), defaulting to today's.

## Layer-affinity assignment

- A worker advertises its held slice on its work poll:
  `GET /shard/work?operator=N&layer_lo=..&layer_hi=..&has_embedding=..&has_classifier=..`
  (`common::shard::run_shard_loop`). The router keeps a live `WorkerCaps`
  registry (`router::shard::ShardCoordinator::advertise`).
- `ShardCoordinator::plan_committee(unit, seed, SegReq)` draws each segment's
  committee **only** from workers that COVER it (`WorkerCaps::covers`):
  - forward segment over `[lo, hi)` → workers with `layer_lo <= lo && layer_hi >= hi`
    (and, when `lo == 0`, `has_embedding` — the embedding lookup lives there);
  - argmax segment → workers with `has_classifier` (the untied classifier lives
    on the last slice; no layer-span requirement).
  Within the eligible set the same deterministic rotation `(seed + unit·k + j) %
  |eligible|` is applied. **Empty registry ⇒ the original modulo rotation over
  `0..n_operators`**, byte-for-byte — so no advertisements means the 0.6B
  behavior. If fewer than `k` workers cover a span, it errors clearly (a fleet
  misconfig: each covered span needs `>= GK_SHARD_K` workers).

## `ModelSpec` (`router/src/model.rs`)

The coordinator is parameterized on a `ModelSpec` selected by `GK_SHARD_MODEL`
(default `qwen3-0.6b`). It centralizes what used to be implicit:

| field | `qwen3-0.6b` | `qwen35` |
|---|---|---|
| `n_layers` | 28 | 40 |
| `full_attention_interval` | `None` (all attention) | `Some(4)` → layers 3,7,…,39 full (10), rest DeltaNet (30) |
| `dim` / `kvd` / `vocab` | 1024 / 1024 / 151936 | 2048 / 512 / 248320 |
| `packed_config_words` | 3 | 4 |
| DeltaNet snapshot | — | conv `(convK-1)·convDim·4` + S `nVH·dK·dV·4` = **2,195,456 B** |
| call ABI | `Qwen3SegEngine` (`forwardRange` 0x568f9e26) | `Qwen35SegEngine` (`forwardRange` 0x4faab046, `argmaxRange` 0x18d6ba7d) |

The spec owns the per-family **call encoding** (`encode_forward`/`encode_argmax`),
the **return decode**, and the **boundary-state wire format**. Boundary state is
threaded generically: full-attention layers APPEND their `K`/`V`; DeltaNet layers
REPLACE their fixed-size recurrent snapshot. For a pure-attention model this is
exactly the original KV concatenation.

### Boundary-cost-aware stage planning

Splitting a stage boundary mid-DeltaNet-run relays a ~2.1 MB S snapshot **per
DeltaNet layer straddled**; cutting right after a full-attention layer relays
only the cheap KV slice (`kvd·4`/pos/side). `ModelSpec::stage_bounds` therefore
SNAPS interior stage boundaries to full-attention-layer boundaries (multiples of
`full_attention_interval`) so every DeltaNet run stays whole inside a stage —
every cut is a min-cost KV boundary. `GK_SHARD_ALIGN_STAGES` (default true)
toggles it; 0.6B is unaffected (even split). **Weight-shard worker layer ranges
should align to these boundaries too** — e.g. `[0,20)`/`[20,40)` cuts after
full-attention layer 19.

### Qwen35Seg ABI — wired (not stubbed)

The 35B segment encoding was scaffolded to be a one-line stub; the engine agent
finalized the ABI (`Qwen35SegEngine`, commit 916ad17) mid-task, so it is now
**real**, in `router/src/model.rs`:
- `encode_forward` qwen35 branch — `router/src/model.rs:262`
  (`seg35::forwardRangeCall`, `bytes32[4]` packedConfig, `Call35.stateIn`);
- `encode_argmax` qwen35 branch — `router/src/model.rs:304`;
- ABI bindings — `common/src/shard.rs:94` (`mod seg35`); selectors pinned in
  `common/tests/shard_selectors.rs` (0x4faab046 / 0x18d6ba7d).
A qwen35 `/shard/infer` request must carry `packed_config_w3` (SPEC §10 w3) and
`n_layers: 40`. The only remaining placeholders are the published **artifact
URLs / per-slice manifests** in `shard-35b-overrides.yaml` (release not yet cut).

## Env / Helm surface added

**Router:** `GK_SHARD_MODEL` (`qwen3-0.6b`|`qwen35`), `GK_SHARD_ALIGN_STAGES`
(default true). `InferRequest` gains `packed_config_w3` (optional 4th packed word).

**Worker (node in shard-executor mode):** `GK_SHARD_LAYER_LO`/`GK_SHARD_LAYER_HI`
(together enable advertisement; unset = full-model, today's behavior),
`GK_SHARD_HAS_EMBEDDING`, `GK_SHARD_HAS_CLASSIFIER`.

**Helm:** `shard.enabled`, `shard.model`, `shard.alignStages`, `shard.consumer`,
`shard.basePort`, `shard.routerUrlOverride`, `shard.nodeSelector`,
`shard.tolerations`, `shard.resources`, and `shard.workers[]` =
`{name, replicas, layerLo, layerHi, hasEmbedding, hasClassifier,
artifactSlice{baseUrl, weightsFile, tokenizerFile?, manifest}}`.
`templates/shard-worker-deployment.yaml` renders one Deployment+Service per
group replica; `shard-35b-overrides.yaml` is the 2-group example.

## Bringing up a weight-sharded 35B fleet on the shard-workers Spot pool

1. **Publish per-slice artifacts.** Run the sdk weight-shard tooling to produce
   `weights.layers-0-19.bin` / `weights.layers-20-39.bin` (+ `tokenizer.bin` for
   the embedding slice) and their per-slice manifests; upload to a release.
2. **Fill in `shard-35b-overrides.yaml`** — `artifactSlice.baseUrl` and each
   group's `manifest` (the `0x…` placeholders), and `shard.consumer` once
   `GasKillerChat35Sharded` is deployed. Keep the cut at a full-attention
   boundary (layer 20).
3. **Ensure the pool + operator identities exist.** The `shard-workers` Spot pool
   (n2-highmem-4, autoscale 0-8) must be tainted `shard-worker=true:NoSchedule`
   and labeled `role=shard-worker`. `eigenlayer.quorum.maxOperatorCount` and the
   setup job must cover `sum(replicas)` operators (ids 0..N-1 map to
   `testacc{id+1}` keys / `node-{id+1}` GCP secrets).
4. **Install** (arm sharding from initial bring-up so the p2p mesh forms once —
   never recreate workers mid-run):
   ```bash
   helm upgrade --install gas-killer ./helm/gas-killer \
     -f helm/gas-killer/<network>-overrides.yaml \
     -f helm/gas-killer/shard-35b-overrides.yaml \
     --set router.image.tag=<tag> --set node.image.tag=<tag> \
     --set 'shard.consumer=0x<GasKillerChat35Sharded>'
   ```
   Set `GK_SHARD_MODEL=qwen35` on the router (via `shard.model`, echoed to
   workers). Router + all workers must agree on `GK_SIM_PROFILE=unbounded-v1-xl`
   and `GK_SIM_EXECUTOR=local` (the 35B overlay is only servable in-process).
5. **Drive an inference.** `POST /shard/infer` with the 35B model facts
   (`n_layers: 40`, `vocab: 248320`, `packed_config` + `packed_config_w3`,
   `seg_engine` = the deployed `Qwen35SegEngine`, `stages: 2`). The coordinator
   plans `[0,20)`/`[20,40)` stages, assigns stage-0+embedding to the `front`
   group, stage-1 to `back`, argmax to `back` (classifier), assembles the chain,
   and settles via the unchanged `fulfil()` round with each worker's validator
   gate verifying its executed segments.

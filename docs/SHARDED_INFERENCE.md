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

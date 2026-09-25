# Gas Killer

[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org)
[![Docker](https://img.shields.io/badge/docker-ghcr.io/gas--killer/service-blue.svg)](https://github.com/gas-killer/service/pkgs/container/service)

Gas Killer service implementation built on EigenLayer with aggregate Schnorr signatures for optimized transaction execution.

## Overview

The service coordinates multiple operator nodes to sign task digests with a two-round MuSig2 protocol, producing one constant-size aggregate Schnorr signature per task, and renders the result as a `verifyAndUpdate` payload that the client submits onchain.

## Repository Structure

- **`router/`** — Router service: sequences tasks, coordinates the Schnorr signing rounds, and renders the onchain payload
- **`node/`** — Operator node: validates tasks and signs them as a Schnorr participant
- **`common/`** — Shared types, validation logic, and EVM gas analysis
- **`config/`** — Operator and orchestrator key/config files
- **`scripts/`** — Helper binaries for deployment and end-to-end testing
- **`helm/`** — Kubernetes Helm chart for full-stack deployment
- **`docker-compose.yml`** — One-command local deployment

## Quick Start

### Prerequisites
- Docker and Docker Compose
- Git

### Local Development

1. **Configure environment:**
```bash
cp example.env .env
```

The example.env is pre-configured for LOCAL mode with Anvil test keys. No changes are needed to run locally.

2. **Start all services:**
```bash
docker compose up -d
```

This will automatically pull the latest pre-built images from the GitHub Container Registry (ghcr.io) and start:
- Ethereum node (Anvil fork of Sepolia)
- EigenLayer contract deployment
- 3 operator nodes
- Router/orchestrator

3. **Monitor services:**
```bash
# View logs
docker compose logs -f router

# Check service status
docker compose ps
```

### Stop Services

```bash
# Stop all services
docker compose down

# Stop and remove volumes (clean state)
docker compose down -v
```

### Building from Source (Development Only)

If you're developing locally and want to test changes:

```bash
# Build the router image
docker build -t ghcr.io/gas-killer/service:router-local -f router/Dockerfile .

# Build the node image
docker build -t ghcr.io/gas-killer/service:node-local -f node/Dockerfile .

# Run with locally built images
docker compose up -d
```

## Architecture

Signing is an interactive two-round MuSig2 protocol over secp256k1: the router coordinates,
the operator nodes participate, and each task yields one aggregate Schnorr signature whose
on-chain verification cost does not grow with the signer count. Target contracts inherit the
solidity-sdk's `GasKillerSDK` and verify that signature against a `SchnorrStakeRegistry`.
Operator discovery goes through EigenLayer (`RegistryCoordinator`/`BLSApkRegistry`, read via
`avs_deploy.json`); an operator's registered BN254 key is its p2p transport identity only.

```
                         POST /tasks
                             │
                     router: HTTP ingress
                             │
                     router: sequencer
        assigns the task the next height H and broadcasts
        TaskDirective::Announce{H, task} on p2p channel 1
        (rebroadcast until resolved; Skip{H} after ROUND_TIMEOUT)
                             │
  ┌──────────────────────────┼───────────────────────────┐
  │ node 1..N (Schnorr participants)                     │ router: Schnorr coordinator
  │  TaskBook: records the directive for H               │  p2p channel 2, per attempt:
  │  channel 2:                                          │   NonceRequest  → all operators
  │   NonceCommit: fresh nonce pair, no validation       │   NonceCommit   ← responders
  │   PartialSig: validate the task via EVMSketch,       │   SignRequest   → signer subset
  │   sign only if the local digest matches              │   PartialSig    ← subset
  └──────────────────────────────────────────────────────┘            │
                                                            submitter: renders
                                                            verifyAndUpdate(s, rAddr,
                                                            nonSigners) for the client
```

- **Sequencer** (router, `commonware_avs_router::sequencer`): dequeues ingress tasks,
  computes the expected storage updates via EVMSketch, and assigns each task the next
  height. Exactly one height is outstanding at a time; the next task is assigned only after
  the current height resolves (and, for a signed digest, its payload is rendered).
- **Task-directive channel** (p2p channel 1): the router broadcasts
  `TaskDirective::Announce { height, task }` and, after `ROUND_TIMEOUT`,
  `TaskDirective::Skip { height }`. Nodes record directives in their TaskBook and reply on
  this channel only with rate-limited `TipReport`s to directives below their own tip.
- **Schnorr coordinator** (router, p2p channel 2): per assigned height, runs attempts of
  `NonceRequest`/`NonceCommit` then `SignRequest`/`PartialSig`, with fresh nonces each
  attempt. A `NonceCommit` is accepted only if its public key maps to the sender's registered
  operator address; each partial is verified against the signer's own nonce commitment, and
  the assembled signature is self-verified. It also serves as the sequencer's certificate
  index. The certified log lives in memory only.
- **Schnorr participant** (node, p2p channel 2): answers `NonceRequest` from a fresh nonce
  pair without touching the task. On `SignRequest` it derives the height's digest locally
  (TaskBook + EVMSketch via `DigestResolver`), refuses unless it equals the requested message,
  checks that every signer point maps to a known operator address, and only then produces a
  partial. A secret nonce signs at most one context; sessions live in memory only.
- **Digest**: nodes sign the 32-byte task digest, which binds
  `(transitionIndex, target, selector, storageUpdates)`. The signature does not bind the
  height, and the contract enforces transition-index ordering, so an identical digest at a
  different height is harmless.
- **Submitter** (router): takes the aggregate signature `(s, rAddr)` and the strictly
  ascending non-signer address list and renders `verifyAndUpdate` calldata, checked with
  `eth_estimateGas` as the requesting account. The task settles `ready` with that payload;
  `SchnorrStakeRegistry` subtracts the non-signers' stake when the client submits it.
- **Validator** (`common/`): EVM gas analysis (EVMSketch) computing the storage
  updates and the expected task digest on both router and nodes.

### Quorum model

The coordinator only runs round 2 when at least `min_signers = ceil(N · num / den)` of the
`N` registered operators returned a nonce commitment, where `num/den` is
`QUORUM_THRESHOLD`/`THRESHOLD_DENOMINATOR` (default 2/3). Operator weights are uniform, so
this count approximates the stake fraction that the `SchnorrStakeRegistry` enforces on chain
with the same threshold:

| N | min_signers | tolerates |
|---|-------------|-----------|
| 3 | 2           | 1 offline |
| 4 | 3           | 1 offline |
| 7 | 5           | 2 offline |

### Runtime storage

`STORAGE_DIR` is the commonware runtime's storage directory. Neither binary keeps protocol
state on disk: the coordinator's certified log, the node's TaskBook, and its signing sessions
are all in memory. docker-compose mounts a named volume per service at `/app/data`; the Helm chart
mounts a dedicated volume. Without any writable directory the binaries fall back to
`$TMPDIR/gas-killer`, which is for bare-metal dev runs only.

### Failure modes and recovery

Every height the router assigns resolves one of two ways: an aggregate signature over the
task digest, or a skip once `ROUND_TIMEOUT` passes from the coordinator's first sight of the
assignment. The router rebroadcasts `Announce` every `REBROADCAST_INTERVAL` and switches to
`Skip` after `ROUND_TIMEOUT`. A skip runs no signing session and puts nothing on chain; the
submitter resolves the height as `skipped` and the task fails.

- **Offline or unresponsive node**: it misses the nonce stage (`SCHNORR_STAGE_TIMEOUT_SECS`)
  and the round proceeds without it, listed as a non-signer, as long as `min_signers`
  responded.
- **Divergent node**: a node whose locally derived digest differs from the requested message
  refuses to sign. A signer that sends no partial by the sign-stage deadline
  (`SCHNORR_SIGN_STAGE_TIMEOUT_SECS`) or sends an invalid one becomes a suspect, and the next
  attempt runs with fresh nonces on a subset that excludes suspects. An invalid partial ends
  the attempt at once. Suspects are re-admitted only if the rest cannot reach `min_signers`.
- **Too few signers**: if fewer than `min_signers` honest operators respond, every attempt
  fails, the height is skipped at `ROUND_TIMEOUT`, and the sequencer moves on to the next
  task. There is no wedge to clear by hand.
- **Node restart**: secret nonces and sessions are in memory only, so a restarted node
  refuses the sessions it forgot; it becomes a non-signer and the coordinator retries with
  fresh nonces. Its TaskBook refills from the router's rebroadcast directives.
- **Router restart**: tasks still `queued` or `processing` are re-queued, except one whose
  `transition_index` the contract has already consumed. That settles `expired`, counted in
  `gas_killer_tasks_expired_at_requeue_total`. There is no persisted height, and TipReports
  cannot recover one: a node reports only directives below the highest height it has seen,
  while a router restarting from 0 would re-announce exactly that height, which the node drops
  as a conflict. Each router life instead starts its heights at the wall clock in
  milliseconds, above anything a previous life announced. Heights never reach the chain, and
  nodes resolve the skipped range as skips. A clock stepped back by more than the previous
  life's uptime would bring the wedge back; restart the nodes to clear it.
- **Operator-set changes**: the participant set is frozen per process at startup from the
  on-chain registry. Registering or deregistering an operator requires restarting the router
  and all nodes together. `SchnorrStakeRegistry` changes are also subject to
  `SCHNORR_NOTICE_WINDOW` (see `example.env`).

## Configuration

### Environment Variables

Required environment variables:
- `ENVIRONMENT`: `LOCAL` or `TESTNET`
- `HTTP_RPC`: HTTP RPC endpoint
- `WS_RPC`: WebSocket RPC endpoint
- `AVS_DEPLOYMENT_PATH`: Path to deployment JSON file
- `PRIVATE_KEY`: Private key for transactions
- `FUNDED_KEY`: Funded key for testnet ETH (required for `TESTNET` mode)

LOCAL-mode-only:
- `FORK_URL`: Sepolia RPC URL to fork from (Anvil uses this)

Optional environment variables:
- `STORAGE_DIR`: Writable directory handed to the commonware runtime (default: `/app/data` if writable, else `$TMPDIR/gas-killer`). docker-compose and Helm mount a dedicated volume here — see "Runtime storage" above.
- `AGG_WINDOW`: Heights above a node's tip it expects the router to be driving (default: 8). A directive past `tip + window` is logged as evidence the node has fallen behind. Part of the config fingerprint.
- `ROUND_TIMEOUT`: Max seconds the Schnorr coordinator keeps retrying signing attempts on an assigned height before resolving it as a skip, and the point at which the sequencer switches from `Announce` to `Skip` broadcasts (accepts fractional seconds). Also the nodes' retry budget for transient validation errors. A height resolves as soon as an attempt assembles a signature, so this only affects heights that stall. Library default: 30; the chart sets 300 and a heavy-trace deployment raises it in its own overrides. Must exceed worst-case node compute + sign time. It is also the base for the Schnorr coordinator's partial-collection deadline (`SCHNORR_SIGN_STAGE_TIMEOUT_SECS`, default `ROUND_TIMEOUT/2`), so a fleet running multi-minute EVMSketch traces raises this one value and the stage that holds that compute grows with it.
- `REBROADCAST_INTERVAL`: How often (in seconds) the router re-sends the in-flight `TaskDirective` until the height resolves (accepts fractional seconds); also the minimum interval between a node's `TipReport`s. Library default: 5; Helm deployments set 15. Must stay well below `ROUND_TIMEOUT`: a node that misses every `Announce` can only resolve the height as a skip.
- `INGRESS`: Enable HTTP ingress mode (true/false)
- `INGRESS_ADDRESS`: Address for ingress server (default: 0.0.0.0:8080)
- `INGRESS_TIMEOUT_MS`: Timeout for waiting on ingress tasks in milliseconds (default: 0, no timeout)
- `ADMIN_KEY`: Shared secret guarding the `/admin/keys` endpoints, used to mint and revoke the per-client API keys that authenticate `POST /tasks`. Omit or leave empty to disable the admin API.
- `RATE_LIMIT_RPM`: Default per-API-key request rate on `POST /tasks`, in requests per minute (default: 60). Applies to every key without a per-key override (set at creation via the `rpm_limit` field of `POST /admin/keys`). Over-limit requests get `429 Too Many Requests` with a `Retry-After` header. Counters are in-memory per router process and reset on restart.
- `AVS_REFERENCE_TARGET`, `AVS_REFERENCE_TARGET_FILE`, `DEMO_TARGET_ADDRESS`, `DEMO_TARGET_FILE`, `DEMO_FACTORY_ADDRESS`, `DEMO_FACTORY_FILE`, `SCHNORR_STAKE_REGISTRY_ADDRESS`, `SCHNORR_STAKE_REGISTRY_FILE`: feed the `contracts` block on `GET /avs-metadata` — `chainId`, `avsAddress`, `schnorrStakeRegistry`, an advisory `registryCoordinator`, plus the demo contracts the docs point at. Publishing the live pair is what stops a target being wired to a superseded registry, which passes every router-side check and then reverts on chain.

  | Variable | Sets |
  |---|---|
  | `AVS_REFERENCE_TARGET` | Target whose `avsAddress()` and `schnorrRegistry()` establish the published pair |
  | `AVS_REFERENCE_TARGET_FILE` | File holding that address — the chart points this at the deploy job's record |
  | `DEMO_TARGET_ADDRESS` / `DEMO_FACTORY_ADDRESS` | `demoTarget` / `demoFactory` |
  | `DEMO_TARGET_FILE` / `DEMO_FACTORY_FILE` | Files holding those |
  | `SCHNORR_STAKE_REGISTRY_ADDRESS` | `schnorrStakeRegistry`, overriding both sources below |
  | `SCHNORR_STAKE_REGISTRY_FILE` | File holding it, which the chart points at the operator-set job's record |

  Reference-target precedence: pin → playground record → deploy-job record → `demo_target.txt` beside `avs_deploy.json`; the playground record wins so the published registry is the one `demoTarget` itself returns. The target's `schnorrRegistry()` is cross-checked against the fleet's registry — on mismatch the block is omitted. `registryCoordinator` comes from `avs_deploy.json`; nothing on chain ties it to the registry, so it is advisory only. Addresses publish EIP-55 checksummed so they paste into Solidity; a demo address with no code on the published chain is dropped. `example.env` covers resolution and retry behaviour.

  `schnorrStakeRegistry` precedence: `SCHNORR_STAKE_REGISTRY_ADDRESS` → the operator-set job's record (`SCHNORR_STAKE_REGISTRY_FILE`) → `avs_deploy.json`. The record exists because the job and the router do not always read the same copy of that JSON: under Secret Manager the router's comes from a secrets volume the job never writes to. Whichever source answers, the address publishes only once `nextPossibleMutationBlock()` answers at it — the call that tells a registry apart from some other contract pasted in its place. See the chart's [Schnorr section](helm/gas-killer/README.md#publishing-the-registry).
- `RPC_FAILURE_THRESHOLD`: consecutive RPC failures against one chain before that chain is marked unavailable (default: 5). A down provider breaks the whole task lifecycle — validation, analysis, and submission all read the chain — so on reaching the threshold the router sets `gas_killer_rpc_healthy{chain="l1|l2"}` to 0, logs the transition at `WARN` with the chain, failure count, and timestamp, and refuses new submissions with `503 RPC_UNAVAILABLE` rather than queueing work that cannot complete. Only L1 sheds traffic: every round's signature is anchored to an L1 reference block, so an L1 outage blocks submissions whichever chain the target runs on, while a degraded L2 is alerted but leaves L1 targets alone. The count is a run, not a total — a single success clears it. Every chain read on the submission and freshness paths reports its outcome, and a background probe (`eth_blockNumber`, every 15s) checks each chain independently, so the gauge reflects reality on an idle router and a recovered provider clears the breaker without waiting for traffic.
- `TASK_TTL_SECONDS`: How long a task may sit without reaching a terminal state before a background sweep settles it as `expired` (default: 600, the wall-clock life of a rendered payload on L1 — `PAYLOAD_BLOCK_BUFFER` blocks at a 12s slot time — so the sweep never withdraws a payload the chain would still accept; a target chain with faster blocks has a proportionally shorter payload window and can run a shorter TTL). A `queued` task past its TTL is expired with `QUEUE_TTL_EXCEEDED` — its pinned `block_height` has gone stale, so aggregating it would only produce a payload the contract rejects — and a `ready` payload nobody collected is expired with `READY_TTL_EXCEEDED` on the same TTL, counted from when aggregation recorded it, which frees the deduplication slot for its transition index. A `processing` task is never cancelled mid-round: its height is already assigned and must resolve either way. The sweep runs every 60 seconds and counts what it settles in `gas_killer_tasks_expired_total`.
- `INGRESS_STALENESS_WINDOW_BLOCKS`: How far behind the target chain's head a submitted `block_height` may be and still be admitted; past it `POST /tasks` returns `400 STALE_BLOCK`. Defaults to `BLOCK_STALE_MEASURE - PAYLOAD_BLOCK_BUFFER` (250 at the stock 300/50), floored at 1 block — a task's analysis is anchored at its `block_height`, and both the aggregation round and the rendered payload's validity window have to fit in what is left of the contract's staleness window, so admission holds the payload buffer back instead of accepting work that can only just finish simulating. An explicit value above `BLOCK_STALE_MEASURE` is clamped to it (and warned about at startup); `0` disables the check entirely. The effective window is logged at startup.
- `QUORUM_NUMBER`: Quorum number to use (default: 0)
- `QUORUM_THRESHOLD` / `THRESHOLD_DENOMINATOR`: Signing threshold `num/den` (default: 2/3). Sets the coordinator's `min_signers` floor and the `SchnorrStakeRegistry`'s on-chain threshold, so the two checks stay in lockstep (see "Quorum model" above).
- `SCHNORR_STAGE_TIMEOUT_SECS`: Nonce-collection deadline per attempt (default: `min(5, ROUND_TIMEOUT/6)`).
- `SCHNORR_SIGN_STAGE_TIMEOUT_SECS`: Partial-signature collection deadline per attempt (default: `ROUND_TIMEOUT/2`). This stage holds the node's EVMSketch, so it must cover a full cold trace.
- `P2P_MESSAGES_PER_SECOND`: Per-peer rate for the task-directive channel, channel 1 (default: 1.0).
- `P2P_SCHNORR_MESSAGES_PER_SECOND`: Per-peer rate for the Schnorr channel, channel 2 (default: 64). The p2p sender silently drops over-rate messages, and a dropped signing-round message costs a whole attempt.
- `SCHNORR_NOTICE_WINDOW`: Blocks a `SchnorrStakeRegistry` operator-set change must be announced ahead of taking effect (default: 0). `example.env` covers when to raise it.

Each node also takes `--schnorr-key-file`, its secp256k1 signing key; `--key-file` is the BN254 p2p transport identity. See the Schnorr section of `helm/gas-killer/README.md`.

Operator (node) key files are generated automatically by the Docker setup and do not need to be set manually.

### Metrics

Both binaries serve Prometheus text exposition on `/metrics` at `HEALTHZ_PORT` (default 8081):
the commonware runtime's own registries, then the process's custom metrics, then its
configuration fingerprint. The Helm chart scrapes both at 15s and dashboards them.

Metrics from the runtime registries are named after the subsystem that registered them —
`network_*` for p2p. The router's own metrics are all prefixed `gas_killer_`.

The pipeline's shape, as opposed to the cost of one round:

| Metric | Meaning |
|---|---|
| `gas_killer_in_flight_heights` | Heights assigned and not yet resolved |
| `gas_killer_window_base`, `gas_killer_highest_assigned_height` | Edges of the live window. Both pinned while work is queued is a wedge |
| `gas_killer_height_age_seconds` | Age of the oldest unresolved height |
| `gas_killer_height_outcomes_total{outcome}` | Final disposition per height: `executed`, `skipped`, `foreign`, `superseded` |
| `gas_killer_node_safe_tip` | Tip floor from the operators' reports; explains a `superseded` spike |
| `gas_killer_directive_sends_total{result}` | Per-recipient directive delivery: `delivered`, `rate_limited`, `rejected` |
| `gas_killer_settlement_conflicts_total` | Terminal-state transitions the store refused. Must be 0 |
| `gas_killer_config_fingerprint{fingerprint}` | Always 1, labelled with this process's consensus-critical config |
| `network_spawner_messages_rate_limited_total{peer,message}` | Messages the *receiving* peer throttled, by channel (`data_1` directives, `data_2` Schnorr) |

Where the time inside one gas analysis goes. Both the router and the operators publish these,
separated by the scrape target:

| Metric | Meaning |
|---|---|
| `gas_killer_evmsketch_trace_fetch_seconds{extraction}` | Awaiting trace RPCs: network plus *remote* node CPU |
| `gas_killer_evmsketch_parse_seconds{extraction}` | Turning struct logs into state updates: local CPU, `O(execution steps)` |
| `gas_killer_evmsketch_executor_build_seconds{extraction}` | Resolving the revm executor. Overlaps trace fetch — never add the two |
| `gas_killer_evmsketch_state_prefetch_seconds{extraction}` | One `eth_getProof` per hinted address |
| `gas_killer_evmsketch_revm_estimate_seconds{extraction}` | Pricing the payload under revm: local CPU |
| `gas_killer_evmsketch_executor_cache_total{result}` | Executor-cache hit/miss — the speculative pre-build's scorecard |
| `gas_killer_evmsketch_digest_cache_total{result}` | Digest-cache hit/miss; a hit skips the whole analysis |
| `gas_killer_node_evmsketch_duration_seconds` | The whole analysis call, cache-miss path only. The `node` in the name is historical: both the router and the operators emit it, distinguished by the scrape target |
| `gas_killer_storage_computation_seconds` | Router only, and broader: chain detection plus the transition-index read plus the analysis |

`extraction` is `prestate_net`, `struct_log`, or `prestate_fallback`. The net form reads two cheap
tracers and never fetches or parses a struct-log trace, so it has **no** `parse_seconds` series at
all — that absence is the `STATE_ENCODING=prestate-net` saving, measured rather than inferred. A
`prestate_fallback` run attempted the net form, could not represent the call, and paid for both
paths, so a high fallback share is the workload shape where `prestate-net` costs more than it
saves.

To answer "is this workload RPC-bound or CPU-bound", compare seconds of work per second from the
histogram sums rather than percentiles: `trace_fetch + state_prefetch` against
`parse + revm_estimate`.

Three of these need reading together rather than alone.

`gas_killer_directive_sends_total` and `network_spawner_messages_rate_limited_total` are opposite
ends of the same channel and neither substitutes for the other. The send-side counter exists
because the p2p sender returns the peers it will attempt and silently omits the ones over quota,
so a partial drop and a full delivery are indistinguishable at the call site — and a dropped
`Announce` is how an operator ends up refusing to sign a height everyone else signed. The
receive-side counter is the peer's own view, where a throttled message is not dropped but sleeps
the entire connection, blocking every channel on it.

`gas_killer_config_fingerprint` is a hash of the settings that must match across the router and
every operator: `GK_SIM_PROFILE`, `STATE_ENCODING`, the application namespace,
`AGG_WINDOW`, and the directive wire version. A fleet that disagrees on any of them
does not fail loudly — peers stay connected, quorum never forms, and every pod reports healthy —
so `count(count by (fingerprint) (gas_killer_config_fingerprint))` must be exactly 1. It is also
the pre-flight check for a rolling upgrade: none of these may be changed on a live fleet.

## Ingress Mode

Enable HTTP endpoints for external task requests. These endpoints are specified in
[`router/docs/openapi.json`](#regenerating-the-openapi-documents).

1. **Enable ingress in .env:**
```bash
INGRESS=true
```

2. **Restart the router:**
```bash
docker compose restart router
```

3. **Submit tasks via HTTP:**
```bash
curl -X POST http://localhost:8080/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "body": {
      "target_address": "0x0000000000000000000000000000000000000001",
      "from_address": "0x0000000000000000000000000000000000000002",
      "call_data": [171, 205, 239, 1],
      "transition_index": 0,
      "value": "0x0",
      "block_height": 1
    }
  }'
```

Note: `call_data` is a JSON array of bytes (not a hex string), `value` is a U256 hex string, and `block_height` must be non-zero.

When the router has a persistent store (the default), `POST /tasks` requires a valid API key, minted
through the admin API (`POST /admin/keys`) using `ADMIN_KEY`. The raw key is returned exactly
once. Locally (docker-compose) the admin API is on `localhost:8080`:
```bash
curl -X POST http://localhost:8080/admin/keys \
  -H "Authorization: Bearer <ADMIN_KEY>" \
  -H "Content-Type: application/json" \
  -d '{"label": "my-client", "invalid_at": 1893456000}'   # invalid_at optional; unix ts, future
# → {"id":"...","key":"gk_...","label":"my-client","created_at":...,"invalid_at":1893456000}
```

Add `rpm_limit` (requests per minute) to the create body to give a key a custom rate; omit it to
use the global `RATE_LIMIT_RPM` default. Each key is rate-limited on `POST /tasks`: over-limit requests
return `429 Too Many Requests` with a `Retry-After` header (seconds until the next request is
allowed). The limiter is a token bucket, not a strict rolling window — a key may burst up to its
full per-minute allowance at once, after which requests refill at roughly `rpm / 60` per second.
Counters are in-memory per router process and reset on restart.

On a Kubernetes deployment the `/admin/*` endpoints are **not** exposed through the public Ingress
(only `/tasks`, `/avs-metadata`, `/healthz` are — see `ingress.publicPaths` in the chart).
Reach them in-cluster. The `create_api_key` tool ships in the router image, defaults to the
in-cluster `http://localhost:8080`, and reads `ADMIN_KEY` from the pod env — so `kubectl exec`
needs no target flag:
```bash
POD=$(kubectl get pods -l app.kubernetes.io/component=router -o jsonpath='{.items[0].metadata.name}')
kubectl exec "$POD" -- create_api_key --label my-client --expires-at "7 days"
# fallback if the binary predates the image: curl localhost:8080/admin/keys with $ADMIN_KEY
```
Or `kubectl port-forward svc/<release>-router 8080:8080` and run the tool locally, reading
`ADMIN_KEY` from the Secret (it still defaults to `http://localhost:8080`):
```bash
ADMIN_KEY=$(kubectl get secret <release>-secret -o jsonpath='{.data.ADMIN_KEY}' | base64 -d) \
  create_api_key --label my-client --expires-at "7 days"
```
`--expires-at` accepts `never`, a relative duration like `7 days`, or a unix timestamp. The tool's
`--env prod`/`--env testnet` shortcuts target the public hostnames, so they work only if you have
deliberately added `/admin` to `ingress.publicPaths` — otherwise they 404 at the edge.

Then include the minted key as the Bearer token on task requests (revoke via the same in-cluster
admin path when no longer needed):
```bash
curl -X POST https://<host>/tasks \
  -H "Authorization: Bearer gk_..." \
  -H "Content-Type: application/json" \
  -d '...'
```

Use the `send_request` script for a complete end-to-end run against an ArraySummation contract.
Set `GAS_KILLER_API_KEY` to a minted key when the router requires auth, and
`GAS_KILLER_TASKS_URL` to submit somewhere other than the default
`http://localhost:8080/tasks`:
```bash
GAS_KILLER_API_KEY=gk_... cargo run -p scripts --bin send_request
GAS_KILLER_TASKS_URL=https://<host>/tasks GAS_KILLER_API_KEY=gk_... cargo run -p scripts --bin send_request
```

## Development

### Dependencies
- `alloy`: Ethereum interaction
- `commonware-avs-*` (git, `commonware-restaking`): the upstream sequencer, node TaskBook, BN254 p2p identity, EigenLayer operator discovery, and contract bindings
- `commonware-p2p`, `commonware-runtime`, `commonware-cryptography`, `commonware-codec`: P2P networking, runtime, and primitives
- `k256`: secp256k1 arithmetic for the MuSig2 implementation in `common/src/schnorr/`
- `gas-analyzer-evmsketch`: EVM gas analysis and storage update computation
- `eigen-*` / `ark-*`: EigenLayer SDK and BN254 curve arithmetic for the p2p identity

### Code Quality
```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

### Regenerating the OpenAPI documents

Generated from the `#[utoipa::path]` annotations on the ingress handlers. Rewrite after changing an
annotation or a type it exposes:

```bash
cargo run --bin openapi
```

- `router/docs/openapi.json`: the integrator API, rendered by the docs site.
- `router/docs/openapi.internal.json`: the same, plus `/admin/keys` and the operator port's
  `/healthz`, `/readyz` and `/metrics`.

The generator drops `Admin`- and `Health`-tagged operations from the published document, along
with the credential and types only they use, so the operator surface cannot reach the public
playground.

`cargo test` gates all of it: both committed documents against what the code produces, the
operator surface staying out of the published one, and each document's internal consistency.

### Testing

Run unit tests:
```bash
cargo test --lib
```

Run end-to-end tests:
```bash
chmod +x scripts/run_e2e_test.sh
./scripts/run_e2e_test.sh
```

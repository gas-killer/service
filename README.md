# Gas Killer

[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org)
[![Docker](https://img.shields.io/badge/docker-ghcr.io/gas--killer/service-blue.svg)](https://github.com/gas-killer/service/pkgs/container/service)

Gas Killer service implementation built on EigenLayer with ECDSA (secp256k1) signature aggregation for optimized transaction execution.

## Overview

The service coordinates multiple operator nodes to sign task digests with their EigenLayer operator ECDSA keys, assembles the signatures into quorum certificates via the commonware-consensus aggregation engine, and executes the result onchain via `verifyAndUpdate` (which recovers every signer with `ecrecover`).

## Repository Structure

- **`router/`** — Router service: sequences tasks, assembles certificates (verifier-only engine), and executes onchain
- **`node/`** — Operator node: validates and signs tasks
- **`common/`** — Shared types, validation logic, and EVM gas analysis
- **`contracts/`** — Foundry project: ECDSA `GasKillerSDK`, example `ArraySummation` + factory
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
- Signer service (Cerberus)

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

Aggregation is built on the `commonware-consensus` **aggregation engine** with a custom
**secp256k1 ECDSA attributable multisig scheme** (signer bitmap + one 65-byte
`r || s || v` signature per signer). An operator's signing identity is its registered
EigenLayer **Ethereum address**: nodes sign with the same ECDSA key they registered
with, and the on-chain `GasKillerSDK.verifyAndUpdate` delegates quorum verification to
EigenLayer's **`ECDSAStakeRegistry`** (eigenlayer-middleware, ERC-1271
`isValidSignature`), which validates every signature against the operator's registered
signing key at the reference block and enforces the stake-weight threshold — no BLS
aggregation, pairing checks, or `NonSignerStakesAndSignature` assembly anywhere. The
contracts live in this repo under `contracts/` (Foundry project; `eigenlayer-middleware`
is a git submodule, so clone with `--recurse-submodules` for contract development);
their compiled artifacts feed the alloy bindings in `common/src/bindings/` and
`scripts/bindings/`.

```
                        POST /trigger
                             │
                     router: HTTP ingress
                             │
                     router: sequencer
        assigns the task the next height H and broadcasts
        TaskDirective::Announce{H, task} on p2p channel 1
        (rebroadcast until certified; Skip{H} after ROUND_TIMEOUT)
                             │
  ┌──────────────────────────┼───────────────────────────┐
  │ node 1..N (signing participants)                     │ router (verifier-only)
  │  aggregation engine, p2p channel 0:                  │  aggregation engine:
  │   propose(H): wait for the directive for H,          │  validates acks, assembles an
  │   validate the task via EVMSketch, sign the          │  ECDSA certificate at quorum
  │   expected digest (or the skip digest) with          │            │
  │   the node's operator ECDSA key, gossip TipAcks      │  submitter: bitmap → signatures
  └──────────────────────────────────────────────────────┘  (sorted by signer address),
                                                            GasKillerSDK.verifyAndUpdate
```

- **Sequencer** (router): dequeues ingress tasks, computes the expected storage updates
  via EVMSketch, and assigns each task the next aggregation height. Exactly one height
  is outstanding at a time; the next task is assigned only after the current height
  certifies (and, for real digests, on-chain execution finishes).
- **Task-directive channel** (p2p channel 1): the router broadcasts
  `TaskDirective::Announce { height, task }` and, after `ROUND_TIMEOUT`,
  `TaskDirective::Skip { height }`. Nodes only receive on this channel. Engine `TipAck`
  gossip runs on channel 0.
- **Aggregation engine**: every process runs
  `commonware_consensus::aggregation::Engine`. Nodes run it as signing participants;
  the router runs it verifier-only (it holds no share of any signing key) — it
  validates the nodes' acks, assembles certificates at quorum, journals them, and
  hands them to the submitter.
- **ECDSA multisig scheme**: nodes sign the raw 32-byte task digest with their
  operator secp256k1 key (Ethereum prehash semantics, **no namespace**, no EIP-191
  prefix) — exactly what the `ECDSAStakeRegistry` validates on-chain against the
  operator's registered signing key. The certificate binds only the digest, not the
  height; the digest itself binds `(transitionIndex, target, selector,
  storageUpdates)` and the contract enforces transition-index ordering, so replaying
  an identical digest across heights is harmless.
- **Submitter** (router): pairs the certificate's signer bitmap with its 65-byte
  signatures, sorts them by ascending operator address (the registry's dedupe
  order), pins a reference block (one L1 `eth_blockNumber`), and calls
  `GasKillerSDK.verifyAndUpdate`, which forwards to
  `ECDSAStakeRegistry.isValidSignature`.
- **AVS bootstrap** (deploy-time): the EigenLayer setup image
  (`BreadchainCoop/eigenlayer-bls-local`, `main.sh`) runs the forge scripts in
  `contracts/script/` — `DeployECDSAStack` (registry + `GasKillerServiceManager`,
  quorum init fail-closed), `RegisterOperatorECDSA` per operator (AVSDirectory
  registration signature, signing key = operator address), and `FinalizeECDSAStack`
  (stake threshold = 66% of total registered weight). The addresses land in
  `avs_deploy.json`; `scripts/deploy_array_summation.rs` then only reads
  `ecdsaStakeRegistry` from it and deploys the example target.
- **Quorum vs stake** (operational note): the aggregation engine forms a
  certificate at `n − ⌊(n−1)/3⌋` *signers* (a count), while `ECDSAStakeRegistry`
  enforces a *stake-weight* threshold on-chain. These agree when operator stake is
  roughly even (the assumption the local/testnet deployments run under); with
  highly uneven stake a count-quorum certificate can fall below the weight
  threshold and be rejected on-chain (the height is released as a failed
  execution, not silently lost). Stake-aware quorum formation is a follow-up.
- **Validator** (`common/`): EVM gas analysis (EVMSketch) computing the storage
  updates and the expected task digest on both router and nodes.

### Quorum model

The consensus engine fixes the certificate quorum at `n - floor((n - 1) / 3)` of the
`n` registered operators (Byzantine fault model, f = floor((n-1)/3)):

| n | quorum | tolerates |
|---|--------|-----------|
| 3 | 3-of-3 | 0 faulty  |
| 4 | 3-of-4 | 1 faulty  |
| 7 | 5-of-7 | 2 faulty  |

This is stricter than the old `ceil(2n/3)` threshold for n=3: **with 3 operators every
node must sign every certificate, so a single offline or divergent node halts
certification until it recovers.** Run **n ≥ 4 operators in production**. The old
`THRESHOLD` override is gone; the on-chain stake-fraction check
(`QUORUM_THRESHOLD`/`THRESHOLD_DENOMINATOR`) still runs in the contract at submission.

### Journal storage

Each process needs a writable directory (`STORAGE_DIR`) for the engine's journal — a
write-ahead log of acks and certificates replayed on restart. docker-compose mounts a
named volume per service at `/app/data`; the Helm chart mounts an `emptyDir`. A node
that loses its journal forgets what it acked (safe: it re-signs the same digests); the
router's journal only caches certificates and is likewise safe to lose. Without any
writable directory the binaries fall back to `$TMPDIR/gas-killer`, which is for
bare-metal dev runs only.

### Failure modes and recovery

The engine's tip advances only when heights certify contiguously — **every height the
router assigns must eventually resolve to a certificate**, either the task digest or
the skip digest. The router rebroadcasts `Announce` every `REBROADCAST_INTERVAL`,
switches to `Skip` after `ROUND_TIMEOUT`, and keeps rebroadcasting until the height
certifies. Nodes sign the skip digest only when told to (`Skip` directive) or when the
router has demonstrably moved past the height (a directive for a later height exists
while this one has none) — never on a bare timer.

- **Offline node (n=3)**: quorum is 3-of-3, so certification stalls until the node
  returns; acks are re-gossiped and the pipeline resumes on its own.
- **Split-digest stall**: if signers split between the task digest and the skip digest
  at the same height such that neither reaches quorum (with n=3 any single divergent
  signer is fatal; with n=4, two), the pipeline wedges at that height and **does not
  recover automatically**. Manual recovery: stop all node processes, wipe each node's
  engine journal — `docker compose down` + `docker volume rm <project>_node-{1,2,3}-data`,
  or delete the node pods in Kubernetes (journals are `emptyDir`) — and restart. Nodes
  then re-propose the height with no memory of their previous acks and follow the
  router's current directive. Running n ≥ 4 makes a single divergent signer non-fatal.
- **Router restart**: the engine replays its certificate journal, the sequencer waits
  a short settle delay, then resumes from the observed tip. A certificate whose digest
  matches neither the expected digest nor the skip digest (a node resolved the height
  from a directive issued by a previous router life) consumes the height and the
  in-flight task is re-assigned to the next one.
- **Router journal loss**: if the router's journal is wiped while the nodes keep
  theirs (e.g. only the router pod is rescheduled), the sequencer would restart at
  height 0 — below heights the nodes will ever propose again. Nodes detect directives
  below their own tip and reply with a rate-limited `TipReport`; the router takes the
  `(f+1)`-th highest reported tip (the same trust rule as the engine's safe-tip) and
  fast-forwards its next assignment, re-assigning the in-flight task there.
- **Operator-set changes**: the participant set (and therefore every participant
  index) is frozen per process at startup from the on-chain registry. Registering or
  deregistering an operator requires restarting the router and all nodes together —
  processes with different participant sets reject (and eventually disconnect) each
  other's acks.

### Slashing

A Gas Killer commitment is fraudulent when the quorum signed storage updates that
differ from the ones produced by actually executing the committed call. Slashing is a
fraud proof: anyone can re-execute the call inside the Gas Killer challenger SP1
program (`BreadchainCoop/sp1-contract-call`, `examples/gas-killer` — the same
`gas-analyzer-core` pipeline the operators run) and submit the resulting Groth16 proof
to `GasKillerSlasher.slash`.

- **Signed message**: operators sign `sha256(abi.encode(transitionIndex, target,
  anchorHash, caller, contractCalldata, storageUpdates))` — enough context to
  re-execute the exact call. `anchorHash` is the hash of the block the updates were
  computed at; nodes derive it themselves and refuse to sign a task whose claimed
  anchor differs (`common/src/validator.rs`), so honest operators can't be tricked
  into a slashable signature.
- **`GasKillerSlasher.slash`** (`contracts/src/GasKillerSlasher.sol`): verifies the
  operators ECDSA-signed the commitment for a valid quorum via the same
  `ECDSAStakeRegistry.isValidSignature` that `verifyAndUpdate` trusts (so the
  attributed signer set is exactly the quorum that authorized the update), verifies
  the SP1 proof and the anchor block hash (Helios light client), requires the proven
  storage updates to differ from the signed ones, then slashes every signer through
  EigenLayer's `AllocationManager.slashOperator` — burning their allocated stake.
- **Operator-set integration**: `GasKillerServiceManager` is the AVS identity on the
  `AllocationManager`. It creates the slashable operator set (`createOperatorSet`),
  serves as the AVS's `IAVSRegistrar` (operators may only join the slashable set
  after registering an ECDSA signing key with the `ECDSAStakeRegistry`), and forwards
  its default PermissionController admin rights (`setAppointee` grants the slasher
  `slashOperator`).
- **Deployment order** (forge scripts in `contracts/script/`):
  1. `DeployECDSAStack` — with `ALLOCATION_MANAGER_ADDRESS` set, also registers AVS
     metadata with the AllocationManager and creates operator set 0 on the LST
     strategy.
  2. `RegisterOperatorECDSA` per operator — ECDSA signing key + AVSDirectory
     registration (unchanged).
  3. `DeployGasKillerSlasher` — deploys the slasher (+ vendored SP1 verifier if
     needed) and appoints it for `AllocationManager.slashOperator`. Needs
     `HELIOS_ADDRESS`, `PROGRAM_VKEY`, `CHAIN_CONFIG_HASH` (see `example.env`).
  4. `EnrollOperatorSlashing` per operator — allocates magnitude to the operator set
     and registers for it. Must run after the operator's allocation delay is
     effective: the AllocationManager activates it `ALLOCATION_CONFIGURATION_DELAY + 1`
     blocks after `DelegationManager.registerAsOperator` (read the delay with
     `cast call $ALLOCATION_MANAGER_ADDRESS "ALLOCATION_CONFIGURATION_DELAY()(uint32)"`;
     on a local anvil devnet mine past it with `cast rpc anvil_mine <n>`).
  5. `FinalizeECDSAStack` — unchanged.
- **End-to-end validation**: `contracts/test/GasKillerSlasherE2E.t.sol` runs the
  whole path with real crypto against a real EigenLayer deployment — a real
  3-operator ECDSA quorum with deposited-and-allocated stake signs a fraudulent
  commitment, a real Groth16 proof of the correct execution is verified on-chain, and
  the test asserts every signer's magnitude, delegated shares, and quorum weight drop
  to zero with the staked tokens transferred to EigenLayer's burn address. Only the
  Helios light client is a test double. Hardfork governance: the accepted
  chain-config hash is an owner-managed allowlist on the slasher
  (`setChainConfigHashAccepted`).

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
- `STORAGE_DIR`: Writable directory for the aggregation engine's journal (default: `/app/data` if writable, else `$TMPDIR/gas-killer`). docker-compose and Helm mount a dedicated volume here — see "Journal storage" above.
- `AGG_WINDOW`: Heights the aggregation engine works on concurrently above its tip (default: 8).
- `AGG_ACTIVITY_TIMEOUT`: Heights below the tip the engine keeps tracking before pruning (default: 256). Keep generous — heights pruned past this window can never certify locally.
- `ROUND_TIMEOUT`: Max seconds the router waits for a certificate on its assigned height before switching from `Announce` to `Skip` broadcasts (accepts fractional seconds). Also the nodes' retry budget for transient validation errors. The engine certifies as soon as quorum signs, so this only affects heights that stall. Library default: 30; Helm deployments set 300. Must exceed worst-case node compute + sign time.
- `REBROADCAST_INTERVAL`: How often (in seconds) the router re-sends the in-flight `TaskDirective` until the height certifies (accepts fractional seconds); also the engine's internal `TipAck` rebroadcast timeout. Library default: 5; Helm deployments set 15. Must stay well below `ROUND_TIMEOUT`: a node that misses every `Announce` can only resolve the height as a skip.
- `INGRESS`: Enable HTTP ingress mode (true/false)
- `INGRESS_ADDRESS`: Address for ingress server (default: 0.0.0.0:8080)
- `INGRESS_TIMEOUT_MS`: Timeout for waiting on ingress tasks in milliseconds (default: 0, no timeout)
- `ADMIN_KEY`: Shared secret guarding the `/admin/keys` endpoints, used to mint and revoke the per-client API keys that authenticate `/trigger`. Omit or leave empty to disable the admin API.
- `QUORUM_NUMBER`: Quorum number to use (default: 0)
- `P2P_ACK_MESSAGES_PER_SECOND`: Per-peer rate for the engine's TipAck channel. Defaults to `2 * AGG_ACTIVITY_TIMEOUT / REBROADCAST_INTERVAL + 8`, sized so steady-state ack rebroadcast never hits the p2p limiter (which silently drops over-rate messages). Only override to constrain bandwidth. The legacy `P2P_MESSAGES_PER_SECOND` knob now only governs the task-directive channel.

Removed after the aggregation migration:
- `THRESHOLD`: the minimum-signature override no longer exists — the quorum is fixed by the consensus engine at `n - floor((n-1)/3)` (see "Quorum model" above).

Operator (node) key files are generated automatically by the Docker setup and do not need to be set manually.

## Ingress Mode

Enable HTTP endpoints for external task requests:

1. **Enable ingress in .env:**
```bash
INGRESS=true
```

2. **Restart the router:**
```bash
docker compose restart router
```

3. **Trigger tasks via HTTP:**
```bash
curl -X POST http://localhost:8080/trigger \
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

When the router has a persistent store (the default), `/trigger` requires a valid API key, minted
through the admin API (`POST /admin/keys`) using `ADMIN_KEY`. The raw key is returned exactly
once. Locally (docker-compose) the admin API is on `localhost:8080`:
```bash
curl -X POST http://localhost:8080/admin/keys \
  -H "Authorization: Bearer <ADMIN_KEY>" \
  -H "Content-Type: application/json" \
  -d '{"label": "my-client", "invalid_at": 1893456000}'   # invalid_at optional; unix ts, future
# → {"id":"...","key":"gk_...","label":"my-client","created_at":...,"invalid_at":1893456000}
```

On a Kubernetes deployment the `/admin/*` endpoints are **not** exposed through the public Ingress
(only `/trigger`, `/avs-metadata`, `/healthz` are — see `ingress.publicPaths` in the chart).
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
curl -X POST https://<host>/trigger \
  -H "Authorization: Bearer gk_..." \
  -H "Content-Type: application/json" \
  -d '...'
```

Use the `send_request` script for a complete end-to-end trigger against an ArraySummation contract.
Set `GAS_KILLER_API_KEY` to a minted key when the router requires auth:
```bash
GAS_KILLER_API_KEY=gk_... cargo run -p scripts --bin send_request
```

## Development

### Dependencies
- `alloy`: Ethereum interaction
- `commonware-consensus`: Aggregation engine (certificate assembly over heights)
- `commonware-cryptography`: Cryptographic primitives and the certificate `Scheme` trait implemented by our secp256k1 ECDSA multisig scheme
- `commonware-p2p`: P2P networking
- `commonware-runtime`: Runtime utilities and journal-backed storage
- `gas-analyzer-evmsketch`: EVM gas analysis and storage update computation
- `eigen-*`: EigenLayer SDK (operator-set discovery)

The former `commonware-avs-*` (`commonware-restaking`) git dependencies were dropped in
the aggregation migration; the parts still needed are vendored under `common/src/`
(`ecdsa/`, `eigenlayer.rs`, `bindings/`). The ECDSA `GasKillerSDK` contract sources are
under `contracts/` (`forge build` / `forge test`).

### Code Quality
```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

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

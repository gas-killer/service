# Gas Killer

[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org)
[![Docker](https://img.shields.io/badge/docker-ghcr.io/gas--killer/service-blue.svg)](https://github.com/gas-killer/service/pkgs/container/service)

Gas Killer service implementation built on EigenLayer with BLS signature aggregation for optimized transaction execution.

## Overview

The service coordinates multiple operator nodes to sign messages, aggregates their BLS signatures when a threshold is reached, and executes the result onchain via `verifyAndUpdate`.

## Repository Structure

- **`router/`** — Orchestrator service: aggregates signatures and executes onchain
- **`node/`** — Operator node: validates and signs tasks
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

The system consists of:

- **Orchestrator**: Coordinates the aggregation process
- **Creator**: Generates payloads and manages rounds
- **Executor**: Handles onchain execution
- **Validator**: Validates messages and signatures using EVM gas analysis
- **Contributors**: Operator nodes that sign messages (implemented in `node/`)

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
- `ROUND_TIMEOUT`: Max seconds the router waits for operator signatures on a round before abandoning it and moving to the next task (accepts fractional seconds). The orchestrator submits immediately once the signature threshold is reached, so this only affects rounds that fail to reach quorum. Library default: 30; Helm deployments set 300. Must exceed worst-case node compute + sign time.
- `REBROADCAST_INTERVAL`: How often (in seconds) the router re-sends the `Start` broadcast for an in-flight round while waiting for signatures (accepts fractional seconds). Set longer than `ROUND_TIMEOUT` to disable intra-round rebroadcasting. Library default: 30; Helm deployments set 300.
- `THRESHOLD`: Minimum signatures required for aggregation
- `INGRESS`: Enable HTTP ingress mode (true/false)
- `INGRESS_ADDRESS`: Address for ingress server (default: 0.0.0.0:8080)
- `INGRESS_TIMEOUT_MS`: Timeout for waiting on ingress tasks in milliseconds (default: 0, no timeout)
- `ADMIN_KEY`: Shared secret guarding the `/admin/keys` endpoints, used to mint and revoke the per-client API keys that authenticate `/trigger`. Omit or leave empty to disable the admin API.
- `QUORUM_NUMBER`: Quorum number to use (default: 0)

Contributor key files are generated automatically by the Docker setup and do not need to be set manually.

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

## Ethereum JSON-RPC Ingress

The router also speaks standard Ethereum JSON-RPC at `POST /rpc`, so existing wallets and
tooling can submit to Gas Killer by **switching their RPC URL over** — no payload changes:

```bash
# Wallet-style URL with the API key in the path (for clients that cannot set headers):
#   http://localhost:8080/rpc/gk_...
# Or programmatic clients with a Bearer header:
cast send <target> "sum(uint256[])" "[1,2,3]" \
  --rpc-url http://localhost:8080/rpc/gk_... \
  --private-key $PRIVATE_KEY
```

How it works:

- **`eth_sendRawTransaction`** is intercepted: the signed transaction is decoded, the sender is
  **recovered from its ECDSA signature** and used as the task's `from_address` (so acting as an
  address requires holding its key — operators simulate the call with it as `msg.sender`), the
  target/calldata/value become the task, `block_height` is pinned to the current head, and the
  transition index is auto-resolved. The transaction hash is returned, but the transaction itself
  is never broadcast: the state change lands via the router's `verifyAndUpdate`, the sender pays
  **zero gas**, and no receipt will exist for the returned hash. Duplicate submissions of the
  same raw bytes return the same hash without enqueueing a second task (a duplicate racing a
  still-in-flight first submission gets the geth-style `already known` error). When a
  transaction carries an EIP-155 chain id, that signed intent picks the chain: the target must
  have code on that chain, and same-address deployments on multiple configured chains are
  refused as ambiguous.
- **Read methods** (`eth_chainId`, `eth_blockNumber`, `eth_getTransactionCount`, `eth_gasPrice`,
  `eth_call`, `eth_estimateGas`, ... — any `eth_`/`net_`/`web3_` read) are proxied to the
  upstream chain RPC, so nonce fetching and gas estimation keep working. `?chain=l2` selects the
  L2 upstream when configured. Batches (JSON arrays) are supported.
- **Rejected with clear JSON-RPC errors**: contract creation, plain ETH transfers (no calldata),
  blob/set-code transaction types, transactions whose chain id does not match the chain where
  the target contract lives, value transfers the sender cannot fund, and unknown/unsafe methods
  (`eth_sendTransaction`, `eth_sign*`, `debug_*`, ...). Auth failures return `-32001`; queue
  saturation returns `-32005`.

Sender attribution and replay (trustless): `eth_sendRawTransaction` tasks settle through
`GasKillerSDK.verifyAndUpdateWithAuth`, which **reconstructs the transaction's EIP-1559 signing
hash on-chain and recovers the sender**, binding the executed call to the signer cryptographically
(not by operator trust), and rejects a reused `(signer, nonce)` — on-chain replay protection for a
transaction that is never broadcast. Only EIP-1559 (type-2) transactions with an empty access list
are accepted on this path; legacy/2930/blob/set-code types are rejected. This requires the deployed
`GasKillerSDK` to include `verifyAndUpdateWithAuth` (the solidity-sdk trustless-auth change); the
`/trigger` path continues to use the permissionless `verifyAndUpdate`. Use `send_raw_tx_request`
for the full e2e flow:
```bash
GAS_KILLER_API_KEY=gk_... GAS_KILLER_TARGET_ADDRESS=0x... cargo run -p scripts --bin send_raw_tx_request
```

## Development

### Dependencies
- `alloy`: Ethereum interaction
- `commonware-avs-*`: AVS protocol types, node, and router libraries
- `gas-analyzer-evmsketch`: EVM gas analysis and storage update computation
- `commonware-cryptography`: Cryptographic operations
- `commonware-p2p`: P2P networking
- `commonware-runtime`: Runtime utilities

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

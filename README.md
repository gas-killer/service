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
- `AGGREGATION_FREQUENCY`: Signature aggregation frequency in seconds (default: 90)
- `THRESHOLD`: Minimum signatures required for aggregation
- `INGRESS`: Enable HTTP ingress mode (true/false)
- `INGRESS_ADDRESS`: Address for ingress server (default: 0.0.0.0:8080)
- `INGRESS_TIMEOUT_MS`: Timeout for waiting on ingress tasks in milliseconds (default: 0, no timeout)
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
      "target_address": "0x...",
      "call_data": "0x...",
      "transition_index": 1,
      "from_address": "0x...",
      "value": 0,
      "block_height": 0
    }
  }'
```

Use the `send_request` script for a complete end-to-end trigger against an ArraySummation contract:
```bash
cargo run -p scripts --bin send_request
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

#!/bin/bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
LOG_DIR="$PROJECT_ROOT/logs"

# Task submission is authenticated with per-client API keys minted through the admin API, which
# is guarded by ADMIN_KEY. Use a fixed dev value unless the caller overrides it; docker-compose
# reads the same default for the router so the two stay in sync.
export ADMIN_KEY="${ADMIN_KEY:-ci-admin-key}"

# Capture the caller's signature scheme (default: bls) before `cp example.env .env`
# and `source ../.env` can let the example default win. Re-exported after the
# source below so it reaches the deploy binary; docker-compose reads it directly
# from the environment for the node/router containers.
SIGNATURE_SCHEME_CHOICE="${SIGNATURE_SCHEME:-bls}"
export SIGNATURE_SCHEME="$SIGNATURE_SCHEME_CHOICE"
echo "Signature scheme: $SIGNATURE_SCHEME_CHOICE"

# STATE_ENCODING (legacy|canonical) and E2E_EXAMPLE (array-summation|reentrant) follow the
# same capture-then-re-export discipline as SIGNATURE_SCHEME so the containers AND the
# host-side deploy/send binaries agree. `reentrant` deploys a ReentrantCheckpoint whose
# task re-enters mid-transition; pair it with `canonical` to prove re-entrancy is safe.
STATE_ENCODING_CHOICE="${STATE_ENCODING:-legacy}"
export STATE_ENCODING="$STATE_ENCODING_CHOICE"
E2E_EXAMPLE_CHOICE="${E2E_EXAMPLE:-array-summation}"
export E2E_EXAMPLE="$E2E_EXAMPLE_CHOICE"
echo "State encoding: $STATE_ENCODING_CHOICE | e2e example: $E2E_EXAMPLE_CHOICE"

# STAKE_SOURCE (eigenlayer|commitments) selects the operator-set root. `commitments`
# skips the EigenLayer setup container entirely (a no-op override satisfies the compose
# dependency) and deploys the Commitments stack from the host via forge instead —
# requires foundry plus local checkouts of the two contract repos:
#   COMMITMENTS_DIR   — AInima-Collective/commitments
#   SOLIDITY_SDK_DIR  — gas-killer/solidity-sdk (branch with src/commitments/)
STAKE_SOURCE_CHOICE="${STAKE_SOURCE:-eigenlayer}"
export STAKE_SOURCE="$STAKE_SOURCE_CHOICE"
COMPOSE="docker compose"
if [ "$STAKE_SOURCE_CHOICE" = "commitments" ]; then
    if [ "$SIGNATURE_SCHEME_CHOICE" != "schnorr" ]; then
        echo -e "${RED}STAKE_SOURCE=commitments requires SIGNATURE_SCHEME=schnorr (the BLS path is EigenLayer-only)${NC}"
        exit 1
    fi
    COMPOSE="docker compose -f docker-compose.yml -f docker-compose.commitments.yml"
fi
echo "Stake source: $STAKE_SOURCE_CHOICE"

# Track if test passed
TEST_PASSED=false

# Create logs directory
mkdir -p "$LOG_DIR"

# Cleanup function
cleanup() {
    echo -e "${YELLOW}Cleaning up Docker containers...${NC}"

    # If test didn't pass, dump all container logs for debugging
    if [ "$TEST_PASSED" != "true" ]; then
        echo -e "${YELLOW}=== Dumping all container logs for debugging ===${NC}"
        echo -e "${YELLOW}Ethereum logs:${NC}"
        docker compose logs ethereum 2>/dev/null || true
        echo -e "${YELLOW}Eigenlayer logs:${NC}"
        docker compose logs eigenlayer 2>/dev/null || true
        echo -e "${YELLOW}Router logs:${NC}"
        docker compose logs router 2>/dev/null || true
        echo -e "${YELLOW}Node-1 logs:${NC}"
        docker compose logs node-1 2>/dev/null || true
        echo -e "${YELLOW}Node-2 logs:${NC}"
        docker compose logs node-2 2>/dev/null || true
        echo -e "${YELLOW}Node-3 logs:${NC}"
        docker compose logs node-3 2>/dev/null || true
        echo -e "${YELLOW}Signer logs:${NC}"
        docker compose logs signer 2>/dev/null || true
    fi

    cd "$PROJECT_ROOT"
    docker compose down || true
    echo -e "${GREEN}Cleanup completed${NC}"
}

# Parse flags/env for keeping containers up after script finishes
KEEP_UP=false
for arg in "$@"; do
    case "$arg" in
        --keep-up|--no-cleanup)
            KEEP_UP=true
            ;;
    esac
done

if [ "${KEEP_CONTAINERS:-}" = "1" ] || [ "${KEEP_CONTAINERS:-}" = "true" ]; then
    KEEP_UP=true
fi

# Set trap for cleanup unless explicitly keeping containers up
if [ "$KEEP_UP" = true ]; then
    echo -e "${YELLOW}Skipping auto-cleanup; containers will remain running. Use 'docker compose down' to stop.${NC}"
else
    trap cleanup EXIT INT TERM
fi

echo -e "${GREEN}Starting Gas Killer E2E Test${NC}"
echo "Project root: $PROJECT_ROOT"
echo "Logs directory: $LOG_DIR"

# Step 1: Build scripts
echo -e "${YELLOW}Step 1: Building scripts...${NC}"
cd "$PROJECT_ROOT/scripts"
cargo build --release -p scripts --bin deploy_array_summation
cargo build --release -p scripts --bin send_request
cargo build --release -p scripts --bin verify_message_hash_parity
cd "$PROJECT_ROOT"

# Step 2: Assume .env already exists and contains required values
echo -e "${YELLOW}Step 2: Using existing .env without modification...${NC}"
if [ ! -f .env ]; then
    cp example.env .env
    echo ".env created from example.env"
else
    echo ".env already exists; leaving it unchanged"
fi

echo "Environment configuration complete"

# Step 3: Pull Docker images
echo -e "${YELLOW}Step 3: Pulling Docker images...${NC}"
$COMPOSE pull

# Step 4: Build service images
echo -e "${YELLOW}Step 4: Building service Docker images...${NC}"
$COMPOSE build

# Step 4b (commitments): the node containers bind-mount per-operator key files, so the
# files must exist before compose up. In eigenlayer mode the setup container writes
# them; here the generator binary does (idempotent — existing keys are kept).
if [ "$STAKE_SOURCE_CHOICE" = "commitments" ]; then
    echo -e "${YELLOW}Step 4b: Generating operator key files...${NC}"
    mkdir -p config/.nodes/operator_keys
    (cd "$PROJECT_ROOT/scripts" && \
        OPERATOR_KEYS_DIR="$PROJECT_ROOT/config/.nodes/operator_keys" \
        cargo run --release -p scripts --bin generate_operator_keys)
fi

# Step 5: Start Docker Compose services
echo -e "${YELLOW}Step 5: Starting Docker Compose services...${NC}"
$COMPOSE up -d

# Show running containers
$COMPOSE ps

# Step 6: Provision the stake root. EigenLayer mode waits for the setup container;
# commitments mode deploys the Commitments stack from the host, interleaving the two
# contract repos (upstream deploy scripts for the Commitments legs, solidity-sdk's
# CommitmentsGasKiller.s.sol for the Gas Killer legs — the arbiter must exist before
# the OperatorRegistry bakes it in as its immutable requiredArbiter, and the
# SchnorrCommitmentsAdapter before the SchnorrStakeRegistry takes it as its immutable
# owner).
if [ "$STAKE_SOURCE_CHOICE" = "commitments" ]; then
    echo -e "${YELLOW}Step 6: Deploying the Commitments stack (host forge)...${NC}"
    COMMITMENTS_DIR="${COMMITMENTS_DIR:?COMMITMENTS_DIR must point at a checkout of AInima-Collective/commitments}"
    SOLIDITY_SDK_DIR="${SOLIDITY_SDK_DIR:?SOLIDITY_SDK_DIR must point at a checkout of gas-killer/solidity-sdk}"

    # The forge legs need the deployer key and quorum settings from .env, but the
    # caller's stack choices must survive the source (same discipline as Step 7).
    set -a; source .env; set +a
    export SIGNATURE_SCHEME="$SIGNATURE_SCHEME_CHOICE"
    export STATE_ENCODING="$STATE_ENCODING_CHOICE"
    export E2E_EXAMPLE="$E2E_EXAMPLE_CHOICE"
    export STAKE_SOURCE="$STAKE_SOURCE_CHOICE"

    E2E_RPC="http://localhost:8545"
    DEPLOYER_ADDR=$(cast wallet address "$PRIVATE_KEY")
    OPERATOR_STAKE="${OPERATOR_STAKE_AMOUNT:-100}"

    echo "Waiting for anvil at $E2E_RPC..."
    for _ in $(seq 1 60); do
        if cast block-number --rpc-url "$E2E_RPC" >/dev/null 2>&1; then break; fi
        sleep 2
    done
    cast block-number --rpc-url "$E2E_RPC" >/dev/null

    forge_leg() { # forge_leg <dir> <script target> [env pairs...]
        local dir="$1"; local target="$2"; shift 2
        (cd "$dir" && env "$@" forge script "$target" \
            --rpc-url "$E2E_RPC" --broadcast --private-key "$PRIVATE_KEY" 2>&1)
    }
    grab() { # grab <output> <label>  — parses "label: 0x..." or "label=0x..."
        echo "$1" | grep -E "$2[:=]" | tail -1 | sed -E 's/.*[:=][[:space:]]*//' | tr -d '[:space:]'
    }

    echo "Leg 1/5: CommitmentManager..."
    OUT=$(forge_leg "$COMMITMENTS_DIR" script/DeployCommitmentManager.s.sol \
        ADMIN_ADDRESS="$DEPLOYER_ADDR" UNBONDING_PERIOD=86400) || { echo "$OUT"; exit 1; }
    MANAGER=$(grab "$OUT" "CommitmentManager proxy")
    [ -n "$MANAGER" ] || { echo -e "${RED}Leg 1 failed:${NC}"; echo "$OUT" | tail -30; exit 1; }

    echo "Leg 2/5: backing adapters..."
    OUT=$(forge_leg "$COMMITMENTS_DIR" script/DeployAdaptersAndStrategies.s.sol \
        COMMITMENT_MANAGER_ADDRESS="$MANAGER") || { echo "$OUT"; exit 1; }
    BACKING_ADAPTER=$(grab "$OUT" "ERC20StaticAdapter")
    [ -n "$BACKING_ADAPTER" ] || { echo -e "${RED}Leg 2 failed:${NC}"; echo "$OUT" | tail -30; exit 1; }

    echo "Leg 3/5: stake token + SP1 arbiter (solidity-sdk)..."
    OUT=$(forge_leg "$SOLIDITY_SDK_DIR" script/CommitmentsGasKiller.s.sol:GasKillerCommitmentsPhase1 \
        COMMITMENT_MANAGER_ADDRESS="$MANAGER" ADMIN_ADDRESS="$DEPLOYER_ADDR") || { echo "$OUT"; exit 1; }
    STAKE_TOKEN=$(grab "$OUT" "GK_STAKE_TOKEN")
    ARBITER=$(grab "$OUT" "GK_ARBITER")
    SP1_VERIFIER=$(grab "$OUT" "GK_SP1_VERIFIER")
    [ -n "$ARBITER" ] || { echo -e "${RED}Leg 3 failed:${NC}"; echo "$OUT" | tail -30; exit 1; }

    echo "Leg 4/5: OperatorRegistry..."
    OUT=$(forge_leg "$COMMITMENTS_DIR" script/DeployOperatorRegistry.s.sol \
        COMMITMENT_MANAGER_ADDRESS="$MANAGER" MIN_OPERATOR_STAKE="$OPERATOR_STAKE" \
        ADMIN_ADDRESS="$DEPLOYER_ADDR" REQUIRED_ARBITER_ADDRESS="$ARBITER" \
        REQUIRED_TOKEN_ADDRESS="$STAKE_TOKEN") || { echo "$OUT"; exit 1; }
    OPERATOR_REGISTRY=$(grab "$OUT" "OperatorRegistry proxy")
    [ -n "$OPERATOR_REGISTRY" ] || { echo -e "${RED}Leg 4 failed:${NC}"; echo "$OUT" | tail -30; exit 1; }
    cast send --rpc-url "$E2E_RPC" --private-key "$PRIVATE_KEY" \
        "$MANAGER" "setOperatorRegistry(address)" "$OPERATOR_REGISTRY" >/dev/null

    echo "Leg 5/5: Schnorr adapter + registry + wiring (solidity-sdk)..."
    OUT=$(forge_leg "$SOLIDITY_SDK_DIR" script/CommitmentsGasKiller.s.sol:GasKillerCommitmentsPhase2 \
        OPERATOR_REGISTRY_ADDRESS="$OPERATOR_REGISTRY" ARBITER_ADDRESS="$ARBITER" \
        ADMIN_ADDRESS="$DEPLOYER_ADDR" WEIGHT_SCALE="$OPERATOR_STAKE" \
        QUORUM_THRESHOLD="${QUORUM_THRESHOLD:-2}" THRESHOLD_DENOMINATOR="${THRESHOLD_DENOMINATOR:-3}" \
        SCHNORR_NOTICE_WINDOW="${SCHNORR_NOTICE_WINDOW:-0}") || { echo "$OUT"; exit 1; }
    SCHNORR_ADAPTER=$(grab "$OUT" "GK_SCHNORR_ADAPTER")
    SCHNORR_REGISTRY=$(grab "$OUT" "GK_SCHNORR_STAKE_REGISTRY")
    [ -n "$SCHNORR_REGISTRY" ] || { echo -e "${RED}Leg 5 failed:${NC}"; echo "$OUT" | tail -30; exit 1; }

    mkdir -p config/.nodes
    jq -n \
        --arg manager "$MANAGER" --arg opreg "$OPERATOR_REGISTRY" \
        --arg backing "$BACKING_ADAPTER" --arg token "$STAKE_TOKEN" \
        --arg arbiter "$ARBITER" --arg sp1 "$SP1_VERIFIER" \
        --arg adapter "$SCHNORR_ADAPTER" --arg registry "$SCHNORR_REGISTRY" \
        '{addresses: {commitmentManager: $manager, operatorRegistry: $opreg,
          backingAdapter: $backing, stakeToken: $token, gasKillerArbiter: $arbiter,
          sp1Verifier: $sp1, schnorrCommitmentsAdapter: $adapter,
          schnorrStakeRegistry: $registry}, lastUpdate: {block_number: 0}}' \
        > config/.nodes/avs_deploy.json
    echo -e "${GREEN}Commitments stack deployed; avs_deploy.json written${NC}"
    cat config/.nodes/avs_deploy.json
else
    echo -e "${YELLOW}Step 6: Waiting for EigenLayer setup to complete...${NC}"
    timeout=500
    elapsed=0

    while [ $elapsed -lt $timeout ]; do
        # Check if eigenlayer container has completed setup
        if docker compose logs eigenlayer 2>/dev/null | grep -q "Operator 3 weight in quorum" && [ -f config/.nodes/avs_deploy.json ]; then
            echo -e "${GREEN}EigenLayer setup completed successfully${NC}"
            break
        fi

        echo "Waiting for EigenLayer setup... ($elapsed/$timeout seconds)"
        sleep 10
        elapsed=$((elapsed + 10))
    done

    if [ $elapsed -ge $timeout ]; then
        echo -e "${RED}Timeout waiting for EigenLayer setup${NC}"
        echo "Eigenlayer logs:"
        docker compose logs eigenlayer
        exit 1
    fi
fi

# Fix permissions on config/.nodes directory so deploy script can write
echo "Fixing file permissions..."
sudo chmod -R 777 config/.nodes || chmod -R 777 config/.nodes

# Give extra time for nodes to initialize
echo "Waiting for nodes to initialize..."
sleep 30

# Step 7: Deploy Gas Killer example contract (ArraySummation)
echo -e "${YELLOW}Step 7: Deploying Gas Killer example contract (ArraySummation)...${NC}"
cd "$PROJECT_ROOT/scripts"

# Source environment and run deployment
source ../.env
# `source ../.env` may reset these to the example defaults; restore the caller's
# choices so deploy_array_summation / send_request pick the right stack, encoding,
# and example target.
export SIGNATURE_SCHEME="$SIGNATURE_SCHEME_CHOICE"
export STATE_ENCODING="$STATE_ENCODING_CHOICE"
export E2E_EXAMPLE="$E2E_EXAMPLE_CHOICE"
export STAKE_SOURCE="$STAKE_SOURCE_CHOICE"
export AVS_DEPLOYMENT_PATH="../config/.nodes/avs_deploy.json"

if [ ! -f "$AVS_DEPLOYMENT_PATH" ]; then
    echo -e "${RED}Deployment file not found at $AVS_DEPLOYMENT_PATH${NC}"
    exit 1
fi

echo "Running ArraySummation deployment..."
cargo run --release -p scripts --bin deploy_array_summation

if [ $? -eq 0 ]; then
    echo -e "${GREEN}ArraySummation deployment completed successfully${NC}"
else
    echo -e "${RED}ArraySummation deployment failed${NC}"
    echo -e "${YELLOW}Recent ethereum logs:${NC}"
    docker compose logs --tail=100 ethereum || true
    echo -e "${YELLOW}Recent eigenlayer logs:${NC}"
    docker compose logs --tail=100 eigenlayer || true
    exit 1
fi

# Extract deployed ArraySummation address from deployment JSON
DEPLOY_JSON_PATH="$AVS_DEPLOYMENT_PATH"
if command -v jq >/dev/null 2>&1; then
    ARRAY_SUMMATION_ADDRESS=$(jq -r '.addresses.arraySummation // empty' "$DEPLOY_JSON_PATH")
else
    ARRAY_SUMMATION_ADDRESS=$(grep -o '"arraySummation"\s*:\s*"[^"]*"' "$DEPLOY_JSON_PATH" | sed 's/.*"arraySummation"\s*:\s*"\([^"]*\)"/\1/')
fi

if [ -z "$ARRAY_SUMMATION_ADDRESS" ]; then
    echo -e "${YELLOW}Warning: Could not determine ArraySummation address from $DEPLOY_JSON_PATH${NC}"
else
    echo "Discovered ArraySummation address: $ARRAY_SUMMATION_ADDRESS"
    # Set as the default target for Gas Killer trigger helper
    export GAS_KILLER_TARGET_ADDRESS="$ARRAY_SUMMATION_ADDRESS"
fi

cd "$PROJECT_ROOT"

# Step 7b: Verify the router's local payload hash matches the contract's getMessageHash
echo -e "${YELLOW}Step 7b: Verifying message-hash parity (build_payload_hash vs on-chain getMessageHash)...${NC}"
cd "$PROJECT_ROOT/scripts"
if ! cargo run --release -p scripts --bin verify_message_hash_parity; then
    echo -e "${RED}❌ Message-hash parity check FAILED — local build_payload_hash diverges from on-chain getMessageHash${NC}"
    cd "$PROJECT_ROOT"
    docker compose logs --tail=100 ethereum || true
    exit 1
fi
echo -e "${GREEN}✅ Message-hash parity verified${NC}"
cd "$PROJECT_ROOT"

# Step 8: Wait for router ingress to be reachable
echo -e "${YELLOW}Step 8: Waiting for router ingress to be ready...${NC}"
ROUTER_HEALTH_URL="http://localhost:8080/healthz"
ROUTER_TIMEOUT=120
ROUTER_INTERVAL=3
elapsed=0
until curl -sf "$ROUTER_HEALTH_URL" > /dev/null 2>&1; do
    if [ "$elapsed" -ge "$ROUTER_TIMEOUT" ]; then
        echo -e "${RED}Timeout: router ingress not ready after ${ROUTER_TIMEOUT}s${NC}"
        docker compose logs --tail=50 router || true
        exit 1
    fi
    echo "Waiting for router ingress... (${elapsed}s)"
    sleep "$ROUTER_INTERVAL"
    elapsed=$((elapsed + ROUTER_INTERVAL))
done
echo -e "${GREEN}Router ingress is ready (${elapsed}s)${NC}"

# Step 9: Brief wait for services to stabilize
echo -e "${YELLOW}Step 9: Waiting briefly for services to stabilize...${NC}"
sleep 5

# Step 9b: Mint an API key so task submission is authenticated. The router requires a valid,
# unrevoked key on /tasks; mint one via the admin API (guarded by ADMIN_KEY) and hand it to
# send_request through GAS_KILLER_API_KEY.
echo -e "${YELLOW}Step 9b: Minting an API key via the admin endpoint...${NC}"
CREATE_RESP=$(curl -s -X POST \
    -H "Authorization: Bearer $ADMIN_KEY" \
    -H "Content-Type: application/json" \
    -d '{"label":"e2e"}' \
    http://localhost:8080/admin/keys)
if command -v jq >/dev/null 2>&1; then
    GAS_KILLER_API_KEY=$(printf '%s' "$CREATE_RESP" | jq -r '.key // empty')
else
    GAS_KILLER_API_KEY=$(printf '%s' "$CREATE_RESP" | grep -o '"key"[[:space:]]*:[[:space:]]*"[^"]*"' | sed 's/.*:[[:space:]]*"\([^"]*\)".*/\1/')
fi
case "$GAS_KILLER_API_KEY" in
    gk_*) ;;
    *)
        echo -e "${RED}Failed to mint API key. Admin response: $CREATE_RESP${NC}"
        docker compose logs --tail=50 router || true
        exit 1
        ;;
esac
export GAS_KILLER_API_KEY
echo -e "${GREEN}Minted API key for task submission${NC}"

# Step 10: Trigger Gas Killer task and verify execution
echo -e "${YELLOW}Step 10: Triggering task and verifying execution...${NC}"
echo "Sending a test task to the router..."
cd "$PROJECT_ROOT/scripts"
# send_request signs and submits the rendered payload and prints the verifyAndUpdate tx hash;
# capture its output so the diagnostics below can trace that tx. The `tee` pipeline exits 0, so
# `set -e` does not abort here — the real status comes from PIPESTATUS.
SEND_REQUEST_LOG="$(mktemp)"
cargo run --release -p scripts --bin send_request 2>&1 | tee "$SEND_REQUEST_LOG"
TRIGGER_STATUS=${PIPESTATUS[0]}
cd "$PROJECT_ROOT"

# The user-submitted verifyAndUpdate tx hash, extracted from send_request's output.
USER_TX_HASH=$(grep -oE 'landed: tx 0x[a-fA-F0-9]{64}' "$SEND_REQUEST_LOG" | sed -E 's/.*tx //' | tail -1)

if [ $TRIGGER_STATUS -eq 0 ]; then
    echo -e "${GREEN}✅ Array summation verified successfully - state was updated!${NC}"
else
    echo -e "${RED}❌ Array summation verification failed - state was not updated within timeout.${NC}"
    echo -e "${YELLOW}Recent router logs:${NC}"
    docker compose logs --tail=100 router || true
    echo -e "${YELLOW}Recent node logs:${NC}"
    docker compose logs --tail=50 node-1 node-2 node-3 || true
    # If the user-submitted verifyAndUpdate reverted, re-simulate it to surface a revert reason.
    # cast run re-simulates the transaction, so this is best-effort diagnostic output.
    if [ -n "$USER_TX_HASH" ] && command -v cast >/dev/null 2>&1; then
        echo -e "${YELLOW}Execution trace for $USER_TX_HASH:${NC}"
        cast run "$USER_TX_HASH" --rpc-url http://localhost:8545 || true
    fi
    exit 1
fi

# Show recent router logs for confirmation
echo -e "${YELLOW}Recent router logs:${NC}"
docker compose logs --tail=50 router || true

# Print the execution trace of the successful user-submitted verifyAndUpdate for inspection.
# debug_traceTransaction reflects the real mined execution, not a re-simulation.
if [ -n "$USER_TX_HASH" ] && command -v cast >/dev/null 2>&1; then
    echo -e "${YELLOW}Execution trace for $USER_TX_HASH:${NC}"
    cast rpc debug_traceTransaction "$USER_TX_HASH" '{"tracer":"callTracer"}' --rpc-url http://localhost:8545 | jq '.' || true
fi

echo -e "${GREEN}✅ Test passed - Stack is up and array summation completed successfully!${NC}"
TEST_PASSED=true
exit 0
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
docker compose pull

# Step 4: Build service images
echo -e "${YELLOW}Step 4: Building service Docker images...${NC}"
docker compose build

# Step 5: Start Docker Compose services
echo -e "${YELLOW}Step 5: Starting Docker Compose services...${NC}"
docker compose up -d

# Show running containers
docker compose ps

# Step 6: Wait for EigenLayer setup to complete
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

# Fix permissions on config/.nodes directory so deploy script can write
echo "Fixing file permissions..."
sudo chmod -R 777 config/.nodes || chmod -R 777 config/.nodes

# Give extra time for nodes to initialize
echo "Waiting for nodes to initialize..."
sleep 30

# Step 7: Deploy the Gas Killer consumer under test (GK_E2E_CONSUMER:
# array-summation [default] or onchain-llm — the solidity-sdk LLM example).
if [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm" ]; then
    echo -e "${YELLOW}Step 7: Deploying Gas Killer on-chain LLM consumer (stories260K)...${NC}"
    # Load harness config for this branch (the Rust helpers read .env themselves)
    set -a
    # shellcheck disable=SC1091
    . ./.env
    set +a
    LLM_ADDRESS=$(bash "$PROJECT_ROOT/scripts/deploy_onchain_llm.sh" | tee /dev/stderr | grep '^LLM_TARGET=' | cut -d= -f2)
    if [ -z "$LLM_ADDRESS" ]; then
        echo -e "${RED}on-chain LLM deployment failed${NC}"
        exit 1
    fi
    echo "Discovered GasKillerLLM address: $LLM_ADDRESS"
    export GAS_KILLER_TARGET_ADDRESS="$LLM_ADDRESS"
    export GAS_KILLER_CALL_DATA=$(cast calldata "tellStory(string,uint256)" "${GK_LLM_PROMPT:-Once upon a time}" "${GK_LLM_MAX_TOKENS:-32}")
    export GAS_KILLER_FROM_ADDRESS=$(cast wallet address --private-key "$PRIVATE_KEY")
    export GAS_KILLER_TRANSITION_INDEX=auto
    export GK_VERIFY_MODE=transition-count
    export GK_VERIFY_TIMEOUT_SECS="${GK_VERIFY_TIMEOUT_SECS:-300}"
else
echo -e "${YELLOW}Step 7: Deploying Gas Killer example contract (ArraySummation)...${NC}"
cd "$PROJECT_ROOT/scripts"

# Source environment and run deployment
source ../.env
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

fi

# Step 7a (unbounded mode only): prove the tracked function is *unexecutable* in a
# real block — direct sum() must cost more than the mainnet block gas limit. The
# Gas Killer pipeline then lands the same state transition in one small
# verifyAndUpdate tx (asserted after step 10). Requires the anvil service to run
# with --disable-block-gas-limit (ANVIL_EXTRA_ARGS) so the estimate can complete.
MAINNET_BLOCK_GAS_LIMIT=30000000
if [ "${GK_SIM_PROFILE:-chain}" = "unbounded-v1" ] && [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm" ]; then
    echo -e "${YELLOW}Step 7a: Asserting direct tellStory() execution exceeds the block gas limit...${NC}"
    MAINNET_BLOCK_GAS_LIMIT=30000000
    DIRECT_GAS=$(cast estimate "$GAS_KILLER_TARGET_ADDRESS" "tellStory(string,uint256)" "${GK_LLM_PROMPT:-Once upon a time}" "${GK_LLM_MAX_TOKENS:-32}" --rpc-url http://localhost:8545)
    echo "Direct tellStory() estimate: $DIRECT_GAS gas (block limit: $MAINNET_BLOCK_GAS_LIMIT)"
    if [ "$DIRECT_GAS" -le "$MAINNET_BLOCK_GAS_LIMIT" ]; then
        echo -e "${RED}Expected direct LLM execution above the block gas limit${NC}"
        exit 1
    fi
    echo -e "${GREEN}✅ Direct execution is unlandable on-chain (${DIRECT_GAS} gas) — proceeding with Gas Killer${NC}"
elif [ "${GK_SIM_PROFILE:-chain}" = "unbounded-v1" ] && [ -n "$ARRAY_SUMMATION_ADDRESS" ]; then
    echo -e "${YELLOW}Step 7a: Asserting direct sum() execution exceeds the block gas limit...${NC}"
    if ! command -v cast >/dev/null 2>&1; then
        echo -e "${RED}cast (foundry) is required for the unbounded e2e assertions${NC}"
        exit 1
    fi
    DIRECT_GAS=$(cast estimate "$ARRAY_SUMMATION_ADDRESS" "sum(uint256[])" "[]" --rpc-url http://localhost:8545)
    echo "Direct sum([]) execution needs $DIRECT_GAS gas (mainnet block limit: $MAINNET_BLOCK_GAS_LIMIT)"
    if [ -z "$DIRECT_GAS" ] || [ "$DIRECT_GAS" -le "$MAINNET_BLOCK_GAS_LIMIT" ]; then
        echo -e "${RED}Expected direct execution above the block gas limit; increase ARRAY_SUMMATION_ARRAY_SIZE${NC}"
        exit 1
    fi
    echo -e "${GREEN}✅ Direct execution cannot fit in a real block ($DIRECT_GAS > $MAINNET_BLOCK_GAS_LIMIT gas)${NC}"
fi

cd "$PROJECT_ROOT"

# Step 7b: Verify the router's local payload hash matches the contract's getMessageHash
# (builds an ArraySummation sum() payload; skipped for other consumers)
if [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm" ]; then
    echo -e "${YELLOW}Step 7b: Skipped (ArraySummation-specific parity harness)${NC}"
else
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
fi

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
# unrevoked key on /trigger; mint one via the admin API (guarded by ADMIN_KEY) and hand it to
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
cargo run --release -p scripts --bin send_request
TRIGGER_STATUS=$?
cd "$PROJECT_ROOT"

if [ $TRIGGER_STATUS -eq 0 ]; then
    echo -e "${GREEN}✅ Array summation verified successfully - state was updated!${NC}"
else
    echo -e "${RED}❌ Array summation verification failed - state was not updated within timeout.${NC}"
    echo -e "${YELLOW}Recent router logs:${NC}"
    docker compose logs --tail=100 router || true
    echo -e "${YELLOW}Recent node logs:${NC}"
    docker compose logs --tail=50 node-1 node-2 node-3 || true
    # Trace the verifyAndUpdate transaction, if one was submitted, to surface a revert reason.
    # cast run re-simulates the transaction, so this is best-effort diagnostic output.
    TX_HASH=$(docker compose logs router 2>/dev/null | grep "Contract execution result" | grep -o "transaction_hash=0x[a-fA-F0-9]*" | sed 's/transaction_hash=//' | tail -1)
    if [ -n "$TX_HASH" ] && command -v cast >/dev/null 2>&1; then
        echo -e "${YELLOW}Execution trace for $TX_HASH:${NC}"
        cast run "$TX_HASH" --rpc-url http://localhost:8545 || true
    fi
    exit 1
fi

# Show recent router logs for confirmation
echo -e "${YELLOW}Recent router logs:${NC}"
docker compose logs --tail=50 router || true

# Step 10b (unbounded mode only): the same transition that could not fit in a real
# block must have landed on-chain as one small verifyAndUpdate tx. This is the
# unbounded-mode claim in one comparison: unbounded compute, O(1) on-chain state.
if [ "${GK_SIM_PROFILE:-chain}" = "unbounded-v1" ]; then
    echo -e "${YELLOW}Step 10b: Asserting verifyAndUpdate landed far below the block gas limit...${NC}"
    VU_TX_HASH=$(docker compose logs router 2>/dev/null | grep "Contract execution result" | grep -o "transaction_hash=0x[a-fA-F0-9]*" | sed 's/transaction_hash=//' | tail -1)
    if [ -z "$VU_TX_HASH" ]; then
        echo -e "${RED}Could not find the verifyAndUpdate transaction hash in router logs${NC}"
        exit 1
    fi
    VU_GAS=$(cast receipt "$VU_TX_HASH" gasUsed --rpc-url http://localhost:8545)
    VU_GAS=$((VU_GAS))  # normalize possible hex to decimal
    echo "verifyAndUpdate used $VU_GAS gas vs $DIRECT_GAS gas for direct execution"
    if [ "$VU_GAS" -ge "$MAINNET_BLOCK_GAS_LIMIT" ]; then
        echo -e "${RED}verifyAndUpdate unexpectedly used a full block's gas${NC}"
        exit 1
    fi
    RATIO=$((DIRECT_GAS / VU_GAS))
    echo -e "${GREEN}✅ Unbounded transition applied on-chain: ${VU_GAS} gas (direct execution: ${DIRECT_GAS} gas, ~${RATIO}x more) — above-block-limit compute, one small on-chain tx${NC}"
fi

# Print the execution trace of the successful verifyAndUpdate for inspection.
# debug_traceTransaction reflects the real mined execution, not a re-simulation.
TX_HASH=$(docker compose logs router 2>/dev/null | grep "Contract execution result" | grep -o "transaction_hash=0x[a-fA-F0-9]*" | sed 's/transaction_hash=//' | tail -1)
if [ -n "$TX_HASH" ] && command -v cast >/dev/null 2>&1; then
    echo -e "${YELLOW}Execution trace for $TX_HASH:${NC}"
    cast rpc debug_traceTransaction "$TX_HASH" '{"tracer":"callTracer"}' --rpc-url http://localhost:8545 | jq '.' || true
fi

# Step 10c (on-chain LLM only): decode the StoryTold event from the applied
# verifyAndUpdate receipt and print the story the quorum signed. The story text
# was produced by transformer inference simulated off-chain by every operator.
if [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm" ] && [ -n "$TX_HASH" ]; then
    echo -e "${YELLOW}Step 10c: Decoding the quorum-signed story...${NC}"
    STORY_TOPIC=$(cast keccak "StoryTold(uint256,bytes32,string,string,uint16[])")
    LOG_DATA=$(cast receipt "$TX_HASH" --json --rpc-url http://localhost:8545 | jq -r ".logs[] | select(.topics[0] == \"$STORY_TOPIC\") | .data")
    if [ -z "$LOG_DATA" ] || [ "$LOG_DATA" = "null" ]; then
        echo -e "${RED}StoryTold event not found in the verifyAndUpdate receipt${NC}"
        exit 1
    fi
    STORY=$(cast abi-decode "x()(string,string,uint16[])" "$LOG_DATA" | sed -n 2p)
    echo -e "${GREEN}📖 On-chain LLM story (prompt: ${GK_LLM_PROMPT:-Once upon a time}):${NC}"
    echo "$STORY"
    case "$STORY" in
        *"${GK_LLM_EXPECT:-Lily}"*) echo -e "${GREEN}✅ Story matches the expected reference generation${NC}" ;;
        *) echo -e "${RED}Story does not contain expected substring '${GK_LLM_EXPECT:-Lily}'${NC}"; exit 1 ;;
    esac
fi

echo -e "${GREEN}✅ Test passed - Stack is up and the tracked transition completed successfully!${NC}"
TEST_PASSED=true
exit 0
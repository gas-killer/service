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

# Router host ports (published by docker-compose). Defaults match the compose
# file; override both when 8080/8081 are taken on the host.
ROUTER_PUBLIC_PORT="${ROUTER_PUBLIC_PORT:-8080}"
ROUTER_INTERNAL_PORT="${ROUTER_INTERNAL_PORT:-8081}"
export ROUTER_PUBLIC_PORT ROUTER_INTERNAL_PORT
# send_request posts here; keep it aligned with the published public port.
export GAS_KILLER_TRIGGER_URL="${GAS_KILLER_TRIGGER_URL:-http://localhost:${ROUTER_PUBLIC_PORT}/trigger}"

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
if [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm-sharded" ]; then
    echo -e "${YELLOW}Step 7: Deploying SHARDED on-chain LLM stack (synthetic engine-v2 model)...${NC}"
    set -a
    # shellcheck disable=SC1091
    . ./.env
    set +a
    SHARD_DEPLOY_OUT=$(bash "$PROJECT_ROOT/scripts/deploy_onchain_llm_sharded.sh")
    echo "$SHARD_DEPLOY_OUT"
    shard_val() { printf '%s\n' "$SHARD_DEPLOY_OUT" | grep "^$1=" | tail -1 | cut -d= -f2; }
    SHARD_TARGET=$(shard_val DEPLOYED_TARGET)
    SHARD_SEG_ENGINE=$(shard_val SEG_ENGINE)
    SHARD_WEIGHTS_ROOT=$(shard_val WEIGHTS_ROOT)
    SHARD_CFG0=$(shard_val PACKED_CFG_0); SHARD_CFG1=$(shard_val PACKED_CFG_1); SHARD_CFG2=$(shard_val PACKED_CFG_2)
    SHARD_N_LAYERS=$(shard_val N_LAYERS); SHARD_KVD=$(shard_val KVD); SHARD_DIM=$(shard_val DIM)
    SHARD_VOCAB=$(shard_val VOCAB); SHARD_STOP0=$(shard_val STOP0); SHARD_STOP1=$(shard_val STOP1)
    SHARD_SEQ_CAP=$(shard_val SEQ_CAP)
    if [ -z "$SHARD_TARGET" ] || [ -z "$SHARD_SEG_ENGINE" ]; then
        echo -e "${RED}sharded LLM deployment failed${NC}"
        exit 1
    fi
    echo "Discovered GasKillerChatSharded address: $SHARD_TARGET (seg engine $SHARD_SEG_ENGINE)"
    export GAS_KILLER_TARGET_ADDRESS="$SHARD_TARGET"
    export GAS_KILLER_FROM_ADDRESS=$(cast wallet address --private-key "$PRIVATE_KEY")
    export GAS_KILLER_TRANSITION_INDEX=auto
    export GK_VERIFY_MODE=transition-count
    export GK_VERIFY_TIMEOUT_SECS="${GK_VERIFY_TIMEOUT_SECS:-300}"

    # Sharding is armed at initial bring-up (compose default GK_SHARD_URL), so
    # the p2p mesh is already healthy — no node recreation, no mesh tear. The
    # gate fires on any fulfil(...) round; this consumer is the only one using it.
    echo "Sharded consumer deployed at $SHARD_TARGET (gate armed at startup)"
elif [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm" ]; then
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
    echo -e "${YELLOW}Step 7a: Asserting direct tellStory() cannot execute within a mainnet block...${NC}"
    # A full estimate binary-searches a ~1.4B-gas call and exceeds cast's client
    # timeout; the sharper, cheap assertion is that a 30M-gas-capped call OOGs.
    MAINNET_BLOCK_GAS_LIMIT=30000000
    DIRECT_GAS="> ${MAINNET_BLOCK_GAS_LIMIT}"
    if cast call "$GAS_KILLER_TARGET_ADDRESS" "tellStory(string,uint256)" \
        "${GK_LLM_PROMPT:-Once upon a time}" "${GK_LLM_MAX_TOKENS:-32}" \
        --gas-limit "$MAINNET_BLOCK_GAS_LIMIT" --rpc-url http://localhost:8545 >/dev/null 2>&1; then
        echo -e "${RED}Direct LLM execution fit in a mainnet block — expected it to OOG${NC}"
        exit 1
    fi
    echo -e "${GREEN}✅ Direct execution OOGs at the ${MAINNET_BLOCK_GAS_LIMIT}-gas block limit — unlandable on-chain, proceeding with Gas Killer${NC}"
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
if [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm" ] || [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm-sharded" ]; then
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
ROUTER_HEALTH_URL="http://localhost:${ROUTER_PUBLIC_PORT}/healthz"
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
    http://localhost:${ROUTER_PUBLIC_PORT}/admin/keys)
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

# Step 9c (sharded LLM only): drive the sharded inference through the router's
# shard coordinator — k=2 operator committees execute hash-committed segments of
# the transformer, the coordinator assembles the commit chain — then build the
# pure-commit fulfil() task that settles the answer through the normal round.
if [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm-sharded" ]; then
    echo -e "${YELLOW}Step 9c: Running SHARDED inference through the operator committees...${NC}"
    SDK_DIR_FOR_FIXTURES="${GK_SDK_DIR:-.gk-solidity-sdk}"
    VECTORS="$SDK_DIR_FOR_FIXTURES/test/fixtures/onchain-llm-v2/vectors.json"
    SHARD_PROMPT_IDS=$(jq -c '.promptIds' "$VECTORS")
    SHARD_MAX_NEW=$(jq -r '.genShort.maxNew' "$VECTORS")
    SHARD_EXPECTED_IDS=$(jq -c '.genShort.ids' "$VECTORS")

    SHARD_REQ=$(jq -n \
        --arg consumer "$SHARD_TARGET" \
        --arg seg_engine "$SHARD_SEG_ENGINE" \
        --arg weights_root "$SHARD_WEIGHTS_ROOT" \
        --arg cfg0 "$SHARD_CFG0" --arg cfg1 "$SHARD_CFG1" --arg cfg2 "$SHARD_CFG2" \
        --argjson n_layers "$SHARD_N_LAYERS" --argjson kvd "$SHARD_KVD" \
        --argjson dim "$SHARD_DIM" --argjson vocab "$SHARD_VOCAB" \
        --argjson stop0 "$SHARD_STOP0" --argjson stop1 "$SHARD_STOP1" \
        --argjson seq_cap "$SHARD_SEQ_CAP" \
        --argjson prompt_ids "$SHARD_PROMPT_IDS" --argjson max_new "$SHARD_MAX_NEW" \
        '{consumer: $consumer, seg_engine: $seg_engine, weights_root: $weights_root,
          manifest: "0x0000000000000000000000000000000000000000000000000000000000000000",
          packed_config: [$cfg0, $cfg1, $cfg2],
          n_layers: $n_layers, kvd: $kvd, dim: $dim, vocab: $vocab,
          stop0: $stop0, stop1: $stop1, seq_cap: $seq_cap,
          prompt_ids: $prompt_ids, max_new: $max_new,
          stages: 2, argmax_shards: 2}')
    echo "shard/infer request: $SHARD_REQ"
    SHARD_RESP=$(curl -sf --max-time 600 -X POST -H 'Content-Type: application/json' \
        -d "$SHARD_REQ" http://localhost:${ROUTER_INTERNAL_PORT}/shard/infer)
    if [ -z "$SHARD_RESP" ]; then
        echo -e "${RED}shard/infer failed${NC}"
        docker compose logs --tail=80 router node-1 node-2 node-3 || true
        exit 1
    fi
    echo "shard/infer response: $SHARD_RESP"
    SHARD_ANSWER_IDS=$(printf '%s' "$SHARD_RESP" | jq -c '.answer_ids')
    SHARD_ROOT=$(printf '%s' "$SHARD_RESP" | jq -r '.pipeline_root')
    SHARD_SEGMENTS=$(printf '%s' "$SHARD_RESP" | jq -r '.segments')

    if [ "$SHARD_ANSWER_IDS" != "$SHARD_EXPECTED_IDS" ]; then
        echo -e "${RED}sharded answer ids $SHARD_ANSWER_IDS != expected $SHARD_EXPECTED_IDS${NC}"
        exit 1
    fi
    echo -e "${GREEN}✅ Sharded inference BIT-EXACT: $SHARD_ANSWER_IDS over $SHARD_SEGMENTS committee segments (root $SHARD_ROOT)${NC}"

    export GAS_KILLER_CALL_DATA=$(cast calldata "fulfil(uint32[],uint256,uint32[],bytes32)" \
        "$SHARD_PROMPT_IDS" "$SHARD_MAX_NEW" "$SHARD_ANSWER_IDS" "$SHARD_ROOT")
fi

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

# Step 10d (sharded LLM only): assert the DISTRIBUTION claims — the answer was
# settled on-chain, each operator executed only its committee's share of the
# segments, every committee agreed, and every node's validator gate verified the
# commit chain before signing.
if [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm-sharded" ]; then
    echo -e "${YELLOW}Step 10d: Verifying sharded settlement + distribution claims...${NC}"
    SH_TX=$(docker compose logs router 2>/dev/null | grep "Contract execution result" | grep -o "transaction_hash=0x[a-fA-F0-9]*" | sed 's/transaction_hash=//' | tail -1)
    if [ -z "$SH_TX" ]; then
        echo -e "${RED}no verifyAndUpdate tx found in router logs${NC}"; exit 1
    fi
    CHAT_TOPIC=$(cast keccak "ChatAnswered(uint256,bytes32,bytes32,uint32[],uint32[])")
    CHAT_DATA=$(cast receipt "$SH_TX" --json --rpc-url http://localhost:8545 | jq -r ".logs[] | select(.topics[0] == \"$CHAT_TOPIC\") | .data")
    if [ -z "$CHAT_DATA" ] || [ "$CHAT_DATA" = "null" ]; then
        echo -e "${RED}ChatAnswered event not found in the verifyAndUpdate receipt${NC}"; exit 1
    fi
    ONCHAIN_ANSWER=$(cast abi-decode "x()(uint32[],uint32[])" "$CHAT_DATA" | sed -n 2p | tr -d ' ')
    EXPECT_COMPACT=$(printf '%s' "$SHARD_EXPECTED_IDS" | tr -d ' ')
    if [ "$ONCHAIN_ANSWER" != "$EXPECT_COMPACT" ]; then
        echo -e "${RED}on-chain answer $ONCHAIN_ANSWER != expected $EXPECT_COMPACT${NC}"; exit 1
    fi
    echo -e "${GREEN}📖 On-chain sharded LLM answer ids: $ONCHAIN_ANSWER (tx $SH_TX)${NC}"

    AGREED=$(docker compose logs router 2>/dev/null | grep -c "shard: committee agreed on segment" || true)
    if [ "$AGREED" -lt "$SHARD_SEGMENTS" ]; then
        echo -e "${RED}committee agreement logged for $AGREED/$SHARD_SEGMENTS segments${NC}"; exit 1
    fi
    echo -e "${GREEN}✅ Committee agreement on all $SHARD_SEGMENTS segments (k=2 replicas byte-identical)${NC}"

    TOTAL_EXEC=0
    for n in 1 2 3; do
        EXEC_N=$(docker compose logs node-$n 2>/dev/null | grep -c "shard: executed segment" || true)
        GATE_N=$(docker compose logs node-$n 2>/dev/null | grep -c "shard gate: verified commit chain" || true)
        echo "node-$n: executed $EXEC_N/$SHARD_SEGMENTS segments, gate verifications: $GATE_N"
        if [ "$EXEC_N" -le 0 ] || [ "$EXEC_N" -ge "$SHARD_SEGMENTS" ]; then
            echo -e "${RED}node-$n executed $EXEC_N segments — expected a strict SHARE (0 < share < $SHARD_SEGMENTS)${NC}"; exit 1
        fi
        if [ "$GATE_N" -lt 1 ]; then
            echo -e "${RED}node-$n never verified the commit chain before signing${NC}"; exit 1
        fi
        TOTAL_EXEC=$((TOTAL_EXEC + EXEC_N))
    done
    if [ "$TOTAL_EXEC" -ne $((SHARD_SEGMENTS * 2)) ]; then
        echo -e "${RED}total executions $TOTAL_EXEC != segments*k = $((SHARD_SEGMENTS * 2))${NC}"; exit 1
    fi
    echo -e "${GREEN}✅ Work was SHARDED: each node executed only its committee share (total $TOTAL_EXEC = $SHARD_SEGMENTS segments x k=2), and every node verified the chain before signing${NC}"
fi

echo -e "${GREEN}✅ Test passed - Stack is up and the tracked transition completed successfully!${NC}"
TEST_PASSED=true
exit 0
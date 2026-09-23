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

# GK_E2E_CONSUMER=chat-native (UNBOUNDED_V3): the router and every node run the local
# executor with the consumer's guest program installed behind the gkvm precompile —
# docker-compose.gkvm.yml layers that onto the base file for every compose call below,
# cleanup included. The chain stays a vanilla anvil.
#
# GK_E2E_NEGATIVE=1 (chat-native only) adds the negative leg: node-3 runs WITHOUT the
# guest program (docker-compose.gkvm-negative.yml). The round must still land on the
# other two operators' signatures, and node-3 must have refused at the guest-VM gate
# and signed nothing (step 10d).
if [ "${GK_E2E_CONSUMER:-array-summation}" = "chat-native" ]; then
    if [ "${GK_E2E_NEGATIVE:-0}" = "1" ]; then
        export COMPOSE_FILE="${COMPOSE_FILE:-$PROJECT_ROOT/docker-compose.yml:$PROJECT_ROOT/docker-compose.gkvm.yml:$PROJECT_ROOT/docker-compose.gkvm-negative.yml}"
    else
        export COMPOSE_FILE="${COMPOSE_FILE:-$PROJECT_ROOT/docker-compose.yml:$PROJECT_ROOT/docker-compose.gkvm.yml}"
    fi
fi

# Track if test passed
TEST_PASSED=false

# Create logs directory
mkdir -p "$LOG_DIR"

# Cleanup function
cleanup() {
    # `set -e` can fire while the script sits in scripts/ (the send_request step): compose
    # must run from the project root or it finds no containers and the dump comes out empty.
    cd "$PROJECT_ROOT" || true
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

# Step 4b (chat-native only): stage the guest program the operators install. The
# staged image must hash to the PROGRAM_HASH the consumer's binding commits to; the
# same hash is what every operator verifies the file against at startup.
if [ "${GK_E2E_CONSUMER:-array-summation}" = "chat-native" ]; then
    if [ "${GK_E2E_GUEST:-answer}" = "qwen" ]; then
        # The flagship: gas-analyzer's qwen guest over the real Qwen3-0.6B release bytes,
        # mounted into every operator as a manifest-v3 artifact (GK_GUEST_ARTIFACT[_ROOT]).
        echo -e "${YELLOW}Step 4b: Staging the qwen guest + the qwen3-0.6b-onchain-v1 weights...${NC}"
        STAGED=$(bash "$PROJECT_ROOT/scripts/stage_qwen_guest.sh" | tee /dev/stderr)
        GK_GUEST_PROGRAM_HASH=$(printf '%s\n' "$STAGED" | grep '^GUEST_PROGRAM_HASH=' | cut -d= -f2)
        GK_GUEST_ARTIFACT_ROOT=$(printf '%s\n' "$STAGED" | grep '^ARTIFACT_ROOT=' | cut -d= -f2)
        if [ -z "$GK_GUEST_PROGRAM_HASH" ] || [ -z "$GK_GUEST_ARTIFACT_ROOT" ]; then
            echo -e "${RED}qwen guest staging failed${NC}"
            exit 1
        fi
        export GK_GUEST_PROGRAM_HASH GK_GUEST_ARTIFACT_ROOT
        export GK_E2E_GUEST_ELF=qwen.elf
        export GK_GUEST_ARTIFACT=/app/guest/weights.bin:/app/guest/tokenizer.bin
    else
        echo -e "${YELLOW}Step 4b: Staging the chat-native guest program...${NC}"
        GK_GUEST_PROGRAM_HASH=$(bash "$PROJECT_ROOT/scripts/stage_guest_program.sh" | tee /dev/stderr | grep '^GUEST_PROGRAM_HASH=' | cut -d= -f2)
        if [ -z "$GK_GUEST_PROGRAM_HASH" ]; then
            echo -e "${RED}guest program staging failed${NC}"
            exit 1
        fi
        export GK_GUEST_PROGRAM_HASH
    fi
fi

# Step 5: Start Docker Compose services
echo -e "${YELLOW}Step 5: Starting Docker Compose services...${NC}"
if [ "${GK_E2E_CONSUMER:-array-summation}" = "chat-native" ]; then
    # Chain + AVS setup only. The operators start in step 7c, once the consumer exists
    # and can be entered in their registry (GK_GUEST_VM_CONSUMERS is read at startup).
    docker compose up -d ethereum eigenlayer
else
    docker compose up -d
fi

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

# Give extra time for nodes to initialize (chat-native starts them in step 7c)
if [ "${GK_E2E_CONSUMER:-array-summation}" != "chat-native" ]; then
    echo "Waiting for nodes to initialize..."
    sleep 30
fi

# Step 7: Deploy the Gas Killer consumer under test (GK_E2E_CONSUMER:
# array-summation [default], onchain-llm — the solidity-sdk LLM example, or
# chat-native — the same chat consumer with the engine replaced by a guest program).
if [ "${GK_E2E_CONSUMER:-array-summation}" = "chat-native" ]; then
    if [ "${GK_E2E_GUEST:-answer}" = "qwen" ]; then
        echo -e "${YELLOW}Step 7: Deploying Gas Killer native chat consumer (guest: qwen, Qwen3-0.6B)...${NC}"
        export GK_CHAT_CONTRACT=GasKillerChatQwen
        export GK_ARTIFACT_ROOT="$GK_GUEST_ARTIFACT_ROOT"
        # Qwen3Engine's packedConfig for qwen3-0.6b-onchain-v1 (gas-analyzer scripts/flagship/run.sh)
        export GK_PACKED_CONFIG="${GK_PACKED_CONFIG:-0x04000c001c100800800002518004000101000000000000000000000000000000,0x0000000010c6f7a10000000016a09e6600000000239791f10000000000000000,0x00182bc20002505d0002505b0000000000000000000000000000000000000000}"
        # the chat-templated "What is Ethereum?" and the answer the guest gives it
        export GK_CHAT_PROMPT_IDS="${GK_CHAT_PROMPT_IDS:-[151644,872,198,3838,374,33946,30,151645,198,151644,77091,198,151667,271,151668,271]}"
        export GK_CHAT_MAX_TOKENS="${GK_CHAT_MAX_TOKENS:-8}"
        export GK_CHAT_EXPECT="${GK_CHAT_EXPECT:-Ethereum is a decentralized blockchain platform}"
    else
        echo -e "${YELLOW}Step 7: Deploying Gas Killer native chat consumer (guest: answer.py)...${NC}"
    fi
    # Load harness config for this branch (the Rust helpers read .env themselves)
    set -a
    # shellcheck disable=SC1091
    . ./.env
    set +a
    export AVS_DEPLOYMENT_PATH="$PROJECT_ROOT/config/.nodes/avs_deploy.json"
    CHAT_ADDRESS=$(bash "$PROJECT_ROOT/scripts/deploy_chat_native.sh" | tee /dev/stderr | grep '^CHAT_NATIVE_TARGET=' | cut -d= -f2)
    if [ -z "$CHAT_ADDRESS" ]; then
        echo -e "${RED}native chat consumer deployment failed${NC}"
        exit 1
    fi
    echo "Discovered ${GK_CHAT_CONTRACT:-GasKillerChatNative} address: $CHAT_ADDRESS"
    export GAS_KILLER_TARGET_ADDRESS="$CHAT_ADDRESS"
    # Default task = the sdk's "doc-vector" (test/fixtures/gkvm/native_tasks.json).
    export GAS_KILLER_CALL_DATA=$(cast calldata "ask(uint256[],uint256)" "${GK_CHAT_PROMPT_IDS:-[9707,11,151644]}" "${GK_CHAT_MAX_TOKENS:-6}")
    export GAS_KILLER_FROM_ADDRESS=$(cast wallet address --private-key "$PRIVATE_KEY")
    export GAS_KILLER_TRANSITION_INDEX=auto
    export GK_VERIFY_MODE=transition-count
    export GK_VERIFY_TIMEOUT_SECS="${GK_VERIFY_TIMEOUT_SECS:-300}"
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
if [ "${GK_E2E_CONSUMER:-array-summation}" = "chat-native" ]; then
    # Native consumers are unlandable for a different reason than gas: the gkvm
    # precompile exists only inside the operators' simulation environment. On the chain —
    # this vanilla anvil, like every real chain — the guest call finds an empty account
    # and the consumer reverts GkVmUnavailable, at any gas limit.
    echo -e "${YELLOW}Step 7a: Asserting direct ask() reverts GkVmUnavailable on the chain...${NC}"
    UNAVAILABLE_SELECTOR=$(cast sig "GkVmUnavailable()")
    if DIRECT_OUT=$(cast call "$GAS_KILLER_TARGET_ADDRESS" "ask(uint256[],uint256)(string)" \
        "${GK_CHAT_PROMPT_IDS:-[9707,11,151644]}" "${GK_CHAT_MAX_TOKENS:-6}" \
        --from "$GAS_KILLER_FROM_ADDRESS" --rpc-url http://localhost:8545 2>&1); then
        echo -e "${RED}Direct ask() executed on the chain — expected GkVmUnavailable: $DIRECT_OUT${NC}"
        exit 1
    fi
    case "$DIRECT_OUT" in
        *GkVmUnavailable*|*"${UNAVAILABLE_SELECTOR#0x}"*) ;;
        *)
            echo -e "${RED}Direct ask() failed, but not with GkVmUnavailable ($UNAVAILABLE_SELECTOR): $DIRECT_OUT${NC}"
            exit 1
            ;;
    esac
    echo -e "${GREEN}✅ Direct execution reverts GkVmUnavailable — the guest runs only off-chain, proceeding with Gas Killer${NC}"

    # Step 7c: start the operators with the consumer registered as a guest-VM consumer
    # (requiresGuestVm): each analyzes its tasks only under the local executor with
    # exactly this program installed, and abstains otherwise.
    echo -e "${YELLOW}Step 7c: Starting router and nodes (guest VM installed, consumer registered)...${NC}"
    export GK_GUEST_VM_CONSUMERS="${GAS_KILLER_TARGET_ADDRESS}=${GK_GUEST_PROGRAM_HASH}"
    # --no-deps: the operators depend on the one-shot `eigenlayer` setup container, which
    # has already run and exited. A plain `up -d` starts it AGAIN, re-running the AVS
    # setup against the live chain and moving the quorum state out from under the
    # consumer deployed in step 7 — the settled round then reverts InvalidQuorumApkHash.
    docker compose up -d --no-deps signer node-1 node-2 node-3 router
    docker compose ps
    echo "Waiting for nodes to initialize..."
    sleep 30
elif [ "${GK_SIM_PROFILE:-chain}" = "unbounded-v1" ] && [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm" ]; then
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
if [ "${GK_E2E_CONSUMER:-array-summation}" = "onchain-llm" ] || [ "${GK_E2E_CONSUMER:-array-summation}" = "chat-native" ]; then
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
    # The router logs the result only after it has the receipt, which can trail the state
    # change this script just observed by a few polls — wait for the line, don't race it.
    VU_TX_HASH=""
    for _ in $(seq 1 30); do
        VU_TX_HASH=$(docker compose logs router 2>/dev/null | grep "Contract execution result" | grep -o "transaction_hash=0x[a-fA-F0-9]*" | sed 's/transaction_hash=//' | tail -1)
        [ -n "$VU_TX_HASH" ] && break
        sleep 2
    done
    if [ -z "$VU_TX_HASH" ]; then
        echo -e "${RED}Could not find the verifyAndUpdate transaction hash in router logs${NC}"
        exit 1
    fi
    VU_GAS=$(cast receipt "$VU_TX_HASH" gasUsed --rpc-url http://localhost:8545)
    VU_GAS=$((VU_GAS))  # normalize possible hex to decimal
    if [ "${GK_E2E_CONSUMER:-array-summation}" = "chat-native" ]; then
        # No direct-execution gas figure exists to compare against: the guest cannot
        # run on the chain at all (step 7a).
        echo "verifyAndUpdate used $VU_GAS gas; direct execution reverts GkVmUnavailable"
    else
        echo "verifyAndUpdate used $VU_GAS gas vs $DIRECT_GAS gas for direct execution"
    fi
    if [ "$VU_GAS" -ge "$MAINNET_BLOCK_GAS_LIMIT" ]; then
        echo -e "${RED}verifyAndUpdate unexpectedly used a full block's gas${NC}"
        exit 1
    fi
    if [ "${GK_E2E_CONSUMER:-array-summation}" = "chat-native" ]; then
        echo -e "${GREEN}✅ Native transition applied on-chain: ${VU_GAS} gas — off-chain guest compute, one small on-chain tx${NC}"
    else
        RATIO=$((DIRECT_GAS / VU_GAS))
        echo -e "${GREEN}✅ Unbounded transition applied on-chain: ${VU_GAS} gas (direct execution: ${DIRECT_GAS} gas, ~${RATIO}x more) — above-block-limit compute, one small on-chain tx${NC}"
    fi
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

# Step 10c (chat-native only): decode the ChatAnswered event from the applied
# verifyAndUpdate receipt and print the answer the quorum signed. The text was
# computed by the guest program inside every operator's simulation environment;
# the chain never ran it. The default expectation is the doc-vector's answer
# (gk-run on the staged image reproduces it).
if [ "${GK_E2E_CONSUMER:-array-summation}" = "chat-native" ] && [ -n "$TX_HASH" ]; then
    echo -e "${YELLOW}Step 10c: Decoding the quorum-signed answer...${NC}"
    CHAT_TOPIC=$(cast keccak "ChatAnswered(uint256,bytes32,uint256[],string,uint256[])")
    LOG_DATA=$(cast receipt "$TX_HASH" --json --rpc-url http://localhost:8545 | jq -r ".logs[] | select(.topics[0] == \"$CHAT_TOPIC\") | .data")
    if [ -z "$LOG_DATA" ] || [ "$LOG_DATA" = "null" ]; then
        echo -e "${RED}ChatAnswered event not found in the verifyAndUpdate receipt${NC}"
        exit 1
    fi
    ANSWER=$(cast abi-decode "x()(uint256[],string,uint256[])" "$LOG_DATA" | sed -n 2p)
    echo -e "${GREEN}💬 Native chat answer (prompt ids: ${GK_CHAT_PROMPT_IDS:-[9707,11,151644]}):${NC}"
    echo "$ANSWER"
    case "$ANSWER" in
        *"${GK_CHAT_EXPECT:-because native so operator stay native}"*) echo -e "${GREEN}✅ Answer matches the guest's reference generation${NC}" ;;
        *) echo -e "${RED}Answer does not contain expected substring '${GK_CHAT_EXPECT:-because native so operator stay native}'${NC}"; exit 1 ;;
    esac
fi

# Step 10d (chat-native negative leg only): the transition above landed WITHOUT node-3.
# An operator without the guest program abstains — it refuses the task at the guest-VM
# gate and never signs; it must not have produced a signature over anything else
# either (the GkVmUnavailable revert transition is what a guest-less analysis yields).
# The in-process version of this check, with the signatures themselves verified, is
# node/tests/gkvm_abstain_quorum.rs.
if [ "${GK_E2E_CONSUMER:-array-summation}" = "chat-native" ] && [ "${GK_E2E_NEGATIVE:-0}" = "1" ]; then
    echo -e "${YELLOW}Step 10d: Asserting node-3 (no guest program) abstained...${NC}"
    NODE3_LOGS=$(docker compose logs node-3 2>/dev/null)
    case "$NODE3_LOGS" in
        *"guest-VM gate"*) ;;
        *)
            echo -e "${RED}node-3 never refused the task at the guest-VM gate${NC}"
            exit 1
            ;;
    esac
    case "$NODE3_LOGS" in
        *"Generating signature for round"*|*"Sending signature for round"*)
            echo -e "${RED}node-3 signed a round without the guest program installed${NC}"
            exit 1
            ;;
    esac
    for signer_node in node-1 node-2; do
        if ! docker compose logs "$signer_node" 2>/dev/null | grep -q "Sending signature for round"; then
            echo -e "${RED}$signer_node did not sign — the quorum was not the two operators with the program${NC}"
            exit 1
        fi
    done
    echo -e "${GREEN}✅ node-3 abstained; node-1 and node-2 carried the round${NC}"
fi

echo -e "${GREEN}✅ Test passed - Stack is up and the tracked transition completed successfully!${NC}"
TEST_PASSED=true
exit 0
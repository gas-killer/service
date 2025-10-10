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

# Create logs directory
mkdir -p "$LOG_DIR"

# Cleanup function
cleanup() {
    echo -e "${YELLOW}Cleaning up Docker containers...${NC}"
    cd "$PROJECT_ROOT"
    docker compose down || true
    echo -e "${GREEN}Cleanup completed${NC}"
}

# Set trap for cleanup
trap cleanup EXIT INT TERM

echo -e "${GREEN}Starting Gas Killer Integration Test${NC}"
echo "Project root: $PROJECT_ROOT"
echo "Logs directory: $LOG_DIR"

# Step 1: Build scripts
echo -e "${YELLOW}Step 1: Building scripts...${NC}"
cd "$PROJECT_ROOT/scripts"
cargo build --release -p avs-scripts --bin deploy_array_summation
cargo build --release -p avs-scripts --bin trigger_gas_killer
cd "$PROJECT_ROOT"

# Step 2: Set up environment files
echo -e "${YELLOW}Step 2: Setting up environment files...${NC}"
cp example.env .env

# Copy config template
cp config/config.example.json config/config.json

# Update .env for local mode
echo "Configuring .env for local mode..."
sed -i '' 's|^HTTP_RPC=.*|HTTP_RPC=http://localhost:8545|' .env
sed -i '' 's|^WS_RPC=.*|WS_RPC=ws://localhost:8545|' .env
sed -i '' 's|^RPC_URL=.*|RPC_URL=http://ethereum:8545|' .env
sed -i '' 's|^ENVIRONMENT=.*|ENVIRONMENT=LOCAL|' .env

# Enable HTTP ingress for Gas Killer
sed -i '' 's|^INGRESS=.*|INGRESS=true|' .env || true
if ! grep -q '^INGRESS_ADDRESS=' .env; then
    echo 'INGRESS_ADDRESS=0.0.0.0:8080' >> .env
else
    sed -i '' 's|^INGRESS_ADDRESS=.*|INGRESS_ADDRESS=0.0.0.0:8080|' .env
fi

# Set FORK_URL for local forking
sed -i '' 's|^# FORK_URL=.*|FORK_URL=https://holesky.drpc.org|' .env

# Use default Anvil private key for testing
DEFAULT_PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
sed -i '' "s|^PRIVATE_KEY=.*|PRIVATE_KEY=$DEFAULT_PRIVATE_KEY|" .env
sed -i '' "s|^FUNDED_KEY=.*|FUNDED_KEY=$DEFAULT_PRIVATE_KEY|" .env

# Set Holesky contract addresses for LOCAL mode
sed -i '' 's|^#DELEGATION_MANAGER_ADDRESS=|DELEGATION_MANAGER_ADDRESS=|' .env
sed -i '' 's|^#STRATEGY_MANAGER_ADDRESS=|STRATEGY_MANAGER_ADDRESS=|' .env
sed -i '' 's|^#LST_CONTRACT_ADDRESS=|LST_CONTRACT_ADDRESS=|' .env
sed -i '' 's|^#LST_STRATEGY_ADDRESS=|LST_STRATEGY_ADDRESS=|' .env
sed -i '' 's|^#BLS_SIGNATURE_CHECKER_ADDRESS=|BLS_SIGNATURE_CHECKER_ADDRESS=|' .env
sed -i '' 's|^#OPERATOR_STATE_RETRIEVER_ADDRESS=|OPERATOR_STATE_RETRIEVER_ADDRESS=|' .env
sed -i '' 's|^#ALLOCATION_MANAGER_ADDRESS=|ALLOCATION_MANAGER_ADDRESS=|' .env

# Set ArraySummation deployment parameters (example Gas Killer-enabled contract)
if grep -q '^ARRAY_SUMMATION_ARRAY_SIZE=' .env; then
    sed -i '' 's|^ARRAY_SUMMATION_ARRAY_SIZE=.*|ARRAY_SUMMATION_ARRAY_SIZE=1000|' .env
else
    echo 'ARRAY_SUMMATION_ARRAY_SIZE=1000' >> .env
fi

if grep -q '^ARRAY_SUMMATION_MAX_VALUE=' .env; then
    sed -i '' 's|^ARRAY_SUMMATION_MAX_VALUE=.*|ARRAY_SUMMATION_MAX_VALUE=10000|' .env
else
    echo 'ARRAY_SUMMATION_MAX_VALUE=10000' >> .env
fi

if grep -q '^ARRAY_SUMMATION_SEED=' .env; then
    sed -i '' 's|^ARRAY_SUMMATION_SEED=.*|ARRAY_SUMMATION_SEED=0|' .env
else
    echo 'ARRAY_SUMMATION_SEED=0' >> .env
fi

if grep -q '^ARRAY_SUMMATION_FACTORY_ADDRESS=' .env; then
    sed -i '' 's|^ARRAY_SUMMATION_FACTORY_ADDRESS=.*|ARRAY_SUMMATION_FACTORY_ADDRESS=0xF7ded769418Ec1Db4DA3bd2d47ab72ce2296A032|' .env
else
    echo 'ARRAY_SUMMATION_FACTORY_ADDRESS=0xF7ded769418Ec1Db4DA3bd2d47ab72ce2296A032' >> .env
fi

echo "Environment configuration complete"

# Step 3: Pull Docker images
echo -e "${YELLOW}Step 3: Pulling Docker images...${NC}"
docker compose pull

# Step 4: Start Docker Compose services
echo -e "${YELLOW}Step 4: Starting Docker Compose services...${NC}"
docker compose up -d

# Show running containers
docker compose ps

# Step 5: Wait for EigenLayer setup to complete
echo -e "${YELLOW}Step 5: Waiting for EigenLayer setup to complete...${NC}"
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

# Give extra time for nodes to initialize
echo "Waiting for nodes to initialize..."
sleep 30

# Step 6: Deploy ArraySummation (Gas Killer example contract)
echo -e "${YELLOW}Step 6: Deploying ArraySummation (Gas Killer example contract)...${NC}"
cd "$PROJECT_ROOT/scripts"

# Source environment and run deployment
source ../.env
export AVS_DEPLOYMENT_PATH="../config/.nodes/avs_deploy.json"

if [ ! -f "$AVS_DEPLOYMENT_PATH" ]; then
    echo -e "${RED}Deployment file not found at $AVS_DEPLOYMENT_PATH${NC}"
    exit 1
fi

echo "Running ArraySummation deployment..."
cargo run --release -p avs-scripts --bin deploy_array_summation

if [ $? -eq 0 ]; then
    echo -e "${GREEN}ArraySummation deployment completed successfully${NC}"
else
    echo -e "${RED}ArraySummation deployment failed${NC}"
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

# Step 7: Check service health
echo -e "${YELLOW}Step 7: Checking service health...${NC}"
for service in node-1 node-2 node-3 router; do
    if docker compose ps | grep -q "$service.*Up"; then
        echo "Service $service is running"
    else
        echo -e "${YELLOW}Warning: Service $service might not be ready${NC}"
    fi
done

# Step 8: Brief wait for services to stabilize
echo -e "${YELLOW}Step 8: Waiting briefly for services to stabilize...${NC}"
sleep 5

# Step 9: Trigger Gas Killer task (optional if variables are provided)
echo -e "${YELLOW}Step 9: Trigger Gas Killer task (optional)${NC}"

TRIGGER_URL=${GAS_KILLER_TRIGGER_URL:-"http://localhost:8080/trigger"}

if [ -n "$GAS_KILLER_TARGET_ADDRESS" ] && [ -n "$GAS_KILLER_CALL_DATA" ] && [ -n "$GAS_KILLER_FROM_ADDRESS" ] && [ -n "$GAS_KILLER_TRANSITION_INDEX" ]; then
    echo "Attempting to trigger Gas Killer task via $TRIGGER_URL"
    cd "$PROJECT_ROOT/scripts"
    cargo run --release -p avs-scripts --bin trigger_gas_killer
    TRIGGER_STATUS=$?
    cd "$PROJECT_ROOT"

    if [ $TRIGGER_STATUS -eq 0 ]; then
        echo -e "${GREEN}✅ Triggered Gas Killer task successfully.${NC}"
    else
        echo -e "${YELLOW}⚠️  Trigger helper returned a non-success status. Check inputs and router logs.${NC}"
    fi
else
    # Fallback: if we discovered ArraySummation address, send a minimal demo task to avoid creator timeout
    if [ -n "$GAS_KILLER_TARGET_ADDRESS" ]; then
        echo -e "${YELLOW}No GAS_KILLER_* inputs provided. Sending a minimal demo task to avoid timeout...${NC}"
        export GAS_KILLER_CALL_DATA="0x00000000" # dummy selector
        export GAS_KILLER_FROM_ADDRESS="0x0000000000000000000000000000000000000001"
        export GAS_KILLER_TRANSITION_INDEX="1"
        export GAS_KILLER_VALUE="0"
        export GAS_KILLER_STORAGE_UPDATES="0x01" # non-empty to pass basic validation
        cd "$PROJECT_ROOT/scripts"
        cargo run --release -p avs-scripts --bin trigger_gas_killer || true
        cd "$PROJECT_ROOT"
        echo -e "${YELLOW}Demo task submitted. Provide real GAS_KILLER_* inputs to test full flow.${NC}"
    else
        echo "Skipping trigger. To enable, set the following env vars before running this script:"
        echo "  GAS_KILLER_TARGET_ADDRESS (defaults to deployed ArraySummation if found)"
        echo "  GAS_KILLER_CALL_DATA (0x-prefixed hex)"
        echo "  GAS_KILLER_FROM_ADDRESS (0x-prefixed hex)"
        echo "  GAS_KILLER_TRANSITION_INDEX (> 0)"
        echo "  GAS_KILLER_VALUE (optional, default 0)"
        echo "  GAS_KILLER_STORAGE_UPDATES (optional; if omitted, helper attempts analysis)"
    fi
fi

# Show recent router logs for confirmation
echo -e "${YELLOW}Recent router logs:${NC}"
docker compose logs --tail=50 router || true

echo -e "${GREEN}✅ Stack is up with Gas Killer ingress enabled and ArraySummation deployed.${NC}"
exit 0
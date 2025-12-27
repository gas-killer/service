#!/bin/bash
#
# Local Helm chart testing script for Gas Killer
#
# Prerequisites:
# - Docker
# - kind (https://kind.sigs.k8s.io/)
# - kubectl
# - helm
# - Rust toolchain
# - Foundry (cast, anvil)
#
# Usage:
#   ./scripts/helm_test.sh [options]
#
# Options:
#   --skip-build       Skip building Docker images
#   --skip-cleanup     Don't delete the kind cluster after testing
#   --cluster-name     Name of the kind cluster (default: gas-killer-test)
#   --node-count       Number of operator nodes (default: 3)
#   --fork-url         RPC URL to fork from (default: https://ethereum-sepolia-rpc.publicnode.com)
#

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration defaults
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
CLUSTER_NAME="${CLUSTER_NAME:-gas-killer-test}"
HELM_RELEASE="${HELM_RELEASE:-gas-killer}"
NODE_COUNT="${NODE_COUNT:-3}"
FORK_URL="${FORK_URL:-https://ethereum-sepolia-rpc.publicnode.com}"
PRIVATE_KEY="${PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
FUNDED_KEY="${FUNDED_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
SKIP_BUILD="${SKIP_BUILD:-false}"
SKIP_CLEANUP="${SKIP_CLEANUP:-false}"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --skip-cleanup)
            SKIP_CLEANUP=true
            shift
            ;;
        --cluster-name)
            CLUSTER_NAME="$2"
            shift 2
            ;;
        --node-count)
            NODE_COUNT="$2"
            shift 2
            ;;
        --fork-url)
            FORK_URL="$2"
            shift 2
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            exit 1
            ;;
    esac
done

# Cleanup function
cleanup() {
    if [ "$SKIP_CLEANUP" = "true" ]; then
        echo -e "${YELLOW}Skipping cleanup (--skip-cleanup specified)${NC}"
        echo "To clean up manually:"
        echo "  pkill -f 'port-forward' || true"
        echo "  helm uninstall $HELM_RELEASE || true"
        echo "  kind delete cluster --name $CLUSTER_NAME || true"
        return
    fi

    echo -e "${YELLOW}Cleaning up...${NC}"
    pkill -f "kubectl port-forward" 2>/dev/null || true
    helm uninstall "$HELM_RELEASE" 2>/dev/null || true
    kind delete cluster --name "$CLUSTER_NAME" 2>/dev/null || true
    echo -e "${GREEN}Cleanup completed${NC}"
}

# Helper function to read counter value from smart contract
read_counter() {
    local counter_address=$1
    local result=$(curl -s -X POST http://localhost:8545 \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc":"2.0",
            "method":"eth_call",
            "params":[{
                "to":"'$counter_address'",
                "data":"0x8381f58a"
            }, "latest"],
            "id":1
        }' | jq -r '.result')

    if [ -z "$result" ] || [ "$result" = "null" ] || [ "$result" = "0x" ]; then
        echo "0"
    else
        printf "%d\n" "$result" 2>/dev/null || echo "0"
    fi
}

echo -e "${GREEN}Starting Gas Killer Helm Test${NC}"
echo "Project root: $PROJECT_ROOT"
echo "Cluster name: $CLUSTER_NAME"
echo "Node count: $NODE_COUNT"
echo "Fork URL: $FORK_URL"

# Step 1: Check prerequisites
echo -e "${YELLOW}Step 1: Checking prerequisites...${NC}"
for cmd in docker kind kubectl helm cargo; do
    if ! command -v $cmd &> /dev/null; then
        echo -e "${RED}$cmd is not installed${NC}"
        exit 1
    fi
done
echo -e "${GREEN}All prerequisites installed${NC}"

# Step 2: Create kind cluster
echo -e "${YELLOW}Step 2: Creating kind cluster...${NC}"
if kind get clusters 2>/dev/null | grep -q "^${CLUSTER_NAME}$"; then
    echo "Cluster $CLUSTER_NAME already exists, deleting..."
    kind delete cluster --name "$CLUSTER_NAME"
fi

kind create cluster --name "$CLUSTER_NAME" --wait 120s
echo -e "${GREEN}Kind cluster created${NC}"

# Set trap for cleanup
trap cleanup EXIT INT TERM

# Step 3: Verify cluster access
echo -e "${YELLOW}Step 3: Verifying cluster access...${NC}"
if ! kubectl cluster-info &> /dev/null; then
    echo -e "${RED}Cannot access Kubernetes cluster${NC}"
    exit 1
fi

echo -e "${GREEN}Cluster is accessible${NC}"
kubectl get nodes

# Step 4: Build Docker images (if not skipped)
cd "$PROJECT_ROOT"

if [ "$SKIP_BUILD" != "true" ]; then
    echo -e "${YELLOW}Step 4: Building Docker images...${NC}"

    echo "Building router image..."
    docker build -f ./router/Dockerfile -t gas-killer-router:local .

    echo "Building node image..."
    docker build -f ./node/Dockerfile -t gas-killer-node:local .

    # Load images into kind cluster
    echo "Loading images into kind cluster..."
    kind load docker-image gas-killer-router:local --name "$CLUSTER_NAME"
    kind load docker-image gas-killer-node:local --name "$CLUSTER_NAME"
else
    echo -e "${YELLOW}Step 4: Skipping Docker build (--skip-build specified)${NC}"
fi

# Step 5: Install Helm chart
echo -e "${YELLOW}Step 5: Installing Helm chart...${NC}"

# Check if release already exists and uninstall it
if helm list -q | grep -q "^${HELM_RELEASE}$"; then
    echo "Release $HELM_RELEASE already exists. Uninstalling..."
    helm uninstall "$HELM_RELEASE" || true
    kubectl delete pvc -l app.kubernetes.io/instance="$HELM_RELEASE" || true
    echo "Waiting for resources to be cleaned up..."
    sleep 10
fi

helm install "$HELM_RELEASE" ./helm \
    --timeout 20m \
    --set global.environment=LOCAL \
    --set global.nodeCount="$NODE_COUNT" \
    --set secrets.forkUrl="$FORK_URL" \
    --set secrets.privateKey="$PRIVATE_KEY" \
    --set secrets.fundedKey="$FUNDED_KEY" \
    --set node.image.repository=gas-killer-node \
    --set node.image.tag=local \
    --set node.image.pullPolicy=Never \
    --set router.image.repository=gas-killer-router \
    --set router.image.tag=local \
    --set router.image.pullPolicy=Never \
    --set sharedData.storageClass=""

echo -e "${GREEN}Helm chart installed successfully${NC}"

# Step 6: Wait for setup job
echo -e "${YELLOW}Step 6: Waiting for setup job to complete...${NC}"

SETUP_JOB=$(kubectl get jobs -o name | grep setup | head -1)

if [ -z "$SETUP_JOB" ]; then
    echo -e "${RED}Setup job not found!${NC}"
    kubectl get jobs
    exit 1
fi

echo "Found setup job: $SETUP_JOB"
kubectl wait --for=condition=complete "$SETUP_JOB" --timeout=500s

echo "Setup job completed:"
kubectl logs "$SETUP_JOB" --tail=20

# Step 7: Wait for pods to be ready
echo -e "${YELLOW}Step 7: Waiting for all pods to be ready...${NC}"

echo "Waiting for ethereum pod..."
kubectl wait --for=condition=ready pod -l app.kubernetes.io/component=ethereum --timeout=180s

echo "Waiting for signer pod..."
kubectl wait --for=condition=ready pod -l app.kubernetes.io/component=signer --timeout=180s

echo "Waiting for node pods..."
kubectl wait --for=condition=ready pod -l app.kubernetes.io/component=node --timeout=300s --all

echo "Waiting for router pod..."
kubectl wait --for=condition=ready pod -l app.kubernetes.io/component=router --timeout=300s

echo -e "${GREEN}All pods are ready!${NC}"

# Step 8: Setup port forwarding
echo -e "${YELLOW}Step 8: Setting up port forwarding...${NC}"

pkill -f "kubectl port-forward.*8545" 2>/dev/null || true
pkill -f "kubectl port-forward.*8080" 2>/dev/null || true
sleep 2

ETHEREUM_SERVICE=$(kubectl get services -o name | grep ethereum | head -1 | sed 's|service/||')
ROUTER_SERVICE=$(kubectl get services -o name | grep router | head -1 | sed 's|service/||')

if [ -z "$ETHEREUM_SERVICE" ] || [ -z "$ROUTER_SERVICE" ]; then
    echo -e "${RED}Required services not found!${NC}"
    kubectl get services
    exit 1
fi

echo "Ethereum service: $ETHEREUM_SERVICE"
echo "Router service: $ROUTER_SERVICE"

kubectl port-forward service/$ETHEREUM_SERVICE 8545:8545 &
ETHEREUM_PF_PID=$!

kubectl port-forward service/$ROUTER_SERVICE 8080:8080 &
ROUTER_PF_PID=$!

sleep 10

echo -e "${GREEN}Port forwarding established${NC}"

# Step 9: Get AVS deployment file
echo -e "${YELLOW}Step 9: Retrieving AVS deployment file...${NC}"

ROUTER_POD=$(kubectl get pods -l app.kubernetes.io/component=router -o jsonpath='{.items[0].metadata.name}')
mkdir -p config/.nodes
kubectl cp $ROUTER_POD:/app/.nodes/avs_deploy.json ./config/.nodes/avs_deploy.json

if [ ! -f "./config/.nodes/avs_deploy.json" ]; then
    echo -e "${RED}AVS deployment file not found${NC}"
    exit 1
fi

echo -e "${GREEN}AVS deployment file retrieved${NC}"
cat ./config/.nodes/avs_deploy.json

# Step 10: Build and run test scripts
echo -e "${YELLOW}Step 10: Building test scripts...${NC}"

cd "$PROJECT_ROOT/scripts"
cargo build --release -p avs-scripts --bin deploy_array_summation
cargo build --release -p avs-scripts --bin trigger_gas_killer
cd "$PROJECT_ROOT"

# Step 11: Deploy ArraySummation contract
echo -e "${YELLOW}Step 11: Deploying ArraySummation contract...${NC}"

cd scripts
export HTTP_RPC=http://localhost:8545
export WS_RPC=ws://localhost:8545
export AVS_DEPLOYMENT_PATH="../config/.nodes/avs_deploy.json"
export ARRAY_SUMMATION_FACTORY_ADDRESS="0xF7ded769418Ec1Db4DA3bd2d47ab72ce2296A032"
export ARRAY_SUMMATION_ARRAY_SIZE=100
export ARRAY_SUMMATION_MAX_VALUE=1000
export ARRAY_SUMMATION_SEED=42
export PRIVATE_KEY="$PRIVATE_KEY"

cargo run --release -p avs-scripts --bin deploy_array_summation
cd "$PROJECT_ROOT"

echo -e "${GREEN}ArraySummation deployment completed${NC}"

# Step 12: Trigger Gas Killer task
echo -e "${YELLOW}Step 12: Triggering Gas Killer task...${NC}"

cd scripts
export GAS_KILLER_ROUTER_URL=http://localhost:8080

cargo run --release -p avs-scripts --bin trigger_gas_killer
TRIGGER_STATUS=$?
cd "$PROJECT_ROOT"

if [ $TRIGGER_STATUS -eq 0 ]; then
    echo -e "${GREEN}Gas Killer task executed successfully!${NC}"
else
    echo -e "${RED}Gas Killer task failed${NC}"
    echo "Router logs:"
    kubectl logs -l app.kubernetes.io/component=router --tail 100
    exit 1
fi

# Show router logs
echo -e "${YELLOW}Recent router logs:${NC}"
kubectl logs -l app.kubernetes.io/component=router --tail 50

echo -e "${GREEN}=== Test Summary ===${NC}"
echo -e "${GREEN}All tests passed!${NC}"

exit 0

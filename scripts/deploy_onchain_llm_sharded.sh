#!/bin/bash
#
# Deploy the SHARDED on-chain LLM e2e stack (gas-killer/solidity-sdk
# `onchain-llm` example, synthetic engine-v2 fixture model): weight/tokenizer
# chunks + directory, Qwen3Engine, Qwen3SegEngine, and the GasKillerChatSharded
# settlement consumer, wired to the harness AVS and signature checker. Prints
# grep-friendly KEY=value lines (DEPLOYED_TARGET, SEG_ENGINE, WEIGHTS_ROOT,
# PACKED_CFG_*, N_LAYERS, KVD, DIM, VOCAB, STOP0, STOP1, SEQ_CAP) the shard
# coordinator request is built from.
#
# Env:
#   HTTP_RPC              chain RPC        (default http://localhost:8545)
#   PRIVATE_KEY           funded deployer  (required)
#   AVS_DEPLOYMENT_PATH   avs deploy json  (required)
#   GK_SDK_REPO / GK_SDK_REF   consumer source (default: the onchain-llm branch)
#   GK_SDK_DIR            checkout cache dir (default .gk-solidity-sdk)
set -euo pipefail

if [ -z "${PRIVATE_KEY:-}" ] && [ -f .env ]; then
    set -a
    # shellcheck disable=SC1091
    . ./.env
    set +a
fi

HTTP_RPC="${HTTP_RPC:-http://localhost:8545}"
SDK_REPO="${GK_SDK_REPO:-https://github.com/gas-killer/solidity-sdk}"
SDK_REF="${GK_SDK_REF:-RonTuretzky/onchain-solidity-llm}"
SDK_DIR="${GK_SDK_DIR:-.gk-solidity-sdk}"

: "${PRIVATE_KEY:?PRIVATE_KEY is required}"
: "${AVS_DEPLOYMENT_PATH:?AVS_DEPLOYMENT_PATH is required}"

AVS_ADDRESS=$(jq -r '.addresses.avsServiceManagerWrapper' "$AVS_DEPLOYMENT_PATH")
SIG_CHECKER_ADDRESS=$(jq -r '.addresses.IncredibleSquaringTaskManager' "$AVS_DEPLOYMENT_PATH")
if [ -z "$AVS_ADDRESS" ] || [ "$AVS_ADDRESS" = "null" ]; then
    echo "could not read avsServiceManagerWrapper from $AVS_DEPLOYMENT_PATH" >&2
    exit 1
fi
echo "AVS: $AVS_ADDRESS  checker: $SIG_CHECKER_ADDRESS" >&2

if [ ! -e "$SDK_DIR/.git" ]; then
    git clone --depth 1 -b "$SDK_REF" "$SDK_REPO" "$SDK_DIR" >&2
    git -C "$SDK_DIR" submodule update --init --recursive --depth 1 >&2
fi

OUT=$(cd "$SDK_DIR" && AVS_ADDRESS="$AVS_ADDRESS" SIG_CHECKER_ADDRESS="$SIG_CHECKER_ADDRESS" \
    forge script script/DeployOnchainLLMSharded.s.sol --rpc-url "$HTTP_RPC" \
    --private-key "$PRIVATE_KEY" --broadcast --slow -v 2>&1)
echo "$OUT" >&2

# Re-emit the machine-readable lines on stdout for the harness to parse.
FOUND=0
while IFS= read -r line; do
    case "$line" in
        *DEPLOYED_TARGET=*|*SEG_ENGINE=*|*ENGINE=*|*WEIGHTS_ROOT=*|*PACKED_CFG_*=*|*N_LAYERS=*|*KVD=*|*DIM=*|*VOCAB=*|*STOP0=*|*STOP1=*|*SEQ_CAP=*)
            # strip forge's leading indentation
            echo "${line#"${line%%[![:space:]]*}"}"
            FOUND=1
            ;;
    esac
done <<< "$OUT"

if [ "$FOUND" != "1" ]; then
    echo "DEPLOYED_TARGET not found in forge script output" >&2
    exit 1
fi

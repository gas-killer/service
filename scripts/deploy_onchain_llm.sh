#!/bin/bash
#
# Deploy the on-chain LLM consumer (gas-killer/solidity-sdk `onchain-llm` example,
# stories260K checkpoint) against the harness chain: 22 weight-chunk data contracts +
# tokenizer + directory + LlamaEngine + GasKillerLLM, wired to the harness AVS and
# signature checker. Prints `LLM_TARGET=<address>` on success.
#
# Env:
#   HTTP_RPC              chain RPC        (default http://localhost:8545)
#   PRIVATE_KEY           funded deployer  (required)
#   AVS_DEPLOYMENT_PATH   avs deploy json  (required — same file deploy_array_summation reads)
#   GK_SDK_REPO / GK_SDK_REF   consumer source (default: the onchain-llm branch/PR #56)
#   GK_SDK_DIR            checkout cache dir (default .gk-solidity-sdk)
set -euo pipefail

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

if [ ! -d "$SDK_DIR/.git" ]; then
    git clone --depth 1 -b "$SDK_REF" "$SDK_REPO" "$SDK_DIR" >&2
    git -C "$SDK_DIR" submodule update --init --recursive --depth 1 >&2
fi

# 22 weight chunks + tokenizer + directory + engine + consumer, all real CREATEs
# (~26 txs; the stories260K model is small enough that no setCode shortcut is needed).
OUT=$(cd "$SDK_DIR" && AVS_ADDRESS="$AVS_ADDRESS" SIG_CHECKER_ADDRESS="$SIG_CHECKER_ADDRESS" \
    forge script script/DeployOnchainLLM.s.sol --rpc-url "$HTTP_RPC" \
    --private-key "$PRIVATE_KEY" --broadcast --slow -v 2>&1)
echo "$OUT" >&2

TARGET=$(printf '%s\n' "$OUT" | grep -o 'DEPLOYED_TARGET=0x[a-fA-F0-9]*' | tail -1 | cut -d= -f2)
if [ -z "$TARGET" ]; then
    echo "DEPLOYED_TARGET not found in forge script output" >&2
    exit 1
fi
echo "LLM_TARGET=$TARGET"

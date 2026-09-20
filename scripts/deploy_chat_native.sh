#!/bin/bash
#
# Deploy the native chat consumer (gas-killer/solidity-sdk `onchain-llm-native` example,
# GasKillerChatNative) against the harness chain, wired to the harness AVS and signature
# checker. One CREATE: there is no engine and there are no data contracts — the answer is
# computed by the guest program the operators have installed, behind the gkvm precompile
# (UNBOUNDED_V3). The consumer takes the canonical precompile address and a zero
# artifactRoot (answer.py mounts no artifact). The chain itself stays vanilla.
# Prints `CHAT_NATIVE_TARGET=<address>` on success.
#
# Env:
#   HTTP_RPC              chain RPC        (default http://localhost:8545)
#   PRIVATE_KEY           funded deployer  (required)
#   AVS_DEPLOYMENT_PATH   avs deploy json  (required — same file deploy_array_summation reads)
#   GK_SDK_REPO / GK_SDK_REF   consumer source (default: the gkvm M5 branch)
#   GK_SDK_DIR            checkout cache dir (default .gk-solidity-sdk-native); point it
#                         at an existing solidity-sdk checkout to use that as-is
set -euo pipefail

# The compose harness keeps its configuration in .env (the Rust helpers read it
# via dotenv); load it the same way when invoked without explicit env.
if [ -z "${PRIVATE_KEY:-}" ] && [ -f .env ]; then
    set -a
    # shellcheck disable=SC1091
    . ./.env
    set +a
fi

HTTP_RPC="${HTTP_RPC:-http://localhost:8545}"
SDK_REPO="${GK_SDK_REPO:-https://github.com/gas-killer/solidity-sdk}"
SDK_REF="${GK_SDK_REF:-Rubydusa/gkvm-m5-dx}"
SDK_DIR="${GK_SDK_DIR:-.gk-solidity-sdk-native}"

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

ZERO_ADDRESS=0x0000000000000000000000000000000000000000
ZERO_ROOT=0x0000000000000000000000000000000000000000000000000000000000000000

OUT=$(cd "$SDK_DIR" && forge create \
    src/examples/onchain-llm-native/GasKillerChatNative.sol:GasKillerChatNative \
    --rpc-url "$HTTP_RPC" --private-key "$PRIVATE_KEY" --broadcast \
    --constructor-args "$AVS_ADDRESS" "$SIG_CHECKER_ADDRESS" "$ZERO_ADDRESS" "$ZERO_ROOT" 2>&1)
echo "$OUT" >&2

TARGET=$(printf '%s\n' "$OUT" | grep -o 'Deployed to: 0x[a-fA-F0-9]*' | tail -1 | grep -o '0x[a-fA-F0-9]*')
if [ -z "$TARGET" ]; then
    echo "deployed address not found in forge create output" >&2
    exit 1
fi
echo "CHAT_NATIVE_TARGET=$TARGET"

#!/bin/bash

# Native chat (UNBOUNDED_V3) end-to-end test.
#
# Runs the standard e2e flow with the solidity-sdk `onchain-llm-native` consumer
# (GasKillerChatNative): `ask(promptIds, maxNewTokens)` is ONE staticcall into the gkvm
# precompile, which exists only inside the operators' simulation environment — the
# router and every node run GK_SIM_EXECUTOR=local with the guest program
# (`guest/answer.py`, built by the sdk's `gk build`) installed and hash-verified at
# startup (docker-compose.gkvm.yml). The quorum signs the single-STORE + ChatAnswered
# diff, verifyAndUpdate lands it in one small transaction, and the harness decodes and
# asserts the answer text from the applied receipt.
#
# What run_e2e_test.sh does differently for GK_E2E_CONSUMER=chat-native:
#   - step 4b: stages the guest image into config/guest (scripts/stage_guest_program.sh)
#   - step 5:  starts the chain + AVS setup only
#   - step 7:  deploys the consumer (scripts/deploy_chat_native.sh, CHAT_NATIVE_TARGET=)
#   - step 7a: direct ask() on the chain must revert GkVmUnavailable — the chain anvil
#              stays VANILLA (no --disable-block-gas-limit, no setCode, no precompile)
#   - step 7c: starts router + nodes with the consumer in GK_GUEST_VM_CONSUMERS
#   - step 10c: decodes ChatAnswered from the applied receipt
#
# The simulation profile is unbounded-v1, the profile guest-VM consumers run under (the
# precompile charges guest cycles as gas against the profile's budget). `chain` is not an
# option with the local executor on an Osaka chain today: its default tx gas exceeds the
# EIP-7825 cap and every analysis fails before any code runs.
#
# Knobs:
#   GK_GUEST_ELF            prebuilt guest image (skips the docker guest build)
#   GK_SDK_DIR              existing solidity-sdk checkout to use instead of cloning
#   GK_SDK_REPO / GK_SDK_REF
#   GK_CHAT_PROMPT_IDS / GK_CHAT_MAX_TOKENS / GK_CHAT_EXPECT
#   GK_VERIFY_TIMEOUT_SECS  poll window for the applied transition (default 300)
#   GK_E2E_NEGATIVE=1       negative leg: node-3 runs without the guest program
#                           (docker-compose.gkvm-negative.yml); the round must land on
#                           node-1 + node-2 alone and step 10d asserts node-3 refused at
#                           the guest-VM gate and signed nothing

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"
if [ ! -f .env ]; then
    cp example.env .env
    echo ".env created from example.env"
fi

set_env_var() {
    local key="$1" value="$2"
    if grep -q "^${key}=" .env; then
        sed -i.bak "s|^${key}=.*|${key}=${value}|" .env && rm -f .env.bak
    else
        echo "${key}=${value}" >> .env
    fi
}

# Written into .env (not just exported) because the main script `source`s .env mid-run
# and docker compose reads it for interpolation. The executor and the guest slots are
# NOT written there: docker-compose.gkvm.yml sets them on the containers only.
set_env_var GK_SIM_PROFILE unbounded-v1
set_env_var ANVIL_EXTRA_ARGS ""

export GK_SIM_PROFILE=unbounded-v1
export ANVIL_EXTRA_ARGS=
export GK_E2E_CONSUMER=chat-native
export GK_VERIFY_TIMEOUT_SECS="${GK_VERIFY_TIMEOUT_SECS:-300}"

echo "Native chat e2e: GK_SIM_PROFILE=unbounded-v1, GK_SIM_EXECUTOR=local (containers), consumer=chat-native"
exec bash "$SCRIPT_DIR/run_e2e_test.sh" "$@"

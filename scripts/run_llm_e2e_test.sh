#!/bin/bash

# On-chain LLM end-to-end test.
#
# Runs the standard e2e flow with the solidity-sdk `onchain-llm` consumer
# (GasKillerLLM, stories260K — a full Llama-2 transformer in pure Solidity):
# real operators simulate `tellStory("Once upon a time", 32)` under
# SimProfile::UnboundedV1 (~1.4B gas — ~47x over a mainnet block), the quorum
# signs the single-STORE + StoryTold-log diff, verifyAndUpdate lands it in one
# small transaction, and the harness decodes and asserts the story text
# ("...little girl named Lily...") from the applied receipt.
#
# Knobs (this consumer needs more room than ArraySummation):
#   ROUND_TIMEOUT           default raised to 120s (simulation is seconds, the
#                           margin covers tracer overhead on slow runners)
#   GK_VERIFY_TIMEOUT_SECS  poll window for the applied transition (default 300)
#   GK_LLM_PROMPT / GK_LLM_MAX_TOKENS / GK_LLM_EXPECT
#
# For the Qwen3-0.6B variant (hundreds of billions of gas, ~10min+ simulation)
# see helm/gas-killer/llm-overrides.yaml — that profile needs ROUND_TIMEOUT
# on the order of 1800s plus node memory headroom, and is not CI material.

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

set_env_var GK_SIM_PROFILE unbounded-v1
set_env_var ANVIL_EXTRA_ARGS --disable-block-gas-limit
set_env_var ROUND_TIMEOUT "${ROUND_TIMEOUT:-120}"

export GK_SIM_PROFILE=unbounded-v1
export ANVIL_EXTRA_ARGS=--disable-block-gas-limit
export ROUND_TIMEOUT="${ROUND_TIMEOUT:-120}"
export GK_E2E_CONSUMER=onchain-llm
export GK_VERIFY_TIMEOUT_SECS="${GK_VERIFY_TIMEOUT_SECS:-300}"

echo "LLM e2e: GK_SIM_PROFILE=unbounded-v1, consumer=onchain-llm, ROUND_TIMEOUT=$ROUND_TIMEOUT"
exec bash "$SCRIPT_DIR/run_e2e_test.sh" "$@"

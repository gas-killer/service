#!/bin/bash

# SHARDED on-chain LLM end-to-end test.
#
# Runs the e2e flow with the solidity-sdk `onchain-llm-sharded` consumer stack
# (synthetic engine-v2 fixture model): the router's shard coordinator splits one
# inference into hash-committed segments executed by k=2-of-3 operator
# committees (`Qwen3SegEngine.forwardRange`/`argmaxRange` view calls — each node
# executes only its share), assembles the commit chain, and the answer settles
# through the NORMAL round path as a pure-commit `fulfil()` task on
# GasKillerChatSharded — with every node's validator gate verifying the chain
# (including the digests of the segments it executed itself) before signing.
#
# Asserts: bit-exact answer ids vs the fixture reference, committee agreement on
# every segment, a strict work SHARE per node (nobody executed everything), gate
# verification on all three nodes, and the applied ChatAnswered event on-chain.
#
# Knobs:
#   GK_SDK_REPO / GK_SDK_REF / GK_SDK_DIR   consumer source checkout
#   GK_SHARD_POLL_MS                        node work-poll cadence (default 300)
#   GK_VERIFY_TIMEOUT_SECS                  poll window for the applied transition

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

# The fulfil() settlement task is ~100k gas, so the default `chain` profile
# suffices for the ROUND; --disable-block-gas-limit lets the segment
# eth_calls run with the lifted simulation gas override on the harness anvil.
set_env_var ANVIL_EXTRA_ARGS --disable-block-gas-limit
set_env_var ROUND_TIMEOUT "${ROUND_TIMEOUT:-120}"
set_env_var GK_VERIFY_MODE transition-count

export ANVIL_EXTRA_ARGS=--disable-block-gas-limit
export ROUND_TIMEOUT="${ROUND_TIMEOUT:-120}"
# Arm sharding from initial bring-up (before docker compose up) so the p2p
# mesh forms once, healthy — the node segment executor + validator gate read
# this at startup (compose interpolates ${GK_SHARD_URL}).
export GK_SHARD_URL="${GK_SHARD_URL:-http://router:8081}"
export GK_E2E_CONSUMER=onchain-llm-sharded
export GK_VERIFY_MODE=transition-count
export GK_VERIFY_TIMEOUT_SECS="${GK_VERIFY_TIMEOUT_SECS:-300}"

echo "Sharded LLM e2e: consumer=onchain-llm-sharded, k=2 committees over 3 operators, ROUND_TIMEOUT=$ROUND_TIMEOUT"
exec bash "$SCRIPT_DIR/run_e2e_test.sh" "$@"

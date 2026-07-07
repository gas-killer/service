#!/bin/bash

# Unbounded-mode end-to-end test.
#
# Runs the standard e2e flow with the ArraySummation array sized so that a direct
# on-chain sum() would cost far more than the mainnet block gas limit (30M). The
# stack simulates the tracked function under SimProfile::UnboundedV1 (pinned 2^40
# gas limits — see gas-analyzer docs/UNBOUNDED_MODE.md), the quorum signs the
# O(1) state diff, and verifyAndUpdate lands it in one small transaction.
#
# run_e2e_test.sh asserts both halves when GK_SIM_PROFILE=unbounded-v1:
#   - step 7a: cast estimate of direct sum() > 30,000,000 gas
#   - step 10b: the verifyAndUpdate receipt used a small fraction of a block
#
# The overrides are written into .env (not just exported) because the main script
# `source`s .env mid-run and docker compose reads it for interpolation.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# ~20k elements ≈ 43M gas for a direct sum() (one cold SLOAD per element), safely
# above the 30M assertion threshold while keeping the anvil-side deploy quick.
UNBOUNDED_ARRAY_SIZE="${ARRAY_SUMMATION_ARRAY_SIZE:-20000}"

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
set_env_var ARRAY_SUMMATION_ARRAY_SIZE "$UNBOUNDED_ARRAY_SIZE"

export GK_SIM_PROFILE=unbounded-v1
export ANVIL_EXTRA_ARGS=--disable-block-gas-limit
export ARRAY_SUMMATION_ARRAY_SIZE="$UNBOUNDED_ARRAY_SIZE"

echo "Unbounded e2e: GK_SIM_PROFILE=unbounded-v1, ARRAY_SUMMATION_ARRAY_SIZE=$UNBOUNDED_ARRAY_SIZE"
exec bash "$SCRIPT_DIR/run_e2e_test.sh" "$@"

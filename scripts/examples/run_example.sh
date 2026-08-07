#!/usr/bin/env bash
#
# End-to-end harness for one example contract: fetch and build it, deploy it wired to the
# local AVS, then drive a task through the router and confirm the state transition landed.
#
#   ./scripts/examples/run_example.sh onchainLife
#   ./scripts/examples/run_example.sh guardedVault
#
# Expects the local stack to already be running (`./scripts/run_e2e_test.sh --keep-up`, or
# `docker compose up -d`). Set SKIP_FETCH=1 to reuse an existing artifact build.
#
# Requires: git, forge, cargo, and an authenticated router (GAS_KILLER_API_KEY) if ADMIN_KEY
# minting is enabled on the deployment.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

EXAMPLE="${1:-}"
if [ -z "$EXAMPLE" ]; then
  echo "Usage: $0 <example-name>" >&2
  echo >&2
  echo "Examples declared in scripts/examples/examples.toml:" >&2
  grep -E '^name\s*=' scripts/examples/examples.toml | sed 's/name *= *//; s/"//g; s/^/  /' >&2
  exit 1
fi

ROUTER_URL="${GAS_KILLER_ROUTER_URL:-http://localhost:8080}"
if ! curl -fsS --max-time 5 "$ROUTER_URL/healthz" >/dev/null 2>&1; then
  echo "❌ router not reachable at $ROUTER_URL/healthz" >&2
  echo "   Start the local stack first:  ./scripts/run_e2e_test.sh --keep-up" >&2
  echo "   (or point GAS_KILLER_ROUTER_URL at a running router)" >&2
  exit 1
fi
echo "✅ router healthy at $ROUTER_URL"

# onchainLife's tracked function is ~16.5M gas. Only the prestate-net encoding, which reads a
# net diff sized by changed slots, can extract it; the struct-log encoders behind legacy and
# canonical exhaust the node first. Warn rather than block — the running stack's encoding is
# what actually matters and this only sees the local environment.
if [ "$EXAMPLE" = "onchainLife" ] && [ "${STATE_ENCODING:-legacy}" != "prestate-net" ]; then
  echo "⚠️  STATE_ENCODING is '${STATE_ENCODING:-legacy}', but onchainLife only settles under 'prestate-net'." >&2
  echo "   Expect the task to never reach 'ready' (the struct-log trace exhausts the node)." >&2
  echo "   Recreate the stack with the encoding set on the nodes AND the router:" >&2
  echo "     STATE_ENCODING=prestate-net docker compose up -d --force-recreate" >&2
  echo >&2
fi

if [ "${SKIP_FETCH:-0}" != "1" ]; then
  "$REPO_ROOT/scripts/examples/fetch_examples.sh"
else
  echo "⏭️  SKIP_FETCH=1, reusing the existing artifact build"
fi

echo
echo "═══ deploying $EXAMPLE ═══"
cargo run -q -p scripts --bin deploy_example -- --example "$EXAMPLE"

SCENARIO="scripts/scenarios/generated/$EXAMPLE.toml"
if [ ! -f "$SCENARIO" ]; then
  echo "❌ $SCENARIO was not generated; does the example declare an [[examples.exercise]]?" >&2
  exit 1
fi

echo
echo "═══ running $SCENARIO ═══"
cargo run -q -p scripts --bin run_scenario -- "$SCENARIO"

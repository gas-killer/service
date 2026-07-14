#!/bin/bash
# 10-fresh-operator 0.6B sharded e2e on Sepolia — full bring-up runbook.
#
# Prereqs: gcloud authed (ron@gaskiller.xyz), kubectl context
# gke_gas-killer-testnet_us-east4_gas-killer, forge + cast, the solidity-sdk
# checkout (SDK_DIR), and the pushed pr-321 images.
#
# Idempotent-ish: each phase checks its outcome before acting. Run phases
# selectively with: bash scripts/tenop_runbook.sh <phase>
#   pool | install | wait-setup | arm | consumer | infer | verify | down
set -euo pipefail

NS=tenop
CTX=gke_gas-killer-testnet_us-east4_gas-killer
SDK_DIR="${SDK_DIR:-/Users/wk/conductor/workspaces/solidity-sdk/monterrey-v3}"
SVC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RPC=https://ethereum-sepolia-rpc.publicnode.com
K="kubectl --context $CTX -n $NS"
PHASE="${1:-all}"

phase() { [ "$PHASE" = "all" ] || [ "$PHASE" = "$1" ]; }

deployer_key() {
  kubectl --context "$CTX" -n default get secret gas-killer-secret -o json \
    | python3 -c "import json,sys,base64;print(base64.b64decode(json.load(sys.stdin)['data']['PRIVATE_KEY']).decode())"
}

if phase pool; then
  echo "== phase: pool (single 16-vCPU Spot VM — RWO PVC forces one-VM topology) =="
  if ! gcloud container node-pools describe tenop-pool --cluster gas-killer --region us-east4 >/dev/null 2>&1; then
    gcloud container node-pools create tenop-pool --cluster gas-killer --region us-east4 \
      --machine-type n2-standard-8 --spot --num-nodes 1 --node-locations us-east4-a \
      --node-labels role=tenop --node-version "$(gcloud container clusters describe gas-killer --region us-east4 --format='value(currentNodeVersion)')"
  else echo "pool exists"; fi
fi

if phase install; then
  echo "== phase: install =="
  kubectl --context "$CTX" get ns $NS >/dev/null 2>&1 || kubectl --context "$CTX" create namespace $NS
  # ghcr pull secret: copy from the live namespace
  if ! $K get secret ghcr-pull-secret >/dev/null 2>&1; then
    kubectl --context "$CTX" -n default get secret ghcr-pull-secret -o yaml \
      | sed 's/namespace: default/namespace: tenop/' | $K apply -f -
  fi
  PK=$(deployer_key)
  ADMIN=$(openssl rand -hex 16)
  helm --kube-context "$CTX" upgrade --install gas-killer "$SVC_DIR/helm/gas-killer" -n $NS \
    -f "$SVC_DIR/helm/gas-killer/tenop-0.6b-overrides.yaml" \
    --set secrets.privateKey="$PK" --set secrets.fundedKey="$PK" \
    --set secrets.adminKey="$ADMIN"
  echo "ADMIN_KEY=$ADMIN" > /tmp/tenop_admin.env
  # pin everything to the tenop pool (not values-expressible; re-apply after upgrades)
  for d in $($K get deploy -o name); do
    $K patch "$d" -p '{"spec":{"template":{"spec":{"nodeSelector":{"role":"tenop"}}}}}' || true
  done
fi

if phase wait-setup; then
  echo "== phase: wait-setup (10 Sepolia registrations + ~15min allocation delay) =="
  for i in $(seq 1 240); do
    if $K logs job/gas-killer-setup --tail=2000 2>/dev/null | grep -q "Operator 10 weight in quorum"; then
      echo "setup complete"; break
    fi
    [ "$i" = 240 ] && { echo "TIMEOUT waiting for setup"; $K logs job/gas-killer-setup --tail=50; exit 1; }
    sleep 15
  done
fi

if phase arm; then
  echo "== phase: arm sharding on the 10 nodes (node-N <-> operator id N-1) =="
  for i in $(seq 1 10); do
    $K set env deploy/gas-killer-node-$i \
      GK_SHARD_URL=http://gas-killer-router:8081 GK_SHARD_OPERATOR_ID=$((i-1))
  done
  $K rollout status deploy/gas-killer-router --timeout=300s || true
fi

if phase consumer; then
  echo "== phase: consumer (fresh Qwen3SegEngine + GasKillerChatSharded vs fresh AVS) =="
  POD=$($K get pods -o name | grep node-1 | head -1)
  $K exec "$POD" -c node -- cat /app/.nodes/avs_deploy.json > /tmp/tenop_avs_deploy.json
  RC=$(python3 -c "import json;print(json.load(open('/tmp/tenop_avs_deploy.json'))['addresses']['registryCoordinator'])")
  AVS=$(python3 -c "import json;print(json.load(open('/tmp/tenop_avs_deploy.json'))['addresses']['avsServiceManagerWrapper'])")
  echo "fresh registryCoordinator=$RC avsWrapper=$AVS"
  PK=$(deployer_key)
  (cd "$SDK_DIR" && AVS_ADDRESS="$AVS" REGISTRY_COORDINATOR_ADDRESS="$RC" \
    forge script script/DeployOnchainLLMShardedOverlay.s.sol:DeployOnchainLLMShardedOverlayScript \
    --rpc-url "$RPC" --private-key "$PK" --broadcast --slow 2>&1 | tee /tmp/tenop_consumer_deploy.log | tail -12)
  CONSUMER=$(grep "^  DEPLOYED_TARGET=" /tmp/tenop_consumer_deploy.log | tail -1 | cut -d= -f2)
  SEG=$(grep "^  SEG_ENGINE=" /tmp/tenop_consumer_deploy.log | tail -1 | cut -d= -f2)
  echo "CONSUMER=$CONSUMER" > /tmp/tenop_contracts.env
  echo "SEG_ENGINE=$SEG" >> /tmp/tenop_contracts.env
  # scope the gate + coordinator to the consumer
  for i in $(seq 1 10); do $K set env deploy/gas-killer-node-$i GK_SHARD_CONSUMER="$CONSUMER"; done
  $K set env deploy/gas-killer-router GK_SHARD_CONSUMER="$CONSUMER"
fi

if phase infer; then
  echo "== phase: infer (POST /shard/infer inside the router pod) =="
  . /tmp/tenop_contracts.env
  python3 - "$CONSUMER" "$SEG_ENGINE" <<'EOF' > /tmp/tenop_req.json
import json, sys
req = json.load(open('/Users/wk/conductor/workspaces/solidity-sdk/monterrey-v3/.context/tenop/shard06_req.template.json'))
req.pop('_comment', None)
req['consumer'] = sys.argv[1]
req['seg_engine'] = sys.argv[2]
json.dump(req, sys.stdout)
EOF
  RP=$($K get pods -o name | grep router | head -1)
  $K exec -i "$RP" -c router -- sh -c 'cat > /tmp/req.json' < /tmp/tenop_req.json
  $K exec "$RP" -c router -- sh -c '
    date +%s > /tmp/t0
    nohup curl -s --max-time 7200 -X POST -H "Content-Type: application/json" \
      -d @/tmp/req.json http://localhost:8081/shard/infer > /tmp/resp.json 2>/tmp/curl.err &
    echo launched'
  echo "poll with: $K exec ${RP#pod/} -c router -- cat /tmp/resp.json"
fi

if phase verify; then
  echo "== phase: verify (answer + settle through the round + indexed event) =="
  RP=$($K get pods -o name | grep router | head -1)
  $K exec "$RP" -c router -- cat /tmp/resp.json | python3 -m json.tool
  echo "then: build fulfil() calldata + submit via /trigger (see run_e2e_test.sh Step 9c/10)"
fi

if phase down; then
  echo "== phase: down (turn the experiment off) =="
  helm --kube-context "$CTX" -n $NS uninstall gas-killer || true
  kubectl --context "$CTX" delete ns $NS || true
  gcloud container node-pools delete tenop-pool --cluster gas-killer --region us-east4 --quiet || true
fi

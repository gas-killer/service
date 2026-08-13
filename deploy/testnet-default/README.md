# deploy/testnet-default — live unmanaged resources (default namespace)

Git capture of the kubectl-only resources running alongside the `gas-killer`
Helm release in the **default** namespace of the `gas-killer` GKE cluster
(gas-killer-testnet, us-east4). Exported read-only on **2026-07-16**; these are
NOT rendered by the Helm chart and a `helm upgrade` neither creates nor deletes
them.

## What's in sim-fork-stack.yaml

| Resource | Role |
| --- | --- |
| `deploy/qwen-sim-fork` + `svc/qwen-sim-fork` | Anvil Sepolia fork on **:8547** (forks the publicnode RPC, `--disable-block-gas-limit`, 64KB code-size limit). This is the `GK_SIM_RPC=http://qwen-sim-fork:8547` endpoint the router/nodes trace against. |
| `deploy/gk-sim-proxy` + `svc/gk-sim-proxy` + `configmap/gk-sim-proxy-src` | Python JSON-RPC proxy on **:8545**: routes `debug_traceCall` for the pinned overlay consumers to the fork, everything else upstream; serves `/forkhead` and `/status` (in-flight trace count, used by the ensurer to avoid reforking mid-trace). |
| `deploy/qwen-overlay-ensurer` | Loop that keeps the Qwen3-0.6B overlay chunks `anvil_setCode`'d into the fork (manifest-verified download from the solidity-sdk release) and reforks to ~head when drift exceeds 150 blocks and no trace is in flight. |

## What's NOT here

- **gk-shard-bridge + its cron jobs** live in [`bridge/k8s.yaml`](../../bridge/k8s.yaml).
- Everything else in the namespace (router, nodes, monitoring, ingress) is the
  `gas-killer` Helm release — chart in [`helm/gas-killer/`](../../helm/gas-killer/),
  with the live env drift captured in
  [`helm/gas-killer/default-live-overrides.yaml`](../../helm/gas-killer/default-live-overrides.yaml).

## Scrubbing / re-apply notes

- Runtime fields (`status`, `uid`, `resourceVersion`, `creationTimestamp`,
  `generation`, `managedFields`, `annotations`, Service `clusterIP(s)`) were
  stripped from the export.
- **Secrets were scrubbed**: the API-keyed Alchemy Sepolia RPC URL used as
  `UPSTREAM_URL` (gk-sim-proxy) and `UPSTREAM` (qwen-overlay-ensurer) was
  replaced with a `secretKeyRef` to a `gk-sim-upstream` secret (key `url`).
  Create it before applying:

  ```sh
  kubectl create secret generic gk-sim-upstream \
    --from-literal=url='https://eth-sepolia.g.alchemy.com/v2/<KEY>'
  ```

  (The live deployments carry the URL inline; applying this file switches them
  to the secret — functionally identical, but it IS a spec diff vs live.)
- `qwen-sim-fork`'s `FORK_URL` is the keyless publicnode endpoint and was kept
  inline.
- All three deployments pin to the `role=sim` node pool via `nodeSelector`.

## Revival order (after demo-expiry fires, or a namespace rebuild)

demo-expiry deletes the `ops-highmem` + `sim-pool-c4` node pools, strips the
hostname pins, and shrinks default-pool to one e2-standard-4 — nothing
schedules until capacity is back. Revive in this order:

1. Recreate pools:
   `gcloud container node-pools create ops-highmem --cluster gas-killer --region us-east4 --machine-type n2-highmem-8 --num-nodes 1 --node-locations us-east4-b`
   `gcloud container node-pools create sim-pool-c4 --cluster gas-killer --region us-east4 --machine-type c4-highcpu-8 --num-nodes 1 --node-locations us-east4-b --node-labels role=sim`
2. Edit BOTH `nodeSelector` blocks in `helm/gas-killer/default-live-overrides.yaml`
   to the new ops-highmem hostname (`kubectl get nodes`) — the committed pin is
   the old VM name and will never schedule.
3. Ensure secrets exist: `gk-sim-upstream` (keyed archive RPC URL for the sim
   proxy/ensurer), `gk-bridge-key` (mint a gk_ key via the router admin API with
   ADMIN_KEY, then `kubectl create secret generic gk-bridge-key --from-literal=GK_KEY=gk_...`).
4. Re-stage weights on the gas-killer-shared-data PVC if it was lost:
   /app/.nodes/qwen35/{weights.bin,tokenizer.bin} + /app/.nodes/qwen06/... —
   fetch from the solidity-sdk release artifacts and verify the two keccak
   manifests pinned in default-live-overrides.yaml.
5. `helm upgrade gas-killer helm/gas-killer -n default -f <release values> -f helm/gas-killer/default-live-overrides.yaml`
6. `kubectl apply -f deploy/testnet-default/sim-fork-stack.yaml`
7. Bridge: `kubectl create configmap gk-bridge-script --from-file=bridge.py=bridge/bridge.py --dry-run=client -o yaml | kubectl apply -f -` then `kubectl apply -f bridge/k8s.yaml`
   (the exported demo-expiry ships suspend:true on purpose — unsuspend deliberately).

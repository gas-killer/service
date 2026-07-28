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

## Other files here

| File | Role |
| --- | --- |
| `extras-ingress.yaml` | Standalone `gas-killer-extras` ingress exposing `/forkhead`. Separate from the chart ingress because helm re-renders wiped the path 3×. |
| `demo-expiry-rbac.yaml` | ServiceAccount/Role the demo-expiry cron uses to patch and delete workloads. |
| `verify-artifacts.yaml` | One-shot keccak integrity check for the PVC weights — see "Re-staging weights". |

## What's NOT here

- **gk-shard-bridge + its cron jobs** live in [`bridge/k8s.yaml`](../../bridge/k8s.yaml).
  Both crons ship suspended. `qwen-fork-refresh` is suspended *permanently*: its
  unconditional `anvil_reset` wiped the overlay chunks every 8 minutes, so the
  ~24k-chunk load could never converge. The ensurer's own drift-aware refork
  replaces it.
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

## Node pools (as of 2026-07-27)

| Pool | Machine | Label | Hosts |
| --- | --- | --- | --- |
| `ops-highmem` | n2-highmem-8 ×1, 100GB pd-balanced | `role=ops-highmem` | router + node-1/2/3. 64Gi is required: the RWO `gas-killer-shared-data` PVC forces all four onto one VM, and cgroup v2 charges the 34GB overlay mmap to the first-faulting pod, so every pod's limit must exceed it. |
| `sim-pool` | c4-standard-4 ×1, 50GB hyperdisk-balanced | `role=sim` | qwen-sim-fork + gk-sim-proxy + qwen-overlay-ensurer. C4 **requires** hyperdisk; `pd-*` is rejected. |
| `default-pool` | e2-standard-4 ×1, 20GB pd-standard | — | ingress, bridge, cert-manager, kube-prometheus-stack. |

These are declared in Terraform at `infra/terraform/llm-testnet` (repo
[gas-killer/infra](https://github.com/gas-killer/infra)) — prefer `terraform apply` over
ad-hoc `gcloud`, so the cluster and the code stay in agreement.

## Revival order (after demo-expiry fires, or a namespace rebuild)

demo-expiry deletes the `ops-highmem` + `sim-pool` node pools and shrinks
default-pool to one e2-standard-4 — nothing schedules until capacity is back.
It deliberately leaves the `nodeSelector`s alone (see below). Revive in this order:

1. Recreate the pools — `cd infra/terraform/llm-testnet && terraform apply`.
   Fallback if Terraform is unavailable:
   `gcloud container node-pools create ops-highmem --cluster gas-killer --region us-east4 --machine-type n2-highmem-8 --num-nodes 1 --node-locations us-east4-b --disk-type pd-balanced --disk-size 100 --node-labels role=ops-highmem`
   `gcloud container node-pools create sim-pool --cluster gas-killer --region us-east4 --machine-type c4-standard-4 --num-nodes 1 --node-locations us-east4-b --disk-type hyperdisk-balanced --disk-size 50 --node-labels role=sim`
2. Ensure secrets exist: `gk-sim-upstream` (keyed archive RPC URL for the sim
   proxy/ensurer), `gk-bridge-key` (mint a gk_ key via the router admin API with
   ADMIN_KEY, then `kubectl create secret generic gk-bridge-key --from-literal=GK_KEY=gk_...`).
3. Re-stage weights on the gas-killer-shared-data PVC **only if it was lost** — the
   PVC survives demo-expiry, so this is normally a no-op. See "Re-staging weights" below.
4. `helm upgrade gas-killer helm/gas-killer -n default -f <release values> -f helm/gas-killer/default-live-overrides.yaml`
5. `kubectl apply -f deploy/testnet-default/sim-fork-stack.yaml`
6. Bridge: `kubectl create configmap gk-bridge-script --from-file=bridge.py=bridge/bridge.py --dry-run=client -o yaml | kubectl apply -f -` then `kubectl apply -f bridge/k8s.yaml`
   (both crons ship `suspend: true` on purpose — unsuspend deliberately).

**Do not re-pin by hostname.** Both `nodeSelector`s in `default-live-overrides.yaml`
select the node **pool** (`role: ops-highmem`), which is a pool-level label and so
survives VM recreation and pool upgrades. The previous hostname pin went stale on
every node replacement, and stripping it entirely is what caused the 2026-07-27
outage: node-1 landed on the sim pool while the RWO PVC was attached to
ops-highmem, deadlocking it in `Init:0/2` with no events, which in turn crashlooped
the router's `wait-for-nodes` init container.

**Rolls need force-deletes.** With four 12Gi-request pods on one 64Gi node, the
default 25% maxSurge cannot fit, so a rollout leaves surge pods `Pending` behind
`Terminating` ones. `kubectl delete pod <old> --force --grace-period=0` to clear it.

## Re-staging weights (only if the PVC is lost)

Artifacts live on GitHub releases in `gas-killer/solidity-sdk`, and a copy of the
35B set is in `gs://gk-35b-artifacts-gas-killer-testnet`.

```sh
# 35B: 19 parts, ~1.9GB each -> /app/.nodes/qwen35/weights.bin
gh release download qwen3.5-35b-a3b-onchain-v1 -R gas-killer/solidity-sdk
cat weights.bin.part* > /app/.nodes/qwen35/weights.bin   # tools/fetch_release_parts.sh automates this
# 0.6B
gh release download qwen3-0.6b-onchain-v1 -R gas-killer/solidity-sdk
```

Expected sizes — a mismatch means a truncated or partial download:

| File | Bytes |
| --- | --- |
| `qwen35/weights.bin` | 34,714,656,811 |
| `qwen35/tokenizer.bin` | 2,836,678 |
| `qwen06/weights.bin` | 597,135,857 |
| `qwen06/tokenizer.bin` | 1,584,066 |

Then verify integrity — size alone is not enough, and the original staging Job
checked only sizes. `manifest = keccak(keccak(weights) || keccak(tokenizer))` must
equal the value pinned in `default-live-overrides.yaml`:

| Model | Manifest |
| --- | --- |
| Qwen3.5-35B-A3B | `0x7bdf4876a6861287521dadab3d3870f74dfa557507ed200d49f75bcb09f01fa9` |
| Qwen3-0.6B | `0x23216cb9ed9ef2b4bc20c84d27b68fa62ab194fc0845dfa707836f48ec4a7ae9` |

Both were recomputed byte-for-byte on 2026-07-27 and matched. `kubectl apply -f
deploy/testnet-default/verify-artifacts.yaml` re-runs the check (~2 min; the 34GB
hash runs at ~294 MiB/s once page-cached).

> **keccak256, not SHA3-256.** Python's `hashlib.sha3_256` is the NIST variant and
> uses different padding — it will silently produce wrong digests. Use
> `pycryptodome`'s `Crypto.Hash.keccak`. Sanity check: `keccak256("")` is
> `c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470`.

The node containers mmap the overlay **lazily at first analysis**, not eagerly at
boot — so a healthy fleet proves nothing about weight integrity. Verify explicitly.

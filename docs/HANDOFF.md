# GasKillerLLM — deployment handoff

Written 2026-07-21 ~19:45 UTC. Covers the full llm.gaskiller.xyz demo: pure-Solidity LLM
inference (Qwen3.5-35B-A3B and Qwen3-0.6B) executed off-chain by a Gas Killer operator
quorum and settled on Sepolia in one BLS-verified transaction.

---

## 0. Current status — READ FIRST

**UPDATE 2026-07-21 ~20:40 UTC: revived.** The teardown turned out to be partial — the cron's GKE REST pool deletion failed on missing IAM (node SA lacked container permissions; scopes were fine) and the job hit its 3600s deadline before self-suspending. Only the sim stack was deleted and the node pins stripped; pools, PVC, secrets and the helm release all survived. Revival = re-apply sim-fork-stack + extras-ingress, re-patch the 4 nodeSelector pins, redeploy the bridge configmap. `roles/container.clusterAdmin` has now been granted to the node compute SA so the next expiry (re-dated `30 20 25 7 *` = July 25 20:30 UTC, armed) can actually delete the pools. Original incident text kept below for the record.

**The demo backend was DOWN.** The `demo-expiry` CronJob fired today at 12:00 UTC
(schedule `0 12 21 7 *`, Etc/UTC). The planned extension to July 26 was blocked by an
expired gcloud auth token ("Reauthentication failed. cannot prompt during non-interactive
execution") — a recurring blocker that only an interactive `gcloud auth login` clears.

Verified state as of 19:40 UTC:

| Surface | State |
|---|---|
| `llm.gaskiller.xyz` (static frontend, GitHub Pages) | **UP** — model pills will show offline; asks fail |
| `testnet.gaskiller.xyz/healthz` (router) | 503 — router/nodes pool deleted |
| `testnet.gaskiller.xyz/forkhead` (gk-sim-proxy) | 503 — sim stack deleted |
| `testnet.gaskiller.xyz/bridge/healthz` (gk-shard-bridge) | UP (`asks: 13`) but non-functional — its upstream router is gone |
| Sepolia contracts + settled history | **Permanent** — 0.6B `stateTransitionCount`=14, 35B=3 |

What the cron deleted (its exact script is tracked in `bridge/k8s.yaml`): deployments/svcs
`qwen-sim-fork`, `gk-sim-proxy`, `qwen-overlay-ensurer` (+ configmap `gk-sim-proxy-src`);
stripped the nodeSelector pins off router + node-1/2/3; deleted node pools `sim-pool-c4`
and `ops-highmem` via the GKE REST API; shrank `default-pool` to 1 node; then suspended
itself. It does **not** delete the `gas-killer-shared-data` PVC (34GB weights), the helm
release object, secrets, or the bridge — so revival may be able to skip weight re-staging
if the PVC survived (verify with `kubectl get pvc` before re-downloading 34GB).

**To revive:** `gcloud auth login`, then follow `deploy/testnet-default/README.md`,
expanded here with the pieces the runbook leaves implicit:

1. Recreate pools with gcloud: `ops-highmem` (n2-highmem-8 ×1, us-east4-b) and
   `sim-pool-c4` (c4-highcpu-8 ×1, node-label `role=sim`).
2. Update the two hostname nodeSelector pins in
   `helm/gas-killer/default-live-overrides.yaml` to the NEW ops-highmem VM name (the
   committed pin is the old VM and will never schedule).
3. Check what survived: `kubectl get secret gk-bridge-key gk-sim-upstream ghcr-pull-secret`
   and `kubectl get pvc gas-killer-shared-data`. Secrets and the PVC are not touched by
   the teardown, so this step is usually a no-op. If the PVC is gone, re-stage weights:
   run a helper pod mounting the PVC, download the release parts
   (`gh release download qwen3.5-35b-a3b-onchain-v1 -R gas-killer/solidity-sdk`, 19×1.9GB;
   sdk `tools/fetch_release_parts.sh` automates this), `cat` parts into
   `/app/.nodes/qwen35/weights.bin` (expect exactly 34,714,656,811 bytes) + tokenizer,
   same for `qwen06/` from `qwen3-0.6b-onchain-v1`, then verify both keccak manifests
   (`0x7bdf4876…f01fa9` / `0x23216cb9…c4a7ae9` = `keccak(keccak(weights)||keccak(tok))`).
4. Helm: the release object survives teardown —
   `helm get values gas-killer -n default -o yaml > /tmp/release-values.yaml`, then
   `helm upgrade gas-killer helm/gas-killer -f /tmp/release-values.yaml -f
   helm/gas-killer/default-live-overrides.yaml`.
5. `kubectl apply -f deploy/testnet-default/sim-fork-stack.yaml` plus
   `deploy/testnet-default/extras-ingress.yaml` (the public `/forkhead` ingress — a
   reconstruction; the live one was never exported).
6. Bridge: recreate the configmap from the CURRENT `bridge/bridge.py` (this also ships
   the pending token-config change — the pre-teardown pod silently clamps `max_new` to
   8), `kubectl apply -f bridge/k8s.yaml` and `deploy/testnet-default/demo-expiry-rbac.yaml`
   (also a reconstruction — the SA/RBAC were never exported). If `gk-bridge-key` was
   lost, mint a new `gk_` key via the router `/admin` API (port-forward the router,
   authenticate with the ADMIN_KEY from the router's secret mount) — only possible
   AFTER step 4 brings the router back.
7. **Only after everything is verified**: edit the demo-expiry `schedule:` to a future
   date FIRST, then set `suspend: false`. Never unsuspend with a past schedule — the
   cron has no `startingDeadlineSeconds` and Kubernetes may fire the missed run
   immediately, re-destroying the environment you just built.
8. Wait for fleet stability (pod count == ready count, sustained — see footgun 2), then
   smoke-test: `curl testnet.gaskiller.xyz/healthz`, `/forkhead`, `/bridge/healthz`; ask
   the 0.6B a question on llm.gaskiller.xyz (or POST `/bridge/ask` with
   `{"model":"qwen","prompt_ids":[151644,872,198,…prompt…,151645,198,151644,77091,198,151667,271,151668,271],"max_new":8}`)
   and expect `queued→inferring→settling→done` in ~4 min with the 0.6B
   `stateTransitionCount` moving 14→15 and a fresh `ChatAnswered` log on Sepolia.

---

## 1. What this is

A public demo that a language model can "run on Ethereum": the transformer forward pass
is written in pure, integer-only Solidity; the Gas Killer operator committee executes it
off-chain as hash-committed sharded segments (up to ~3.6T gas of simulated EVM compute per
answer), agrees byte-exactly, BLS-signs the single storage-slot diff, and lands **one**
`verifyAndUpdate` transaction (~350–384k gas) on Sepolia. Visitors need no key and no
wallet. Answers are reproducible: the on-chain event carries raw token ids that the
browser BPE-decodes; the quorum's answer matches the Python/HF reference token-for-token.

Three models were built; two are on the site (stories260K was removed from the UI):

| Model | Params | Engine | Answer time (8 tok) | Sim gas | Settled gas |
|---|---|---|---|---|---|
| Llama-2 stories260K | 260K | v1, weights fully on-chain | ~20 s | ~1.4B | 354,811 (tx `0x584e7358…`) |
| Qwen3-0.6B | 0.6B | v2, overlay weights, sharded | ~4 min | ~545B | 383,686 (tx `0x24443bd6…`) |
| Qwen3.5-35B-A3B | 35B MoE | v3 (DeltaNet + gated attn + MoE), overlay, sharded | ~72–75 min | ~3.6T | ~384k (tx `0xa14d8c00…`) |

## 2. Live surfaces

- **Frontend**: https://llm.gaskiller.xyz — GitHub Pages, repo
  `RonTuretzky/gaskiller-onchain-llm`, branch `main` (push = deploy), CNAME
  `llm.gaskiller.xyz`. HEAD `3cd5e05`.
- **Router public API**: https://testnet.gaskiller.xyz — nginx ingress → router :8080.
  Public paths `/healthz`, `/trigger` (Bearer key), `/avs-metadata`; `/bridge/*` →
  gk-shard-bridge :8090; `/forkhead` → gk-sim-proxy (standalone `gas-killer-extras`
  ingress). `/admin` only via port-forward.
- **Edge relay**: https://gk-router-proxy.ronturetzky.workers.dev — Cloudflare Worker →
  `http://8-228-87-183.nip.io`, for networks that TLS-intercept the GKE LB IP. The
  frontend auto-falls-back to it.
- **GCP**: project `gas-killer-testnet`, GKE cluster `gas-killer`, region `us-east4`,
  namespace `default`. Requires `gke-gcloud-auth-plugin`. gcloud tokens expire ~daily →
  interactive `gcloud auth login` (the single recurring operational blocker).

**Access & ownership** (a new operator needs grants for each): GCP project IAM,
the frontend GitHub repo (`RonTuretzky/gaskiller-onchain-llm`), the Cloudflare Worker
(`ronturetzky.workers.dev` account), and the `gaskiller.xyz` DNS zone all live under
Ron Turetzky's personal accounts — nothing is org-owned except the `gas-killer` GitHub
org repos. There is no shared credential store; ask Ron directly for each grant.

## 3. End-to-end request flow (when up)

1. **Browser** tokenizes the question locally (`qwen-bpe.js`: byte-level BPE, exact HF
   `tokenizers` parity, verified 30/30 token-for-token on 0.6B). Chat template
   `<|im_start|>user\nQ<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n`;
   0.6B prefix ids `[151644,872,198]`, 35B `[248045,846,198]`.
2. **POST `/bridge/ask`** `{model: "qwen"|"qwen35", prompt_ids, max_new}` → `{ask_id}`.
3. **Bridge → router :8081 `/shard/infer`** (internal only) with the full engine request
   (seg engine addr, manifest, packed config words, layer/dim/vocab/stop ids, stages=4,
   argmax_shards=2). The coordinator plans a (position × layer-range) segment DAG;
   each segment executes on a k=2-of-3 committee of the registered operator nodes via
   in-process overlay-mounted view calls (`Qwen3SegEngine.forwardRange/argmaxRange`);
   k byte-identical results required; per-segment digests fold into
   `pipeline_root = keccak(concat(chk))`. Wavefront/batched prefill; decode is serial.
4. **Bridge settles**: reads `stateTransitionCount()` (`0xf4833e20`) for the transition
   index, pins `block_height` at the **sim-fork's** head (`/forkhead`, fallback live−60),
   hand-encodes `fulfil(uint32[],uint256,uint32[],bytes32)` (`0x9c98c06e`), POSTs
   `/trigger` with `Authorization: Bearer $GK_KEY`.
5. **Operators re-verify** the segment commit chain for that pipelineRoot (node gate keys
   on the fulfil **selector**, verifies its own executed-segment digests, refuses to sign
   otherwise), BLS-sign the single-slot diff; router submits `verifyAndUpdate` — one tx.
   `ChatAnswered(uint256 indexed ti, bytes32 indexed newRoot, bytes32 indexed pipelineRoot,
   uint32[] promptIds, uint32[] answerIds)`, topic0
   `0x3d3288922fb750f4c301145bee4ae0c63a6229879e85c43079b8fb56fc81d187`.
6. **Frontend** polls the bridge status (live `segments_done/total` proxied from the
   router's `/shard/active`) plus the chain every ~12s; **completion ground truth is the
   chain** (`stateTransitionCount` increments), then it decodes the newest log and links
   the Etherscan tx.

Trust model: the k-of-N committee members are the registered, staked operators — their
agreement is the security (fraud-proof framing; not a trustless compute market). Sharded
speedup is bounded by registered operators (quorum `maxOperatorCount: 4`), not by VMs.
The operator gate binds by pipelineRoot only, not consumer.

**Single-slot commitment rule** (why consumer contracts look the way they do): the
unbounded sim profile admits at most **one** consumer SSTORE per settlement (plus the
exempt StateTracker slot `keccak("gasKiller.stateTracker")-1`); logs unlimited (128KB
transport cap); no CREATE; STATICCALLs are never extracted; a revert means nothing is
signed. `GK_SIM_PROFILE` must match across router+nodes or signatures diverge.

## 4. Sepolia contracts (chain 11155111)

**Demo settlement consumers (the ones that matter):**

| Role | Address | Notes |
|---|---|---|
| 0.6B sharded consumer `GasKillerChatSharded` | `0xd3f7F985F14f1942Fb09e5735e5499FEFF56E80b` | single-slot pre-resume deploy; frontend fromBlock 11256000 |
| 0.6B seg engine `Qwen3SegEngine` | `0x18C8b1677a731f7507ea51D99e23e513D9613Aa4` | overlay mode, manifest `0x23216cb9…c4a7ae9` |
| 35B sharded consumer `GasKillerChat35Sharded` | `0xfd0EF988216D0346BF115530387021c1b699336d` | single-slot pre-resume; fromBlock 11286700 |
| 35B seg engine `Qwen35SegEngine` | `0xcA459C95ee034D21339cd5ad7209441fD54bcd51` | manifest `0x7bdf4876…f01fa9` |
| 35B seg forward `Qwen35SegForward` | `0x5097fA57CdB792e188a086EB79d3Ef5DC495679b` | separate deploy STATICCALL'd by the engine (EIP-170); **fast-executor nodes MUST set `GK_FAST_VIEW_EXTRA_ACCOUNTS` to this address** or 35B reverts at ~2.35e11 gas |

Selectors on both sharded consumers: `fulfil` `0x9c98c06e`, `settlePrefix` `0x7e8de12c`,
`fulfilResumed` `0x6c4d43bc`, `stateTransitionCount` `0xf4833e20`, `settledRoots`
`0x56408a4f`. Monolithic `ask` `0xdf5b7e31`; stories `tellStory` `0x361e37b1`.

**Resume-line consumers** (prefix-cache settlement, issue #41 — proven live):
- 2-slot resume deploys that **cannot settle** under the single-slot rule (fulfil wrote
  chatRoot + settledRoots): 0.6B `0xb4Db9a9Ed60eeA6306881bFB71eaA90B96d8886e`, 35B
  `0x25d5e13f7cd467DD5d0CeE815cCc000735f9a627`. Do not point settlement at these.
- Post-fix single-slot resume (SDK `8b4f4cc`/`d44f65d`, drops the settledRoots store from
  fulfil): 0.6B `0x16a066E8bEcf278Df3D1B4377Ebe9f675D22bF05` (engine `0x6785256C…`,
  count 4 — carries the live resume proofs). Pre-resume fallback `0xBa75846F…`.
- Live proof txs: `settlePrefix` round 21s; resumed inference 368s (2.3× vs 852s full);
  `fulfilResumed` tx `0xb48fce97…` block 11292971; plain fulfil 16s. Roots byte-identical
  across 3 runs/14h (`0xa75e8338`).

**Monolithic + legacy:**

| Contract | Address |
|---|---|
| `GasKillerChat` (0.6B mono) / `Qwen3Engine` | `0x1d28318f8488633EA80285cA3BB25e7e1dADb886` / `0x3386985D9A964224D681929F57D8927701E07C14` |
| `GasKillerChat35` (35B mono) / `Qwen35Engine` / `Qwen35Forward` | `0xb7D8D5C215135Ac557Ea385eEA744764B8F03a66` / `0x1D9B339Ee353897A5B072844146F0F4414c64aBE` / `0xE9eD2E01852E6292eD07b18F70dE03D6f91d7561` |
| `GasKillerLLM` (stories, good redeploy) / original (wired to wrong checker) | `0x6cfae63A4F553Cd2b16895277f5bFde5F00e31bc` / `0xc35Db9E7765147a545861A5457240283fBCf3119` |
| `LlamaEngine` / chunk directory | `0xC6F8aD5622E43804114bd9e7d0c5FF006c6e6948` / `0xa5f46CcC10AC139822fe4df18F88CC04171c51fD` |
| Tenop stack (env torn down 07-17, contracts remain) | consumer `0x833c59D2…`, seg engine `0xEe2723cB…`, registryCoordinator `0x574369b8…` |

**AVS infra (default env):** serviceManager `0xdCec8ce0a03848B55989Bcc711e424Ca31d9eeD9`;
correct BLSSignatureChecker `0x7568336e17d3f52e0ba7a393f144ce16c8924ba5`
(`0xc3BEF9ec…` is a WRONG checker that reverts — do not wire consumers to it);
registryCoordinator `0x0a032D62…`. Ground truth for the live AVS addresses:
`kubectl exec <router-pod> -- cat /app/secrets/avs_deploy.json`. Every stack deploy
creates a fresh AVS — identify the live one via the newest consumer.

**Overlay scheme (how 34GB of weights are "on-chain"):** weights are split into
≤24,575-byte chunks; chunk *i* lives at the derived phantom address
`keccak("gaskiller.llm.overlay.v1" || manifest || u64be(i))[12:]`;
`manifest = keccak(keccak(weights) || keccak(tokenizer))` — a single 32-byte on-chain
commitment (9,400,000× compression for 35B). Operators mmap the real files
(`GK_OVERLAY_*` env) and the analyzer mounts them as EXTCODECOPY-able code. The manifest
is flat (no per-chunk Merkle) — lazy verification is impossible byte-safely; eager verify
costs ~9 min CPU hashing per daemon boot for 35B. Weight artifacts: GitHub releases
`qwen3-0.6b-onchain-v1` (597MB) and `qwen3.5-35b-a3b-onchain-v1` (19×1.9GB parts +
tokenizer; weights.bin 34,714,656,811 B) on `gas-killer/solidity-sdk`.

## 5. Repos, branches, working copies

| Repo | Branch | HEAD | vs main | Working copy |
|---|---|---|---|---|
| `gas-killer/solidity-sdk` | `RonTuretzky/onchain-solidity-llm` | `d44f65d` | +22 / −17 | `/Users/wk/conductor/workspaces/solidity-sdk/monterrey-v3` |
| `gas-killer/service` | `ron/sharded-inference` | `997228c` | +52 / −27 | `/Users/wk/conductor/service-sharded-wt` |
| `gas-killer/gas-analyzer` | `ron/local-execution` | `541c9e7` | single-branch clone | `/Users/wk/conductor/gas-analyzer-localexec` (dirty, 24 files) |
| `RonTuretzky/gaskiller-onchain-llm` (frontend) | `main` | `3cd5e05` | — | `…/monterrey-v3/.context/gh-pages-llm-chat` |

- **solidity-sdk** (PRs #56/#57): contracts in `src/examples/onchain-llm/` — consumers
  (GasKillerLLM/Chat/Chat35/ChatSharded/Chat35Sharded), engines (LlamaEngine, Qwen3Engine,
  Qwen35Engine+Qwen35Forward, Qwen3SegEngine, Qwen35SegEngine+Qwen35SegForward), kernels
  (Llama2, Qwen3, Qwen35, LlamaMath), `DataContractLib` (SSTORE2-style). Deploy scripts:
  `DeployOnchainLLM{,35,Sharded,35Sharded,ShardedOverlay}.s.sol`, `OperatorReplay.s.sol`;
  e2e drivers `script/e2e_{operator_replay,sharded_infer,weight_shard}.sh`. Docs:
  `src/examples/onchain-llm/{README,TESTNET,RESEARCH,UNBOUNDED_V2_OVERLAYS}.md`. Repo
  convention: **optimizer OFF** (EIP-170 margins are tight: Qwen35SegForward 23,204B).
- **service** (PR #321, verified superset of deployed pr-319): router shard coordinator +
  `/shard/active`, node validator gate, prefix-cache/resume, fast-executor dispatch,
  `bridge/`, `helm/gas-killer/` (+ `default-live-overrides.yaml`,
  `llm-overrides.yaml`, `shard-35b-overrides.yaml`, `tenop-0.6b-overrides.yaml`),
  `deploy/testnet-default/` (revival runbook), `deploy/tenop/` (10-op field notes),
  `docs/SHARDED_INFERENCE.md` (design), `.context/SUBMINUTE-35B-ROADMAP.md`.
- **gas-analyzer** (PR #172): `call_view_local_multi` in-process overlay view calls;
  `gk-fast-view` revmc AOT/JIT executor (persistent `--serve` daemon, per-engine JIT
  module pool, amortized overlay verify, signed-opcode differential tests).
- **Postmortem**: `gas-killer/service#326` (tenop learnings: 3 service resilience bugs +
  4 vendor registration bugs). **Roadmap decks** (presentation only):
  `/tmp/gk-roadmap-slides/index.html` (13 slides — bagelface PR reconciliation, Schnorr/
  slashing/perf integration, endgame from the GK spec) and `/tmp/gk-onetx-slides/index.html`.

Branch-reconciliation state vs bagelface's fortnight (details in the deck): service branch
is missing the task-lifecycle stack (#300→#324: POST /tasks, GET /tasks/{id}, startup
re-queue) and the consensus-aggregation merge (#299+#322, commit `d4e0bbf`); a structural
two-generation port is needed (validator.rs hand-merge, prewarm re-hang, requeue→
PrewarmSlot gap); the bridge should eventually migrate from `/trigger` to POST /tasks +
GET /tasks/{id}. Schnorr line: analyzer#164 + sdk#58 merged, service#323 CI-green awaiting
review.

## 6. GKE / GCP infrastructure (pre-teardown reference)

**Node pools** (as they existed; `sim-pool-c4` + `ops-highmem` now deleted by the cron):

| Pool | Machine | Purpose |
|---|---|---|
| `ops-highmem` | n2-highmem-8 ×1 (us-east4-b, 60Gi) | router + node-1/2/3 co-located (RWO PVC), 34GB weights mmap |
| `sim-pool-c4` | c4-highcpu-8 ×1, label `role=sim` | qwen-sim-fork anvil + gk-sim-proxy + overlay-ensurer |
| `default-pool` | e2-standard-4 | ingress, bridge, misc (shrunk to 1 by expiry) |
| `shard-workers` | Spot n2-highmem-4, autoscale 0–8 | idle — the 3 registered operators are the executors |

**Workloads** (helm release `gas-killer` rev 46 + tracked drift):
- Router: image `us-east4-docker.pkg.dev/gas-killer-testnet/gk-fast/router-live:v1`
  (adds `/shard/active`); `BLOCK_STALE_MEASURE=50000`; multi-overlay env sextet
  (`GK_OVERLAY_WEIGHTS/TOKENIZER/MANIFEST` = 35B slot 1, `*_2` = 0.6B slot 2, files on
  the shared PVC `/app/.nodes/qwen{35,06}/…`); shard config: k=2, operators=3, gas
  `8796093022208` (2^43, `unbounded-v1-xl`), alignStages, segmentTimeout 7200s, router
  gate consumer = 35B `0xfd0E…`.
- Nodes 1–3: image `…/gk-fast/node-fast:v6` (revmc; v4 per-engine JIT fix, v5 watchdog,
  v6 both); same overlay sextet; `GK_SHARD_URL=http://gas-killer-router:8081`,
  `GK_SHARD_CONSUMER=0xd3f7…` (0.6B — node gate intentionally differs from router gate),
  `GK_SHARD_NODE_CONCURRENCY=3`, `GK_SHARD_FAST_EXECUTOR=1`,
  `GK_FAST_VIEW_EXTRA_ACCOUNTS=0x5097fA57…`, `GK_SHARD_OPERATOR_ID` 0/1/2. Memory req
  12Gi / limit 50Gi (baked into values — see footguns). Rollback to interpreter =
  `GK_SHARD_FAST_EXECUTOR=0`.
- `gk-shard-bridge`: python:3.12-slim running `bridge.py` from configmap
  `gk-bridge-script`; secret `gk-bridge-key` → `GK_KEY`; svc+ingress `/bridge` on
  `testnet.gaskiller.xyz` and `8-228-87-183.nip.io`.
- Sim stack (`deploy/testnet-default/sim-fork-stack.yaml`): `qwen-sim-fork` — anvil :8547
  forking publicnode (`--code-size-limit 65536 --disable-block-gas-limit`, 5–7.5 cpu /
  4–12Gi); `gk-sim-proxy` :8545 — routes `debug_traceCall` for the pinned overlay
  consumers to the fork, everything else to the keyed upstream (secret `gk-sim-upstream`),
  serves `/forkhead`; `qwen-overlay-ensurer` — keeps the 0.6B overlay chunks
  `anvil_setCode`'d on the fork, reforks on >150-block drift only when `inflight==0`.
- CronJobs: `qwen-fork-refresh` (`*/8 * * * *`, anvil_reset to publicnode head — the
  permanent fork-staleness fix) and `demo-expiry` (tracked schedule now `0 12 26 7 *`,
  ships `suspend: true` deliberately; the live one fired today and self-suspended).

**RPC matrix** (all hard-won): operators/router sim against the fork
(`GK_SIM_RPC=http://qwen-sim-fork:8547`); the fork forks FROM publicnode (drpc's
`eth_getProof` is broken — settlement dies on it); helm `secrets.httpRpc` = drpc
(Alchemy key is over monthly quota); frontend reads via drpc → tenderly fallback
(publicnode 403s deep `eth_getLogs`).

**Helm rule:** NEVER `helm upgrade` the live release casually — `--reuse-values` resets
kubectl-patched memory/nodeSelector/probes and re-renders wipe the `/forkhead` ingress
(happened 3×). Live drift is codified in `helm/gas-killer/default-live-overrides.yaml`;
day-to-day changes go through `kubectl set image` / `kubectl set env`. The two hostname
nodeSelector pins in the overrides are FRAGILE — pool recreation renames the VM; re-check
before every apply.

**Images** live in Artifact Registry `us-east4-docker.pkg.dev/gas-killer-testnet/gk-fast/`
and were built with Cloud Build (the default compute SA needed
`cloudbuild.builds.builder` + `artifactregistry.writer`; `DOCKER_BUILDKIT=1` for cache
mounts). Sources: `router/Dockerfile` and `node/Dockerfile` on service
`ron/sharded-inference`; the fast images additionally bundle the `gk-fast-view` revmc
binary from gas-analyzer `ron/local-execution` @ `541c9e7` (`node-fast:v6` = Cloud Build
`b914e52e`; `router-live:v1` = the branch state that added `/shard/active`, `85a54d4`).
The exact `gcloud builds submit` invocations were ad-hoc and are not tracked — Cloud
Build history in the console is the record; rebuilds should reproduce from the
Dockerfiles + those commits.

## 7. The bridge (`bridge/bridge.py`, ~250 lines, stdlib-only)

- `POST /bridge/ask` `{model, prompt_ids, max_new}` → `{ask_id}` (uuid hex[:16]).
  Validation: `1 ≤ max_new ≤ cap` (qwen 24, qwen35 8), `0 < len(prompt) ≤ max_prompt`
  (992 / 56), `len(prompt)+max_new ≤ seq_cap` (1024 / 64), token ids < vocab
  (151,936 / 248,320). One in-flight ask per model (429 otherwise). Work runs in a
  daemon thread: `/shard/infer` (timeout 14,400s) → encode fulfil → `/trigger`.
- `GET /bridge/ask/<id>` → state `queued→inferring→settling→done|error` + timings; while
  inferring it merges `segments_done/segments_total/infer_elapsed_ms` from the router's
  `/shard/active` (matched by consumer address, best-effort, never stored).
- `GET /bridge/healthz` → `{ok, asks}`.
- Env: `ROUTER_SHARD` (:8081), `ROUTER_API` (:8080), `FORKHEAD_URL`
  (`http://gk-sim-proxy:8545/forkhead`), `GK_KEY` (required, from secret), `RPC_URL`,
  `FROM_ADDR` (`0x6636A1CC…D9F0` — this is only the task's simulated from-address; no
  signature is made with it and no private key is needed, so the fact that this
  address's key is burned (§9) is irrelevant here. The actual settlement tx is signed
  by the router's operator key).
- Every outbound request sends `User-Agent: gk-shard-bridge/1` — the ingress WAF and
  publicnode deterministically 403 the default Python-urllib UA.
- RPC fallback order: drpc → `$RPC_URL` → tenderly → publicnode, 2 tries each.
- **Deploy** (state lives in the cluster, not the pod image):
  `kubectl create configmap gk-bridge-script --from-file=bridge.py=bridge/bridge.py
  --dry-run=client -o yaml | kubectl apply -f -` then
  `kubectl rollout restart deploy/gk-shard-bridge`. **Pending right now**: the deployed
  configmap predates the token-config change, so it silently clamps `max_new` to 8.

## 8. The frontend

Single-page `index.html` (~1,090 lines) + `qwen-bpe.js` + per-model `tokenizer.json`
(0.6B 4.7MB, 35B 9.8MB, lazily fetched once per model).

- Constants: `ROUTER_URL`/`BRIDGE` = `https://testnet.gaskiller.xyz`; `ROUTER_RELAY` =
  the Cloudflare Worker; `rpcUrl()` = drpc with tenderly/rpc.sepolia.org fallbacks;
  per-model config `C.qwen` / `C.qwen35` (addr, ChatAnswered topic, fromBlock, gasPerTok
  28.6e9 / 3.6e12).
- Response-length selector `#maxtok`: `TOK_CHOICES = {qwen: [4,8,16,24], qwen35: [2,4,8]}`;
  `roundExpect(model, toks)` = 90s + 20s/tok (0.6B) or 30min + 5.6min/tok (35B);
  `roundMax` = 6× / 2.5× expect. The 35B path also enforces prompt+response ≤ 64 client-side.
- Round state machine: `startRound` persists `{model, ti, t0, askId, tokens}` to
  localStorage `gk-round` (survives reloads); tick every 3s (UI) with chain+bridge polls
  every ~12s; progress = real segment fraction when the bridge reports it, else synthetic
  creep; **done = chain counter > recorded ti**, then decode + Etherscan link; bridge
  `error` state or elapsed > roundMax stops the watcher.
- Page: animated CSS landing (pipeviz + 3 step diagrams, crowdstake-style, GK brand /
  bread-ui-kit language), model picker with LIVE/AWAITING/OFFLINE pills, chat, round
  card with segment progress, on-chain history (last 12 `ChatAnswered` logs, decoded
  in-browser), Bisection Proof Lab (client-side dispute-game toy), footer with the
  Sepolia contract addresses + PR links.
- **Edit workflow** (used for every deploy): edit → extract inline scripts and
  `node --check` them → verify every `$("id")`/`getElementById` has a matching `id=` in
  the HTML → commit + push to `main` → wait for Pages (curl with cache-buster). Note:
  localhost CDP screenshots show phantom black bands (stale composite tiles) — verify via
  DOM probes or the live site, don't chase them as layout bugs.

## 9. Secrets & key material (locations only — NEVER commit)

- `.context/tenop-archive/` in the sdk working copy (gitignored): tenop `setup.log`
  containing **all 10 operator private keys**, `register.log`, admin key, API key,
  `avs_deploy.json`.
- Cluster secrets: `gk-bridge-key` (router API key the bridge uses), `gk-sim-upstream`
  (keyed Alchemy RPC URL — scrubbed from the repo export; over monthly quota),
  `ghcr-pull-secret`. Router admin key lives in the router's secret mount
  (`/app/secrets/`); `/admin` API reachable only by port-forward.
- Deployer addresses: `0x6636A1CC…D9F0` (original; its key was pasted into a chat once →
  treat as BURNED, testnet-only) and `0x5DD2e7db…` (the router operator key — it signs
  and pays for every settlement tx; the key material lives in the router's secret mount
  on-cluster; ~0.5 ETH Sepolia left as of mid-July). To fund: send Sepolia ETH to
  `0x5DD2e7db…`; settlements cost ~350–384k gas each, so even heavy demo use burns
  little — but if it ever empties, drpc masks the underfunded error as an HTTP 500 on
  `sendRawTransaction` (footgun 8).
- The Claude memory directory for this project references some key values at specific
  file lines — treat it as sensitive.

## 10. Operational footguns (each cost hours — read before touching anything)

1. **gcloud auth expires ~daily**; kubectl+gcloud die with "cannot prompt during
   non-interactive execution" until an interactive `gcloud auth login`. This is what
   killed the expiry-cron extension.
2. **Never fire an inference into an unstable fleet.** Node restarts change pod IPs; the
   router keeps dead p2p peer connections and signatures never arrive; segment records
   are in-memory and the refusal path exits the process (crash loops). Recovery =
   coordinated whole-fleet restart, then wait until pod count == ready count == N,
   sustained across ~3 checks. Node rolls kill leased segments (1,800s timeout).
3. **Helm**: see §6 rule. `--reuse-values` also doesn't merge new values keys.
4. **RWO PVC** forces router+nodes onto one VM; rolls need force-delete of Terminating
   pods; restarts cost ~7 min each (34GB overlay re-init + eager manifest verify ~9 min
   for 35B daemons).
5. **cgroup v2 mmap accounting**: the 34GB shared mmap is charged to the first-faulting
   pod — every pod's memory **limit** must exceed 34GB (50Gi works; the 4Gi chart default
   OOMKills), while requests stay 12Gi so they schedule.
6. **35B must NOT use `overlay.enabled`** (chart initContainer downloads to per-pod
   emptyDir — fatal at 34GB); weights are pre-staged on the PVC + `GK_OVERLAY_*` env.
7. **Block-height pinning**: tasks must pin the sim-fork's head (`/forkhead`), not live
   head; `BLOCK_STALE_MEASURE=50000`; the `*/8` refork cron keeps the fork fresh. If the
   fork lacks a contract deployed after its fork point, `anvil_setCode` it in and restart
   nodes (they negative-cache empty code lookups).
8. **RPC quirks**: drpc `eth_getProof` broken; drpc masks underfunded-tx errors as
   HTTP-500; publicnode 403s the default Python UA and deep getLogs; Alchemy over quota.
9. **Liveness probes**: long synchronous revm traces starve the tokio runtime → SIGKILL;
   fixed via spawn_blocking + relaxed probe (timeout 5 / period 30 / failures 6) — keep
   that if probes are ever regenerated.
10. **Apple silicon**: linux/amd64 images under QEMU/Rosetta break the commonware p2p
    noise handshake (aws-lc-rs ChaCha20-Poly1305 miscomputes ≥512B) — build arm64
    locally; GKE unaffected.
11. **Registration** (fresh operator envs): `RegisterOperator.s.sol` is non-idempotent
    (SameMagnitude revert → `SKIP_ALLOCATION` patch); operators need ~0.003–0.005 ETH;
    register via publicnode; full gauntlet documented in `deploy/tenop/README.md` +
    service#326.
12. **Frontend/browser**: user networks may TLS-intercept the LB (use the Worker relay);
    forge drops leading zeros on config words (zfill to 64 hex chars when regenerating).

## 11. Performance & bit-exactness (measured, live)

- **0.6B sharded + fast executor**: ~4 min end-to-end for 8 tokens (bridge-measured
  ≈2 min inference + ~1 min settle); interpreter path 17.2 min; monolithic 25–49 min.
  Fast executor = 4.2× vs interpreter (warm decode 1.1s vs 8.4s/segment).
- **35B sharded**: 8 tokens = 4,423s / 4,292s (two runs) ≈ 72–75 min (monolithic ~95 min
  – 2h13m). The fast executor is ~1.08× on 35B — it is weight-bandwidth-bound
  (EXTCODECOPY over 34GB), not compute-bound. Decode's serial floor is ~4 min/token.
- **Resume (issue #41)**: warm-prefix `settlePrefix` then `fulfilResumed` — resumed
  inference 368s vs 852s full (2.3×); settlement rounds land in 16–31s on a fresh mesh.
- **Sub-minute 35B verdict** (task #36, open): unreachable EVM-exactly without a trust
  change; latency saturates at ~10–14 operators. Roadmap in
  `.context/SUBMINUTE-35B-ROADMAP.md` + the slide deck.
- **Bit-exactness chain**: Solidity ≡ integer Python reference ≡ float reference (greedy,
  temp 0); browser tokenizer ≡ HF; sharded ≡ monolithic; 10-op ≡ 3-op; revmc ≡
  interpreter (3 independent live confirmations); the live 35B answer ids matched
  `artifacts/vectors.json` token-for-token across ~3.6T simulated gas.
- **Tenop (10 operators)**: full 0.6B 852s vs 3-op 1,030s — single-ask gain is modest
  (~1.3–1.7× ceiling); the real win of more operators is concurrent committees.

## 12. Costs

List-price estimates (billing API needs auth): burn was **$26–30/day** — ops-highmem
n2-highmem-8 ~$15/day + c4 ~$8/day + default-pool + $2.40/day GKE fee + LB. July 1–20
≈ **$580**; project total since early June ≈ **$1,300 ±30%**. The expiry teardown (now
fired) cuts the burn ~75%; remaining: default-pool 1 node + LB + PVC storage + GKE
fee ≈ $6–8/day. Reviving restores the full burn while up.

## 13. Open work

1. **Revive the demo** (if wanted): runbook §0. Includes redeploying the token-config
   bridge and re-dating + unsuspending the expiry cron.
2. **Task #36**: 35B toward sub-minute (roadmap exists; needs trust-model decision).
3. **Bagelface reconciliation** (deck at `/tmp/gk-roadmap-slides/index.html`):
   two-generation port of `ron/sharded-inference` onto post-#299/#322 main; migrate the
   bridge to POST /tasks; validator.rs hand-merge; prewarm re-hang; requeue→PrewarmSlot.
4. **Schnorr line**: service#323 awaiting review; then the 250k→27k settlement floor.
5. **Slashing/challenger**: sp1-cc `gas-killer-challenger` branch (watcher daemon) —
   env_commitment binding into the SP1 guest is spec-only, pending.

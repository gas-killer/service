# GasKillerLLM — quick handoff

**What it is**: https://llm.gaskiller.xyz — chat with Qwen3.5-35B-A3B / Qwen3-0.6B running
as pure-Solidity inference. The Gas Killer operator quorum executes it off-chain as
hash-committed sharded segments (byte-exact, k=2-of-3), BLS-signs the one-slot diff, and
settles ONE ~380k-gas tx on Sepolia per answer. Visitors need no key or wallet.

**Where things live**
- Frontend: `RonTuretzky/gaskiller-onchain-llm` — GitHub Pages, push to `main` = deploy.
- Cluster: GKE `gas-killer`, us-east4, project `gas-killer-testnet`, ns `default`.
  gcloud auth expires ~daily → interactive `gcloud auth login` (the recurring blocker).
- Code: service `ron/sharded-inference` (bridge/, helm drift, runbooks, full
  `docs/HANDOFF.md`); gas-analyzer `ron/local-execution` (gk-fast-view revmc executor);
  solidity-sdk `RonTuretzky/onchain-solidity-llm` (contracts + deploy scripts).
- Images: `us-east4-docker.pkg.dev/gas-killer-testnet/gk-fast/{router-live:v1,node-fast:v6}`
  — recipes + rebuild script in `deploy/images/` (reproducible from pushed branches).
  Chart-default ghcr `pr-NNN` images are GitHub-built but overridden on the live fleet.

**Key contracts (Sepolia)**: 0.6B consumer `0xd3f7F985…E80b`, 35B `0xfd0EF988…336d`;
`fulfil` = `0x9c98c06e`; ChatAnswered topic `0x3d328892…d187`; counter `stateTransitionCount()`
`0xf4833e20`. Rule: at most ONE consumer SSTORE per settlement (single-slot commitment).

**Request flow**: browser BPE-tokenizes (HF-exact) → `POST /bridge/ask` → router `:8081
/shard/infer` (segment DAG over the operator committee, byte-identical results required)
→ bridge encodes `fulfil` + `POST /trigger` (Bearer key from secret `gk-bridge-key`) →
operators re-verify the commit chain, BLS-sign → `verifyAndUpdate` lands. Completion
ground truth = the on-chain counter, which the frontend polls.

**Operating rules**
1. NEVER `helm upgrade` casually. Live drift = `helm/gas-killer/default-live-overrides.yaml`
   (the two hostname pins must match the CURRENT ops-highmem VM). Day-to-day:
   `kubectl set image` / `set env` / patches.
2. Expiry cron `demo-expiry` is SUSPENDED (demo up indefinitely, ~$26–30/day; torn down
   ≈$6–8/day). To re-arm: set a FUTURE `schedule:` first, THEN `suspend: false`.
3. Bridge deploy = recreate configmap `gk-bridge-script` from `bridge/bridge.py` + rollout
   restart. It's stock python:3.12-slim — no image build.
4. First ask after any fleet restart pays ~15–20 min cold start (each node re-verifies the
   34GB overlay manifest). Never fire asks into an unstable fleet; if BLS signatures stop
   flowing after restarts, restart the WHOLE fleet together (p2p keeps dead peer conns).
5. Revival/teardown: `deploy/testnet-default/README.md` + `.context/gk-revive.sh`;
   everything else (all addresses, footguns, perf, history): `docs/HANDOFF.md`.

**Measured**: 0.6B ≈4 min per 8-token answer (~545B gas simulated); 35B ≈75 min
(~3.6T gas); settlement 350–384k gas. Answers byte-identical to the Python/HF reference.

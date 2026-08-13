# gk-shard-bridge

Public ask → **sharded** inference → single on-chain settlement. This is what
makes the llm.gaskiller.xyz demo's Qwen buttons drive the sharded path
end-to-end without a visitor API key.

```
POST /bridge/ask {"model":"qwen"|"qwen35","prompt_ids":[...],"max_new":8}
  -> {"ask_id"} immediately; a worker thread then:
     1. POSTs the router's internal shard coordinator (/shard/infer) — the
        inference runs as hash-committed segments across the operator committee;
     2. builds fulfil(promptIds,maxNew,answerIds,pipelineRoot) calldata and
        submits ONE settlement round via the router's public /trigger (the
        bridge holds the gk_ API key); the operator gate re-verifies the
        segment commit chain before signing and the quorum's verifyAndUpdate
        lands the answer on Sepolia.
GET /bridge/ask/<id>  -> queued | inferring | settling | done | error
GET /bridge/healthz
```

One inference per model at a time (429 while busy). Stdlib-only Python.

## Deploy

The script ships to the cluster as a ConfigMap mounted into a plain
`python:3.12-slim` pod:

```sh
kubectl create configmap gk-bridge-script --from-file=bridge.py=bridge/bridge.py \
  --dry-run=client -o yaml | kubectl apply -f -
kubectl rollout restart deploy/gk-shard-bridge
```

`k8s.yaml` holds the Deployment/Service/Ingress (path `/bridge` on
testnet.gaskiller.xyz) plus the `qwen-fork-refresh` CronJob that periodically
`anvil_reset`s the qwen-sim-fork to the current head (the fork-staleness fix).
The `GK_KEY` env comes from the `gk-bridge-key` secret (a router API key).

## Hard-won constraints (violate these and settlement dies silently)

- **`block_height` must be the sim-fork's head** (`gk-sim-proxy /forkhead`),
  not live head − 3: operators simulate against the fork, which lags the live
  chain until its periodic re-fork; a height the fork lacks kills the round
  with "block don't exists".
- **Send a real User-Agent**: the ingress WAF and publicnode both 403 the
  default `Python-urllib/*` UA deterministically.
- **Settlement consumers must be the single-store deploys** (0.6B
  `0xd3f7F985F14f1942Fb09e5735e5499FEFF56E80b`, 35B
  `0xfd0EF988216D0346BF115530387021c1b699336d`): the unbounded gas profile
  enforces at most one storage write per consumer, so the resume-capable
  consumers (0xb4Db / 0x25d5, two stores in fulfil) cannot settle until the
  profile allows the 2-slot pattern or the consumer packs one slot.

Measured live end-to-end (2026-07-16): 0.6B ≈2 min inference + ~1 min
settlement; 35B ≈72-74 min + ~1 min settlement.

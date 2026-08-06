# Local Test Scripts

This directory contains scripts for running local version of the BLS signature aggregation system.

## Running the Test Locally

### Prerequisites

- Docker
- Rust
- All submodules initialized (`git submodule update --init --recursive`)

### Run the Test

```bash
# From the project root
./scripts/run_e2e_test.sh
```

### Expected Output

```
currentSum: 0, Initial: 0, Elapsed: 0.0s
currentSum: 1352, Initial: 0, Elapsed: 10.0s
✅ SUCCESS: currentSum changed from 0 to 1352
✅ Array summation verified successfully - state was updated!
```

## Troubleshooting

### Common Issues

1. **Docker containers fail to start**
   - Check if ports 8545, 3333, 3334 are available
   - Ensure Docker daemon is running

2. **Contract deployment timeout**
   - Increase timeout in the script
   - Check Docker logs: `docker compose logs`

3. **Contributors fail to connect**
   - Verify keyfiles exist in `eigenlayer-bls-local/.nodes/operator_keys/`
   - Check network connectivity between processes

4. **Not Using Funded Private Key**
   - Ensure PRIVATE_KEY in .env has sufficient ETH for transactions
   - Check balance: `cast balance $(cast --from-utf8 $(cast --private-key $PRIVATE_KEY))`
   - Fund if needed: `cast send --private-key $PRIVATE_KEY --value 1ether <address>`

### Debug Information

The script creates detailed logs in the `logs/` directory:
- `orchestrator.log` - Main orchestrator output
- `contributor1.log`, `contributor2.log`, `contributor3.log` - Individual contributor logs

On test failure, recent log excerpts are displayed automatically.

### Manual Verification

You can also run the verification script separately:

```bash
# Start the system manually (follow README steps)
# Then run verification from the project root
source .env
cargo run -p scripts --bin send_request
```

## Running Scenarios

The `run_scenario` script runs a collection of requests against a live router in either serial or parallel mode, with optional on-chain verification after each request.

```bash
cargo run -p scripts --bin run_scenario -- scripts/scenarios/example.toml
```

Any string value in the config can reference an environment variable using `$VAR_NAME` syntax. The script loads `.env` automatically, so no manual sourcing is needed:

```toml
http_rpc = "$HTTP_RPC"
```

To run specific scenarios by name, pass `--scenarios` with a comma-separated list:

```bash
cargo run -p scripts --bin run_scenario -- scripts/scenarios/example.toml --scenarios smoke
cargo run -p scripts --bin run_scenario -- scripts/scenarios/example.toml --scenarios smoke,stress
```

An annotated example config lives at `scripts/scenarios/example.toml`.

### Config Reference

**Top-level fields**

| Field | Required | Default | Description |
|---|---|---|---|
| `router_url` | No | `http://localhost:8080` | Router endpoint |
| `http_rpc` | Conditional | — | Required when any request uses `block_height = 0`, `verify = true`, or `transition_index = "auto"` |

**`[[scenarios]]`**

| Field | Required | Default | Description |
|---|---|---|---|
| `name` | Yes | — | Label used in output |
| `mode` | No | `serial` | `serial` or `parallel` |
| `delay_between_ms` | No | `0` | Milliseconds between requests (serial only) |

**`[[scenarios.requests]]`**

| Field | Required | Default | Description |
|---|---|---|---|
| `label` | No | `request N` | Human-readable label for output |
| `submit` | No | `false` | Poll `GET /tasks/{id}` after a `202` and submit the rendered payload with `FUNDED_KEY` (or `PRIVATE_KEY`). Required for `verify` to mean anything when the router runs in ingress mode — see below |
| `target_address` | Yes | — | Contract address to call. Or `"kubectl"` to resolve it at runtime from the `gas-killer-smoke-target` ConfigMap via `kubectl` (also accepts `"kubectl:<configmap>[/<key>]"`; namespace follows the current kube-context unless `SMOKE_TARGET_NAMESPACE` is set). Or `"local"` to read `addresses.arraySummation` from the local deploy JSON at `AVS_DEPLOYMENT_PATH` (default `config/.nodes/avs_deploy.json`; `"local:<key>"` selects a different deployed contract) |
| `call_data` | Yes | — | ABI-encoded calldata as a `0x`-prefixed hex string |
| `from_address` | Yes | — | Sender address, or `"local"` to derive it from the `PRIVATE_KEY` the local stack signs with |
| `transition_index` | No | `"auto"` | State transition sequence number, or `"auto"` to fetch `stateTransitionCount()` from the contract (requires `http_rpc`) |
| `value` | No | `"0"` | Wei value as decimal or `0x`-prefixed hex string |
| `block_height` | No | `0` | Block to use; `0` auto-fetches current block via `http_rpc` |
| `verify` | No | `false` | Poll `stateTransitionCount()` after a `202` to confirm `verifyAndUpdate` ran |
| `verify_timeout_secs` | No | `150` | How long to wait for the payload to render and for on-chain confirmation |

### Why `submit` matters

With `INGRESS=true` (the default, see `example.env`) the router **renders** a `verifyAndUpdate`
transaction rather than broadcasting it: the quorum signs, the executor persists a payload, and
the *caller* signs and sends it. Nothing reaches the chain until someone does.

So `verify = true` on its own can only ever time out — it polls `stateTransitionCount()` for an
effect that no one has caused. Pair it with `submit = true`, which waits for the task to reach
`ready`, then submits `payload.{to, data, value}`. `submit` defaults to `false`, so scenario
files written before it existed behave exactly as they did.

## Example Contracts

The public [gas-killer/example-contracts](https://github.com/gas-killer/example-contracts)
library holds contracts that demonstrate different Gas Killer use cases. The `deploy_example`
binary builds them, deploys one wired to the local AVS, records its address where the rest of
the tooling already looks, and writes a ready-to-run scenario file.

One command, assuming the local stack is already up:

```bash
./scripts/examples/run_example.sh onchainLife
```

Or step by step:

```bash
# Clone/update the pinned revision, sync submodules recursively, forge build
./scripts/examples/fetch_examples.sh

# Validate the manifest and ABI encoding without a chain or a running stack
cargo run -p scripts --bin deploy_example -- --dry-run

# Deploy, assert the target is routable, run its setup calls, emit a scenario
cargo run -p scripts --bin deploy_example -- --example onchainLife

# Trigger a task, submit the rendered payload, confirm the transition landed
cargo run -p scripts --bin run_scenario -- scripts/scenarios/generated/onchainLife.toml
```

Currently available:

| Example | Status against the local stack |
|---|---|
| `guardedVault` | Runs green end to end. An O(N) invariant re-validated on every transition. |
| `onchainLife` | Deploys and is routable, but its `step(1)` task does **not** render — see below. Conway's Life; heavy compute, tiny flat diff. |

**`onchainLife` currently exceeds the trace-based diff encoder.** The router derives the diff via
`debug_traceCall`, and one generation of Life is ~16.9M gas. The structLog trace exhausts the
anvil container and the process is OOM-killed (exit 137), so the task never reaches `ready`:

```
ERROR gas_killer_router::sequencer: failed to enrich task, dropping request
  error=Gas analysis failed: debug_trace_call failed: error sending request
```

`generations` is already at its floor of 1, so this is a limit of the encoder rather than the
manifest. The example repo's `docs/APPROACH-A-PRESTATE.md` describes the intended fix — a
prestate diff extractor that avoids structLogs. The manifest entry is kept so the case stays
one command to reproduce.

### Adding an example

Edit `scripts/examples/examples.toml` and add the artifact to `EXPECTED_ARTIFACTS` in
`fetch_examples.sh`. No Rust changes: constructor argument *types* are read from the Foundry
artifact's ABI at runtime, and the manifest supplies only values.

```toml
[[examples]]
name      = "myExample"                       # key in avs_deploy.json; also --example and the scenario filename
artifact  = "MyExample.sol:MyExample"
ctor_args = ["$avs", "$sigChecker", "42"]

  [[examples.setup]]                          # optional; run after deploy
  sig    = "prepare(uint256)"
  args   = ["1"]
  signer = 1                                  # index into the manifest's `signers`

  [[examples.exercise]]                       # one generated scenario request each
  sig  = "doExpensiveThing(uint32)"
  args = ["1"]
```

Placeholders: `$avs`, `$sigChecker`, `$deploy:<key>`, `$signer:<n>`, `$env:VAR`. Array and
tuple parameters take a TOML list.

To be settleable, a contract must inherit `GasKillerSDK` and pass the AVS service manager plus
a signature checker to its constructor. `deploy_example` asserts this after deploying — it
checks the same ERC-165 interface the router gates on, plus `stateTransitionCount()` and
`getMessageHash()` — so a mis-wired or SDK-mismatched target fails at deploy time instead of
mid-round.

### Signature checker

`verifyAndUpdate` calls `checkSignatures` on whatever address the constructor received.
`addresses.blsSigCheck` in the AVS deployment JSON is the `BLSSigCheckOperatorStateRetriever`,
which has no such function — a target wired to it reverts with an empty `0x` at settlement.
`deploy_example` defaults `$sigChecker` to `addresses.IncredibleSquaringTaskManager` (which
does inherit `BLSSignatureChecker`), rejects the retriever by name, and probes for
`registryCoordinator()` before deploying. Override with `--sig-checker` or
`EXAMPLE_SIG_CHECKER_ADDRESS`.

### Deploying against a testnet

```bash
export AVS_DEPLOYMENT_PATH=config/.nodes/sepolia_deploy.json   # keeps local state intact
cargo run -p scripts --bin deploy_example -- \
  --example onchainLife \
  --avs 0x... --sig-checker 0x... \
  --router-url https://testnet.gaskiller.xyz
```

The deployment JSON is created if absent. `guardedVault`'s setup calls need several funded
accounts, so replace the manifest's `signers` list before using it outside a local fork.

### `deploy_example` flags

| Flag | Default | Description |
|---|---|---|
| `--manifest` | `scripts/examples/examples.toml` | Manifest to read |
| `--example` | all | Example to deploy, by `name`; repeatable |
| `--artifacts` | `$EXAMPLES_DIR/out` | Foundry `out/` tree to load artifacts from |
| `--deploy-json` | `$AVS_DEPLOYMENT_PATH` | Deployment JSON to read wiring from and record into |
| `--avs` / `--sig-checker` | from env, then the deployment JSON | Constructor wiring |
| `--router-url` | `$GAS_KILLER_ROUTER_URL` | Written into the generated scenario |
| `--scenario-dir` | `scripts/scenarios/generated` | Where scenarios are written |
| `--reuse` | off | Reuse the recorded address instead of deploying again |
| `--dry-run` | off | Resolve, encode, and emit scenarios without sending transactions |

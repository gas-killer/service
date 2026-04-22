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
| `target_address` | Yes | — | Contract address to call |
| `call_data` | Yes | — | ABI-encoded calldata as a `0x`-prefixed hex string |
| `from_address` | Yes | — | Sender address |
| `transition_index` | No | `"auto"` | State transition sequence number, or `"auto"` to fetch `stateTransitionCount()` from the contract (requires `http_rpc`) |
| `value` | No | `"0"` | Wei value as decimal or `0x`-prefixed hex string |
| `block_height` | No | `0` | Block to use; `0` auto-fetches current block via `http_rpc` |
| `verify` | No | `false` | Poll `stateTransitionCount()` after a `200` to confirm `verifyAndUpdate` ran |
| `verify_timeout_secs` | No | `150` | How long to wait for on-chain confirmation |

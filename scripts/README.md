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
./scripts/router_e2e_local.sh
```

### Expected Output

```
✅ SUCCESS: Counter was incremented 2 times (target: 2)
Total time elapsed: 95.3 seconds
✅ Integration test PASSED! Counter was incremented and ArraySummation was deployed successfully.
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
cargo run -p avs-scripts --bin verify_increments
```

# GitHub Actions Workflows

This directory contains CI/CD workflows for the Gas Killer Router project.

## Workflows

### 1. `gas-killer-e2e.yml` - Gas Killer End-to-End Test
**Purpose:** Complete end-to-end test of the Gas Killer functionality including ArraySummation deployment and task processing.

**Triggers:**
- Push to `main`, `dev`, or `staging` branches
- Pull requests to `main`, `dev`, or `staging`
- Manual workflow dispatch

**What it tests:**
1. ✅ Full Docker Compose stack startup
2. ✅ EigenLayer AVS deployment
3. ✅ ArraySummation contract deployment via factory
4. ✅ Gas Killer task submission via HTTP ingress
5. ✅ Task processing by orchestrator
6. ✅ BLS signature aggregation
7. ✅ Target address propagation through the system

**Runtime:** ~15-20 minutes

**Key features:**
- Reuses `scripts/router_e2e_local.sh` logic
- Deploys fresh ArraySummation instance
- Verifies debug logs for target address tracking
- Checks task acceptance, queueing, and processing

### 2. `integration-test.yml` - Counter Integration Test
**Purpose:** Tests the basic BLS signature aggregation with counter contract.

**Triggers:**
- Push to `main`, `dev`, or `staging` branches
- Pull requests to `main`, `dev`, or `staging`
- Manual workflow dispatch

**What it tests:**
1. ✅ Counter contract increments with default aggregation (30s)
2. ✅ Fast aggregation mode (0.5s) - only on `dev` branch pushes
3. ✅ HTTP ingress functionality - only on `dev` branch pushes

**Runtime:** ~5-10 minutes (basic), ~15 minutes (full on dev)

### 3. `rust-ci.yml` - Rust Code Quality
**Purpose:** Validates Rust code quality and formatting.

**Triggers:**
- Push and pull requests

**What it checks:**
- Cargo fmt (formatting)
- Cargo clippy (linting)
- Cargo build (compilation)

**Runtime:** ~5 minutes

### 4. `docker-image.yml` - Docker Image Build
**Purpose:** Builds and optionally publishes Docker images.

**Triggers:**
- Push to specific branches
- Release tags

**Runtime:** ~10 minutes

## Running Workflows Locally

### Option 1: Using the shell script (recommended)
```bash
# From project root
./scripts/router_e2e_local.sh
```

This runs the same steps as the Gas Killer E2E workflow but on your local machine.

### Option 2: Using act (GitHub Actions local runner)
```bash
# Install act: https://github.com/nektos/act

# Run a specific workflow
act -W .github/workflows/gas-killer-e2e.yml

# Note: Docker-in-Docker might have limitations with act
```

## Workflow Comparison

| Feature | gas-killer-e2e.yml | integration-test.yml |
|---------|-------------------|---------------------|
| **Tests Gas Killer** | ✅ Yes | ❌ No (Counter only) |
| **Deploys ArraySummation** | ✅ Yes | ❌ No |
| **Tests Ingress** | ✅ Always | ✅ Only on dev branch |
| **Runtime** | ~15-20 min | ~5-10 min |
| **Best for** | Gas Killer features | Quick smoke tests |

## Required Secrets

See [SECRETS.md](../SECRETS.md) for detailed information on required repository secrets:

- **GH_PAT** (required): GitHub Personal Access Token for submodule access
- **RPC_URL** (optional): Custom RPC endpoint for forking (defaults to public Holesky)

## Adding New Workflows

When creating new workflows:

1. Follow the naming convention: `<feature>-<type>.yml`
2. Add appropriate triggers (push, pull_request, workflow_dispatch)
3. Set reasonable timeouts (default is 360 minutes)
4. Use caching for Rust/Docker where possible
5. Always include cleanup step with `if: always()`
6. Add detailed logging for debugging failures
7. Update this README with the new workflow description

## Debugging Failed Workflows

### 1. Check the workflow run logs
- Go to Actions tab → Select the failed run
- Click on the failed job
- Expand failed steps to see detailed logs

### 2. Common failure points

**"Timeout waiting for EigenLayer"**
- Check eigenlayer container logs in the workflow output
- Verify AVS_DEPLOYMENT_PATH is being created
- Increase timeout if legitimate delay

**"ArraySummation deployment failed"**
- Check if factory address is correct in .env
- Verify PRIVATE_KEY has sufficient balance
- Check ethereum container logs

**"Task trigger failed"**
- Verify INGRESS=true in router environment
- Check router logs for HTTP server startup
- Confirm port 8080 is accessible

**"Authentication failed" or submodule errors**
- Verify GH_PAT secret exists and is valid
- Check PAT has `repo` scope
- Regenerate PAT if expired

### 3. Run locally for faster iteration
```bash
# Export secrets as environment variables
export GIT_AUTH_TOKEN="your-github-pat"

# Run the e2e script
./scripts/router_e2e_local.sh --keep-up

# Inspect services
docker compose ps
docker compose logs router
```

## Workflow Best Practices

1. **Use caching:** Cache Rust dependencies to speed up builds
2. **Parallel when possible:** Run independent steps concurrently
3. **Fail fast:** Set appropriate timeouts to avoid hanging
4. **Clean up always:** Use `if: always()` for cleanup steps
5. **Meaningful names:** Use descriptive step names
6. **Detailed logs:** Add echo statements for debugging
7. **Secrets safety:** Never log or expose secrets

## CI/CD Pipeline Flow

```
┌─────────────────────────────────────────────────────────────┐
│                     Push/PR to branch                        │
└──────────────────┬──────────────────────────────────────────┘
                   │
         ┌─────────┴─────────┬─────────────┬──────────────────┐
         │                   │             │                  │
         ▼                   ▼             ▼                  ▼
  ┌─────────────┐   ┌──────────────┐  ┌────────┐   ┌──────────────┐
  │  rust-ci    │   │ integration  │  │ docker │   │ gas-killer   │
  │  (fast)     │   │ -test (med)  │  │ -image │   │ -e2e (slow)  │
  └─────────────┘   └──────────────┘  └────────┘   └──────────────┘
         │                   │             │                  │
         └─────────┬─────────┴─────────────┴──────────────────┘
                   │
                   ▼
           All checks pass ✅
                   │
                   ▼
           Ready to merge 🚀
```

## Performance Optimization

Current workflow optimization strategies:

1. **Caching:** Rust dependencies cached per Cargo.lock hash
2. **Docker layer caching:** BuildKit with layer caching enabled
3. **Parallel jobs:** Workflows run in parallel when possible
4. **Conditional steps:** Some tests only run on specific branches
5. **Shared Docker images:** Reuse pre-built images from GHCR

**Benchmark times (approximate):**
- rust-ci: ~3-5 minutes
- integration-test (basic): ~5-8 minutes
- integration-test (full): ~12-15 minutes
- gas-killer-e2e: ~15-20 minutes
- docker-image: ~8-12 minutes

## Future Improvements

Potential enhancements for the CI/CD pipeline:

- [ ] Add test coverage reporting
- [ ] Implement smoke tests for quick validation
- [ ] Add performance benchmarking workflow
- [ ] Create deployment workflows for staging/production
- [ ] Add automatic dependency updates (Dependabot/Renovate)
- [ ] Implement canary deployments
- [ ] Add security scanning (Snyk, Trivy)
- [ ] Create workflow for contract verification on Etherscan

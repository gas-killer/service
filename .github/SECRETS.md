# GitHub Secrets Configuration

This document lists the repository secrets required for the CI/CD workflows.

## Required Secrets

### 1. `GH_PAT` (GitHub Personal Access Token)
**Required:** Yes
**Used in:** All workflows that checkout submodules
**Purpose:** Authenticate to GitHub when checking out private submodule repositories

**How to create:**
1. Go to GitHub Settings → Developer settings → Personal access tokens → Tokens (classic)
2. Click "Generate new token (classic)"
3. Name: `CI Access Token` or similar
4. Expiration: Set according to your security policy (90 days recommended)
5. Select scopes:
   - `repo` (Full control of private repositories)
   - `read:packages` (Read packages)
6. Click "Generate token"
7. Copy the token immediately (you won't see it again)

**How to add to repository:**
1. Go to Repository Settings → Secrets and variables → Actions
2. Click "New repository secret"
3. Name: `GH_PAT`
4. Value: Paste your personal access token
5. Click "Add secret"

### 2. `RPC_URL` (Optional RPC Endpoint)
**Required:** No (optional)
**Used in:** Integration and E2E tests
**Purpose:** Provide a custom RPC endpoint for forking Holesky testnet

**Default if not set:** `https://holesky.drpc.org` (public endpoint)

**When to use:**
- If you want faster/more reliable RPC access during tests
- If you have rate limits on public endpoints
- If you want to use a private RPC provider (Alchemy, Infura, etc.)

**How to add:**
1. Get an RPC URL from your provider (e.g., Alchemy: `https://eth-holesky.g.alchemy.com/v2/YOUR-API-KEY`)
2. Go to Repository Settings → Secrets and variables → Actions
3. Click "New repository secret"
4. Name: `RPC_URL`
5. Value: Your RPC endpoint URL
6. Click "Add secret"

## Secrets Summary Table

| Secret Name | Required | Default/Fallback | Workflows Using It |
|-------------|----------|------------------|-------------------|
| `GH_PAT` | ✅ Yes | None | integration-test.yml, gas-killer-e2e.yml |
| `RPC_URL` | ❌ No | `https://holesky.drpc.org` | integration-test.yml, gas-killer-e2e.yml |

## Testing Secrets Configuration

To verify your secrets are configured correctly:

1. **Test the workflow manually:**
   ```bash
   # Go to Actions tab in GitHub
   # Select "Gas Killer E2E Test" workflow
   # Click "Run workflow"
   # Select branch and click "Run workflow"
   ```

2. **Check for common errors:**
   - **Authentication errors:** Usually means `GH_PAT` is missing or expired
   - **Submodule checkout failures:** Check `GH_PAT` has correct scopes
   - **RPC timeout errors:** Consider adding `RPC_URL` with a private endpoint

## Security Best Practices

1. **Rotate tokens regularly:** Update `GH_PAT` every 90 days
2. **Use minimal scopes:** Only grant necessary permissions
3. **Use organization secrets:** If working in an org, consider org-level secrets
4. **Monitor usage:** Check Actions logs for any suspicious activity
5. **Never commit secrets:** Secrets are handled by GitHub Actions, never in code

## Troubleshooting

### "fatal: could not read Username" or submodule errors
- **Cause:** `GH_PAT` is missing or has insufficient permissions
- **Fix:** Ensure `GH_PAT` exists and has `repo` scope

### "rate limit exceeded" during tests
- **Cause:** Using public RPC endpoint with rate limits
- **Fix:** Add `RPC_URL` secret with a private RPC endpoint

### "Permission denied (publickey)" during Docker build
- **Cause:** Docker build secret not passed correctly
- **Fix:** Verify workflow passes `GH_PAT` as `GIT_AUTH_TOKEN` to Docker build

## Additional Notes

- **GITHUB_TOKEN:** This is automatically provided by GitHub Actions and doesn't need to be configured
- **Workflow permissions:** Workflows have `contents: read` and `packages: read` permissions by default
- **Environment variables vs Secrets:** Use secrets for sensitive data, environment variables for non-sensitive configuration

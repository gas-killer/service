# Gas Killer Helm Chart

A Helm chart for deploying the Gas Killer Router AVS with EigenLayer integration.

## Prerequisites

- Kubernetes 1.19+
- Helm 3.2+
- PV provisioner support in the underlying infrastructure (for shared data volume)

## Installation

```bash
helm install gas-killer ./helm \
  --set secrets.forkUrl="https://your-rpc-url" \
  --set secrets.privateKey="0x..." \
  --set secrets.fundedKey="0x..."
```

## Important Gotchas

### Setup Job Behavior

The setup job (`helm.sh/hook: post-install`) only runs on fresh installs, NOT on upgrades. This means:

- **Operator registration changes require reinstall**: If you modify operator configuration (number of nodes, keys, etc.), running `helm upgrade` will NOT re-register operators. You must uninstall and reinstall the chart, or manually run the setup.

- **To re-run setup after changes**:
  ```bash
  # Option 1: Uninstall and reinstall
  helm uninstall gas-killer
  helm install gas-killer ./helm --set ...

  # Option 2: Delete the job and PVC, then upgrade
  kubectl delete job gas-killer-setup
  kubectl delete pvc gas-killer-shared-data
  helm upgrade gas-killer ./helm --set ...
  ```

### DNS Label Length Limits

Kubernetes DNS labels are limited to 63 characters. If your release name is long, resource names may be truncated. The chart handles this automatically, but be aware that very long release names combined with component suffixes may result in truncated names.

### Priority Classes

The Ethereum (Anvil) pod uses `system-cluster-critical` priority class to ensure it stays running, as it holds critical blockchain state. Consider creating a custom priority class if you don't want to use system-reserved classes:

```yaml
apiVersion: scheduling.k8s.io/v1
kind: PriorityClass
metadata:
  name: gas-killer-critical
value: 1000000
globalDefault: false
description: "Priority class for Gas Killer critical components"
```

Then set in values:
```yaml
ethereum:
  priorityClassName: gas-killer-critical
```

### Node Readiness

The current node readiness probe checks if the `gas-killer` process is running. For production deployments, consider implementing a proper health/readiness endpoint in the node application that verifies:
- Connection to Ethereum RPC
- BLS key loaded
- P2P network connectivity

### Init Container Timeouts

All init containers have a configurable timeout (default: 300 seconds). If your setup takes longer (e.g., slow RPC, large state), increase the timeout:

```bash
helm install gas-killer ./helm \
  --set global.initTimeout=600
```

## Configuration

See `values.yaml` for all available configuration options.

### Key Configuration Options

| Parameter | Description | Default |
|-----------|-------------|---------|
| `global.environment` | Environment mode (LOCAL or TESTNET) | `LOCAL` |
| `global.nodeCount` | Number of operator nodes | `3` |
| `global.initTimeout` | Init container timeout in seconds | `300` |
| `secrets.forkUrl` | Anvil fork URL (required for LOCAL mode) | `""` |
| `secrets.privateKey` | Deployer private key | `""` |
| `secrets.fundedKey` | Funded account private key | `""` |

## Architecture

The chart deploys the following components:

1. **Ethereum (Anvil)** - Local blockchain with forked Sepolia state
2. **Signer (Cerberus)** - BLS signature service
3. **Setup Job** - EigenLayer contract deployment and operator registration
4. **Gas Killer Nodes** - Operator nodes (configurable count)
5. **Router** - Request routing and aggregation

### Startup Order

Components start in a specific order enforced by init containers:

1. Ethereum pod starts first
2. Setup job waits for Ethereum, then deploys contracts and registers operators
3. Signer waits for setup completion (needs operator keys)
4. Nodes wait for setup completion and Ethereum availability
5. Router waits for setup, Ethereum, and all nodes

## Troubleshooting

### Pods stuck in Init state

Check init container logs:
```bash
kubectl logs <pod-name> -c wait-for-setup
kubectl logs <pod-name> -c wait-for-ethereum
```

### Setup job failed

Check setup job logs:
```bash
kubectl logs job/gas-killer-setup
```

### Shared data issues

Verify PVC is bound:
```bash
kubectl get pvc gas-killer-shared-data
kubectl describe pvc gas-killer-shared-data
```

# Helm Chart

A Helm chart for deploying the Gas Killer Router AVS with EigenLayer integration.

## Prerequisites

- Kubernetes 1.19+
- Helm 3.2+
- PV provisioner support in the underlying infrastructure (for shared data volume)

## Installation

Fetch chart dependencies before installing:

```bash
helm dependency update ./helm/gas-killer
```

Then install:

```bash
helm install gas-killer ./helm/gas-killer \
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
  helm install gas-killer ./helm/gas-killer --set ...

  # Option 2: Delete the job and PVC, then upgrade
  kubectl delete job gas-killer-setup
  kubectl delete pvc gas-killer-shared-data
  helm upgrade gas-killer ./helm/gas-killer --set ...
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
helm install gas-killer ./helm/gas-killer \
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

## HTTPS / TLS Ingress

To expose the router ingress over HTTPS on a public domain, use the nginx-ingress
controller with cert-manager for automated Let's Encrypt certificates.

### One-time cluster setup

**1. Install nginx-ingress:**
```bash
helm repo add ingress-nginx https://kubernetes.github.io/ingress-nginx
helm install ingress-nginx ingress-nginx/ingress-nginx
```

**2. Install cert-manager:**
```bash
helm repo add jetstack https://charts.jetstack.io
helm install cert-manager jetstack/cert-manager --set crds.enabled=true
```

**3. Create a Let's Encrypt ClusterIssuer** (substitute your email):
```bash
kubectl apply -f - <<EOF
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-prod
spec:
  acme:
    server: https://acme-v02.api.letsencrypt.org/directory
    email: dev@gaskiller.xyz
    privateKeySecretRef:
      name: letsencrypt-prod
    solvers:
      - http01:
          ingress:
            class: nginx
EOF
```

**4. Get the LoadBalancer IP** assigned to the nginx-ingress controller:
```bash
kubectl get svc ingress-nginx-controller \
  -o jsonpath='{.status.loadBalancer.ingress[0].ip}'
```

**5. Create a DNS A-record** pointing your domain at that IP.

### Deploy with TLS

Enable ingress and pass your hostnames at install/upgrade time:

```bash
helm upgrade --install gas-killer ./helm/gas-killer \
  --set ingress.enabled=true \
  --set ingress.host=testnet.gaskiller.xyz \
  --set monitoring.grafana.ingress.enabled=true \
  --set monitoring.grafana.ingress.host=grafana-testnet.gaskiller.xyz \
  --set kube-prometheus-stack.grafana.adminPassword="..." \
  --set secrets.privateKey="0x..." \
  ...
```

Both ingresses default to `nginx` as the ingress class, cert-manager's `letsencrypt-prod`
cluster issuer, and `gaskiller-tls` / `grafana-tls` as their TLS secret names respectively.
Override any of these with `--set ingress.tlsSecretName=...`,
`--set monitoring.grafana.ingress.tlsSecretName=...`, etc.

cert-manager will automatically provision the TLS certificates. The nginx-ingress
controller handles HTTP → HTTPS redirects automatically.

## Monitoring (Prometheus + Grafana)

Metrics are exposed at `/metrics` on port 8081 of the router and node pods. The monitoring stack
(Prometheus Operator, Grafana, AlertManager) is deployed as a subchart and is off by default.

### One-time cluster setup

The Prometheus Operator CRDs must exist in the cluster before the chart can create
`ServiceMonitor`, `Prometheus`, `Alertmanager`, and `PrometheusRule` resources. This only needs
to be done once per cluster.

**1. Fetch chart dependencies** (if not already done):
```bash
helm dependency update ./helm/gas-killer
```

**2. Install Prometheus Operator CRDs:**
```bash
helm show crds helm/gas-killer/charts/kube-prometheus-stack-*.tgz | kubectl apply --server-side -f -
```

**3. Wait for CRDs to be registered** before running the helm upgrade:
```bash
kubectl wait --for=condition=Established \
  crd/prometheuses.monitoring.coreos.com \
  crd/servicemonitors.monitoring.coreos.com \
  crd/prometheusrules.monitoring.coreos.com \
  crd/alertmanagers.monitoring.coreos.com \
  --timeout=30s
```

**4. Create a DNS A-record** pointing `grafana-testnet.gaskiller.xyz` at the nginx-ingress
LoadBalancer IP (same IP used for the router ingress):
```bash
kubectl get svc ingress-nginx-controller \
  -o jsonpath='{.status.loadBalancer.ingress[0].ip}'
```

### Deploy with monitoring enabled

An example override file is provided at `helm/gas-killer/testnet-monitoring-overrides.yaml`.

```bash
helm upgrade --install gas-killer ./helm/gas-killer \
  -f helm/gas-killer/testnet-overrides.yaml \
  -f helm/gas-killer/testnet-ingress-overrides.yaml \
  -f helm/gas-killer/testnet-monitoring-overrides.yaml \
  --set secrets.privateKey=0x... \
  --set secrets.fundedKey=0x... \
  --set secrets.httpRpc=https://... \
  --set secrets.l2HttpRpc=https://... \
  --set router.image.tag=router-<sha> \
  --set node.image.tag=node-<sha> \
  --set kube-prometheus-stack.grafana.adminPassword=<password>
```

### Accessing Grafana

Once deployed, Grafana is available at `https://grafana-testnet.gaskiller.xyz` (if the ingress
is enabled and DNS is configured), or via port-forward:

```bash
kubectl port-forward svc/gas-killer-grafana 3000:80
```

Then open `http://localhost:3000` and log in with username `admin` and the password you set.

The **Gas Killer** dashboard is pre-loaded automatically via the Grafana sidecar. It includes:
- Router and node up/down status
- Pod restart counts
- CPU and memory usage per pod
- Placeholder panels for aggregation and ingress metrics (populated once custom metrics are instrumented)

### Verifying scrape targets

Port-forward the Prometheus UI and check that all targets show as `UP`:

```bash
kubectl port-forward svc/gas-killer-kube-prometheus-prometheus 9090:9090
```

Then open `http://localhost:9090/targets`.

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

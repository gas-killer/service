# Gas Killer — GCP Infrastructure

Terraform configuration for deploying gas-killer to GKE. Provisions a VPC, GKE cluster, and
GCP Service Account with Workload Identity so pods can authenticate to GCP Secret Manager
without static credentials.

## Prerequisites

- [Terraform](https://developer.hashicorp.com/terraform/install) >= 1.0
- [gcloud CLI](https://cloud.google.com/sdk/docs/install) authenticated (`gcloud auth login && gcloud auth application-default login`)
- A GCP project with billing enabled

## First-time setup

```bash
cd terraform

cp terraform.tfvars.example terraform.tfvars
# Edit terraform.tfvars — set project_id at minimum.
# For testnet/dev, also set:
#   deletion_protection = false
#   spot_nodes          = true

terraform init
terraform plan
terraform apply
```

After `apply` completes, configure `kubectl`:

```bash
$(terraform output -raw kubeconfig_command)
```

Copy the service account email into your Helm values:

```bash
terraform output gcp_service_account_email
# → gas-killer-app@your-project.iam.gserviceaccount.com
```

## Deploying the Helm chart

```bash
helm install gas-killer ./helm/gas-killer \
  --set global.environment=TESTNET \
  --set secretManager.enabled=true \
  --set secretManager.projectId=your-gcp-project-id \
  --set secretManager.gcpServiceAccount=$(terraform output -raw gcp_service_account_email) \
  --set secrets.httpRpc=https://eth-sepolia.g.alchemy.com/v2/YOUR_KEY \
  --set secrets.privateKey=0x... \
  --set secrets.fundedKey=0x... \
  --set node.image.tag=<tag> \
  --set router.image.tag=<tag>
```

On install, Helm runs three hook jobs in order:

| Weight | Job | What it does |
|--------|-----|--------------|
| 0 | `setup` | Deploys AVS contracts, generates operator BLS keys, registers operators on-chain. Writes all output to the shared PVC. |
| 1 | `key-export` | Reads generated keys from the PVC and writes them to GCP Secret Manager. Idempotent — safe to re-run. |
| 10 | `bridge` | Bridges EigenLayer operator state from L1 to L2. |

Node and router pods start only after the bridge job completes, fetching their keys directly
from Secret Manager via Workload Identity.

## Key management

### Where keys live

| Secret name | Contents |
|-------------|----------|
| `gas-killer-avs-deploy` | `avs_deploy.json` — contract addresses |
| `gas-killer-node-1-bls-key` | BLS private key for operator node 1 |
| `gas-killer-node-2-bls-key` | BLS private key for operator node 2 |
| `gas-killer-node-N-bls-key` | … |

These names use the default `secretManager.keyPrefix = "gas-killer"`. If you deploy multiple
environments into the same GCP project, set a distinct prefix per environment
(`--set secretManager.keyPrefix=gas-killer-staging`).

### Initial provisioning

Keys are generated and exported automatically on `helm install`. No manual steps required.
To verify secrets were created:

```bash
gcloud secrets list --project=your-gcp-project-id --filter="name~gas-killer"
```

### Rotation

Operator BLS keys are registered on-chain. Rotation requires re-registering operators, which
means running a fresh setup. The process is:

1. Delete the `.setup_complete` marker so the setup job will re-run:

   ```bash
   kubectl run pvc-reset --rm -it --restart=Never \
     --image=busybox:1.36 \
     --overrides='{"spec":{"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":"gas-killer-shared-data"}}],"containers":[{"name":"pvc-reset","image":"busybox:1.36","command":["rm","-f","/data/.setup_complete"],"volumeMounts":[{"name":"data","mountPath":"/data"}]}]}}'
   ```

2. Re-run the Helm hooks:

   ```bash
   helm upgrade gas-killer ./helm/gas-killer [same flags as install]
   ```

   The setup job will re-deploy contracts, generate new keys, and re-register operators.
   The key-export job will add new versions of each secret in Secret Manager.
   Node and router pods will restart and fetch the new key versions (`latest`).

3. Verify nodes are healthy:

   ```bash
   kubectl get pods
   kubectl logs -l app.kubernetes.io/component=node --tail=50
   ```

> **Note:** GCP Secret Manager retains all previous versions. Old versions are not
> automatically deleted but are never fetched (pods always request `latest`). To clean up:
> `gcloud secrets versions destroy <version> --secret=gas-killer-node-1-bls-key --project=...`

### Inspecting the shared PVC

The setup job writes keys and logs to a PVC. To inspect it while no setup pod is running:

```bash
kubectl run pvc-inspect --rm -it --restart=Never \
  --image=busybox:1.36 \
  --overrides='{"spec":{"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":"gas-killer-shared-data"}}],"containers":[{"name":"pvc-inspect","image":"busybox:1.36","command":["sh"],"stdin":true,"tty":true,"volumeMounts":[{"name":"data","mountPath":"/data"}]}]}}'
```

Useful files inside:

| Path | Contents |
|------|----------|
| `/data/.setup_complete` | Marker file — exists if setup succeeded |
| `/data/avs_deploy.json` | Deployed contract addresses |
| `/data/operator_keys/` | Generated ECDSA and BLS key files |
| `/data/setup.log` | Full log output from the setup job |

## Tearing down

```bash
# Remove the Helm release first (deletes pods, PVC, jobs)
helm uninstall gas-killer

# If deletion_protection = false:
terraform destroy
```

If `deletion_protection = true` (the production default), you must disable it before destroy:

```bash
terraform apply -var="deletion_protection=false"
terraform destroy
```

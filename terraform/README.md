# Gas Killer Router - Terraform AWS Deployment

Deploy Gas Killer Router to AWS EKS using Terraform.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                          AWS Cloud                              │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                         VPC                                │  │
│  │  ┌─────────────┐    ┌─────────────────────────────────┐   │  │
│  │  │   Public    │    │         Private Subnets          │   │  │
│  │  │  Subnets    │    │  ┌─────────────────────────────┐ │   │  │
│  │  │             │    │  │      EKS Node Group         │ │   │  │
│  │  │  ┌───────┐  │    │  │  ┌────────┐ ┌────────────┐  │ │   │  │
│  │  │  │  ALB  │──┼────┼──│  │ Router │ │  Nodes 1-3 │  │ │   │  │
│  │  │  └───────┘  │    │  │  └────────┘ └────────────┘  │ │   │  │
│  │  │             │    │  │  ┌────────┐ ┌────────────┐  │ │   │  │
│  │  │  ┌───────┐  │    │  │  │ Signer │ │  Ethereum  │  │ │   │  │
│  │  │  │  NAT  │──┼────┼──│  └────────┘ └────────────┘  │ │   │  │
│  │  │  └───────┘  │    │  └─────────────────────────────┘ │   │  │
│  │  └─────────────┘    └─────────────────────────────────┘   │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Prerequisites

1. **Terraform** >= 1.5.0
2. **AWS CLI** v2, configured with credentials
3. **kubectl** for cluster access
4. **AWS IAM permissions**: EKS, EC2, VPC, IAM, ELB

## Quick Start

```bash
# 1. Navigate to dev environment
cd terraform/environments/dev

# 2. Initialize Terraform
terraform init

# 3. Create configuration file
cp terraform.tfvars.example terraform.tfvars

# 4. Edit terraform.tfvars with your secrets
#    - fork_url: Sepolia RPC URL (e.g., Alchemy, Infura)
#    - private_key: Deployer private key
#    - funded_key: Funded account key

# 5. Plan the deployment
terraform plan

# 6. Apply (creates all resources)
terraform apply

# 7. Configure kubectl
$(terraform output -raw kubeconfig_command)

# 8. Check deployment status
kubectl get pods -n gas-killer

# 9. Get router URL (wait for ALB to be ready)
terraform output router_url
```

## Configuration

### Environment Variables (Secrets)

You can pass secrets via environment variables instead of tfvars:

```bash
export TF_VAR_fork_url="https://eth-sepolia.g.alchemy.com/v2/YOUR_KEY"
export TF_VAR_private_key="your_private_key_without_0x"
export TF_VAR_funded_key="your_funded_key_without_0x"

terraform apply
```

### Key Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `aws_region` | AWS region | `us-east-1` |
| `environment_mode` | `LOCAL` (Anvil) or `TESTNET` (Sepolia) | `LOCAL` |
| `node_count` | Number of operator nodes | `3` |
| `node_instance_types` | EC2 instance types | `["t3.medium"]` |
| `node_desired_size` | Number of EKS nodes | `2` |
| `enable_ingress` | Enable ALB ingress | `true` |

### LOCAL vs TESTNET Mode

**LOCAL Mode** (default):
- Deploys Anvil pod that forks Sepolia
- Runs setup job to deploy contracts
- Self-contained environment
- Requires `fork_url` (Sepolia RPC to fork from)

**TESTNET Mode**:
- Connects directly to Sepolia
- No Anvil pod
- Requires pre-deployed contracts
- Requires funded account with Sepolia ETH

## Directory Structure

```
terraform/
├── README.md                    # This file
├── environments/
│   └── dev/                     # Development environment
│       ├── main.tf              # Main configuration
│       ├── variables.tf         # Variable definitions
│       ├── outputs.tf           # Output definitions
│       ├── backend.tf           # Optional S3 backend
│       └── terraform.tfvars.example
└── modules/
    ├── vpc/                     # VPC, subnets, NAT
    ├── eks/                     # EKS cluster, node groups
    ├── eks-addons/              # EBS CSI, ALB controller
    └── gas-killer/              # Helm deployment
```

## Outputs

After `terraform apply`, useful outputs include:

```bash
# Get all outputs
terraform output

# Specific outputs
terraform output kubeconfig_command   # kubectl configuration
terraform output router_url           # Router HTTP URL
terraform output helpful_commands     # Useful kubectl commands
```

## Cost Estimate

| Resource | Monthly Cost |
|----------|-------------|
| EKS Cluster | ~$73 |
| NAT Gateway | ~$32 |
| EC2 (2x t3.medium) | ~$60 |
| EBS Volumes | ~$10 |
| ALB | ~$16 |
| **Total** | **~$190-250** |

### Cost Optimization

1. **Destroy when not in use**: `terraform destroy`
2. **Use spot instances**: Modify node group config
3. **Reduce node count**: `node_desired_size = 1`

## Troubleshooting

### Pods not starting

```bash
# Check pod status
kubectl get pods -n gas-killer

# Check events
kubectl get events -n gas-killer --sort-by='.lastTimestamp'

# Check logs
kubectl logs -n gas-killer -l app.kubernetes.io/component=router
```

### ALB not created

```bash
# Check ingress status
kubectl get ingress -n gas-killer

# Check ALB controller logs
kubectl logs -n kube-system -l app.kubernetes.io/name=aws-load-balancer-controller
```

### Helm deployment failed

```bash
# Check Helm release status
helm list -n gas-killer

# Get release history
helm history gas-killer -n gas-killer
```

## Cleanup

```bash
# Destroy all resources
cd terraform/environments/dev
terraform destroy
```

This removes:
- EKS cluster and node groups
- VPC and all networking
- ALB and target groups
- All IAM roles created by Terraform

## IAM Access

See [docs/eks-iam-access.md](../docs/eks-iam-access.md) for instructions on granting team members access to the EKS cluster.

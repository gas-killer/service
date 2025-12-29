# AWS Prerequisites for Gas Killer Router Deployment

This guide walks you through everything you need to configure on AWS before running `terraform apply`. Start from scratch - we assume you have nothing set up.

## Table of Contents

1. [Create AWS Account](#1-create-aws-account)
2. [Install Required Tools](#2-install-required-tools)
3. [Create IAM User for Terraform](#3-create-iam-user-for-terraform)
4. [Configure AWS CLI](#4-configure-aws-cli)
5. [Get Ethereum RPC URL](#5-get-ethereum-rpc-url)
6. [Generate Application Keys](#6-generate-application-keys)
7. [Create Terraform Configuration](#7-create-terraform-configuration)
8. [Run Terraform](#8-run-terraform)

---

## 1. Create AWS Account

If you don't have an AWS account:

1. Go to [https://aws.amazon.com/](https://aws.amazon.com/)
2. Click "Create an AWS Account"
3. Follow the signup process (requires credit card)
4. Verify your email and phone number

**Cost Warning**: This deployment costs approximately $190-250/month. Set up billing alerts:

```
AWS Console → Billing → Budgets → Create Budget → Cost Budget → $200/month
```

---

## 2. Install Required Tools

### macOS

```bash
# Install Homebrew (if not installed)
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

# Install Terraform
brew tap hashicorp/tap
brew install hashicorp/tap/terraform

# Install AWS CLI v2
brew install awscli

# Install kubectl
brew install kubectl

# Verify installations
terraform version    # Should show >= 1.5.0
aws --version        # Should show aws-cli/2.x.x
kubectl version --client
```

### Linux (Ubuntu/Debian)

```bash
# Install Terraform
sudo apt-get update && sudo apt-get install -y gnupg software-properties-common
wget -O- https://apt.releases.hashicorp.com/gpg | \
  gpg --dearmor | \
  sudo tee /usr/share/keyrings/hashicorp-archive-keyring.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/hashicorp-archive-keyring.gpg] \
  https://apt.releases.hashicorp.com $(lsb_release -cs) main" | \
  sudo tee /etc/apt/sources.list.d/hashicorp.list
sudo apt update && sudo apt-get install terraform

# Install AWS CLI v2
curl "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip" -o "awscliv2.zip"
unzip awscliv2.zip
sudo ./aws/install

# Install kubectl
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
sudo install -o root -g root -m 0755 kubectl /usr/local/bin/kubectl

# Verify
terraform version
aws --version
kubectl version --client
```

### Windows

```powershell
# Install Chocolatey (if not installed)
Set-ExecutionPolicy Bypass -Scope Process -Force
[System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))

# Install tools
choco install terraform awscli kubectl

# Verify
terraform version
aws --version
kubectl version --client
```

---

## 3. Create IAM User for Terraform

Terraform needs an IAM user with permissions to create AWS resources.

### Step 3.1: Log into AWS Console

1. Go to [https://console.aws.amazon.com/](https://console.aws.amazon.com/)
2. Sign in with your root account or existing admin user

### Step 3.2: Create IAM Policy

1. Go to **IAM** → **Policies** → **Create Policy**
2. Click **JSON** tab
3. Paste this policy:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "EKSFullAccess",
      "Effect": "Allow",
      "Action": [
        "eks:*"
      ],
      "Resource": "*"
    },
    {
      "Sid": "EC2ForEKS",
      "Effect": "Allow",
      "Action": [
        "ec2:CreateVpc",
        "ec2:DeleteVpc",
        "ec2:DescribeVpcs",
        "ec2:DescribeVpcAttribute",
        "ec2:ModifyVpcAttribute",
        "ec2:DescribeNetworkAcls",
        "ec2:DescribeDhcpOptions",
        "ec2:CreateSubnet",
        "ec2:DeleteSubnet",
        "ec2:DescribeSubnets",
        "ec2:ModifySubnetAttribute",
        "ec2:CreateRouteTable",
        "ec2:DeleteRouteTable",
        "ec2:DescribeRouteTables",
        "ec2:AssociateRouteTable",
        "ec2:DisassociateRouteTable",
        "ec2:CreateRoute",
        "ec2:DeleteRoute",
        "ec2:ReplaceRoute",
        "ec2:CreateInternetGateway",
        "ec2:DeleteInternetGateway",
        "ec2:DescribeInternetGateways",
        "ec2:AttachInternetGateway",
        "ec2:DetachInternetGateway",
        "ec2:CreateNatGateway",
        "ec2:DeleteNatGateway",
        "ec2:DescribeNatGateways",
        "ec2:AllocateAddress",
        "ec2:ReleaseAddress",
        "ec2:DescribeAddresses",
        "ec2:DescribeAddressesAttribute",
        "ec2:CreateSecurityGroup",
        "ec2:DeleteSecurityGroup",
        "ec2:DescribeSecurityGroups",
        "ec2:DescribeSecurityGroupRules",
        "ec2:AuthorizeSecurityGroupIngress",
        "ec2:AuthorizeSecurityGroupEgress",
        "ec2:RevokeSecurityGroupIngress",
        "ec2:RevokeSecurityGroupEgress",
        "ec2:UpdateSecurityGroupRuleDescriptionsIngress",
        "ec2:UpdateSecurityGroupRuleDescriptionsEgress",
        "ec2:CreateTags",
        "ec2:DeleteTags",
        "ec2:DescribeTags",
        "ec2:DescribeAvailabilityZones",
        "ec2:DescribeAccountAttributes",
        "ec2:DescribeNetworkInterfaces",
        "ec2:CreateNetworkInterface",
        "ec2:DeleteNetworkInterface",
        "ec2:ModifyNetworkInterfaceAttribute",
        "ec2:DescribeLaunchTemplates",
        "ec2:DescribeLaunchTemplateVersions",
        "ec2:CreateLaunchTemplate",
        "ec2:DeleteLaunchTemplate",
        "ec2:CreateLaunchTemplateVersion",
        "ec2:DescribeImages",
        "ec2:DescribeInstanceTypes",
        "ec2:RunInstances",
        "ec2:TerminateInstances",
        "ec2:DescribeInstances",
        "ec2:DescribeInstanceStatus",
        "ec2:DescribeKeyPairs",
        "ec2:DescribeVolumes",
        "ec2:DescribeVolumeStatus"
      ],
      "Resource": "*"
    },
    {
      "Sid": "IAMForEKS",
      "Effect": "Allow",
      "Action": [
        "iam:CreateRole",
        "iam:DeleteRole",
        "iam:GetRole",
        "iam:ListRoles",
        "iam:PassRole",
        "iam:AttachRolePolicy",
        "iam:DetachRolePolicy",
        "iam:ListAttachedRolePolicies",
        "iam:ListRolePolicies",
        "iam:PutRolePolicy",
        "iam:GetRolePolicy",
        "iam:DeleteRolePolicy",
        "iam:CreatePolicy",
        "iam:DeletePolicy",
        "iam:GetPolicy",
        "iam:GetPolicyVersion",
        "iam:ListPolicyVersions",
        "iam:CreatePolicyVersion",
        "iam:DeletePolicyVersion",
        "iam:CreateOpenIDConnectProvider",
        "iam:DeleteOpenIDConnectProvider",
        "iam:GetOpenIDConnectProvider",
        "iam:TagRole",
        "iam:UntagRole",
        "iam:TagPolicy",
        "iam:UntagPolicy",
        "iam:TagOpenIDConnectProvider",
        "iam:CreateServiceLinkedRole"
      ],
      "Resource": "*"
    },
    {
      "Sid": "ELBForALB",
      "Effect": "Allow",
      "Action": [
        "elasticloadbalancing:*"
      ],
      "Resource": "*"
    },
    {
      "Sid": "AutoScaling",
      "Effect": "Allow",
      "Action": [
        "autoscaling:CreateAutoScalingGroup",
        "autoscaling:DeleteAutoScalingGroup",
        "autoscaling:DescribeAutoScalingGroups",
        "autoscaling:UpdateAutoScalingGroup",
        "autoscaling:CreateLaunchConfiguration",
        "autoscaling:DeleteLaunchConfiguration",
        "autoscaling:DescribeLaunchConfigurations"
      ],
      "Resource": "*"
    },
    {
      "Sid": "CloudWatchLogs",
      "Effect": "Allow",
      "Action": [
        "logs:CreateLogGroup",
        "logs:DeleteLogGroup",
        "logs:DescribeLogGroups",
        "logs:PutRetentionPolicy"
      ],
      "Resource": "*"
    },
    {
      "Sid": "KMS",
      "Effect": "Allow",
      "Action": [
        "kms:CreateKey",
        "kms:DescribeKey",
        "kms:CreateAlias",
        "kms:DeleteAlias",
        "kms:ListAliases"
      ],
      "Resource": "*"
    }
  ]
}
```

4. Click **Next**
5. Name it: `GasKillerTerraformPolicy`
6. Click **Create policy**

### Step 3.3: Create IAM User

1. Go to **IAM** → **Users** → **Create user**
2. User name: `gas-killer-terraform`
3. Click **Next**
4. Select **Attach policies directly**
5. Search for and select `GasKillerTerraformPolicy`
6. Click **Next** → **Create user**

### Step 3.4: Create Access Keys

1. Click on the user `gas-killer-terraform`
2. Go to **Security credentials** tab
3. Scroll to **Access keys** → **Create access key**
4. Select **Command Line Interface (CLI)**
5. Check the confirmation box
6. Click **Next** → **Create access key**
7. **IMPORTANT**: Download or copy both:
   - Access key ID (looks like: `AKIAIOSFODNN7EXAMPLE`)
   - Secret access key (looks like: `wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY`)

**Save these securely! You cannot retrieve the secret key later.**

---

## 4. Configure AWS CLI

Configure the AWS CLI with your credentials:

```bash
aws configure
```

Enter the following when prompted:

```
AWS Access Key ID [None]: AKIAIOSFODNN7EXAMPLE
AWS Secret Access Key [None]: wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
Default region name [None]: us-east-1
Default output format [None]: json
```

Verify it works:

```bash
aws sts get-caller-identity
```

Expected output:
```json
{
    "UserId": "AIDAEXAMPLEID",
    "Account": "123456789012",
    "Arn": "arn:aws:iam::123456789012:user/gas-killer-terraform"
}
```

---

## 5. Get Ethereum RPC URL

The Gas Killer Router needs an Ethereum Sepolia RPC URL to fork from.

### Option A: Free Public RPC (Rate Limited)

Use this for testing - no signup required:

```
https://ethereum-sepolia-rpc.publicnode.com
```

**Warning**: Public RPCs have rate limits and may be unreliable.

### Option B: Alchemy (Recommended - Free Tier)

1. Go to [https://www.alchemy.com/](https://www.alchemy.com/)
2. Click **Sign Up** → Create account
3. Click **Create new app**
   - Name: `gas-killer`
   - Chain: **Ethereum**
   - Network: **Sepolia**
4. Click **Create app**
5. Click on your app → **API Key**
6. Copy the **HTTPS** URL:
   ```
   https://eth-sepolia.g.alchemy.com/v2/YOUR_API_KEY
   ```

### Option C: Infura (Free Tier)

1. Go to [https://infura.io/](https://infura.io/)
2. Sign up for a free account
3. Create a new project
4. Select **Sepolia** network
5. Copy your endpoint:
   ```
   https://sepolia.infura.io/v3/YOUR_PROJECT_ID
   ```

---

## 6. Generate Application Keys

The Gas Killer Router needs two private keys for its operation.

### Option A: Use Test Keys (Development Only)

For testing, use Anvil's default funded accounts:

```bash
# Private Key (Deployer)
private_key="ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

# Funded Key (Gas Account)
funded_key="59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
```

**WARNING**: These are publicly known keys. NEVER use them with real funds!

### Option B: Generate New Keys

Using Node.js:

```bash
# Install ethers if needed
npm install ethers

# Generate keys
node -e "
const { Wallet } = require('ethers');
const wallet1 = Wallet.createRandom();
const wallet2 = Wallet.createRandom();
console.log('Private Key (Deployer):');
console.log(wallet1.privateKey.slice(2));  // Remove 0x prefix
console.log('');
console.log('Funded Key (Gas Account):');
console.log(wallet2.privateKey.slice(2));  // Remove 0x prefix
"
```

Using Python:

```bash
pip install eth-account

python3 -c "
from eth_account import Account
import secrets

key1 = secrets.token_hex(32)
key2 = secrets.token_hex(32)
print(f'Private Key (Deployer):\n{key1}\n')
print(f'Funded Key (Gas Account):\n{key2}')
"
```

Using cast (Foundry):

```bash
# Install Foundry if needed
curl -L https://foundry.paradigm.xyz | bash
foundryup

# Generate keys
echo "Private Key (Deployer):"
cast wallet new | grep "Private key" | awk '{print $3}' | sed 's/0x//'
echo ""
echo "Funded Key (Gas Account):"
cast wallet new | grep "Private key" | awk '{print $3}' | sed 's/0x//'
```

**Save these keys securely!**

---

## 7. Clone Repository and Create Terraform Configuration

### Step 7.1: Clone the Repository

```bash
# Clone the repo
git clone https://github.com/BreadchainCoop/gas-killer-router.git
cd gas-killer-router

# Switch to the branch with Terraform support
git checkout RonTuretzky/terraform-aws-deploy
```

### Step 7.2: Create terraform.tfvars

```bash
# Navigate to the Terraform dev environment
cd terraform/environments/dev

# Copy the example configuration
cp terraform.tfvars.example terraform.tfvars

# Edit with your values (use your preferred editor)
nano terraform.tfvars
# or
vim terraform.tfvars
# or
code terraform.tfvars
```

Edit `terraform.tfvars`:

```hcl
# AWS Configuration
aws_region = "us-east-1"

# EKS Configuration
kubernetes_version  = "1.29"
node_instance_types = ["t3.medium"]
node_desired_size   = 2
node_min_size       = 1
node_max_size       = 3

# Gas Killer Configuration
environment_mode = "LOCAL"
node_count       = 3

# =============================================================================
# SECRETS - Replace with your actual values
# =============================================================================

# Your Sepolia RPC URL (from Step 5)
fork_url = "https://eth-sepolia.g.alchemy.com/v2/YOUR_API_KEY"

# Your private keys (from Step 6) - WITHOUT 0x prefix
private_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
funded_key  = "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"

# Ingress
enable_ingress = true
```

---

## 8. Run Terraform

### Step 8.1: Initialize Terraform

```bash
cd terraform/environments/dev
terraform init
```

Expected output:
```
Initializing the backend...
Initializing provider plugins...
- Finding hashicorp/aws versions matching "~> 5.0"...
...
Terraform has been successfully initialized!
```

### Step 8.2: Plan (Preview Changes)

```bash
terraform plan
```

This shows what will be created. Review it carefully.

### Step 8.3: Apply (Create Resources)

```bash
terraform apply
```

Type `yes` when prompted.

**This takes 15-25 minutes** to create:
- VPC and networking
- EKS cluster
- Node groups
- Add-ons (EBS CSI, ALB controller)
- Gas Killer Helm deployment

### Step 8.4: Configure kubectl

After apply completes:

```bash
# Get the kubeconfig command from outputs
$(terraform output -raw kubeconfig_command)

# Verify connection
kubectl get nodes
```

### Step 8.5: Verify Deployment

```bash
# Check pods
kubectl get pods -n gas-killer

# Wait for all pods to be Running (may take 5-10 minutes)
kubectl get pods -n gas-killer -w

# Get the router URL
terraform output router_url
```

---

## Troubleshooting

### "Error: creating IAM Role: AccessDenied"

Your IAM user doesn't have sufficient permissions. Verify:
1. The policy is attached to your user
2. You're using the correct AWS credentials

```bash
aws sts get-caller-identity
aws iam list-attached-user-policies --user-name gas-killer-terraform
```

### "Error: creating EKS Cluster: InvalidParameterException"

Usually means the region doesn't support EKS or the Kubernetes version. Try:
- Different region: `aws_region = "us-west-2"`
- Different version: `kubernetes_version = "1.28"`

### "Error: timeout waiting for resource"

EKS cluster creation can take 15-20 minutes. If it times out:
```bash
terraform apply  # Just run again, it will continue
```

### Pods stuck in Pending

Check if nodes are ready:
```bash
kubectl get nodes
kubectl describe nodes
```

Check for resource constraints:
```bash
kubectl describe pod -n gas-killer <pod-name>
```

---

## Clean Up

To avoid ongoing charges, destroy all resources when done:

```bash
cd terraform/environments/dev
terraform destroy
```

Type `yes` to confirm. This removes everything created by Terraform.

---

## Summary Checklist

Before running `terraform apply`, verify:

- [ ] AWS account created
- [ ] Terraform installed (`terraform version` works)
- [ ] AWS CLI installed (`aws --version` works)
- [ ] kubectl installed (`kubectl version --client` works)
- [ ] IAM user created with `GasKillerTerraformPolicy`
- [ ] AWS CLI configured (`aws sts get-caller-identity` works)
- [ ] Sepolia RPC URL obtained
- [ ] Private keys generated/obtained
- [ ] `terraform.tfvars` created with all values filled in

Once all boxes are checked, you're ready to run:

```bash
cd terraform/environments/dev
terraform init
terraform apply
```

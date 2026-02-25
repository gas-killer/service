# EKS IAM Access Setup

This guide explains how to grant team members access to the Gas Killer EKS cluster.

## Overview

AWS EKS requires IAM authentication for kubectl access. Users need:
1. AWS IAM credentials (user or role)
2. EKS cluster access entry or aws-auth ConfigMap mapping
3. kubectl configured with AWS authentication

## Option 1: EKS Access Entries (Recommended)

EKS Access Entries is the newer, simpler approach (available since late 2023):

```bash
# Grant a user admin access to the cluster
aws eks create-access-entry \
  --cluster-name gas-killer-dev \
  --principal-arn arn:aws:iam::ACCOUNT_ID:user/USERNAME

# Associate an access policy (cluster admin)
aws eks associate-access-policy \
  --cluster-name gas-killer-dev \
  --principal-arn arn:aws:iam::ACCOUNT_ID:user/USERNAME \
  --policy-arn arn:aws:eks::aws:cluster-access-policy/AmazonEKSClusterAdminPolicy \
  --access-scope type=cluster

# Or limit to specific namespace
aws eks associate-access-policy \
  --cluster-name gas-killer-dev \
  --principal-arn arn:aws:iam::ACCOUNT_ID:user/USERNAME \
  --policy-arn arn:aws:eks::aws:cluster-access-policy/AmazonEKSAdminPolicy \
  --access-scope type=namespace,namespaces=gas-killer
```

### Available Access Policies

| Policy | Description |
|--------|-------------|
| `AmazonEKSClusterAdminPolicy` | Full cluster admin |
| `AmazonEKSAdminPolicy` | Admin within scope |
| `AmazonEKSEditPolicy` | Edit resources within scope |
| `AmazonEKSViewPolicy` | Read-only access |

### List Existing Access Entries

```bash
# List all access entries
aws eks list-access-entries --cluster-name gas-killer-dev

# List policies for a principal
aws eks list-associated-access-policies \
  --cluster-name gas-killer-dev \
  --principal-arn arn:aws:iam::ACCOUNT_ID:user/USERNAME
```

### Remove Access

```bash
# Remove access entry
aws eks delete-access-entry \
  --cluster-name gas-killer-dev \
  --principal-arn arn:aws:iam::ACCOUNT_ID:user/USERNAME
```

## Option 2: Legacy aws-auth ConfigMap

For older clusters or if Access Entries aren't enabled:

```bash
# Edit the aws-auth ConfigMap
kubectl edit configmap aws-auth -n kube-system
```

Add user mapping:
```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: aws-auth
  namespace: kube-system
data:
  mapUsers: |
    - userarn: arn:aws:iam::ACCOUNT_ID:user/USERNAME
      username: USERNAME
      groups:
        - system:masters  # Full admin access
```

Add role mapping:
```yaml
data:
  mapRoles: |
    - rolearn: arn:aws:iam::ACCOUNT_ID:role/DeveloperRole
      username: developer
      groups:
        - system:masters
```

## User Setup Instructions

Once access is granted, users configure kubectl:

```bash
# Prerequisites
# - AWS CLI v2 installed
# - IAM credentials configured (aws configure)

# 1. Update kubeconfig
aws eks update-kubeconfig \
  --name gas-killer-dev \
  --region us-east-1

# 2. (Optional) If using an assumed role
aws eks update-kubeconfig \
  --name gas-killer-dev \
  --region us-east-1 \
  --role-arn arn:aws:iam::ACCOUNT_ID:role/DeveloperRole

# 3. Verify access
kubectl get nodes
kubectl get pods -n gas-killer

# 4. View router logs
kubectl logs -n gas-killer -l app.kubernetes.io/component=router -f

# 5. Port-forward to router (alternative to ingress)
kubectl port-forward -n gas-killer svc/gas-killer-router 8080:8080
```

## Creating a Shared Developer Role

For team access, create a shared IAM role:

```bash
# 1. Create the role (replace ACCOUNT_ID and trusted users)
cat > trust-policy.json << 'EOF'
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Principal": {
      "AWS": [
        "arn:aws:iam::ACCOUNT_ID:user/alice",
        "arn:aws:iam::ACCOUNT_ID:user/bob"
      ]
    },
    "Action": "sts:AssumeRole"
  }]
}
EOF

aws iam create-role \
  --role-name gas-killer-dev-access \
  --assume-role-policy-document file://trust-policy.json

# 2. Attach EKS describe policy
cat > eks-policy.json << 'EOF'
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Action": [
      "eks:DescribeCluster",
      "eks:ListClusters"
    ],
    "Resource": "*"
  }]
}
EOF

aws iam put-role-policy \
  --role-name gas-killer-dev-access \
  --policy-name eks-describe \
  --policy-document file://eks-policy.json

# 3. Grant the role EKS access
aws eks create-access-entry \
  --cluster-name gas-killer-dev \
  --principal-arn arn:aws:iam::ACCOUNT_ID:role/gas-killer-dev-access \
  --type STANDARD

aws eks associate-access-policy \
  --cluster-name gas-killer-dev \
  --principal-arn arn:aws:iam::ACCOUNT_ID:role/gas-killer-dev-access \
  --policy-arn arn:aws:eks::aws:cluster-access-policy/AmazonEKSAdminPolicy \
  --access-scope type=namespace,namespaces=gas-killer
```

### Using the Shared Role

Team members assume the role:

```bash
# Configure profile in ~/.aws/config
[profile gas-killer-dev]
role_arn = arn:aws:iam::ACCOUNT_ID:role/gas-killer-dev-access
source_profile = default  # or your base profile

# Update kubeconfig with the role
aws eks update-kubeconfig \
  --name gas-killer-dev \
  --region us-east-1 \
  --role-arn arn:aws:iam::ACCOUNT_ID:role/gas-killer-dev-access

# Or use the profile
AWS_PROFILE=gas-killer-dev kubectl get pods -n gas-killer
```

## Troubleshooting

### "You must be logged in to the server (Unauthorized)"

```bash
# Verify AWS credentials
aws sts get-caller-identity

# Check if your IAM principal has an access entry
aws eks list-access-entries --cluster-name gas-killer-dev

# Ensure you're using the correct region
aws eks describe-cluster --name gas-killer-dev --region us-east-1
```

### "error: You must be logged in to the server (the server has asked for the client to provide credentials)"

```bash
# Kubeconfig may be stale - regenerate it
aws eks update-kubeconfig --name gas-killer-dev --region us-east-1

# Check AWS CLI version (should be v2)
aws --version

# Verify the token works
aws eks get-token --cluster-name gas-killer-dev
```

### Access entry exists but still can't access

```bash
# Ensure an access policy is associated
aws eks list-associated-access-policies \
  --cluster-name gas-killer-dev \
  --principal-arn YOUR_ARN

# If no policies, associate one
aws eks associate-access-policy \
  --cluster-name gas-killer-dev \
  --principal-arn YOUR_ARN \
  --policy-arn arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy \
  --access-scope type=cluster
```

### Permission denied for specific resources

If you can connect but can't access certain resources:

```bash
# Check your Kubernetes RBAC permissions
kubectl auth can-i --list

# Check specific permission
kubectl auth can-i get pods -n gas-killer
```

The access policy scope may be limiting you. Request a broader scope from the cluster admin.

terraform {
  required_version = ">= 1.5.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
    kubernetes = {
      source  = "hashicorp/kubernetes"
      version = "~> 2.25"
    }
    helm = {
      source  = "hashicorp/helm"
      version = "~> 2.12"
    }
    tls = {
      source  = "hashicorp/tls"
      version = "~> 4.0"
    }
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

locals {
  name = "${var.project_name}-${var.environment}"
  tags = {
    Project     = var.project_name
    Environment = var.environment
    ManagedBy   = "terraform"
  }
}

# =============================================================================
# AWS Provider
# =============================================================================

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = local.tags
  }
}

# =============================================================================
# VPC Module
# =============================================================================

module "vpc" {
  source = "../../modules/vpc"

  name               = local.name
  vpc_cidr           = var.vpc_cidr
  availability_zones = var.availability_zones
  enable_nat_gateway = true
  single_nat_gateway = true # Cost savings for dev

  tags = local.tags
}

# =============================================================================
# EKS Module
# =============================================================================

module "eks" {
  source = "../../modules/eks"

  cluster_name        = local.name
  kubernetes_version  = var.kubernetes_version
  vpc_id              = module.vpc.vpc_id
  private_subnet_ids  = module.vpc.private_subnet_ids
  public_subnet_ids   = module.vpc.public_subnet_ids
  node_instance_types = var.node_instance_types
  node_desired_size   = var.node_desired_size
  node_min_size       = var.node_min_size
  node_max_size       = var.node_max_size

  tags = local.tags

  depends_on = [null_resource.vpc_post_cleanup]
}

# =============================================================================
# Kubernetes & Helm Providers (configured after EKS is created)
# =============================================================================

provider "kubernetes" {
  host                   = module.eks.cluster_endpoint
  cluster_ca_certificate = base64decode(module.eks.cluster_certificate_authority_data)

  exec {
    api_version = "client.authentication.k8s.io/v1beta1"
    command     = "aws"
    args        = ["eks", "get-token", "--cluster-name", module.eks.cluster_name]
  }
}

provider "helm" {
  kubernetes {
    host                   = module.eks.cluster_endpoint
    cluster_ca_certificate = base64decode(module.eks.cluster_certificate_authority_data)

    exec {
      api_version = "client.authentication.k8s.io/v1beta1"
      command     = "aws"
      args        = ["eks", "get-token", "--cluster-name", module.eks.cluster_name]
    }
  }
}

# =============================================================================
# EKS Add-ons Module (EBS CSI, ALB Controller)
# =============================================================================

module "eks_addons" {
  source = "../../modules/eks-addons"

  cluster_name              = module.eks.cluster_name
  cluster_oidc_issuer_url   = module.eks.cluster_oidc_issuer_url
  cluster_oidc_provider_arn = module.eks.cluster_oidc_provider_arn
  vpc_id                    = module.vpc.vpc_id
  aws_region                = var.aws_region

  tags = local.tags

  depends_on = [module.eks]
}

# =============================================================================
# Pre-destroy cleanup for ALB resources
# =============================================================================
# This ensures the ALB Controller properly cleans up load balancers before
# the controller itself is deleted, preventing orphaned finalizers.

resource "null_resource" "alb_cleanup" {
  triggers = {
    cluster_name = module.eks.cluster_name
    namespace    = var.namespace
    region       = var.aws_region
    vpc_id       = module.vpc.vpc_id
  }

  provisioner "local-exec" {
    when    = destroy
    command = <<-EOT
      set +e  # Don't exit on errors
      echo "=== Pre-destroy cleanup for ALB resources ==="

      # Update kubeconfig
      aws eks update-kubeconfig --name ${self.triggers.cluster_name} --region ${self.triggers.region} 2>/dev/null || true

      # Step 1: Delete helm release to trigger cleanup
      echo "Step 1: Deleting helm release..."
      helm uninstall gas-killer -n ${self.triggers.namespace} --timeout 60s 2>/dev/null || true

      # Step 2: Delete all ingresses
      echo "Step 2: Deleting ingresses..."
      kubectl delete ingress --all -n ${self.triggers.namespace} --timeout=30s 2>/dev/null || true

      # Step 3: Force remove ingress finalizers
      echo "Step 3: Removing ingress finalizers..."
      for ing in $(kubectl get ingress -n ${self.triggers.namespace} -o name 2>/dev/null); do
        kubectl patch $ing -n ${self.triggers.namespace} -p '{"metadata":{"finalizers":null}}' --type=merge 2>/dev/null || true
      done

      # Step 4: Remove targetgroupbinding finalizers
      echo "Step 4: Removing targetgroupbinding finalizers..."
      for tgb in $(kubectl get targetgroupbindings.elbv2.k8s.aws -n ${self.triggers.namespace} -o name 2>/dev/null); do
        kubectl patch $tgb -n ${self.triggers.namespace} -p '{"metadata":{"finalizers":null}}' --type=merge 2>/dev/null || true
      done

      # Step 5: Delete namespace (force if stuck)
      echo "Step 5: Deleting namespace..."
      kubectl delete namespace ${self.triggers.namespace} --timeout=30s 2>/dev/null || true

      # Force remove namespace finalizers if stuck
      kubectl get namespace ${self.triggers.namespace} 2>/dev/null && \
        kubectl patch namespace ${self.triggers.namespace} -p '{"metadata":{"finalizers":null}}' --type=merge 2>/dev/null || true

      # Step 6: Clean up orphaned AWS resources in VPC
      echo "Step 6: Cleaning up orphaned AWS resources in VPC..."

      echo "Deleting ALBs in VPC..."
      
      aws elbv2 describe-load-balancers \
        --region ${self.triggers.region} \
        --query 'LoadBalancers[?VpcId==`'"${self.triggers.vpc_id}"'`].LoadBalancerArn' \
        --output text | while read arn; do
          echo "Deleting ALB: $arn"
          aws elbv2 delete-load-balancer \
            --region ${self.triggers.region} \
            --load-balancer-arn "$arn" || true
      done
      
      echo "Waiting for ELB ENIs to disappear..."
      for i in {1..60}; do
        COUNT=$(aws ec2 describe-network-interfaces \
          --filters Name=vpc-id,Values=${self.triggers.vpc_id} \
          --query 'NetworkInterfaces[?contains(Description, `ELB`)] | length(@)' \
          --output text)
      
        echo "Remaining ELB ENIs: $COUNT"
        [ "$COUNT" = "0" ] && break
        sleep 20
      done

      # Delete any ENIs that might be stuck
      for eni in $(aws ec2 describe-network-interfaces \
        --filters "Name=vpc-id,Values=${self.triggers.vpc_id}" "Name=status,Values=available" \
        --query 'NetworkInterfaces[*].NetworkInterfaceId' \
        --output text --region ${self.triggers.region} 2>/dev/null); do
        echo "Deleting ENI: $eni"
        aws ec2 delete-network-interface --network-interface-id $eni --region ${self.triggers.region} 2>/dev/null || true
      done

      echo "=== ALB cleanup complete ==="
    EOT
  }

  depends_on = [module.eks_addons]
}

resource "null_resource" "vpc_post_cleanup" {
  triggers = {
    cluster_name = local.name
    region       = var.aws_region
    vpc_id       = module.vpc.vpc_id
  }

  provisioner "local-exec" {
    when    = destroy
    command = <<-EOT
      set +e
      echo "=== Post-destroy VPC cleanup (after EKS deletion) ==="

      echo "Waiting for EKS cluster to be deleted (best-effort)..."
      aws eks wait cluster-deleted \
        --region ${self.triggers.region} \
        --name ${self.triggers.cluster_name} 2>/dev/null || true

      # Function to clean up security groups
      cleanup_security_groups() {
        echo "Cleaning up all non-default security groups in VPC..."
        ALL_SGS=$(aws ec2 describe-security-groups \
          --region ${self.triggers.region} \
          --filters "Name=vpc-id,Values=${self.triggers.vpc_id}" \
          --query "SecurityGroups[?GroupName!='default'].GroupId" \
          --output text 2>/dev/null)

        if [ -z "$ALL_SGS" ]; then
          echo "No security groups to clean up"
          return 0
        fi

        # First pass: remove all ingress/egress rules that reference other SGs
        for sg in $ALL_SGS; do
          echo "Clearing rules from SG: $sg"
          # Get and revoke ingress rules
          aws ec2 describe-security-groups --group-ids "$sg" --region ${self.triggers.region} \
            --query 'SecurityGroups[0].IpPermissions' --output json 2>/dev/null | \
            jq -c 'if . then . else [] end' | \
            xargs -I {} aws ec2 revoke-security-group-ingress --group-id "$sg" --ip-permissions '{}' --region ${self.triggers.region} 2>/dev/null || true
          # Get and revoke egress rules
          aws ec2 describe-security-groups --group-ids "$sg" --region ${self.triggers.region} \
            --query 'SecurityGroups[0].IpPermissionsEgress' --output json 2>/dev/null | \
            jq -c 'if . then . else [] end' | \
            xargs -I {} aws ec2 revoke-security-group-egress --group-id "$sg" --ip-permissions '{}' --region ${self.triggers.region} 2>/dev/null || true
        done

        # Second pass: delete the security groups
        for sg in $ALL_SGS; do
          echo "Trying to delete SG: $sg"
          aws ec2 delete-security-group \
            --region ${self.triggers.region} \
            --group-id "$sg" 2>/dev/null && echo "Deleted SG: $sg" || echo "Failed to delete SG: $sg (will retry)"
        done
      }

      # Function to clean up ENIs
      cleanup_enis() {
        echo "Deleting any leftover ENIs..."
        for eni in $(aws ec2 describe-network-interfaces \
          --region ${self.triggers.region} \
          --filters "Name=vpc-id,Values=${self.triggers.vpc_id}" \
          --query 'NetworkInterfaces[*].NetworkInterfaceId' \
          --output text 2>/dev/null); do
          echo "Processing ENI: $eni"
          # Try to detach first if attached
          ATTACHMENT=$(aws ec2 describe-network-interfaces --network-interface-ids "$eni" \
            --region ${self.triggers.region} \
            --query 'NetworkInterfaces[0].Attachment.AttachmentId' --output text 2>/dev/null)
          if [ -n "$ATTACHMENT" ] && [ "$ATTACHMENT" != "None" ] && [ "$ATTACHMENT" != "null" ]; then
            echo "Detaching ENI: $eni"
            aws ec2 detach-network-interface --attachment-id "$ATTACHMENT" --force --region ${self.triggers.region} 2>/dev/null || true
            sleep 5
          fi
          echo "Deleting ENI: $eni"
          aws ec2 delete-network-interface --region ${self.triggers.region} --network-interface-id "$eni" 2>/dev/null || true
        done
      }

      # Run cleanup multiple times with increasing wait times
      # EKS orphaned security groups can take a while to become deletable
      for attempt in 1 2 3 4 5; do
        echo ""
        echo "=== Cleanup attempt $attempt/5 ==="

        # Check if there are any remaining SGs
        REMAINING_SGS=$(aws ec2 describe-security-groups \
          --region ${self.triggers.region} \
          --filters "Name=vpc-id,Values=${self.triggers.vpc_id}" \
          --query "SecurityGroups[?GroupName!='default'].GroupId" \
          --output text 2>/dev/null)

        if [ -z "$REMAINING_SGS" ]; then
          echo "All security groups cleaned up!"
          break
        fi

        echo "Remaining security groups: $REMAINING_SGS"

        # Clean up ENIs first (they can block SG deletion)
        cleanup_enis

        # Then clean up security groups
        cleanup_security_groups

        if [ $attempt -lt 5 ]; then
          WAIT_TIME=$((attempt * 15))
          echo "Waiting $${WAIT_TIME}s before next attempt..."
          sleep $WAIT_TIME
        fi
      done

      # Final aggressive cleanup for any remaining EKS security groups
      echo ""
      echo "=== Final EKS security group cleanup ==="
      EKS_SGS=$(aws ec2 describe-security-groups \
        --region ${self.triggers.region} \
        --filters "Name=vpc-id,Values=${self.triggers.vpc_id}" "Name=group-name,Values=eks-cluster-sg-*" \
        --query "SecurityGroups[*].GroupId" \
        --output text 2>/dev/null)

      if [ -n "$EKS_SGS" ]; then
        echo "Found EKS security groups: $EKS_SGS"
        for sg in $EKS_SGS; do
          echo "Force deleting EKS SG: $sg"
          # Clear all rules first
          aws ec2 describe-security-groups --group-ids "$sg" --region ${self.triggers.region} \
            --query 'SecurityGroups[0].IpPermissions' --output json 2>/dev/null | \
            jq -c 'if . then . else [] end' | \
            xargs -I {} aws ec2 revoke-security-group-ingress --group-id "$sg" --ip-permissions '{}' --region ${self.triggers.region} 2>/dev/null || true
          aws ec2 describe-security-groups --group-ids "$sg" --region ${self.triggers.region} \
            --query 'SecurityGroups[0].IpPermissionsEgress' --output json 2>/dev/null | \
            jq -c 'if . then . else [] end' | \
            xargs -I {} aws ec2 revoke-security-group-egress --group-id "$sg" --ip-permissions '{}' --region ${self.triggers.region} 2>/dev/null || true
          # Delete with retries
          for i in 1 2 3 4 5 6 7 8 9 10; do
            if aws ec2 delete-security-group --region ${self.triggers.region} --group-id "$sg" 2>/dev/null; then
              echo "Deleted EKS SG: $sg"
              break
            fi
            echo "Retry $i/10 for EKS SG $sg..."
            sleep 10
          done
        done
      else
        echo "No EKS security groups remaining"
      fi

      echo ""
      echo "=== Post cleanup complete ==="
    EOT
  }

  # Must exist while VPC exists → makes this run BEFORE VPC destroy (because destroy is reverse-deps)
  depends_on = [module.vpc]
}

# =============================================================================
# Gas Killer Helm Deployment
# =============================================================================

module "gas_killer" {
  source = "../../modules/gas-killer"

  namespace        = var.namespace
  environment_mode = var.environment_mode
  node_count       = var.node_count

  # Secrets
  fork_url    = var.fork_url
  rpc_url     = var.rpc_url
  private_key = var.private_key
  funded_key  = var.funded_key

  # Images
  node_image_repository   = var.node_image_repository
  node_image_tag          = var.node_image_tag
  router_image_repository = var.router_image_repository
  router_image_tag        = var.router_image_tag

  # Ingress
  enable_ingress = var.enable_ingress
  ingress_host   = var.ingress_host

  # Storage
  storage_class = module.eks_addons.storage_class_name

  # E2E Test
  run_e2e_test                    = var.run_e2e_test
  array_summation_factory_address = var.array_summation_factory_address

  # L1-L2 Bridge
  run_bridge                   = var.run_bridge
  l1_rpc_url                   = var.l1_rpc_url
  l2_rpc_url                   = var.l2_rpc_url
  registry_coordinator_address = var.registry_coordinator_address
  bridge_image                 = var.bridge_image

  # Gnosis Factory
  run_gnosis_factory     = var.run_gnosis_factory
  gnosis_factory_address = var.gnosis_factory_address

  # L2 AVS Trigger
  run_l2_avs_trigger = var.run_l2_avs_trigger

  # L2 AVS Mode
  l2_avs_mode = var.l2_avs_mode

  # Dependency
  addons_ready = module.eks_addons.ready

  depends_on = [module.eks_addons, null_resource.alb_cleanup]
}

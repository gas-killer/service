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
# Gas Killer Helm Deployment
# =============================================================================

module "gas_killer" {
  source = "../../modules/gas-killer"

  namespace        = var.namespace
  environment_mode = var.environment_mode
  node_count       = var.node_count

  # Secrets
  fork_url    = var.fork_url
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

  # Dependency
  addons_ready = module.eks_addons.ready

  depends_on = [module.eks_addons]
}

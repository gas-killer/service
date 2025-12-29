# =============================================================================
# AWS Configuration
# =============================================================================

variable "aws_region" {
  description = "AWS region"
  type        = string
  default     = "us-east-1"
}

variable "project_name" {
  description = "Project name for resource naming"
  type        = string
  default     = "gas-killer"
}

variable "environment" {
  description = "Environment name"
  type        = string
  default     = "dev"
}

# =============================================================================
# VPC Configuration
# =============================================================================

variable "vpc_cidr" {
  description = "VPC CIDR block"
  type        = string
  default     = "10.0.0.0/16"
}

variable "availability_zones" {
  description = "Availability zones to use"
  type        = list(string)
  default     = ["us-east-1a", "us-east-1b"]
}

# =============================================================================
# EKS Configuration
# =============================================================================

variable "kubernetes_version" {
  description = "Kubernetes version"
  type        = string
  default     = "1.29"
}

variable "node_instance_types" {
  description = "EC2 instance types for EKS nodes"
  type        = list(string)
  default     = ["t3.medium"]
}

variable "node_desired_size" {
  description = "Desired number of nodes"
  type        = number
  default     = 2
}

variable "node_min_size" {
  description = "Minimum number of nodes"
  type        = number
  default     = 1
}

variable "node_max_size" {
  description = "Maximum number of nodes"
  type        = number
  default     = 4
}

# =============================================================================
# Gas Killer Configuration
# =============================================================================

variable "namespace" {
  description = "Kubernetes namespace for gas-killer"
  type        = string
  default     = "gas-killer"
}

variable "environment_mode" {
  description = "LOCAL (Anvil fork) or TESTNET (direct Sepolia)"
  type        = string
  default     = "LOCAL"

  validation {
    condition     = contains(["LOCAL", "TESTNET"], var.environment_mode)
    error_message = "environment_mode must be LOCAL or TESTNET"
  }
}

variable "node_count" {
  description = "Number of operator nodes"
  type        = number
  default     = 3
}

# =============================================================================
# Secrets (Sensitive)
# =============================================================================

variable "fork_url" {
  description = "RPC URL for Anvil fork (e.g., Alchemy/Infura Sepolia URL)"
  type        = string
  sensitive   = true
}

variable "private_key" {
  description = "Deployer private key (without 0x prefix)"
  type        = string
  sensitive   = true
}

variable "funded_key" {
  description = "Funded account private key (without 0x prefix)"
  type        = string
  sensitive   = true
}

# =============================================================================
# Container Images
# =============================================================================

variable "node_image_repository" {
  description = "Node container image repository"
  type        = string
  default     = "ghcr.io/breadchaincoop/gas-killer-node"
}

variable "node_image_tag" {
  description = "Node container image tag"
  type        = string
  default     = "dev"
}

variable "router_image_repository" {
  description = "Router container image repository"
  type        = string
  default     = "ghcr.io/breadchaincoop/gas-killer-router"
}

variable "router_image_tag" {
  description = "Router container image tag"
  type        = string
  default     = "dev"
}

# =============================================================================
# Ingress Configuration
# =============================================================================

variable "enable_ingress" {
  description = "Enable ALB ingress for router"
  type        = bool
  default     = true
}

variable "ingress_host" {
  description = "Hostname for ingress (optional, leave empty for ALB default)"
  type        = string
  default     = ""
}

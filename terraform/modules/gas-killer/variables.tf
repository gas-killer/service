variable "namespace" {
  description = "Kubernetes namespace for gas-killer"
  type        = string
  default     = "gas-killer"
}

variable "environment_mode" {
  description = "Environment mode: LOCAL (Anvil fork) or TESTNET (direct Sepolia)"
  type        = string
  default     = "LOCAL"

  validation {
    condition     = contains(["LOCAL", "TESTNET"], var.environment_mode)
    error_message = "environment_mode must be LOCAL or TESTNET"
  }
}

variable "node_count" {
  description = "Number of operator nodes to deploy"
  type        = number
  default     = 3
}

# Secrets
variable "fork_url" {
  description = "RPC URL for Anvil fork (required for LOCAL mode)"
  type        = string
  sensitive   = true
}

variable "private_key" {
  description = "Deployer private key"
  type        = string
  sensitive   = true
}

variable "funded_key" {
  description = "Funded account private key"
  type        = string
  sensitive   = true
}

# Images
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

# Ingress
variable "enable_ingress" {
  description = "Enable ALB ingress for router"
  type        = bool
  default     = true
}

variable "ingress_host" {
  description = "Hostname for ingress (optional)"
  type        = string
  default     = ""
}

# Storage
variable "storage_class" {
  description = "Storage class for PVC"
  type        = string
  default     = "gp3"
}

# Helm chart path
variable "chart_path" {
  description = "Path to the gas-killer Helm chart"
  type        = string
  default     = "../../../helm/gas-killer"
}

# Dependency flag
variable "addons_ready" {
  description = "Flag indicating EKS add-ons are ready"
  type        = bool
  default     = true
}

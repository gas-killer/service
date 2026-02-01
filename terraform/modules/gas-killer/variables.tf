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
  default     = ""
}

variable "rpc_url" {
  description = "RPC URL for direct connection to Sepolia (required for TESTNET mode)"
  type        = string
  sensitive   = true
  default     = ""
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
  default     = "ghcr.io/breadchaincoop/gas-killer-router"
}

variable "node_image_tag" {
  description = "Node container image tag"
  type        = string
  default     = "node-pr-87"
}

variable "router_image_repository" {
  description = "Router container image repository"
  type        = string
  default     = "ghcr.io/breadchaincoop/gas-killer-router"
}

variable "router_image_tag" {
  description = "Router container image tag"
  type        = string
  default     = "pr-87"
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

# E2E Test Configuration
variable "run_e2e_test" {
  description = "Run E2E test job after deployment (deploys ArraySummation and triggers Gas Killer)"
  type        = bool
  default     = true
}

variable "array_summation_factory_address" {
  description = "ArraySummation factory contract address (on Sepolia)"
  type        = string
  default     = "0xF7ded769418Ec1Db4DA3bd2d47ab72ce2296A032"
}

# L1-L2 Bridge Configuration
variable "run_bridge" {
  description = "Run L1-L2 bridge job before deploying gas-killer"
  type        = bool
  default     = true
}

variable "l1_rpc_url" {
  description = "RPC URL for L1 (Sepolia, Holesky, Mainnet)"
  type        = string
  sensitive   = true
  default     = ""
}

variable "l2_rpc_url" {
  description = "RPC URL for L2 (Gnosis, Arbitrum, etc.)"
  type        = string
  sensitive   = true
  default     = ""
}

variable "registry_coordinator_address" {
  description = "EigenLayer RegistryCoordinator address on L1"
  type        = string
  default     = ""
}

variable "bridge_image" {
  description = "Docker image for L1-L2 bridge"
  type        = string
  default     = "ghcr.io/ronturetzky/target-contracts/bridge:pr-1"
}

# Gnosis Factory Configuration
variable "run_gnosis_factory" {
  description = "Run Gnosis factory job after bridge to deploy ArraySummation"
  type        = bool
  default     = true
}

variable "gnosis_factory_address" {
  description = "Gnosis ArraySummation factory contract address"
  type        = string
  default     = "0xCF2e7d5673Ec1b3F174f25A45ddd6d8b2923ca2e"
}

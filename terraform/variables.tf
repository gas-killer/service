variable "project_id" {
  description = "GCP project ID"
  type        = string
}

variable "region" {
  description = "GCP region for the subnet and other regional resources"
  type        = string
  default     = "us-east1"
}

variable "zone" {
  description = "GCP zone for the GKE node pool. Must be within var.region. Using a single zone keeps node_count literal — node_count=1 means exactly 1 node. Regional clusters multiply node_count by the number of zones."
  type        = string
  default     = "us-east1-b"

  validation {
    condition     = startswith(var.zone, var.region)
    error_message = "zone must be within region (e.g. if region is us-east4, zone must be us-east4-a/b/c)."
  }
}

variable "cluster_name" {
  description = "Name of the GKE cluster (also used as prefix for VPC and subnet names)"
  type        = string
  default     = "gas-killer"
}

variable "node_count" {
  description = "Number of GKE nodes. 1 is sufficient for dev/test (no redundancy). Use 2+ for production."
  type        = number
  default     = 1
}

variable "disk_size_gb" {
  description = "Boot disk size per GKE node in GB. 20 GB covers the OS + container image cache for this workload."
  type        = number
  default     = 20
}

variable "deletion_protection" {
  description = "Prevent accidental cluster deletion via Terraform. Set to false for testnet/dev deployments where you want to be able to tear down freely."
  type        = bool
  default     = true
}

variable "spot_nodes" {
  description = "Use spot (preemptible) VMs for the node pool. Reduces cost significantly but nodes can be reclaimed at any time. Set to true for testnet/dev; leave false for production."
  type        = bool
  default     = false
}

variable "machine_type" {
  description = "Machine type for GKE nodes"
  type        = string
  default     = "e2-standard-4"
}

variable "kubernetes_namespace" {
  description = "Kubernetes namespace where gas-killer is deployed. Must match the namespace used in helm install."
  type        = string
  default     = "default"
}

variable "kubernetes_service_account_name" {
  description = "Name of the Kubernetes ServiceAccount created by the Helm chart. Must match secretManager.serviceAccountName in values.yaml."
  type        = string
  default     = "gas-killer"
}

variable "cluster_name" {
  description = "EKS cluster name"
  type        = string
}

variable "cluster_oidc_issuer_url" {
  description = "OIDC issuer URL for the cluster"
  type        = string
}

variable "cluster_oidc_provider_arn" {
  description = "ARN of the OIDC provider"
  type        = string
}

variable "vpc_id" {
  description = "VPC ID"
  type        = string
}

variable "aws_region" {
  description = "AWS region"
  type        = string
  default     = "us-east-1"
}

variable "ebs_csi_driver_version" {
  description = "Version of EBS CSI driver addon"
  type        = string
  default     = "v1.28.0-eksbuild.1"
}

variable "alb_controller_version" {
  description = "Version of AWS Load Balancer Controller Helm chart"
  type        = string
  default     = "1.7.1"
}

variable "tags" {
  description = "Additional tags for resources"
  type        = map(string)
  default     = {}
}

# =============================================================================
# Cluster Information
# =============================================================================

output "cluster_name" {
  description = "EKS cluster name"
  value       = module.eks.cluster_name
}

output "cluster_endpoint" {
  description = "EKS cluster API endpoint"
  value       = module.eks.cluster_endpoint
}

output "cluster_version" {
  description = "Kubernetes version"
  value       = module.eks.cluster_version
}

# =============================================================================
# kubectl Configuration
# =============================================================================

output "kubeconfig_command" {
  description = "Command to configure kubectl"
  value       = "aws eks update-kubeconfig --name ${module.eks.cluster_name} --region ${var.aws_region}"
}

# =============================================================================
# Gas Killer Deployment
# =============================================================================

output "namespace" {
  description = "Kubernetes namespace for gas-killer"
  value       = module.gas_killer.namespace
}

output "router_url" {
  description = "Router URL (HTTP) - may take a few minutes for ALB to be ready"
  value       = module.gas_killer.router_url
}

output "ingress_hostname" {
  description = "ALB hostname"
  value       = module.gas_killer.ingress_hostname
}

# =============================================================================
# Network Information
# =============================================================================

output "vpc_id" {
  description = "VPC ID"
  value       = module.vpc.vpc_id
}

output "nat_gateway_ips" {
  description = "NAT Gateway public IPs"
  value       = module.vpc.nat_gateway_ips
}

# =============================================================================
# Useful Commands
# =============================================================================

output "helpful_commands" {
  description = "Useful commands for working with the deployment"
  value       = <<-EOT

    # Configure kubectl:
    ${self.kubeconfig_command}

    # Check pods:
    kubectl get pods -n ${module.gas_killer.namespace}

    # View router logs:
    kubectl logs -n ${module.gas_killer.namespace} -l app.kubernetes.io/component=router -f

    # View node logs:
    kubectl logs -n ${module.gas_killer.namespace} -l app.kubernetes.io/component=node -f

    # Port-forward to router (alternative to ingress):
    kubectl port-forward -n ${module.gas_killer.namespace} svc/gas-killer-router 8080:8080

    # Trigger gas killer (once router URL is ready):
    curl -X POST ${module.gas_killer.router_url}/trigger \
      -H "Content-Type: application/json" \
      -d '{"body":{"metadata":{"request_id":"1","action":"increment"}}}'

  EOT
}

# =============================================================================
# Cost Estimate
# =============================================================================

output "estimated_monthly_cost" {
  description = "Rough cost estimate (USD)"
  value       = <<-EOT
    Estimated monthly costs (dev configuration):
    - EKS Cluster: ~$73/month
    - NAT Gateway: ~$32/month + data transfer
    - EC2 (${var.node_desired_size}x ${var.node_instance_types[0]}): ~$${var.node_desired_size * 30}/month
    - EBS volumes: ~$10/month
    - ALB: ~$16/month + data transfer
    Total: ~$160-250/month (varies by usage)

    To reduce costs:
    - Destroy when not in use: terraform destroy
    - Use spot instances (requires node group config change)
  EOT
}

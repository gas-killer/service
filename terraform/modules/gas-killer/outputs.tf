output "namespace" {
  description = "Kubernetes namespace"
  value       = var.namespace
}

output "release_name" {
  description = "Helm release name"
  value       = helm_release.gas_killer.name
}

output "release_status" {
  description = "Helm release status"
  value       = helm_release.gas_killer.status
}

output "ingress_hostname" {
  description = "ALB hostname for ingress (may take a few minutes to be available)"
  value       = var.enable_ingress && length(data.kubernetes_ingress_v1.gas_killer) > 0 ? try(data.kubernetes_ingress_v1.gas_killer[0].status[0].load_balancer[0].ingress[0].hostname, "pending...") : "ingress disabled"
}

output "router_url" {
  description = "Router URL (HTTP)"
  value       = var.enable_ingress && length(data.kubernetes_ingress_v1.gas_killer) > 0 ? "http://${try(data.kubernetes_ingress_v1.gas_killer[0].status[0].load_balancer[0].ingress[0].hostname, "pending...")}" : "ingress disabled"
}

output "e2e_test_enabled" {
  description = "Whether E2E test job was run"
  value       = var.run_e2e_test
}

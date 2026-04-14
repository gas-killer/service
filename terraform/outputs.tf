output "cluster_name" {
  description = "Name of the GKE cluster"
  value       = google_container_cluster.primary.name
}

output "cluster_endpoint" {
  description = "Endpoint of the GKE cluster"
  value       = google_container_cluster.primary.endpoint
  sensitive   = true
}

output "gcp_service_account_email" {
  description = "Email of the GCP Service Account. Set this as secretManager.gcpServiceAccount in your Helm values."
  value       = google_service_account.gas_killer.email
}

output "kubeconfig_command" {
  description = "Run this command to configure kubectl for the cluster"
  value       = "gcloud container clusters get-credentials ${google_container_cluster.primary.name} --region ${var.region} --project ${var.project_id}"
}

# Dedicated service account for GKE nodes (replaces the broad Compute Engine default SA).
# Granted only the minimal permissions GKE nodes need (log writing, metrics, image pull).
resource "google_service_account" "gke_node" {
  account_id   = "gas-killer-gke-node"
  display_name = "Gas Killer GKE Node"
  description  = "Minimal-permission SA for GKE nodes. Satisfies the roles/container.defaultNodeServiceAccount recommendation."
}

resource "google_project_iam_member" "gke_node_default" {
  project = var.project_id
  role    = "roles/container.defaultNodeServiceAccount"
  member  = "serviceAccount:${google_service_account.gke_node.email}"
}

# GCP Service Account used by gas-killer pods via Workload Identity
resource "google_service_account" "gas_killer" {
  account_id   = "gas-killer-app"
  display_name = "Gas Killer Application"
  description  = "Used by gas-killer pods to access Secret Manager via Workload Identity. Scope down to secretmanager.secretAccessor for production."
}

# Grant Secret Manager admin to allow the key-export job to create secrets
# and node/router pods to read them. For production, split into two SAs:
# one with secretmanager.admin for the export job, one with
# secretmanager.secretAccessor for node/router pods.
resource "google_project_iam_member" "secret_manager_admin" {
  project = var.project_id
  role    = "roles/secretmanager.admin"
  member  = "serviceAccount:${google_service_account.gas_killer.email}"

  depends_on = [google_project_service.secretmanager]
}

# Bind the Kubernetes ServiceAccount to this GCP SA via Workload Identity.
# The KSA name and namespace must match what is deployed by the Helm chart
# (secretManager.serviceAccountName and the release namespace).
resource "google_service_account_iam_member" "workload_identity_binding" {
  service_account_id = google_service_account.gas_killer.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${var.project_id}.svc.id.goog[${var.kubernetes_namespace}/${var.kubernetes_service_account_name}]"
}

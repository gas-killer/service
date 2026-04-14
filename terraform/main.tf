terraform {
  required_version = ">= 1.0"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 5.0"
    }
  }
}

provider "google" {
  project = var.project_id
  region  = var.region
}

data "google_client_config" "current" {}

# Enable required GCP APIs
resource "google_project_service" "container" {
  service            = "container.googleapis.com"
  disable_on_destroy = false
}

resource "google_project_service" "secretmanager" {
  service            = "secretmanager.googleapis.com"
  disable_on_destroy = false
}

# GKE cluster
resource "google_container_cluster" "primary" {
  name     = var.cluster_name
  location = var.region

  depends_on = [google_project_service.container]

  deletion_protection = var.deletion_protection

  # We manage the node pool separately so we can configure Workload Identity on it
  remove_default_node_pool = true
  initial_node_count       = 1

  network    = google_compute_network.vpc.self_link
  subnetwork = google_compute_subnetwork.subnet.self_link

  # Enable Workload Identity at the cluster level
  workload_identity_config {
    workload_pool = "${var.project_id}.svc.id.goog"
  }

  # Use VPC-native networking with secondary ranges for pods and services
  ip_allocation_policy {
    cluster_secondary_range_name  = "pods"
    services_secondary_range_name = "services"
  }

  # Disable GKE Managed Prometheus — we manage our own monitoring stack (#140).
  # Without this, GMP deploys an Alertmanager that errors on startup because it
  # expects a secret called "alertmanager" in gmp-public that we never create.
  monitoring_config {
    managed_prometheus {
      enabled = false
    }
  }
}

# Node pool with Workload Identity enabled
resource "google_container_node_pool" "primary_nodes" {
  name    = "default-pool"
  cluster = google_container_cluster.primary.id

  # Pin to a single zone so node_count=1 means exactly 1 node.
  # A regional cluster spreads node_count across all zones by default
  # (3 zones in us-east4 = 3 nodes even with node_count=1).
  node_locations = [var.zone]
  node_count     = var.node_count

  node_config {
    machine_type    = var.machine_type
    disk_size_gb    = var.disk_size_gb
    spot            = var.spot_nodes
    service_account = google_service_account.gke_node.email

    # GKE_METADATA mode is required for Workload Identity to work on nodes
    workload_metadata_config {
      mode = "GKE_METADATA"
    }

    # cloud-platform scope allows access to all GCP APIs that IAM controls
    oauth_scopes = [
      "https://www.googleapis.com/auth/cloud-platform",
    ]
  }
}

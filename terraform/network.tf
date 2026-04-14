# VPC for the GKE cluster
resource "google_compute_network" "vpc" {
  name                    = "${var.cluster_name}-vpc"
  auto_create_subnetworks = false
}

# Subnet with secondary ranges for GKE pods and services
resource "google_compute_subnetwork" "subnet" {
  name    = "${var.cluster_name}-subnet"
  region  = var.region
  network = google_compute_network.vpc.id

  ip_cidr_range = "10.0.0.0/24"

  # Allows pods to reach GCP APIs (Secret Manager, etc.) without external IPs
  private_ip_google_access = true

  secondary_ip_range {
    range_name    = "pods"
    ip_cidr_range = "10.1.0.0/16"
  }

  secondary_ip_range {
    range_name    = "services"
    ip_cidr_range = "10.2.0.0/20"
  }
}

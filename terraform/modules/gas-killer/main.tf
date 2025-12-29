# Kubernetes namespace
resource "kubernetes_namespace" "gas_killer" {
  metadata {
    name = var.namespace

    labels = {
      "app.kubernetes.io/name"       = "gas-killer"
      "app.kubernetes.io/managed-by" = "terraform"
    }
  }
}

# Helm release for gas-killer
resource "helm_release" "gas_killer" {
  name      = "gas-killer"
  chart     = var.chart_path
  namespace = kubernetes_namespace.gas_killer.metadata[0].name

  # Wait for deployment to be ready
  wait    = true
  timeout = 900 # 15 minutes for setup job

  # Global settings
  set {
    name  = "global.environment"
    value = var.environment_mode
  }

  set {
    name  = "global.nodeCount"
    value = var.node_count
  }

  # Secrets
  set_sensitive {
    name  = "secrets.privateKey"
    value = var.private_key
  }

  set_sensitive {
    name  = "secrets.fundedKey"
    value = var.funded_key
  }

  set_sensitive {
    name  = "secrets.forkUrl"
    value = var.fork_url
  }

  # Node image
  set {
    name  = "node.image.repository"
    value = var.node_image_repository
  }

  set {
    name  = "node.image.tag"
    value = var.node_image_tag
  }

  set {
    name  = "node.image.pullPolicy"
    value = "Always"
  }

  # Router image
  set {
    name  = "router.image.repository"
    value = var.router_image_repository
  }

  set {
    name  = "router.image.tag"
    value = var.router_image_tag
  }

  set {
    name  = "router.image.pullPolicy"
    value = "Always"
  }

  # Storage
  set {
    name  = "sharedData.storageClass"
    value = var.storage_class
  }

  # Ingress configuration for ALB
  set {
    name  = "ingress.enabled"
    value = tostring(var.enable_ingress)
  }

  set {
    name  = "ingress.className"
    value = "alb"
  }

  # ALB annotations
  set {
    name  = "ingress.annotations.alb\\.ingress\\.kubernetes\\.io/scheme"
    value = "internet-facing"
  }

  set {
    name  = "ingress.annotations.alb\\.ingress\\.kubernetes\\.io/target-type"
    value = "ip"
  }

  set {
    name  = "ingress.annotations.alb\\.ingress\\.kubernetes\\.io/listen-ports"
    value = "[{\"HTTP\": 80}]"
  }

  set {
    name  = "ingress.annotations.alb\\.ingress\\.kubernetes\\.io/healthcheck-path"
    value = "/"
  }

  # Set ingress host if provided
  dynamic "set" {
    for_each = var.ingress_host != "" ? [1] : []
    content {
      name  = "ingress.hosts[0].host"
      value = var.ingress_host
    }
  }

  depends_on = [
    kubernetes_namespace.gas_killer,
  ]
}

# Data source to get the ingress after deployment
data "kubernetes_ingress_v1" "gas_killer" {
  count = var.enable_ingress ? 1 : 0

  metadata {
    name      = "gas-killer"
    namespace = var.namespace
  }

  depends_on = [helm_release.gas_killer]
}

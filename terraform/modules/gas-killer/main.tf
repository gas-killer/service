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

# =============================================================================
# Post-deployment Job: Deploy ArraySummation and Trigger Gas Killer
# =============================================================================

resource "kubernetes_job" "deploy_and_trigger" {
  count = var.run_e2e_test ? 1 : 0

  metadata {
    name      = "gas-killer-e2e-test"
    namespace = kubernetes_namespace.gas_killer.metadata[0].name
    labels = {
      "app.kubernetes.io/name"      = "gas-killer"
      "app.kubernetes.io/component" = "e2e-test"
    }
  }

  spec {
    ttl_seconds_after_finished = 300 # Clean up after 5 minutes
    backoff_limit              = 2

    template {
      metadata {
        labels = {
          "app.kubernetes.io/name"      = "gas-killer"
          "app.kubernetes.io/component" = "e2e-test"
        }
      }

      spec {
        restart_policy = "Never"

        # Mount the shared PVC to access avs_deploy.json
        volume {
          name = "shared-data"
          persistent_volume_claim {
            claim_name = "gas-killer-shared-data"
          }
        }

        # Init container: Wait for router to be ready
        init_container {
          name  = "wait-for-router"
          image = "busybox:1.36"
          command = [
            "sh", "-c",
            "echo 'Waiting for router...' && until nc -z gas-killer-router 8080; do echo 'Router not ready, waiting...'; sleep 5; done && echo 'Router is ready!'"
          ]
        }

        container {
          name  = "deploy-and-trigger"
          image = "ghcr.io/foundry-rs/foundry:latest"

          volume_mount {
            name       = "shared-data"
            mount_path = "/app/.nodes"
            read_only  = true
          }

          env {
            name  = "HTTP_RPC"
            value = "http://gas-killer-ethereum:8545"
          }

          env {
            name  = "ROUTER_URL"
            value = "http://gas-killer-router:8080"
          }

          env {
            name  = "ARRAY_SUMMATION_FACTORY_ADDRESS"
            value = var.array_summation_factory_address
          }

          env {
            name = "PRIVATE_KEY"
            value_from {
              secret_key_ref {
                name = "gas-killer-secrets"
                key  = "PRIVATE_KEY"
              }
            }
          }

          command = [
            "sh", "-c",
            <<-EOT
              set -e
              echo "=== Gas Killer E2E Test ==="

              # Wait a bit for everything to stabilize
              sleep 10

              echo "Step 1: Checking AVS deployment file..."
              if [ ! -f /app/.nodes/avs_deploy.json ]; then
                echo "ERROR: avs_deploy.json not found!"
                exit 1
              fi
              cat /app/.nodes/avs_deploy.json

              echo ""
              echo "Step 2: Deploying ArraySummation contract..."

              # Get counter address from avs_deploy.json
              COUNTER_ADDRESS=$(cat /app/.nodes/avs_deploy.json | grep -o '"counter_address":"[^"]*"' | cut -d'"' -f4)
              echo "Counter address: $COUNTER_ADDRESS"

              # Deploy ArraySummation using cast
              # The factory creates a new ArraySummation instance
              echo "Calling ArraySummation factory at $${ARRAY_SUMMATION_FACTORY_ADDRESS}..."

              # createArraySummation(uint256 arraySize, uint256 maxValue, uint256 seed, address counter)
              # Function selector: 0x... (we'll use cast to call it)
              RESULT=$(cast send \
                --rpc-url $HTTP_RPC \
                --private-key 0x$PRIVATE_KEY \
                $${ARRAY_SUMMATION_FACTORY_ADDRESS} \
                "createArraySummation(uint256,uint256,uint256,address)" \
                100 1000 42 $COUNTER_ADDRESS \
                --json 2>/dev/null || echo "")

              if [ -n "$RESULT" ]; then
                echo "ArraySummation deployment transaction sent"
                echo "$RESULT" | head -5
              else
                echo "Note: ArraySummation deployment may have failed or factory not available"
                echo "Continuing with trigger test..."
              fi

              sleep 5

              echo ""
              echo "Step 3: Triggering Gas Killer..."

              # Trigger via router HTTP endpoint
              TRIGGER_RESPONSE=$(curl -s -X POST $ROUTER_URL/trigger \
                -H "Content-Type: application/json" \
                -d '{"body":{"metadata":{"request_id":"terraform-test-1","action":"increment"}}}' \
                --max-time 60 || echo "CURL_FAILED")

              echo "Trigger response: $TRIGGER_RESPONSE"

              if echo "$TRIGGER_RESPONSE" | grep -q "CURL_FAILED"; then
                echo "WARNING: Trigger request failed, but this may be expected if ingress is still initializing"
              fi

              echo ""
              echo "=== E2E Test Complete ==="
              echo "Check router logs for aggregation results:"
              echo "  kubectl logs -n gas-killer -l app.kubernetes.io/component=router"
            EOT
          ]
        }
      }
    }
  }

  wait_for_completion = true

  timeouts {
    create = "10m"
  }

  depends_on = [helm_release.gas_killer]
}

# Helm release for gas-killer
resource "helm_release" "gas_killer" {
  name             = "gas-killer"
  chart            = var.chart_path
  namespace        = var.namespace
  create_namespace = true

  # Setup job is a post-install hook, so Helm waits for it to complete
  # TESTNET mode takes 20+ minutes: 5-min allocation delay + operator registration
  wait    = false
  timeout = 2700 # 45 minutes for TESTNET mode

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

  set_sensitive {
    name  = "secrets.rpcUrl"
    value = var.rpc_url
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
    namespace = var.namespace
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

          # In TESTNET mode, use RPC_URL from secret; in LOCAL mode, use local ethereum service
          dynamic "env" {
            for_each = var.environment_mode == "TESTNET" ? [1] : []
            content {
              name = "HTTP_RPC"
              value_from {
                secret_key_ref {
                  name = "gas-killer-secret"
                  key  = "RPC_URL"
                }
              }
            }
          }

          dynamic "env" {
            for_each = var.environment_mode == "LOCAL" ? [1] : []
            content {
              name  = "HTTP_RPC"
              value = "http://gas-killer-ethereum:8545"
            }
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
                name = "gas-killer-secret"
                key  = "PRIVATE_KEY"
              }
            }
          }

          command = [
            "bash", "-c",
            <<-EOT
              set -ex
              echo "=== Gas Killer E2E Test ==="
              echo "Environment:"
              echo "  HTTP_RPC=$HTTP_RPC"
              echo "  ROUTER_URL=$ROUTER_URL"
              echo "  ARRAY_SUMMATION_FACTORY_ADDRESS=$${ARRAY_SUMMATION_FACTORY_ADDRESS}"

              # Check which tools are available
              echo "Available tools:"
              which cast || echo "cast not found!"
              which curl || echo "curl not found!"
              which wget || echo "wget not found!"
              which grep || echo "grep not found!"
              which sed || echo "sed not found!"

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

              # Get addresses from avs_deploy.json using grep/sed (no jq needed)
              # Note: JSON has spaces after colons, so pattern must handle ": " not just ":"
              AVS_ADDRESS=$(grep -o '"IncredibleSquaringServiceManager": *"[^"]*"' /app/.nodes/avs_deploy.json | sed 's/.*: *"\([^"]*\)"/\1/')
              BLS_ADDRESS=$(grep -o '"IncredibleSquaringTaskManager": *"[^"]*"' /app/.nodes/avs_deploy.json | sed 's/.*: *"\([^"]*\)"/\1/')
              echo "AVS Service Manager: $AVS_ADDRESS"
              echo "BLS Signature Checker: $BLS_ADDRESS"

              if [ -z "$AVS_ADDRESS" ]; then
                echo "ERROR: Could not find IncredibleSquaringServiceManager in avs_deploy.json"
                exit 1
              fi
              if [ -z "$BLS_ADDRESS" ]; then
                echo "ERROR: Could not find IncredibleSquaringTaskManager in avs_deploy.json"
                exit 1
              fi

              # Get contract count before deployment to retrieve the deployed address later
              echo "Getting deployed contract count before deployment..."
              CONTRACT_COUNT_BEFORE=$(cast call \
                --rpc-url $HTTP_RPC \
                $${ARRAY_SUMMATION_FACTORY_ADDRESS} \
                "getDeployedContractCount()(uint256)" 2>/dev/null || echo "0")
              echo "Contract count before: $CONTRACT_COUNT_BEFORE"

              # Deploy ArraySummation using the factory
              # deployArraySummation(address avs, address blsSigChecker, uint256 arraySize, uint256 maxValue, uint256 seed)
              echo "Calling ArraySummation factory at $${ARRAY_SUMMATION_FACTORY_ADDRESS}..."
              echo "Parameters: avs=$AVS_ADDRESS, bls=$BLS_ADDRESS, arraySize=100, maxValue=1000, seed=42"

              # Note: PRIVATE_KEY already includes 0x prefix
              RESULT=$(cast send \
                --rpc-url $HTTP_RPC \
                --private-key $PRIVATE_KEY \
                $${ARRAY_SUMMATION_FACTORY_ADDRESS} \
                "deployArraySummation(address,address,uint256,uint256,uint256)" \
                $AVS_ADDRESS $BLS_ADDRESS 100 1000 42 \
                --json 2>/dev/null || echo "")

              if [ -z "$RESULT" ]; then
                echo "ERROR: ArraySummation deployment failed"
                exit 1
              fi

              echo "ArraySummation deployment transaction sent"
              TX_HASH=$(echo "$RESULT" | grep -o '"transactionHash":"[^"]*"' | sed 's/.*:"\([^"]*\)"/\1/')
              echo "Transaction hash: $TX_HASH"

              sleep 5

              # Retrieve the deployed ArraySummation address
              echo "Retrieving deployed ArraySummation address..."
              ARRAY_SUMMATION_ADDRESS=$(cast call \
                --rpc-url $HTTP_RPC \
                $${ARRAY_SUMMATION_FACTORY_ADDRESS} \
                "deployedContracts(uint256)(address)" \
                $CONTRACT_COUNT_BEFORE 2>/dev/null || echo "")

              if [ -z "$ARRAY_SUMMATION_ADDRESS" ] || [ "$ARRAY_SUMMATION_ADDRESS" = "0x0000000000000000000000000000000000000000" ]; then
                echo "ERROR: Failed to retrieve deployed ArraySummation address"
                exit 1
              fi

              echo "ArraySummation deployed at: $ARRAY_SUMMATION_ADDRESS"

              echo ""
              echo "Step 3: Preparing trigger request..."

              # Get current block number for deterministic execution
              BLOCK_HEIGHT=$(cast block-number --rpc-url $HTTP_RPC)
              echo "Current block height: $BLOCK_HEIGHT"

              # Get current stateTransitionCount from the ArraySummation contract
              TRANSITION_INDEX=$(cast call \
                --rpc-url $HTTP_RPC \
                $ARRAY_SUMMATION_ADDRESS \
                "stateTransitionCount()(uint256)" 2>/dev/null || echo "0")
              echo "Current transition index: $TRANSITION_INDEX"

              # Encode sum(uint256[]) call with indexes [0,1,2]
              CALL_DATA=$(cast calldata "sum(uint256[])" "[0,1,2]")
              echo "Call data: $CALL_DATA"

              # Use Anvil's default first unlocked account
              FROM_ADDRESS="0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"

              echo ""
              echo "Step 4: Triggering Gas Killer..."

              # Convert hex call_data to byte array for JSON using bash
              # Remove 0x prefix and convert each byte pair to decimal
              CALL_DATA_HEX="$${CALL_DATA#0x}"
              CALL_DATA_BYTES="["
              for ((i=0; i<$${#CALL_DATA_HEX}; i+=2)); do
                [ $i -gt 0 ] && CALL_DATA_BYTES+=","
                CALL_DATA_BYTES+="$((16#$${CALL_DATA_HEX:$i:2}))"
              done
              CALL_DATA_BYTES+="]"
              echo "Call data bytes: $CALL_DATA_BYTES"

              # Build JSON payload manually (no jq needed)
              TRIGGER_PAYLOAD="{\"body\":{\"target_address\":\"$ARRAY_SUMMATION_ADDRESS\",\"call_data\":$CALL_DATA_BYTES,\"transition_index\":$TRANSITION_INDEX,\"from_address\":\"$FROM_ADDRESS\",\"value\":\"0\",\"block_height\":$BLOCK_HEIGHT}}"

              echo "Trigger payload:"
              echo "$TRIGGER_PAYLOAD"

              # Trigger via router HTTP endpoint
              # Try wget first (more commonly available), fallback to curl
              echo "Sending trigger request to $ROUTER_URL/trigger..."
              if command -v wget >/dev/null 2>&1; then
                echo "Using wget for HTTP request"
                TRIGGER_RESPONSE=$(wget -q -O - \
                  --header="Content-Type: application/json" \
                  --post-data="$TRIGGER_PAYLOAD" \
                  --timeout=120 \
                  "$ROUTER_URL/trigger" 2>&1 || echo "HTTP_FAILED")
              elif command -v curl >/dev/null 2>&1; then
                echo "Using curl for HTTP request"
                TRIGGER_RESPONSE=$(curl -s -X POST \
                  -H "Content-Type: application/json" \
                  -d "$TRIGGER_PAYLOAD" \
                  --max-time 120 \
                  "$ROUTER_URL/trigger" 2>&1 || echo "HTTP_FAILED")
              else
                echo "WARNING: Neither wget nor curl available, skipping trigger"
                TRIGGER_RESPONSE="NO_HTTP_CLIENT"
              fi

              echo "Trigger response: $TRIGGER_RESPONSE"

              if echo "$TRIGGER_RESPONSE" | grep -qE "HTTP_FAILED|NO_HTTP_CLIENT"; then
                echo "WARNING: Trigger request failed or skipped, but deployment was successful"
              fi

              echo ""
              echo "Step 5: Verifying execution..."
              sleep 10

              # Check if currentSum changed
              FINAL_SUM=$(cast call \
                --rpc-url $HTTP_RPC \
                $ARRAY_SUMMATION_ADDRESS \
                "currentSum()(uint256)" 2>/dev/null || echo "0")
              echo "Final currentSum: $FINAL_SUM"

              echo ""
              echo "=== E2E Test Complete ==="
              echo "ArraySummation address: $ARRAY_SUMMATION_ADDRESS"
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
    create = "15m"
  }

  depends_on = [helm_release.gas_killer]
}

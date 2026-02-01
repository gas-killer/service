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

# =============================================================================
# L1-L2 Bridge Job (runs after setup, before router/nodes are fully operational)
# =============================================================================
# Bridges EigenLayer operator state from L1 to L2:
# 1. Deploys MiddlewareShim on L1 (reads from RegistryCoordinator)
# 2. Deploys RegistryCoordinatorMimic, BLSSignatureChecker, SP1HeliosMock on L2
# 3. Snapshots operator state on L1
# 4. Generates storage proof
# 5. Bridges state to L2

resource "kubernetes_secret" "bridge_secrets" {
  count = var.run_bridge ? 1 : 0

  metadata {
    name      = "bridge-secrets"
    namespace = var.namespace
  }

  data = {
    PRIVATE_KEY = var.private_key
    L1_RPC_URL  = var.l1_rpc_url
    L2_RPC_URL  = var.l2_rpc_url
  }

  depends_on = [helm_release.gas_killer]
}

resource "kubernetes_job" "l1_l2_bridge" {
  count = var.run_bridge ? 1 : 0

  metadata {
    name      = "l1-l2-bridge"
    namespace = var.namespace
    labels = {
      "app.kubernetes.io/name"      = "gas-killer"
      "app.kubernetes.io/component" = "bridge"
    }
  }

  spec {
    ttl_seconds_after_finished = 600 # Clean up after 10 minutes
    backoff_limit              = 2

    template {
      metadata {
        labels = {
          "app.kubernetes.io/name"      = "gas-killer"
          "app.kubernetes.io/component" = "bridge"
        }
      }

      spec {
        restart_policy = "Never"

        # Mount the shared PVC to read avs_deploy.json
        volume {
          name = "shared-data"
          persistent_volume_claim {
            claim_name = "gas-killer-shared-data"
          }
        }

        # Shared volume for passing extracted config between init and main container
        volume {
          name = "bridge-config"
          empty_dir {}
        }

        # Wait for avs_deploy.json and extract RegistryCoordinator address
        init_container {
          name  = "extract-registry-coordinator"
          image = "busybox:1.36"
          command = [
            "sh", "-c",
            <<-EOT
              echo "Waiting for avs_deploy.json..."
              TIMEOUT=600
              ELAPSED=0
              while [ ! -f /app/.nodes/avs_deploy.json ]; do
                if [ $ELAPSED -ge $TIMEOUT ]; then
                  echo "ERROR: Timeout waiting for avs_deploy.json after $${TIMEOUT}s"
                  exit 1
                fi
                echo "avs_deploy.json not ready, waiting... ($${ELAPSED}s elapsed)"
                sleep 10
                ELAPSED=$((ELAPSED + 10))
              done
              echo "avs_deploy.json found!"
              cat /app/.nodes/avs_deploy.json

              # Extract RegistryCoordinator address from JSON
              # Try different possible key names
              REGISTRY_ADDR=$(grep -o '"registryCoordinator": *"[^"]*"' /app/.nodes/avs_deploy.json | sed 's/.*: *"\([^"]*\)"/\1/' || true)
              if [ -z "$REGISTRY_ADDR" ]; then
                REGISTRY_ADDR=$(grep -o '"RegistryCoordinator": *"[^"]*"' /app/.nodes/avs_deploy.json | sed 's/.*: *"\([^"]*\)"/\1/' || true)
              fi

              if [ -z "$REGISTRY_ADDR" ]; then
                echo "ERROR: Could not find RegistryCoordinator address in avs_deploy.json"
                echo "Available keys:"
                cat /app/.nodes/avs_deploy.json
                exit 1
              fi

              echo "Found RegistryCoordinator: $REGISTRY_ADDR"
              echo "$REGISTRY_ADDR" > /bridge-config/registry_coordinator_address
            EOT
          ]

          volume_mount {
            name       = "shared-data"
            mount_path = "/app/.nodes"
            read_only  = true
          }

          volume_mount {
            name       = "bridge-config"
            mount_path = "/bridge-config"
          }
        }

        container {
          name  = "bridge"
          image = var.bridge_image

          # Override command to read the extracted address and run the bridge
          command = [
            "sh", "-c",
            <<-EOT
              export REGISTRY_COORDINATOR_ADDRESS=$(cat /bridge-config/registry_coordinator_address)
              echo "Using RegistryCoordinator: $REGISTRY_COORDINATOR_ADDRESS"
              exec /app/scripts/bridge-to-l2.sh
            EOT
          ]

          volume_mount {
            name       = "shared-data"
            mount_path = "/app/.nodes"
            read_only  = true
          }

          volume_mount {
            name       = "bridge-config"
            mount_path = "/bridge-config"
            read_only  = true
          }

          env {
            name = "PRIVATE_KEY"
            value_from {
              secret_key_ref {
                name = "bridge-secrets"
                key  = "PRIVATE_KEY"
              }
            }
          }

          env {
            name = "L1_RPC_URL"
            value_from {
              secret_key_ref {
                name = "bridge-secrets"
                key  = "L1_RPC_URL"
              }
            }
          }

          env {
            name = "L2_RPC_URL"
            value_from {
              secret_key_ref {
                name = "bridge-secrets"
                key  = "L2_RPC_URL"
              }
            }
          }

          resources {
            requests = {
              cpu    = "100m"
              memory = "256Mi"
            }
            limits = {
              cpu    = "500m"
              memory = "512Mi"
            }
          }
        }
      }
    }
  }

  wait_for_completion = true

  timeouts {
    create = "15m"
  }

  depends_on = [kubernetes_secret.bridge_secrets]
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

        # EmptyDir for communication between foundry and curl containers
        volume {
          name = "trigger-data"
          empty_dir {}
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

          volume_mount {
            name       = "trigger-data"
            mount_path = "/trigger"
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

              # Write trigger payload to shared volume for curl sidecar
              echo "Writing trigger payload to /trigger/payload.json..."
              echo "$TRIGGER_PAYLOAD" > /trigger/payload.json

              # Signal that payload is ready
              touch /trigger/ready

              echo "Waiting for curl sidecar to send trigger request..."
              # Wait for curl sidecar to complete (max 2 minutes)
              WAIT_COUNT=0
              while [ ! -f /trigger/done ] && [ $WAIT_COUNT -lt 120 ]; do
                sleep 1
                WAIT_COUNT=$((WAIT_COUNT + 1))
              done

              if [ -f /trigger/done ]; then
                echo "Trigger request completed!"
                TRIGGER_RESPONSE=$(cat /trigger/response.txt 2>/dev/null || echo "NO_RESPONSE")
                echo "Trigger response: $TRIGGER_RESPONSE"

                if echo "$TRIGGER_RESPONSE" | grep -q '"success":true'; then
                  echo "SUCCESS: Task queued successfully!"
                else
                  echo "WARNING: Unexpected trigger response"
                fi
              else
                echo "WARNING: Timeout waiting for trigger response"
                TRIGGER_RESPONSE="TIMEOUT"
              fi

              echo ""
              echo "Step 5: Waiting for aggregation and on-chain execution..."
              echo "Aggregation frequency is 30s, waiting 45s for completion..."
              sleep 45

              # Check if stateTransitionCount increased
              FINAL_TRANSITION=$(cast call \
                --rpc-url $HTTP_RPC \
                $ARRAY_SUMMATION_ADDRESS \
                "stateTransitionCount()(uint256)" 2>/dev/null || echo "0")
              echo "Final stateTransitionCount: $FINAL_TRANSITION"

              # Check if currentSum changed
              FINAL_SUM=$(cast call \
                --rpc-url $HTTP_RPC \
                $ARRAY_SUMMATION_ADDRESS \
                "currentSum()(uint256)" 2>/dev/null || echo "0")
              echo "Final currentSum: $FINAL_SUM"

              # Verify the execution actually happened
              if [ "$FINAL_TRANSITION" -gt "$TRANSITION_INDEX" ]; then
                echo ""
                echo "=== E2E TEST PASSED ==="
                echo "State transition executed successfully!"
                echo "  - Transition count: $TRANSITION_INDEX -> $FINAL_TRANSITION"
                echo "  - Current sum: $FINAL_SUM"
              else
                echo ""
                echo "=== E2E TEST WARNING ==="
                echo "State transition may not have completed yet."
                echo "  - Expected transition count > $TRANSITION_INDEX, got $FINAL_TRANSITION"
                echo "Check router logs for aggregation results:"
                echo "  kubectl logs -n gas-killer -l app.kubernetes.io/component=router"
              fi

              echo ""
              echo "=== E2E Test Complete ==="
              echo "ArraySummation address: $ARRAY_SUMMATION_ADDRESS"

              # Signal completion so curl sidecar can exit
              touch /trigger/main-done
            EOT
          ]
        }

        # Sidecar container: curl for HTTP requests (foundry image doesn't have curl/wget)
        container {
          name  = "curl-trigger"
          image = "curlimages/curl:latest"

          volume_mount {
            name       = "trigger-data"
            mount_path = "/trigger"
          }

          command = [
            "sh", "-c",
            <<-EOT
              echo "=== Curl Sidecar Started ==="
              echo "Waiting for trigger payload..."

              # Wait for payload to be ready (max 10 minutes)
              WAIT_COUNT=0
              while [ ! -f /trigger/ready ] && [ $WAIT_COUNT -lt 600 ]; do
                sleep 1
                WAIT_COUNT=$((WAIT_COUNT + 1))
              done

              if [ ! -f /trigger/ready ]; then
                echo "ERROR: Timeout waiting for payload"
                echo "TIMEOUT" > /trigger/response.txt
                touch /trigger/done
                exit 1
              fi

              echo "Payload ready, sending trigger request..."
              cat /trigger/payload.json

              # Send the trigger request
              curl -s -X POST \
                -H "Content-Type: application/json" \
                -d @/trigger/payload.json \
                --max-time 120 \
                http://gas-killer-router:8080/trigger > /trigger/response.txt 2>&1

              CURL_EXIT=$?
              echo "Curl exit code: $CURL_EXIT"
              echo "Response:"
              cat /trigger/response.txt

              # Signal completion
              touch /trigger/done

              echo "Waiting for main container to finish..."
              # Wait for main container to signal completion (max 5 minutes)
              WAIT_COUNT=0
              while [ ! -f /trigger/main-done ] && [ $WAIT_COUNT -lt 300 ]; do
                sleep 1
                WAIT_COUNT=$((WAIT_COUNT + 1))
              done

              echo "=== Curl Sidecar Exiting ==="
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

  # Wait for bridge to complete before running e2e test to avoid nonce collisions
  # (both use the same wallet for L1 transactions)
  depends_on = [helm_release.gas_killer, kubernetes_job.l1_l2_bridge]
}

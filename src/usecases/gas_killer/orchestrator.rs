use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use super::types::{
    GasKillerTask, ValidationRequest, OperatorResponse, 
    ExecutionPackage, OperatorRegistry, OperatorStatus
};
use super::creator::GasKillerCreator;
use super::validator::GasKillerValidator;
use super::executor::{GasKillerExecutor, ExecutionResult};

/// Configuration for the Gas Killer Orchestrator
pub struct GasKillerOrchestratorConfig {
    pub aggregation_frequency: Duration,
    pub validation_timeout: Duration,
    pub quorum_threshold: f64, // e.g., 0.67 for 2/3+1
    pub min_operators: usize,
    pub max_retries: u32,
    pub retry_delay: Duration,
}

impl Default for GasKillerOrchestratorConfig {
    fn default() -> Self {
        Self {
            aggregation_frequency: Duration::from_secs(10),
            validation_timeout: Duration::from_secs(30),
            quorum_threshold: 0.67,
            min_operators: 3,
            max_retries: 3,
            retry_delay: Duration::from_secs(5),
        }
    }
}

/// Gas Killer Orchestrator - coordinates task validation and execution
/// This is the main component that implements the orchestration logic from Issue #16
pub struct GasKillerOrchestrator {
    config: GasKillerOrchestratorConfig,
    creator: GasKillerCreator,
    validator: GasKillerValidator,
    executor: GasKillerExecutor,
    operator_registry: Arc<RwLock<OperatorRegistry>>,
    validation_requests: Arc<RwLock<HashMap<[u8; 32], ValidationRequest>>>,
    operator_responses: Arc<RwLock<HashMap<[u8; 32], Vec<OperatorResponse>>>>,
}

impl GasKillerOrchestrator {
    pub fn new(config: GasKillerOrchestratorConfig) -> Self {
        Self {
            config,
            creator: GasKillerCreator::new(),
            validator: GasKillerValidator::new(),
            executor: GasKillerExecutor::new(),
            operator_registry: Arc::new(RwLock::new(OperatorRegistry::new())),
            validation_requests: Arc::new(RwLock::new(HashMap::new())),
            operator_responses: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Main orchestration loop - implements the flow from Issue #16
    pub async fn run(&self) -> Result<(), String> {
        println!("Starting Gas Killer Orchestrator...");
        
        loop {
            // Step 1: Get task from Creator
            let (payload, round) = match self.creator.get_payload_and_round().await {
                Ok(result) => result,
                Err(e) => {
                    println!("Failed to get payload: {}", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            // Parse task from payload
            let task = GasKillerTask::from_bytes(&payload)?;
            
            println!(
                "Processing Gas Killer task: {:02x?}..., round: {}, chain: {}",
                &task.task_id[..8],
                round,
                task.chain_id
            );

            // Step 2: Query operator registry for active operators
            let operators = self.query_operator_registry(task.chain_id).await?;

            // Step 3: Broadcast ValidationRequest to operators
            self.broadcast_validation_request(&task, operators.clone(), round).await?;

            // Step 4: Trigger Validator for gas analysis
            let hash = self.validator.validate_and_return_expected_hash(&task).await?;

            // Step 5: Collect operator responses (simulated)
            self.simulate_operator_responses(&task, &operators, hash).await?;

            // Step 6: Check quorum
            if !self.check_quorum(&task.task_id).await {
                println!("Quorum not reached for task {:02x?}...", &task.task_id[..8]);
                continue;
            }

            // Step 7: Aggregate signatures (mock BLS aggregation)
            let package = self.aggregate_signatures(&task.task_id).await?;

            // Step 8: Create ExecutionPackage - already done in aggregate_signatures

            // Step 9: Broadcast to Executor
            match self.executor.execute_verification(package).await {
                Ok(result) => {
                    println!(
                        "Successfully executed Gas Killer task: {:02x?}..., tx: {}",
                        &task.task_id[..8],
                        result.transaction_hash
                    );
                }
                Err(e) => {
                    println!(
                        "Failed to execute Gas Killer task: {:02x?}..., error: {}",
                        &task.task_id[..8],
                        e
                    );
                }
            }

            // Optional: Add delay before processing next task
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Query operator registry for active operators
    async fn query_operator_registry(&self, chain_id: u64) -> Result<Vec<[u8; 64]>, String> {
        let registry = self.operator_registry.read().await;
        let operators = registry.get_active_operators(chain_id);
        
        if operators.len() < self.config.min_operators {
            // For demo: add mock operators if not enough
            drop(registry);
            self.add_mock_operators(chain_id).await;
            let registry = self.operator_registry.read().await;
            let operators = registry.get_active_operators(chain_id);
            
            if operators.len() < self.config.min_operators {
                return Err(format!(
                    "Insufficient operators: {} < {}",
                    operators.len(),
                    self.config.min_operators
                ));
            }
        }
        
        Ok(operators)
    }

    /// Add mock operators for testing
    async fn add_mock_operators(&self, chain_id: u64) {
        let mut registry = self.operator_registry.write().await;
        let mut operators = Vec::new();
        
        for i in 0..5 {
            let mut operator = [0u8; 64];
            operator[0] = i as u8;
            operators.push(operator);
            registry.stakes.insert(operator, 1000000);
            registry.statuses.insert(operator, OperatorStatus::Active);
        }
        
        registry.operators.insert(chain_id, operators);
        println!("Added {} mock operators for chain {}", 5, chain_id);
    }

    /// Broadcast validation request to operators
    async fn broadcast_validation_request(
        &self,
        task: &GasKillerTask,
        operators: Vec<[u8; 64]>,
        round: u64,
    ) -> Result<(), String> {
        let deadline = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() + self.config.validation_timeout.as_secs();
        
        let request = ValidationRequest {
            task_id: task.task_id,
            task_data: task.clone(),
            validation_deadline: deadline,
            quorum_threshold: (self.config.quorum_threshold * 100.0) as u8,
            operator_set: operators.clone(),
        };
        
        let mut requests = self.validation_requests.write().await;
        requests.insert(task.task_id, request.clone());
        
        println!(
            "Broadcast validation request for task: {:02x?}..., operators: {}, deadline: {}",
            &task.task_id[..8],
            operators.len(),
            deadline
        );
        
        Ok(())
    }

    /// Simulate operator responses (in production would receive via network)
    async fn simulate_operator_responses(
        &self,
        task: &GasKillerTask,
        operators: &[[u8; 64]],
        hash: [u8; 32],
    ) -> Result<(), String> {
        let mut responses = self.operator_responses.write().await;
        let mut task_responses = Vec::new();
        
        // Simulate that 80% of operators respond positively
        let responding_count = ((operators.len() as f64) * 0.8).ceil() as usize;
        
        for i in 0..responding_count {
            let operator = operators[i];
            
            // Get state updates from validator
            let state_updates = self.validator.get_state_updates(&task.task_id).await
                .unwrap_or_default();
            
            // Create mock signature (in production would be BLS signature)
            let mut signature = vec![0u8; 96];
            signature[0] = i as u8;
            signature[1..33].copy_from_slice(&hash);
            
            let response = OperatorResponse {
                operator,
                validated: true,
                signature,
                state_updates,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            };
            
            task_responses.push(response);
        }
        
        responses.insert(task.task_id, task_responses);
        println!(
            "Received {} operator responses for task {:02x?}...",
            responding_count,
            &task.task_id[..8]
        );
        
        Ok(())
    }

    /// Check if quorum is reached
    async fn check_quorum(&self, task_id: &[u8; 32]) -> bool {
        let responses = self.operator_responses.read().await;
        let task_responses = responses.get(task_id);
        
        if let Some(resps) = task_responses {
            let valid_responses = resps.iter().filter(|r| r.validated).count();
            let requests = self.validation_requests.read().await;
            
            if let Some(request) = requests.get(task_id) {
                let required = ((request.operator_set.len() as f64) * self.config.quorum_threshold).ceil() as usize;
                
                println!(
                    "Quorum check for task {:02x?}...: {}/{} (required: {})",
                    &task_id[..8],
                    valid_responses,
                    request.operator_set.len(),
                    required
                );
                
                return valid_responses >= required;
            }
        }
        
        false
    }

    /// Aggregate signatures from operators (mock BLS aggregation)
    async fn aggregate_signatures(&self, task_id: &[u8; 32]) -> Result<ExecutionPackage, String> {
        let responses = self.operator_responses.read().await;
        let task_responses = responses.get(task_id)
            .ok_or_else(|| "No responses for task".to_string())?;
        
        let valid_responses: Vec<_> = task_responses.iter()
            .filter(|r| r.validated)
            .collect();
        
        if valid_responses.is_empty() {
            return Err("No valid responses to aggregate".to_string());
        }
        
        // Mock BLS signature aggregation
        let mut aggregated_signature = vec![0u8; 96];
        for (i, response) in valid_responses.iter().enumerate() {
            // XOR signatures together (mock aggregation)
            for j in 0..96.min(response.signature.len()) {
                aggregated_signature[j] ^= response.signature[j];
            }
        }
        
        let mut signers = Vec::new();
        let mut all_state_updates = Vec::new();
        
        for response in &valid_responses {
            signers.push(response.operator.clone());
            all_state_updates.extend(response.state_updates.clone());
        }
        
        let requests = self.validation_requests.read().await;
        let request = requests.get(task_id)
            .ok_or_else(|| "No validation request found".to_string())?;
        
        let package = ExecutionPackage {
            task_id: *task_id,
            validated_data: request.task_data.clone(),
            state_updates: all_state_updates,
            aggregated_signature,
            signers,
            validation_timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };
        
        println!(
            "Created execution package for task {:02x?}... with {} signatures",
            &task_id[..8],
            valid_responses.len()
        );
        
        Ok(package)
    }

    /// Handle Byzantine operators (mark as slashed)
    pub async fn handle_byzantine_operators(&self, malicious: Vec<[u8; 64]>) {
        let mut registry = self.operator_registry.write().await;
        
        for operator in malicious {
            registry.statuses.insert(operator, OperatorStatus::Slashed);
            println!(
                "Slashed Byzantine operator: {:02x?}...",
                &operator[..8]
            );
        }
    }

    /// Add a task to the queue
    pub fn add_task(&self, task: GasKillerTask) -> Result<(), String> {
        self.creator.add_task(task)
    }
}
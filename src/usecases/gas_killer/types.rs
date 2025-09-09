use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasKillerTask {
    pub task_id: [u8; 32],
    pub chain_id: u64,
    pub target_contract: [u8; 20],
    pub calldata: Vec<u8>,
    pub priority: u8,
    pub timestamp: u64,
}

impl GasKillerTask {
    /// Serialize task to bytes for hashing/signing
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.task_id);
        bytes.extend_from_slice(&self.chain_id.to_be_bytes());
        bytes.extend_from_slice(&self.target_contract);
        bytes.extend_from_slice(&(self.calldata.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&self.calldata);
        bytes.push(self.priority);
        bytes.extend_from_slice(&self.timestamp.to_be_bytes());
        bytes
    }
    
    /// Deserialize task from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 65 {
            return Err("Invalid bytes length".to_string());
        }
        
        let mut cursor = 0;
        let mut task_id = [0u8; 32];
        task_id.copy_from_slice(&bytes[cursor..cursor+32]);
        cursor += 32;
        
        let chain_id = u64::from_be_bytes(bytes[cursor..cursor+8].try_into().unwrap());
        cursor += 8;
        
        let mut target_contract = [0u8; 20];
        target_contract.copy_from_slice(&bytes[cursor..cursor+20]);
        cursor += 20;
        
        let calldata_len = u32::from_be_bytes(bytes[cursor..cursor+4].try_into().unwrap()) as usize;
        cursor += 4;
        
        let calldata = bytes[cursor..cursor+calldata_len].to_vec();
        cursor += calldata_len;
        
        let priority = bytes[cursor];
        cursor += 1;
        
        let timestamp = u64::from_be_bytes(bytes[cursor..cursor+8].try_into().unwrap());
        
        Ok(Self {
            task_id,
            chain_id,
            target_contract,
            calldata,
            priority,
            timestamp,
        })
    }
}

#[derive(Debug, Clone)]
pub struct StateUpdate {
    pub storage_slot: [u8; 32],
    pub old_value: [u8; 32],
    pub new_value: [u8; 32],
    pub gas_saved: u64,
}

#[derive(Debug, Clone)]
pub struct GasAnalysisResult {
    pub task_id: [u8; 32],
    pub state_updates: Vec<StateUpdate>,
    pub total_gas_saved: u64,
    pub optimization_type: OptimizationType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OptimizationType {
    StoragePacking,
    BatchedUpdates,
    ColdToWarmSlot,
    ZeroToNonZero,
    NonZeroToZero,
}

#[derive(Debug, Clone)]
pub struct ValidationRequest {
    pub task_id: [u8; 32],
    pub task_data: GasKillerTask,
    pub validation_deadline: u64,
    pub quorum_threshold: u8,
    pub operator_set: Vec<[u8; 64]>,
}

#[derive(Debug, Clone)]
pub struct OperatorResponse {
    pub operator: [u8; 64],
    pub validated: bool,
    pub signature: Vec<u8>,
    pub state_updates: Vec<StateUpdate>,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct ExecutionPackage {
    pub task_id: [u8; 32],
    pub validated_data: GasKillerTask,
    pub state_updates: Vec<StateUpdate>,
    pub aggregated_signature: Vec<u8>,
    pub signers: Vec<[u8; 64]>,
    pub validation_timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct OperatorRegistry {
    pub operators: HashMap<u64, Vec<[u8; 64]>>,
    pub stakes: HashMap<[u8; 64], u64>,
    pub statuses: HashMap<[u8; 64], OperatorStatus>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OperatorStatus {
    Active,
    Inactive,
    Slashed,
    Exited,
}

impl OperatorRegistry {
    pub fn new() -> Self {
        Self {
            operators: HashMap::new(),
            stakes: HashMap::new(),
            statuses: HashMap::new(),
        }
    }

    pub fn get_active_operators(&self, chain_id: u64) -> Vec<[u8; 64]> {
        self.operators
            .get(&chain_id)
            .map(|ops| {
                ops.iter()
                    .filter(|op| {
                        self.statuses
                            .get(*op)
                            .map(|s| *s == OperatorStatus::Active)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn check_stake_requirement(&self, operator: &[u8; 64], min_stake: u64) -> bool {
        self.stakes
            .get(operator)
            .map(|stake| *stake >= min_stake)
            .unwrap_or(false)
    }
}
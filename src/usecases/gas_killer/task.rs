use alloy_primitives::{Address, Bytes};
use anyhow::Result;
use bytes::{Buf, BufMut};
use commonware_codec::{EncodeSize, Error as CodecError, Read, ReadExt, Write};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    pub id: uuid::Uuid,
    pub request_id: uuid::Uuid,
    pub status: TaskStatus,
    pub target_contract: Address,
    pub target_method: String,
    pub target_chain_id: u64,
    pub params: Bytes,
    pub caller: Address,
    pub signature: Option<Bytes>,
    pub created_at: u64,
    pub priority: u8,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Pending,
    Processing,
    Validated,
    Executed,
    Failed(String),
}

impl Task {
    pub fn new(
        request_id: uuid::Uuid,
        target_contract: Address,
        target_method: String,
        target_chain_id: u64,
        params: Bytes,
        caller: Address,
        metadata: HashMap<String, String>,
    ) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            id: uuid::Uuid::new_v4(),
            request_id,
            status: TaskStatus::Pending,
            target_contract,
            target_method,
            target_chain_id,
            params,
            caller,
            signature: None,
            created_at,
            priority: Self::calculate_priority(&metadata, target_chain_id),
            metadata,
        }
    }

    pub fn calculate_priority(metadata: &HashMap<String, String>, chain_id: u64) -> u8 {
        let mut priority = 50u8;

        if let Some(gas_price) = metadata.get("gas_price") {
            if let Ok(price) = gas_price.parse::<u64>() {
                if price > 100_000_000_000 {
                    priority += 20;
                } else if price > 50_000_000_000 {
                    priority += 10;
                }
            }
        }

        match chain_id {
            1 => priority += 30,
            137 | 42161 => priority += 20,
            10 | 8453 => priority += 10,
            _ => {}
        }

        if metadata.contains_key("urgent") {
            priority = priority.saturating_add(25);
        }

        priority.min(100)
    }

    pub fn to_event(&self) -> TaskEvent {
        TaskEvent {
            task_id: self.id,
            priority: self.priority,
            chain_id: self.target_chain_id,
            event_type: "task_created".to_string(),
            timestamp: self.created_at,
            target_contract: self.target_contract,
            target_method: self.target_method.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub task_id: uuid::Uuid,
    pub priority: u8,
    pub chain_id: u64,
    pub event_type: String,
    pub timestamp: u64,
    pub target_contract: Address,
    pub target_method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueMessage {
    pub request_id: uuid::Uuid,
    pub target_contract: Address,
    pub target_method: String,
    pub target_chain_id: u64,
    pub params: Bytes,
    pub caller: Address,
    pub metadata: HashMap<String, String>,
}

impl QueueMessage {
    pub fn validate(&self) -> Result<()> {
        if self.target_contract == Address::ZERO {
            return Err(anyhow::anyhow!("Invalid target contract address"));
        }

        if self.target_method.is_empty() {
            return Err(anyhow::anyhow!("Target method cannot be empty"));
        }

        if self.target_chain_id == 0 {
            return Err(anyhow::anyhow!("Invalid chain ID"));
        }

        if self.caller == Address::ZERO {
            return Err(anyhow::anyhow!("Invalid caller address"));
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct GasKillerTaskData {
    pub task_id: uuid::Uuid,
    pub chain_id: u64,
    pub target_contract: Address,
    pub target_method: String,
    pub params: Vec<u8>,
}

impl Write for GasKillerTaskData {
    fn write(&self, buf: &mut impl BufMut) {
        let id_bytes = self.task_id.as_bytes();
        buf.put_slice(id_bytes);
        self.chain_id.write(buf);
        buf.put_slice(self.target_contract.as_slice());
        
        let method_bytes = self.target_method.as_bytes();
        (method_bytes.len() as u32).write(buf);
        buf.put_slice(method_bytes);
        
        (self.params.len() as u32).write(buf);
        buf.put_slice(&self.params);
    }
}

impl Read for GasKillerTaskData {
    type Cfg = ();
    
    fn read_cfg(buf: &mut impl Buf, _: &()) -> Result<Self, CodecError> {
        if buf.remaining() < 16 {
            return Err(CodecError::EndOfBuffer);
        }
        let mut id_bytes = [0u8; 16];
        buf.copy_to_slice(&mut id_bytes);
        let task_id = uuid::Uuid::from_bytes(id_bytes);
        
        let chain_id = u64::read(buf)?;
        
        if buf.remaining() < 20 {
            return Err(CodecError::EndOfBuffer);
        }
        let mut contract_bytes = [0u8; 20];
        buf.copy_to_slice(&mut contract_bytes);
        let target_contract = Address::from_slice(&contract_bytes);
        
        let method_len = u32::read(buf)? as usize;
        if buf.remaining() < method_len {
            return Err(CodecError::EndOfBuffer);
        }
        let mut method_bytes = vec![0u8; method_len];
        buf.copy_to_slice(&mut method_bytes);
        let target_method = String::from_utf8(method_bytes)
            .map_err(|_| CodecError::Invalid("target_method", "decoding from utf8 failed"))?;
        
        let params_len = u32::read(buf)? as usize;
        if buf.remaining() < params_len {
            return Err(CodecError::EndOfBuffer);
        }
        let mut params = vec![0u8; params_len];
        buf.copy_to_slice(&mut params);
        
        Ok(Self {
            task_id,
            chain_id,
            target_contract,
            target_method,
            params,
        })
    }
}

impl EncodeSize for GasKillerTaskData {
    fn encode_size(&self) -> usize {
        16 + // task_id
        8 + // chain_id
        20 + // target_contract
        4 + self.target_method.len() + // method string
        4 + self.params.len() // params
    }
}
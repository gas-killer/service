use anyhow::Result;
use async_trait::async_trait;
use bytes::{Buf, BufMut};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

use crate::creator::core::Creator;
use crate::usecases::gas_killer::types::EnrichedGasKillerRequest;
use commonware_codec::{EncodeSize, Read, ReadExt, Write};

/// Task data for Gas Killer transactions
#[derive(Debug, Clone, PartialEq)]
pub struct GasKillerTaskData {
    pub request_id: String,
    pub target_contract_address: String,
    pub target_method: String,
    pub target_chain_id: u64,
    pub params: Vec<u8>, // Decoded hex params
    pub caller_address: String,
    pub created_at: u64,
}

impl From<EnrichedGasKillerRequest> for GasKillerTaskData {
    fn from(req: EnrichedGasKillerRequest) -> Self {
        // Decode hex params
        let params = decode_hex_string(&req.request.params).unwrap_or_default();
        
        Self {
            request_id: req.request_id.to_string(),
            target_contract_address: req.request.target_contract_address,
            target_method: req.request.target_method,
            target_chain_id: req.request.target_chain_id,
            params,
            caller_address: req.request.caller_address,
            created_at: req.created_at,
        }
    }
}

impl Write for GasKillerTaskData {
    fn write(&self, buf: &mut impl BufMut) {
        // Write request_id
        (self.request_id.len() as u32).write(buf);
        buf.put_slice(self.request_id.as_bytes());
        
        // Write target_contract_address
        (self.target_contract_address.len() as u32).write(buf);
        buf.put_slice(self.target_contract_address.as_bytes());
        
        // Write target_method
        (self.target_method.len() as u32).write(buf);
        buf.put_slice(self.target_method.as_bytes());
        
        // Write target_chain_id
        self.target_chain_id.write(buf);
        
        // Write params
        (self.params.len() as u32).write(buf);
        buf.put_slice(&self.params);
        
        // Write caller_address
        (self.caller_address.len() as u32).write(buf);
        buf.put_slice(self.caller_address.as_bytes());
        
        // Write created_at
        self.created_at.write(buf);
    }
}

impl Read for GasKillerTaskData {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _: &()) -> Result<Self, commonware_codec::Error> {
        // Read request_id
        let request_id = read_string(buf)?;
        
        // Read target_contract_address
        let target_contract_address = read_string(buf)?;
        
        // Read target_method
        let target_method = read_string(buf)?;
        
        // Read target_chain_id
        let target_chain_id = u64::read(buf)?;
        
        // Read params
        let params_len = u32::read(buf)? as usize;
        if buf.remaining() < params_len {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let mut params = vec![0u8; params_len];
        buf.copy_to_slice(&mut params);
        
        // Read caller_address
        let caller_address = read_string(buf)?;
        
        // Read created_at
        let created_at = u64::read(buf)?;
        
        Ok(Self {
            request_id,
            target_contract_address,
            target_method,
            target_chain_id,
            params,
            caller_address,
            created_at,
        })
    }
}

impl EncodeSize for GasKillerTaskData {
    fn encode_size(&self) -> usize {
        const LENGTH_PREFIX_SIZE: usize = std::mem::size_of::<u32>();
        const U64_SIZE: usize = std::mem::size_of::<u64>();
        
        LENGTH_PREFIX_SIZE + self.request_id.len()
            + LENGTH_PREFIX_SIZE + self.target_contract_address.len()
            + LENGTH_PREFIX_SIZE + self.target_method.len()
            + U64_SIZE // target_chain_id
            + LENGTH_PREFIX_SIZE + self.params.len()
            + LENGTH_PREFIX_SIZE + self.caller_address.len()
            + U64_SIZE // created_at
    }
}

// Helper function to read a string from buffer
fn read_string(buf: &mut impl Buf) -> Result<String, commonware_codec::Error> {
    let len = u32::read(buf)? as usize;
    if buf.remaining() < len {
        return Err(commonware_codec::Error::EndOfBuffer);
    }
    let mut bytes = vec![0u8; len];
    buf.copy_to_slice(&mut bytes);
    String::from_utf8(bytes)
        .map_err(|_| commonware_codec::Error::Invalid("string", "decoding from utf8 failed"))
}

// Helper function to decode hex string to bytes
fn decode_hex_string(hex: &str) -> Option<Vec<u8>> {
    let hex = if hex.starts_with("0x") {
        &hex[2..]
    } else {
        hex
    };
    
    if hex.is_empty() {
        return Some(Vec::new());
    }
    
    hex::decode(hex).ok()
}

/// Gas Killer queue interface
pub trait GasKillerQueue: Send + Sync {
    /// Get the next request from the queue
    fn pop(&self) -> Option<EnrichedGasKillerRequest>;
    
    /// Get the current queue size
    fn size(&self) -> usize;
}

/// Simple in-memory queue implementation
pub struct SimpleGasKillerQueue {
    queue: Arc<Mutex<Vec<EnrichedGasKillerRequest>>>,
}

impl SimpleGasKillerQueue {
    pub fn new(queue: Arc<Mutex<Vec<EnrichedGasKillerRequest>>>) -> Self {
        Self { queue }
    }
}

impl GasKillerQueue for SimpleGasKillerQueue {
    fn pop(&self) -> Option<EnrichedGasKillerRequest> {
        self.queue.lock().ok()?.pop()
    }
    
    fn size(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }
}

/// Gas Killer Creator for processing gas optimization requests
pub struct GasKillerCreator<Q: GasKillerQueue> {
    queue: Arc<Q>,
    poll_interval: Duration,
    current_task: Option<GasKillerTaskData>,
    round_counter: u64,
}

impl<Q: GasKillerQueue> GasKillerCreator<Q> {
    pub fn new(queue: Arc<Q>, poll_interval: Duration) -> Self {
        Self {
            queue,
            poll_interval,
            current_task: None,
            round_counter: 0,
        }
    }
    
    /// Poll for the next task from the queue
    pub async fn poll_next_task(&mut self) -> Option<GasKillerTaskData> {
        loop {
            // Check if there's a request in the queue
            if let Some(request) = self.queue.pop() {
                info!(
                    "Processing Gas Killer request: {} for chain {}",
                    request.request_id, request.request.target_chain_id
                );
                let task_data = GasKillerTaskData::from(request);
                self.current_task = Some(task_data.clone());
                self.round_counter += 1;
                return Some(task_data);
            }
            
            // Wait before checking again
            tokio::time::sleep(self.poll_interval).await;
            
            // Log queue status periodically
            let queue_size = self.queue.size();
            if queue_size > 0 {
                info!("Gas Killer queue has {} pending requests", queue_size);
            }
        }
    }
}

#[async_trait]
impl<Q: GasKillerQueue + 'static> Creator for GasKillerCreator<Q> {
    type TaskData = GasKillerTaskData;

    async fn get_payload_and_round(&self) -> Result<(Vec<u8>, u64)> {
        // Get current task data
        let task_data = self.current_task.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No current task available"))?;
        
        // Serialize the task data as the payload
        let mut payload = Vec::new();
        task_data.write(&mut payload);
        
        Ok((payload, self.round_counter))
    }
    
    fn get_task_metadata(&self) -> Self::TaskData {
        self.current_task.clone().unwrap_or_else(|| {
            // Return a default task if none is available
            GasKillerTaskData {
                request_id: String::new(),
                target_contract_address: String::new(),
                target_method: String::new(),
                target_chain_id: 0,
                params: Vec::new(),
                caller_address: String::new(),
                created_at: 0,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_gas_killer_task_data_serialization() {
        let original = GasKillerTaskData {
            request_id: Uuid::new_v4().to_string(),
            target_contract_address: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
            target_method: "transfer(address,uint256)".to_string(),
            target_chain_id: 1,
            params: vec![0x12, 0x34, 0x56, 0x78],
            caller_address: "0x1234567890123456789012345678901234567890".to_string(),
            created_at: 1700000000,
        };

        // Serialize
        let mut buf = Vec::new();
        original.write(&mut buf);

        // Deserialize
        let mut cursor = std::io::Cursor::new(buf);
        let deserialized = GasKillerTaskData::read_cfg(&mut cursor, &()).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_decode_hex_string() {
        assert_eq!(decode_hex_string("0x1234"), Some(vec![0x12, 0x34]));
        assert_eq!(decode_hex_string("1234"), Some(vec![0x12, 0x34]));
        assert_eq!(decode_hex_string("0x"), Some(vec![]));
        assert_eq!(decode_hex_string(""), Some(vec![]));
        assert_eq!(decode_hex_string("0xGGGG"), None); // Invalid hex
    }

    #[tokio::test]
    async fn test_gas_killer_creator_with_queue() {
        let queue = Arc::new(Mutex::new(Vec::new()));
        let gas_killer_queue = SimpleGasKillerQueue::new(queue.clone());
        
        // Add a request to the queue
        let request = EnrichedGasKillerRequest {
            request_id: Uuid::new_v4(),
            request: GasKillerTransactionRequest {
                target_contract_address: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
                target_method: "transfer(address,uint256)".to_string(),
                target_chain_id: 1,
                params: "0x1234".to_string(),
                caller_address: "0x1234567890123456789012345678901234567890".to_string(),
            },
            metadata: crate::ingress::gas_killer_types::RequestMetadata {
                ip_address: None,
                user_agent: None,
                additional: Default::default(),
            },
            created_at: 1700000000,
        };
        
        queue.lock().unwrap().push(request.clone());
        
        let mut creator = GasKillerCreator::new(
            Arc::new(gas_killer_queue),
            Duration::from_millis(100),
        );
        
        // Get the next task
        let task = creator.next_task().await;
        assert!(task.is_some());
        
        let task = task.unwrap();
        assert_eq!(task.request_id, request.request_id.to_string());
        assert_eq!(task.target_chain_id, 1);
    }
}
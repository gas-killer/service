use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GasKillerTransactionRequest {
    pub target_contract_address: String,
    pub target_method: String,
    pub target_chain_id: u64,
    pub params: String, // Hex encoded parameters
    pub caller_address: String,
}

impl GasKillerTransactionRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        // Validate Ethereum address format for target_contract_address
        if !is_valid_ethereum_address(&self.target_contract_address) {
            return Err(ValidationError::invalid_address("target_contract_address".to_string()));
        }

        // Validate Ethereum address format for caller_address
        if !is_valid_ethereum_address(&self.caller_address) {
            return Err(ValidationError::invalid_address("caller_address".to_string()));
        }

        // Validate method signature format (e.g., "transfer(address,uint256)")
        if !is_valid_method_signature(&self.target_method) {
            return Err(ValidationError::invalid_method_signature());
        }

        // Validate chain ID is supported
        if !is_supported_chain_id(self.target_chain_id) {
            return Err(ValidationError::unsupported_chain_id(self.target_chain_id));
        }

        // Validate params are hex encoded
        if !is_valid_hex_string(&self.params) {
            return Err(ValidationError::invalid_params());
        }

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EnrichedGasKillerRequest {
    pub request_id: Uuid,
    pub request: GasKillerTransactionRequest,
    pub metadata: RequestMetadata,
    pub created_at: u64, // Unix timestamp
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RequestMetadata {
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    #[serde(flatten)]
    pub additional: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GasKillerTransactionResponse {
    pub request_id: String,
    pub status: RequestStatus,
    pub estimated_time: u32, // in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestStatus {
    Queued,
    Processing,
    Completed,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ValidationError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<HashMap<String, String>>,
}

impl ValidationError {
    pub fn invalid_address(field: String) -> Self {
        Self {
            code: "INVALID_ADDRESS".to_string(),
            message: format!("Invalid Ethereum address format for field: {}", field),
            details: None,
        }
    }

    pub fn invalid_method_signature() -> Self {
        Self {
            code: "INVALID_METHOD".to_string(),
            message: "Invalid method signature format".to_string(),
            details: None,
        }
    }

    pub fn unsupported_chain_id(chain_id: u64) -> Self {
        let mut details = HashMap::new();
        details.insert("chain_id".to_string(), chain_id.to_string());
        details.insert("supported_chains".to_string(), get_supported_chains().join(", "));
        
        Self {
            code: "UNSUPPORTED_CHAIN".to_string(),
            message: format!("Chain ID {} is not supported", chain_id),
            details: Some(details),
        }
    }

    pub fn invalid_params() -> Self {
        Self {
            code: "INVALID_PARAMS".to_string(),
            message: "Parameters must be hex encoded".to_string(),
            details: None,
        }
    }

    pub fn missing_field(field: String) -> Self {
        Self {
            code: "MISSING_FIELD".to_string(),
            message: format!("Required field '{}' is missing", field),
            details: None,
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ValidationError {}

// Validation helper functions
fn is_valid_ethereum_address(address: &str) -> bool {
    // Check if it starts with 0x and has 40 hex characters after that
    if !address.starts_with("0x") {
        return false;
    }
    
    let address_part = &address[2..];
    address_part.len() == 40 && address_part.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_valid_method_signature(method: &str) -> bool {
    // Basic validation for method signature format
    // Should be like: functionName(type1,type2,...)
    if method.is_empty() {
        return false;
    }
    
    // Check if it contains parentheses
    if let Some(paren_pos) = method.find('(') {
        if !method.ends_with(')') {
            return false;
        }
        
        let function_name = &method[..paren_pos];
        // Function name should be valid identifier
        if function_name.is_empty() || !function_name.chars().next().unwrap().is_alphabetic() {
            return false;
        }
        
        true
    } else {
        false
    }
}

fn is_supported_chain_id(chain_id: u64) -> bool {
    // List of supported chain IDs
    // These are example chain IDs - adjust based on actual requirements
    const SUPPORTED_CHAINS: &[u64] = &[
        1,      // Ethereum Mainnet
        5,      // Goerli
        11155111, // Sepolia
        17000,  // Holesky
        137,    // Polygon
        80001,  // Mumbai
        42161,  // Arbitrum One
        421613, // Arbitrum Goerli
        10,     // Optimism
        420,    // Optimism Goerli
    ];
    
    SUPPORTED_CHAINS.contains(&chain_id)
}

fn get_supported_chains() -> Vec<String> {
    vec![
        "1 (Ethereum)".to_string(),
        "5 (Goerli)".to_string(),
        "11155111 (Sepolia)".to_string(),
        "17000 (Holesky)".to_string(),
        "137 (Polygon)".to_string(),
        "80001 (Mumbai)".to_string(),
        "42161 (Arbitrum)".to_string(),
        "421613 (Arbitrum Goerli)".to_string(),
        "10 (Optimism)".to_string(),
        "420 (Optimism Goerli)".to_string(),
    ]
}

fn is_valid_hex_string(hex: &str) -> bool {
    if hex.is_empty() {
        return true; // Empty params are valid
    }
    
    // Check if it starts with 0x
    if hex.starts_with("0x") {
        let hex_part = &hex[2..];
        // Must have even number of hex characters
        hex_part.len() % 2 == 0 && hex_part.chars().all(|c| c.is_ascii_hexdigit())
    } else {
        // Also accept without 0x prefix
        hex.len() % 2 == 0 && hex.chars().all(|c| c.is_ascii_hexdigit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_ethereum_address() {
        assert!(is_valid_ethereum_address("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0"));
        assert!(is_valid_ethereum_address("0x0000000000000000000000000000000000000000"));
        assert!(!is_valid_ethereum_address("0x123")); // Too short
        assert!(!is_valid_ethereum_address("742d35Cc6634C0532925a3b844Bc9e7595f0bEb0")); // Missing 0x
        assert!(!is_valid_ethereum_address("0xGGGG35Cc6634C0532925a3b844Bc9e7595f0bEb0")); // Invalid hex
    }

    #[test]
    fn test_valid_method_signature() {
        assert!(is_valid_method_signature("transfer(address,uint256)"));
        assert!(is_valid_method_signature("balanceOf(address)"));
        assert!(is_valid_method_signature("approve()"));
        assert!(!is_valid_method_signature("transfer")); // Missing parentheses
        assert!(!is_valid_method_signature("(address,uint256)")); // Missing function name
        assert!(!is_valid_method_signature("")); // Empty
    }

    #[test]
    fn test_valid_hex_string() {
        assert!(is_valid_hex_string("0x1234567890abcdef"));
        assert!(is_valid_hex_string("1234567890abcdef"));
        assert!(is_valid_hex_string("0x"));
        assert!(is_valid_hex_string(""));
        assert!(!is_valid_hex_string("0x123")); // Odd number of hex chars
        assert!(!is_valid_hex_string("0xGGGG")); // Invalid hex chars
    }

    #[test]
    fn test_request_validation() {
        let valid_request = GasKillerTransactionRequest {
            target_contract_address: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
            target_method: "transfer(address,uint256)".to_string(),
            target_chain_id: 1,
            params: "0x1234567890".to_string(),
            caller_address: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        };
        
        assert!(valid_request.validate().is_ok());
        
        let invalid_address_request = GasKillerTransactionRequest {
            target_contract_address: "invalid".to_string(),
            ..valid_request.clone()
        };
        
        assert!(invalid_address_request.validate().is_err());
        
        let invalid_chain_request = GasKillerTransactionRequest {
            target_chain_id: 999999,
            ..valid_request.clone()
        };
        
        assert!(invalid_chain_request.validate().is_err());
    }
}
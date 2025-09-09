use alloy_primitives::{Address, Bytes};
use anyhow::Result;
use commonware_avs_router::validator::gas_killer::{GasKillerValidator, ValidationTask};
use std::collections::HashMap;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    println!("Gas Killer Validator Example");
    println!("============================\n");

    let mut rpc_endpoints = HashMap::new();
    rpc_endpoints.insert(1, "https://eth-mainnet.g.alchemy.com/v2/demo".to_string());
    rpc_endpoints.insert(10, "https://opt-mainnet.g.alchemy.com/v2/demo".to_string());
    rpc_endpoints.insert(
        42161,
        "https://arb-mainnet.g.alchemy.com/v2/demo".to_string(),
    );

    let validator = GasKillerValidator::new(rpc_endpoints)
        .with_retry_config(3, std::time::Duration::from_secs(2));

    let task = ValidationTask {
        task_id: Uuid::new_v4(),
        target_contract: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".parse::<Address>()?,
        target_method: "transfer".to_string(),
        target_chain_id: 1,
        params: Bytes::from(hex::decode(
            "a9059cbb0000000000000000000000001234567890123456789012345678901234567890000000000000000000000000000000000000000000000000000000000000000a",
        )?),
        caller: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb9".parse::<Address>()?,
        block_number: Some(18000000),
    };

    println!("Validation Task:");
    println!("  Task ID: {}", task.task_id);
    println!("  Target Contract: {}", task.target_contract);
    println!("  Target Method: {}", task.target_method);
    println!("  Chain ID: {}", task.target_chain_id);
    println!("  Caller: {}", task.caller);
    println!("  Block Number: {:?}\n", task.block_number);

    println!("Starting validation...\n");

    match validator.validate_task(task).await {
        Ok(response) => {
            println!("Validation Response:");
            println!("  Task ID: {}", response.task_id);
            println!("  Validated: {}", response.validated);
            println!("  Simulation Block: {}", response.simulation_block);

            if let Some(gas_metrics) = response.gas_metrics {
                println!("\n  Gas Metrics:");
                println!("    Original Gas: {}", gas_metrics.original_gas);
                println!("    Optimized Gas: {}", gas_metrics.optimized_gas);
                println!("    Savings: {:.2}%", gas_metrics.savings_percentage);
            }

            if let Some(state_updates) = response.state_updates {
                println!("\n  State Updates:");
                println!(
                    "    Storage Slots Modified: {}",
                    state_updates.storage_slots.len()
                );
                println!(
                    "    Account Changes: {}",
                    state_updates.account_changes.len()
                );
                println!(
                    "    Accessed Addresses: {}",
                    state_updates.accessed_addresses.len()
                );
                println!("    Gas Used: {}", state_updates.gas_used);
                println!("    Gas Saved: {}", state_updates.gas_saved);
            }

            if let Some(error) = response.error_message {
                println!("\n  Error: {error}");
            }
        }
        Err(e) => {
            eprintln!("Validation failed: {e}");
        }
    }

    Ok(())
}

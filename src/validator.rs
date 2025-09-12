use crate::{bindings::counter::Counter, wire};
use alloy::sol_types::SolValue;
use alloy_primitives::{FixedBytes, U256};
use alloy_provider::{
    ProviderBuilder, RootProvider,
    fillers::{BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller},
};
use anyhow::Result;
use commonware_codec::{DecodeExt, ReadExt};
use commonware_cryptography::sha256::Digest;
use commonware_cryptography::{Hasher, Sha256};
use commonware_eigenlayer::config::AvsDeployment;
use crate::tx_extractor::{TxStateExtractor, StateUpdateReport, StateUpdate};
use std::{env, io::Cursor};

// Type alias to reduce complexity
type CounterProvider = FillProvider<
    JoinFill<
        alloy_provider::Identity,
        JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
    >,
    RootProvider,
>;

pub struct Validator {
    counter: Counter::CounterInstance<(), CounterProvider>,
    tx_extractor: TxStateExtractor<CounterProvider>,
}

impl Validator {
    pub async fn new() -> Result<Self> {
        let http_rpc = env::var("HTTP_RPC").expect("HTTP_RPC must be set");
        let provider = ProviderBuilder::new().on_http(url::Url::parse(&http_rpc).unwrap());

        let deployment = AvsDeployment::load()
            .map_err(|e| anyhow::anyhow!("Failed to load AVS deployment: {}", e))?;
        let counter_address = deployment
            .counter_address()
            .map_err(|e| anyhow::anyhow!("Failed to get counter address: {}", e))?;
        let counter = Counter::new(counter_address, provider.clone());
        let tx_extractor = TxStateExtractor::new(provider);

        Ok(Self { counter, tx_extractor })
    }

    pub async fn validate_and_return_expected_hash(&self, msg: &[u8]) -> Result<Digest> {
        // First verify the message round
        self.verify_message_round(msg).await?;

        // Then get the payload hash
        self.get_payload_from_message(msg).await
    }

    pub async fn get_payload_from_message(&self, msg: &[u8]) -> Result<Digest> {
        // Decode the wire message
        let aggregation = wire::Aggregation::decode(msg)?;

        // Create the payload directly
        let payload = U256::from(aggregation.round).abi_encode();

        // Hash the payload
        let mut hasher = Sha256::new();
        hasher.update(&payload);
        let payload_hash = hasher.finalize();

        Ok(payload_hash)
    }

    async fn verify_message_round(&self, msg: &[u8]) -> Result<()> {
        let aggregation = wire::Aggregation::read(&mut Cursor::new(msg))?;
        let current_number = self.counter.number().call().await?;
        let current_number = current_number._0.to::<u64>();

        if aggregation.round != current_number {
            return Err(anyhow::anyhow!(
                "Invalid round number in message. Expected {}, got {}",
                current_number,
                aggregation.round
            ));
        }

        Ok(())
    }

    // New methods leveraging tx_extractor functionality
    pub async fn extract_transaction_state(&self, tx_hash: FixedBytes<32>) -> Result<Vec<StateUpdate>> {
        self.tx_extractor.extract_state_updates(tx_hash).await
    }

    pub async fn extract_transaction_with_metadata(&self, tx_hash: FixedBytes<32>) -> Result<StateUpdateReport> {
        self.tx_extractor.extract_with_metadata(tx_hash).await
    }

    pub async fn validate_transaction_effects(&self, tx_hash: FixedBytes<32>) -> Result<bool> {
        // Extract state updates and validate them
        let updates = self.extract_transaction_state(tx_hash).await?;
        
        // Validate that the transaction has expected effects
        // This is a placeholder for actual validation logic
        for update in &updates {
            match update {
                StateUpdate::Store(store) => {
                    // Validate storage changes
                    tracing::debug!("Storage update: slot={:?}, value={:?}", store.slot, store.value);
                }
                StateUpdate::Call(call) => {
                    // Validate external calls
                    tracing::debug!("External call: target={:?}, value={}", call.target, call.value);
                }
                StateUpdate::Log0(log) => {
                    // Validate events
                    tracing::debug!("Event emitted (LOG0) with {} bytes of data", log.data.len());
                }
                StateUpdate::Log1(log) => {
                    tracing::debug!("Event emitted (LOG1) with {} bytes of data", log.data.len());
                }
                StateUpdate::Log2(log) => {
                    tracing::debug!("Event emitted (LOG2) with {} bytes of data", log.data.len());
                }
                StateUpdate::Log3(log) => {
                    tracing::debug!("Event emitted (LOG3) with {} bytes of data", log.data.len());
                }
                StateUpdate::Log4(log) => {
                    tracing::debug!("Event emitted (LOG4) with {} bytes of data", log.data.len());
                }
            }
        }

        // Return true if validation passes
        Ok(!updates.is_empty())
    }

    pub async fn analyze_gas_killer_transaction(&self, tx_hash: FixedBytes<32>) -> Result<GasKillerAnalysis> {
        let report = self.extract_transaction_with_metadata(tx_hash).await?;
        
        let mut storage_updates = 0;
        let mut external_calls = 0;
        let mut events_emitted = 0;
        let mut total_state_changes = 0;

        for update in &report.state_updates {
            total_state_changes += 1;
            match update {
                StateUpdate::Store(_) => storage_updates += 1,
                StateUpdate::Call(_) => external_calls += 1,
                StateUpdate::Log0(_) | StateUpdate::Log1(_) | StateUpdate::Log2(_) | 
                StateUpdate::Log3(_) | StateUpdate::Log4(_) => events_emitted += 1,
            }
        }

        Ok(GasKillerAnalysis {
            tx_hash,
            block_number: report.block_number,
            from: report.from,
            to: report.to,
            gas_used: report.gas_used,
            storage_updates,
            external_calls,
            events_emitted,
            total_state_changes,
            gas_efficiency_score: calculate_gas_efficiency(report.gas_used, total_state_changes),
        })
    }
}

// Helper struct for gas killer analysis
#[derive(Debug)]
pub struct GasKillerAnalysis {
    pub tx_hash: FixedBytes<32>,
    pub block_number: u64,
    pub from: alloy_primitives::Address,
    pub to: Option<alloy_primitives::Address>,
    pub gas_used: u128,
    pub storage_updates: usize,
    pub external_calls: usize,
    pub events_emitted: usize,
    pub total_state_changes: usize,
    pub gas_efficiency_score: f64,
}

fn calculate_gas_efficiency(gas_used: u128, state_changes: usize) -> f64 {
    if state_changes == 0 {
        return 0.0;
    }
    // Lower gas per state change is better
    let gas_per_change = gas_used as f64 / state_changes as f64;
    // Convert to a score from 0-100 where higher is better
    // Assuming 50000 gas per change is average
    let efficiency = (50000.0 / gas_per_change) * 100.0;
    efficiency.min(100.0).max(0.0)
}
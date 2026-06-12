pub mod bindings;
pub mod config;
pub mod providers;
pub mod task_data;
pub mod validator;

// Re-export commonly used types
pub use config::{
    ChainRole, KeyConfig, OrchestratorConfig, SpeculativePrebuildConfig, aggregation_timeout,
    block_stale_measure, detect_chain_for_address, get_operator_states, load_key_from_file,
    load_orchestrator_config,
};
pub use providers::{build_read_providers, chain_rpc_urls_from_env};
pub use task_data::GasKillerTaskData;
pub use validator::{GasKillerValidator, ValidatorMetrics};

// Re-export QuorumInfo for convenience
pub use commonware_avs_eigenlayer::QuorumInfo;

// Re-export provider types for convenience
pub use commonware_avs_bindings::{ReadOnlyProvider, WalletProvider};

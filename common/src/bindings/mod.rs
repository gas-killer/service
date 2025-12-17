// Re-export common bindings from commonware-avs-router
pub use commonware_avs_router::bindings::{
    ReadOnlyProvider, WalletProvider, blsapkregistry, blssigcheckoperatorstateretriever,
};

#[allow(
    non_camel_case_types,
    non_snake_case,
    clippy::pub_underscore_fields,
    clippy::style,
    clippy::empty_structs_with_brackets,
    missing_docs,
    dead_code
)]
pub mod gaskillersdk;

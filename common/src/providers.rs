//! Construction of read-only RPC providers shared across components.
//!
//! Reads the per-chain RPC endpoints from the environment (`HTTP_RPC`, `L2_HTTP_RPC`)
//! and builds one provider per chain.

use std::collections::HashMap;
use std::env;

use url::Url;

use crate::ReadOnlyProvider;
use crate::config::ChainRole;

/// Reads the per-chain RPC URLs from the environment.
///
/// - `HTTP_RPC` → [`ChainRole::L1`] (required)
/// - `L2_HTTP_RPC` → [`ChainRole::L2`] (optional)
///
/// Returns an error if `HTTP_RPC` is not set.
pub fn chain_rpc_urls_from_env() -> anyhow::Result<HashMap<ChainRole, String>> {
    let mut urls = HashMap::new();

    let l1_rpc = env::var("HTTP_RPC")
        .map_err(|_| anyhow::anyhow!("HTTP_RPC environment variable is not set"))?;
    Url::parse(&l1_rpc).map_err(|e| anyhow::anyhow!("HTTP_RPC is not a valid URL: {e}"))?;
    urls.insert(ChainRole::L1, l1_rpc);

    if let Ok(l2_rpc) = env::var("L2_HTTP_RPC") {
        Url::parse(&l2_rpc).map_err(|e| anyhow::anyhow!("L2_HTTP_RPC is not a valid URL: {e}"))?;
        urls.insert(ChainRole::L2, l2_rpc);
    }

    Ok(urls)
}

/// Builds a read-only HTTP provider for each chain URL.
///
/// URLs that fail to parse are skipped with a warning; a chain with no provider
/// has its lookups error at call time.
pub fn build_read_providers(
    chain_rpc_urls: &HashMap<ChainRole, String>,
) -> HashMap<ChainRole, ReadOnlyProvider> {
    use alloy_provider::ProviderBuilder;

    let mut providers = HashMap::with_capacity(chain_rpc_urls.len());
    for (&chain_id, rpc_url) in chain_rpc_urls {
        match Url::parse(rpc_url) {
            Ok(url) => {
                providers.insert(chain_id, ProviderBuilder::new().connect_http(url));
            }
            Err(e) => {
                tracing::warn!(
                    chain = %chain_id,
                    error = %e,
                    "Skipping read provider for chain with unparseable RPC URL"
                );
            }
        }
    }
    providers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_provider_per_valid_url() {
        let mut urls = HashMap::new();
        urls.insert(ChainRole::L1, "https://example.com".to_string());
        urls.insert(ChainRole::L2, "https://l2.example.com".to_string());

        let providers = build_read_providers(&urls);

        assert_eq!(providers.len(), 2);
        assert!(providers.contains_key(&ChainRole::L1));
        assert!(providers.contains_key(&ChainRole::L2));
    }

    #[test]
    fn skips_unparseable_urls() {
        let mut urls = HashMap::new();
        urls.insert(ChainRole::L1, "https://example.com".to_string());
        urls.insert(ChainRole::L2, "not a url".to_string());

        let providers = build_read_providers(&urls);

        assert!(providers.contains_key(&ChainRole::L1));
        assert!(!providers.contains_key(&ChainRole::L2));
    }
}

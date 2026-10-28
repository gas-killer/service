//! Construction of read-only RPC providers shared across components.
//!
//! Reads the per-chain RPC endpoints from the environment (`HTTP_RPC`, `L2_HTTP_RPC`)
//! and builds one provider per chain.
//!
//! Simulation is addressed separately (`SIM_HTTP_RPC`, `L2_SIM_HTTP_RPC`): extraction may need a
//! node with a lifted `debug_traceCall` cap while settlement still has to reach the real chain.

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

/// Reads the per-chain simulation RPC URLs, defaulting each to that chain's `HTTP_RPC`.
///
/// - `SIM_HTTP_RPC` -> [`ChainRole::L1`]
/// - `L2_SIM_HTTP_RPC` -> [`ChainRole::L2`]
///
/// Only extraction and gas estimation run against these. Settlement, chain detection and
/// `stateTransitionCount` reads stay on the chain RPC, so the simulation endpoint may be a fork
/// that never sees a transaction.
///
/// The split exists because `GK_SIM_PROFILE=unbounded` needs a node whose `debug_traceCall`
/// execution cap is lifted, and hosted endpoints clamp it silently — returning a truncated trace
/// rather than an error. An empty value counts as unset, so a chart that always renders the
/// variable still gets the default.
///
/// A role absent from `chain_rpc_urls` is rejected rather than inserted: a simulation endpoint for
/// a chain the deployment does not serve would never be read, so accepting it would hide the typo.
pub fn sim_rpc_urls_from_env(
    chain_rpc_urls: &HashMap<ChainRole, String>,
) -> anyhow::Result<HashMap<ChainRole, String>> {
    sim_rpc_urls_with(chain_rpc_urls, |var| env::var(var).ok())
}

/// [`sim_rpc_urls_from_env`] with the environment injected, so the precedence rules are testable
/// without mutating process-global state.
fn sim_rpc_urls_with<F>(
    chain_rpc_urls: &HashMap<ChainRole, String>,
    lookup: F,
) -> anyhow::Result<HashMap<ChainRole, String>>
where
    F: Fn(&str) -> Option<String>,
{
    let mut urls = chain_rpc_urls.clone();

    for (role, var) in [
        (ChainRole::L1, "SIM_HTTP_RPC"),
        (ChainRole::L2, "L2_SIM_HTTP_RPC"),
    ] {
        let Some(raw) = lookup(var) else { continue };
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        if !chain_rpc_urls.contains_key(&role) {
            anyhow::bail!("{var} is set but {role} has no chain RPC configured");
        }
        Url::parse(raw).map_err(|e| anyhow::anyhow!("{var} is not a valid URL: {e}"))?;
        urls.insert(role, raw.to_string());
    }

    Ok(urls)
}

/// Logs each chain's effective simulation endpoint, warning where it is the chain RPC itself.
///
/// Trace calls are the heaviest RPC load the fleet makes, so simulating against `HTTP_RPC` puts
/// that load on the endpoint settlement and chain reads depend on (#326). Under
/// [`gas_analyzer::SimProfile::Unbounded`] it is also only truthful if that endpoint's gas cap is
/// lifted. It stays a warning rather than an error because a local anvil is legitimately both.
pub fn log_sim_endpoints(
    chain_rpc_urls: &HashMap<ChainRole, String>,
    sim_rpc_urls: &HashMap<ChainRole, String>,
    profile: gas_analyzer::SimProfile,
) {
    for (role, sim_url) in sim_rpc_urls {
        let endpoint = endpoint_label(sim_url);
        if chain_rpc_urls.get(role) != Some(sim_url) {
            tracing::info!(chain = %role, %endpoint, ?profile, "Simulating against a dedicated endpoint");
        } else if profile == gas_analyzer::SimProfile::Unbounded {
            tracing::warn!(
                chain = %role,
                %endpoint,
                "Simulating unbounded against the chain RPC: set SIM_HTTP_RPC to a node with its gas cap lifted, or traces run out of gas"
            );
        } else {
            tracing::warn!(
                chain = %role,
                %endpoint,
                "Simulating against the chain RPC: trace load shares the endpoint settlement uses; set SIM_HTTP_RPC to separate them"
            );
        }
    }
}

/// Scheme, host and port only: hosted RPC URLs carry their API key in the path or query.
fn endpoint_label(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(url) => match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
            (Some(host), None) => format!("{}://{host}", url.scheme()),
            (None, _) => url.scheme().to_string(),
        },
        Err(_) => "<unparseable>".to_string(),
    }
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

    fn chain_urls() -> HashMap<ChainRole, String> {
        let mut urls = HashMap::new();
        urls.insert(ChainRole::L1, "https://chain.example".to_string());
        urls
    }

    #[test]
    fn sim_urls_default_to_the_chain_urls() {
        let chain = chain_urls();
        let sim = sim_rpc_urls_with(&chain, |_| None).unwrap();
        assert_eq!(sim, chain);
    }

    #[test]
    fn sim_url_overrides_only_its_own_role() {
        let mut chain = chain_urls();
        chain.insert(ChainRole::L2, "https://l2.example".to_string());

        let sim = sim_rpc_urls_with(&chain, |var| {
            (var == "SIM_HTTP_RPC").then(|| "http://anvil:8545".to_string())
        })
        .unwrap();

        assert_eq!(sim[&ChainRole::L1], "http://anvil:8545");
        assert_eq!(sim[&ChainRole::L2], "https://l2.example");
    }

    /// A chart that always renders the variable emits `value: ""` when it is unset.
    #[test]
    fn blank_sim_url_falls_back_rather_than_failing() {
        let chain = chain_urls();
        let sim = sim_rpc_urls_with(&chain, |_| Some("   ".to_string())).unwrap();
        assert_eq!(sim, chain);
    }

    #[test]
    fn unparseable_sim_url_is_an_error() {
        let chain = chain_urls();
        let err = sim_rpc_urls_with(&chain, |var| {
            (var == "SIM_HTTP_RPC").then(|| "not a url".to_string())
        })
        .unwrap_err();
        assert!(err.to_string().contains("SIM_HTTP_RPC"), "{err}");
    }

    #[test]
    fn endpoint_label_drops_credentials_path_and_query() {
        assert_eq!(
            endpoint_label("https://user:secret@eth-sepolia.example.com/v2/KEY?token=abc"),
            "https://eth-sepolia.example.com"
        );
        assert_eq!(
            endpoint_label("http://sim-node:8545"),
            "http://sim-node:8545"
        );
        assert_eq!(endpoint_label("not a url"), "<unparseable>");
    }

    /// Silently inserting it would leave a typo'd variable looking configured but never read.
    #[test]
    fn sim_url_for_an_unserved_chain_is_an_error() {
        let chain = chain_urls();
        let err = sim_rpc_urls_with(&chain, |var| {
            (var == "L2_SIM_HTTP_RPC").then(|| "http://anvil-l2:8545".to_string())
        })
        .unwrap_err();
        assert!(err.to_string().contains("L2_SIM_HTTP_RPC"), "{err}");
    }
}

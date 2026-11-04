//! Startup resolution of registered operator sockets into dialable addresses.

use std::net::SocketAddr;
use std::time::Duration;

/// Attempts per peer before it is skipped; with [`PEER_RESOLVE_DELAY`] this gives DNS about 60s
/// to come up (a Docker or k8s service name may not resolve until its pod starts).
pub const PEER_RESOLVE_ATTEMPTS: u32 = 30;
pub const PEER_RESOLVE_DELAY: Duration = Duration::from_secs(2);

/// Resolves a `host:port` or `ip:port`, retrying while DNS is not yet ready.
pub async fn resolve_with_retry(
    address: &str,
    max_attempts: u32,
    retry_delay: Duration,
) -> Option<SocketAddr> {
    if let Ok(addr) = address.parse::<SocketAddr>() {
        return Some(addr);
    }
    for attempt in 1..=max_attempts {
        match tokio::net::lookup_host(address).await {
            Ok(mut addrs) => {
                if let Some(addr) = addrs.next() {
                    tracing::info!(address, ?addr, attempt, "DNS resolution succeeded");
                    return Some(addr);
                }
                tracing::warn!(address, attempt, "DNS resolution returned no addresses");
            }
            Err(e) => {
                tracing::warn!(address, attempt, max_attempts, error = %e, "DNS resolution failed");
            }
        }
        if attempt < max_attempts {
            tokio::time::sleep(retry_delay).await;
        }
    }
    None
}

/// Resolves every peer concurrently and returns those that resolved, in input order.
///
/// A peer that never resolves is logged and left out rather than failing startup: one stale
/// registry entry would otherwise crash-loop every process in the quorum, even though the
/// remaining peers can still reach a signing threshold. Deciding whether too few resolved is
/// left to the caller.
pub async fn resolve_peers<K: Send + 'static>(
    peers: Vec<(K, String)>,
    max_attempts: u32,
    retry_delay: Duration,
) -> Vec<(K, SocketAddr)> {
    let total = peers.len();
    let handles: Vec<_> = peers
        .into_iter()
        .map(|(key, socket)| {
            tokio::spawn(async move {
                let resolved = resolve_with_retry(&socket, max_attempts, retry_delay).await;
                (key, socket, resolved)
            })
        })
        .collect();

    let mut resolved = Vec::with_capacity(total);
    for handle in handles {
        match handle.await {
            Ok((key, _, Some(addr))) => resolved.push((key, addr)),
            Ok((_, socket, None)) => {
                tracing::error!(socket, "skipping peer whose address could not be resolved");
            }
            Err(e) => tracing::error!(error = %e, "peer resolution task failed; skipping peer"),
        }
    }
    if resolved.len() < total {
        tracing::warn!(
            resolved = resolved.len(),
            total,
            "starting without some registered peers"
        );
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ip_literal_resolves_without_dns() {
        let addr = resolve_with_retry("10.0.0.1:3000", 1, Duration::ZERO).await;
        assert_eq!(addr, Some("10.0.0.1:3000".parse().unwrap()));
    }

    #[tokio::test]
    async fn unresolvable_peer_is_skipped_and_order_is_kept() {
        let peers = vec![
            (1, "10.0.0.1:3000".to_string()),
            (2, "peer.invalid:3000".to_string()),
            (3, "[::1]:3001".to_string()),
        ];
        let resolved = resolve_peers(peers, 1, Duration::ZERO).await;
        assert_eq!(
            resolved,
            vec![
                (1, "10.0.0.1:3000".parse().unwrap()),
                (3, "[::1]:3001".parse().unwrap()),
            ]
        );
    }

    #[tokio::test]
    async fn malformed_socket_is_skipped() {
        let resolved = resolve_peers(vec![(1, "no-port".to_string())], 1, Duration::ZERO).await;
        assert!(resolved.is_empty());
    }
}

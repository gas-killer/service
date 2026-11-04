//! Converts registered operator sockets into p2p peer addresses.

use commonware_p2p::{Address, Ingress};
use commonware_utils::Hostname;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

/// How long a hostname peer may stay unresolvable before it is reported; long enough that pods
/// starting together during a rollout do not trigger it.
const DNS_CHECK_ATTEMPTS: u32 = 30;
const DNS_CHECK_DELAY: Duration = Duration::from_secs(2);

/// Maps a registered `ip:port` or `host:port` socket to a p2p address, or `None` if it is
/// neither.
///
/// A hostname is kept as a DNS ingress that the dialer resolves on every attempt, rather than
/// resolved once at startup. The peer therefore stays in the authorization set while its DNS is
/// missing, and connects as soon as the name resolves, without restarting this process.
///
/// The egress IP is a placeholder: it is only consulted when `bypass_ip_check` is false, and
/// both binaries set it to true because in-cluster source IPs never match registered addresses.
pub fn peer_address(socket: &str) -> Option<Address> {
    if let Ok(addr) = socket.parse::<SocketAddr>() {
        return Some(Address::Symmetric(addr));
    }
    let (host, port) = socket.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    let host = Hostname::new(host).ok()?;
    Some(Address::Asymmetric {
        ingress: Ingress::Dns { host, port },
        egress: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
    })
}

/// Warns once, from a background task, if a DNS-ingress peer's hostname does not resolve within
/// about a minute.
///
/// The p2p dialer reports resolution failures only at debug level, so without this a mistyped or
/// dead operator hostname would leave no visible signal. Must be called inside a tokio runtime.
pub fn warn_if_unresolvable(socket: &str, address: &Address) {
    if !matches!(address.ingress(), Ingress::Dns { .. }) {
        return;
    }
    let socket = socket.to_owned();
    tokio::spawn(async move {
        if !resolves_within(&socket, DNS_CHECK_ATTEMPTS, DNS_CHECK_DELAY).await {
            tracing::warn!(
                socket,
                "peer hostname still does not resolve; the dialer keeps retrying, but this peer cannot connect until it does"
            );
        }
    });
}

async fn resolves_within(socket: &str, attempts: u32, delay: Duration) -> bool {
    for attempt in 1..=attempts {
        if let Ok(mut addrs) = tokio::net::lookup_host(socket).await
            && addrs.next().is_some()
        {
            return true;
        }
        if attempt < attempts {
            tokio::time::sleep(delay).await;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_socket_is_dialed_directly() {
        let addr = peer_address("10.0.0.1:3000").unwrap();
        assert_eq!(addr, Address::Symmetric("10.0.0.1:3000".parse().unwrap()));
        let v6 = peer_address("[::1]:3001").unwrap();
        assert_eq!(v6, Address::Symmetric("[::1]:3001".parse().unwrap()));
    }

    #[test]
    fn hostname_socket_is_resolved_at_dial_time() {
        let addr = peer_address("gas-killer-node-1.default.svc:3000").unwrap();
        assert_eq!(
            addr.ingress(),
            Ingress::Dns {
                host: Hostname::new("gas-killer-node-1.default.svc").unwrap(),
                port: 3000,
            }
        );
    }

    #[tokio::test]
    async fn dns_check_distinguishes_resolvable_hosts() {
        assert!(resolves_within("localhost:3000", 1, Duration::ZERO).await);
        assert!(!resolves_within("peer.invalid:3000", 2, Duration::ZERO).await);
    }

    #[test]
    fn malformed_socket_is_rejected() {
        assert!(peer_address("no-port").is_none());
        assert!(peer_address("host:notaport").is_none());
        assert!(peer_address("bad host:3000").is_none());
    }
}

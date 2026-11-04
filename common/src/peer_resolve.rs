//! Converts registered operator sockets into p2p peer addresses.

use commonware_p2p::{Address, Ingress};
use commonware_utils::Hostname;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

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

    #[test]
    fn malformed_socket_is_rejected() {
        assert!(peer_address("no-port").is_none());
        assert!(peer_address("host:notaport").is_none());
        assert!(peer_address("bad host:3000").is_none());
    }
}

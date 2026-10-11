//! The client address behind the load balancers (T1, S18).
//!
//! `X-Forwarded-For` is a list that every proxy appends to, and anything left of our own proxies' entries is written by the
//! client. So the trusted address is counted from the right: with `hops` proxies of ours in front (default 1, the load
//! balancer), it is the `hops`-th entry from the right. A shorter list, an unparsable entry or no header at all means the
//! request did not come through our proxies as configured, so the socket address is used. Nothing from the left of the
//! trusted entry is ever read, so a client cannot choose the address its rate-limit bucket is keyed by.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use http::HeaderMap;

/// The peer's socket address, put in the request extensions by the listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerAddr(pub SocketAddr);

/// How many of our own proxies sit in front of the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedProxies {
    hops: usize,
}

/// The longest `X-Forwarded-For` value that is looked at. A longer one is attacker padding and is ignored.
const MAX_HEADER_BYTES: usize = 2048;

impl TrustedProxies {
    #[must_use]
    pub fn new(hops: usize) -> Self {
        Self { hops }
    }

    /// The address to key limits and logs by.
    #[must_use]
    pub fn client_ip(&self, headers: &HeaderMap, peer: Option<PeerAddr>) -> IpAddr {
        let socket = peer.map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |p| p.0.ip());
        if self.hops == 0 {
            return socket;
        }
        // Several header lines are one list, in order.
        let mut entries: Vec<&str> = Vec::new();
        let mut total = 0usize;
        for value in headers.get_all(http::header::HeaderName::from_static("x-forwarded-for")) {
            let Ok(text) = value.to_str() else { return socket };
            total += text.len();
            if total > MAX_HEADER_BYTES {
                return socket;
            }
            entries.extend(text.split(',').map(str::trim));
        }
        if entries.len() < self.hops {
            return socket;
        }
        entries[entries.len() - self.hops]
            .parse::<IpAddr>()
            .unwrap_or(socket)
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn headers(xff: &[&str]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for line in xff {
            map.append(
                "x-forwarded-for",
                HeaderValue::from_str(line).unwrap_or_else(|_| HeaderValue::from_static("")),
            );
        }
        map
    }

    #[allow(clippy::unnecessary_wraps)]
    fn peer() -> Option<PeerAddr> {
        Some(PeerAddr(SocketAddr::from(([10, 0, 0, 9], 4444))))
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
    }

    #[test]
    fn one_hop_takes_the_last_entry_and_ignores_what_the_client_wrote() {
        let proxies = TrustedProxies::new(1);
        assert_eq!(
            proxies.client_ip(&headers(&["6.6.6.6, 203.0.113.7"]), peer()),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn two_hops_take_the_second_from_the_right() {
        let proxies = TrustedProxies::new(2);
        assert_eq!(
            proxies.client_ip(&headers(&["6.6.6.6, 203.0.113.7, 35.191.0.1"]), peer()),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn no_header_or_a_short_list_uses_the_socket_address() {
        assert_eq!(
            TrustedProxies::new(1).client_ip(&HeaderMap::new(), peer()),
            ip("10.0.0.9")
        );
        assert_eq!(
            TrustedProxies::new(2).client_ip(&headers(&["203.0.113.7"]), peer()),
            ip("10.0.0.9")
        );
    }

    #[test]
    fn zero_hops_never_reads_the_header() {
        assert_eq!(
            TrustedProxies::new(0).client_ip(&headers(&["1.2.3.4"]), peer()),
            ip("10.0.0.9")
        );
    }

    #[test]
    fn garbage_and_oversized_headers_fall_back_to_the_socket_address() {
        let proxies = TrustedProxies::new(1);
        assert_eq!(
            proxies.client_ip(&headers(&["not-an-ip"]), peer()),
            ip("10.0.0.9")
        );
        let long = format!("{}, 1.2.3.4", "9".repeat(3000));
        assert_eq!(proxies.client_ip(&headers(&[&long]), peer()), ip("10.0.0.9"));
    }

    #[test]
    fn several_header_lines_are_one_list() {
        let proxies = TrustedProxies::new(1);
        assert_eq!(
            proxies.client_ip(&headers(&["6.6.6.6", "203.0.113.7"]), peer()),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn ipv6_entries_parse() {
        assert_eq!(
            TrustedProxies::new(1).client_ip(&headers(&["2001:db8::1"]), peer()),
            ip("2001:db8::1")
        );
    }
}

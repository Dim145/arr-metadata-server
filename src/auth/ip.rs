//! Resolving the real client address.
//!
//! `X-Forwarded-For` is attacker-controlled unless the hop that set it is
//! trusted, and the IP allowlist guarding the Sonarr/Radarr surfaces is only as
//! good as this function. So the header is consulted *only* for peers inside
//! `AMS_TRUSTED_PROXIES`, and then only to walk back past further trusted hops.

use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;
use ipnet::IpNet;

/// Header name, lowercase, as hyper normalises it.
const FORWARDED_FOR: &str = "x-forwarded-for";

/// The address to make access decisions about.
///
/// `peer` is the socket this connection came from. When that peer is a trusted
/// proxy, `X-Forwarded-For` is walked right-to-left and the first address that
/// is *not* itself a trusted proxy is returned.
pub fn resolve(peer: Option<SocketAddr>, headers: &HeaderMap, trusted: &[IpNet]) -> Option<IpAddr> {
    let peer_ip = peer.map(|addr| addr.ip());

    let Some(peer_ip) = peer_ip else {
        return None;
    };

    if !is_trusted(peer_ip, trusted) {
        // The peer is the client. Anything it claims in a header is its own
        // invention and must be ignored.
        return Some(peer_ip);
    }

    let Some(forwarded) = headers.get(FORWARDED_FOR).and_then(|v| v.to_str().ok()) else {
        return Some(peer_ip);
    };

    forwarded
        .split(',')
        .rev()
        .filter_map(|entry| parse_entry(entry.trim()))
        .find(|ip| !is_trusted(*ip, trusted))
        .or(Some(peer_ip))
}

pub fn is_trusted(ip: IpAddr, trusted: &[IpNet]) -> bool {
    trusted.iter().any(|net| net.contains(&ip))
}

/// Whether `ip` is inside any of `allowed`.
pub fn is_allowed(ip: Option<IpAddr>, allowed: &[IpNet]) -> bool {
    // No resolvable address means no way to make a positive decision; deny.
    let Some(ip) = ip else { return false };

    allowed.iter().any(|net| net.contains(&ip))
}

/// One `X-Forwarded-For` element, which may carry a port or be bracketed IPv6.
fn parse_entry(entry: &str) -> Option<IpAddr> {
    if let Ok(ip) = entry.parse::<IpAddr>() {
        return Some(ip);
    }

    // `[2001:db8::1]:443`
    if let Some(rest) = entry.strip_prefix('[') {
        if let Some((addr, _)) = rest.split_once(']') {
            return addr.parse().ok();
        }
    }

    // `203.0.113.7:51234`
    if let Some((addr, _)) = entry.rsplit_once(':') {
        if let Ok(ip) = addr.parse::<IpAddr>() {
            return Some(ip);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nets(values: &[&str]) -> Vec<IpNet> {
        values.iter().map(|v| v.parse().unwrap()).collect()
    }

    fn headers(forwarded: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        if !forwarded.is_empty() {
            h.insert(FORWARDED_FOR, forwarded.parse().unwrap());
        }
        h
    }

    fn peer(addr: &str) -> Option<SocketAddr> {
        Some(format!("{addr}:12345").parse().unwrap())
    }

    #[test]
    fn an_untrusted_peer_cannot_spoof_its_address() {
        let resolved = resolve(
            peer("203.0.113.5"),
            &headers("10.0.0.1"),
            &nets(&["10.0.0.0/8"]),
        );

        assert_eq!(resolved, Some("203.0.113.5".parse().unwrap()));
    }

    #[test]
    fn a_trusted_proxy_reveals_the_client() {
        let resolved = resolve(
            peer("10.0.0.2"),
            &headers("203.0.113.5"),
            &nets(&["10.0.0.0/8"]),
        );

        assert_eq!(resolved, Some("203.0.113.5".parse().unwrap()));
    }

    #[test]
    fn chained_trusted_proxies_are_walked_back_past() {
        // client, edge proxy, inner proxy — the inner one is the peer.
        let resolved = resolve(
            peer("10.0.0.2"),
            &headers("203.0.113.5, 10.0.0.9"),
            &nets(&["10.0.0.0/8"]),
        );

        assert_eq!(resolved, Some("203.0.113.5".parse().unwrap()));
    }

    #[test]
    fn a_client_claiming_to_be_a_proxy_does_not_hide_itself() {
        // Everything in the chain is trusted-looking, so fall back to the peer
        // rather than believing an address the client injected.
        let resolved = resolve(
            peer("10.0.0.2"),
            &headers("10.1.1.1, 10.0.0.9"),
            &nets(&["10.0.0.0/8"]),
        );

        assert_eq!(resolved, Some("10.0.0.2".parse().unwrap()));
    }

    #[test]
    fn entries_with_ports_or_brackets_still_parse() {
        assert_eq!(parse_entry("203.0.113.5"), Some("203.0.113.5".parse().unwrap()));
        assert_eq!(parse_entry("203.0.113.5:443"), Some("203.0.113.5".parse().unwrap()));
        assert_eq!(parse_entry("[2001:db8::1]:443"), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(parse_entry("2001:db8::1"), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(parse_entry("not-an-ip"), None);
    }

    #[test]
    fn a_garbled_header_falls_back_to_the_peer() {
        let resolved = resolve(
            peer("10.0.0.2"),
            &headers("garbage, more-garbage"),
            &nets(&["10.0.0.0/8"]),
        );

        assert_eq!(resolved, Some("10.0.0.2".parse().unwrap()));
    }

    #[test]
    fn allowlists_match_on_containment() {
        let allowed = nets(&["192.168.0.0/16", "127.0.0.1/32"]);

        assert!(is_allowed(Some("192.168.1.50".parse().unwrap()), &allowed));
        assert!(is_allowed(Some("127.0.0.1".parse().unwrap()), &allowed));
        assert!(!is_allowed(Some("8.8.8.8".parse().unwrap()), &allowed));
    }

    #[test]
    fn an_unresolvable_address_is_denied() {
        // Failing open here would expose the arr surfaces to anyone.
        assert!(!is_allowed(None, &nets(&["0.0.0.0/0"])));
    }
}

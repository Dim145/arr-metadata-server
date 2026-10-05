//! Resolving the real client address.
//!
//! `X-Forwarded-For` is attacker-controlled unless the hop that set it is
//! trusted, and the IP allowlist guarding the Sonarr/Radarr surfaces is only as
//! good as this function. So the header is consulted *only* for peers inside
//! `AMS_TRUSTED_PROXIES`, and then only to walk back past further trusted hops.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use axum::http::HeaderMap;
use ipnet::IpNet;

/// Header name, lowercase, as hyper normalises it.
const FORWARDED_FOR: &str = "x-forwarded-for";

/// What a proxy says about the scheme and the name a request reached it by.
const FORWARDED_PROTO: &str = "x-forwarded-proto";
const FORWARDED_HOST: &str = "x-forwarded-host";

/// Every header a hop may name the client in. Only the first is read, and
/// only from a trusted proxy; the others are recognised so that a proxy
/// nobody declared can be told apart from a client calling directly.
const FORWARDING_HEADERS: [&str; 3] = [FORWARDED_FOR, "forwarded", "x-real-ip"];

/// Who a request came from, as far as this server can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The address to make decisions about.
    Known(IpAddr),
    /// A trusted proxy forwarded a chain that cannot be read — a hop that is
    /// no address, bytes that are no text. The client is unknown: it fails
    /// every allowlist and shares one rate-limit bucket with the others like
    /// it. The proxy's address is kept for the record, and only for that.
    Unreadable { proxy: IpAddr },
    /// No socket address at all.
    Missing,
}

impl Caller {
    /// The address decisions are made about; nothing when it is unknown.
    pub fn address(self) -> Option<IpAddr> {
        match self {
            Self::Known(ip) => Some(ip),
            Self::Unreadable { .. } | Self::Missing => None,
        }
    }
}

/// Who the request came from.
///
/// `peer` is the socket this connection came from. When that peer is a trusted
/// proxy, every `X-Forwarded-For` line — in the order they arrived, as one
/// list — is walked right-to-left and the first address that is *not* itself a
/// trusted proxy is the client. A hop that cannot be read stops the walk: the
/// client is then unknown, never the proxy, whose address is very likely on
/// the allowlist. Every hop trusted means the request came from inside the
/// proxies' own networks, and the peer is taken.
pub fn caller(peer: Option<SocketAddr>, headers: &HeaderMap, trusted: &[IpNet]) -> Caller {
    // No peer address means no basis for a decision; the caller denies.
    let Some(peer) = peer else {
        return Caller::Missing;
    };
    let peer_ip = unmap(peer.ip());

    if !is_trusted(peer_ip, trusted) {
        // The peer is the client. Anything it claims in a header is its own
        // invention and must be ignored.
        return Caller::Known(peer_ip);
    }

    // HAProxy appends a line of its own after whatever the client sent, where
    // nginx appends to the client's line: either way, every line counts, and
    // in order. Read as bytes, so that one byte that is not text spoils the
    // hop it is in rather than the whole header.
    let hops: Vec<&[u8]> = headers
        .get_all(FORWARDED_FOR)
        .iter()
        .flat_map(|line| line.as_bytes().split(|b| *b == b','))
        .map(<[u8]>::trim_ascii)
        .filter(|hop| !hop.is_empty())
        .collect();

    for hop in hops.iter().rev() {
        let Some(ip) = std::str::from_utf8(hop).ok().and_then(parse_entry) else {
            return Caller::Unreadable { proxy: peer_ip };
        };
        let ip = unmap(ip);
        if !is_trusted(ip, trusted) {
            return Caller::Known(ip);
        }
    }

    Caller::Known(peer_ip)
}

/// The address to make access decisions about: [`caller`]'s, or nothing when
/// it is unknown — which every allowlist refuses.
pub fn resolve(peer: Option<SocketAddr>, headers: &HeaderMap, trusted: &[IpNet]) -> Option<IpAddr> {
    caller(peer, headers, trusted).address()
}

/// Whether the socket a request came from is one of the trusted proxies —
/// and so whether what it forwards beyond the address, the host and the
/// scheme it was reached by, may be believed.
pub fn is_trusted_peer(peer: Option<SocketAddr>, trusted: &[IpNet]) -> bool {
    peer.is_some_and(|p| is_trusted(unmap(p.ip()), trusted))
}

/// Whether a peer nobody declared a proxy forwarded a request anyway: one of
/// the headers a proxy names the client in, from outside
/// `AMS_TRUSTED_PROXIES`. Sonarr and Radarr calling directly never send one.
pub fn forwarded_by_stranger(
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    trusted: &[IpNet],
) -> bool {
    !is_trusted_peer(peer, trusted) && FORWARDING_HEADERS.iter().any(|h| headers.contains_key(*h))
}

/// The scheme a request reached this server by, when that is known: `https`
/// over a door this server holds the TLS of, otherwise whatever a trusted
/// proxy says it was reached by. Nothing when nobody vouches for either.
pub fn scheme(
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    trusted: &[IpNet],
    tls_here: bool,
) -> Option<&'static str> {
    if tls_here {
        return Some("https");
    }
    if !is_trusted_peer(peer, trusted) {
        return None;
    }
    match first_value(headers, FORWARDED_PROTO)?
        .to_ascii_lowercase()
        .as_str()
    {
        "https" => Some("https"),
        "http" => Some("http"),
        _ => None,
    }
}

/// The name a trusted proxy says the request reached it by.
pub fn forwarded_host(
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    trusted: &[IpNet],
) -> Option<String> {
    is_trusted_peer(peer, trusted)
        .then(|| first_value(headers, FORWARDED_HOST))
        .flatten()
        .map(str::to_string)
}

/// The first of a comma-separated value, as the hop closest to the client
/// wrote it.
fn first_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// What an address is counted under: itself, or for IPv6 its /64 — one home,
/// one phone — since a single machine has a whole /64 to pick from. Rate
/// limits, sign-up quotas and the record of callers all count this way.
pub fn bucket(ip: IpAddr) -> IpAddr {
    match unmap(ip) {
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            segments[4..].fill(0);
            IpAddr::V6(Ipv6Addr::from(segments))
        }
        v4 => v4,
    }
}

/// `::ffff:10.0.0.7` is `10.0.0.7`.
///
/// A socket bound to `[::]` — which is what a dual-stack listener and most
/// Docker networks give you — reports an IPv4 client in that form, and
/// `IpNet::contains` does not match across families. So `10.0.0.0/8` did not
/// cover the very address it was written for, and every Sonarr and Radarr call
/// was refused. It failed closed, which is the right direction; the wrong part
/// is the fix an operator reaches for next, which is to allow `::/0`.
fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        other => other,
    }
}

pub fn is_trusted(ip: IpAddr, trusted: &[IpNet]) -> bool {
    trusted.iter().any(|net| net.contains(&ip))
}

/// Whether `ip` is inside any of `allowed`.
/// Which rule let this address through, if any.
///
/// The narrowest match wins, so a rule for one address beats the block it sits
/// in — which is what lets a single container be configured differently from
/// the network around it.
pub fn matching_rule(ip: Option<IpAddr>, allowed: &[(String, IpNet)]) -> Option<&str> {
    let ip = ip?;

    allowed
        .iter()
        .filter(|(_, net)| net.contains(&ip))
        .max_by_key(|(_, net)| net.prefix_len())
        .map(|(id, _)| id.as_str())
}

/// One `X-Forwarded-For` element, which may carry a port or be bracketed IPv6.
fn parse_entry(entry: &str) -> Option<IpAddr> {
    if let Ok(ip) = entry.parse::<IpAddr>() {
        return Some(ip);
    }

    // `[2001:db8::1]:443`
    if let Some(rest) = entry.strip_prefix('[')
        && let Some((addr, _)) = rest.split_once(']')
    {
        return addr.parse().ok();
    }

    // `203.0.113.7:51234`
    if let Some((addr, _)) = entry.rsplit_once(':')
        && let Ok(ip) = addr.parse::<IpAddr>()
    {
        return Some(ip);
    }

    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_ipv4_client_on_a_dual_stack_socket_matches_its_own_rule() {
        // What `[::]:8080` — a dual-stack listener, and most Docker networks —
        // reports for an IPv4 client. `IpNet::contains` does not match across
        // families, so without unmapping, `10.0.0.0/8` did not cover the very
        // address it was written for and every arr call was refused.
        let mapped: SocketAddr = "[::ffff:10.0.0.7]:51000".parse().unwrap();
        let resolved = resolve(Some(mapped), &HeaderMap::new(), &[]).unwrap();

        assert_eq!(resolved, "10.0.0.7".parse::<IpAddr>().unwrap());

        let lan: Vec<(String, IpNet)> =
            vec![("rule".into(), "10.0.0.0/8".parse::<IpNet>().unwrap())];
        assert_eq!(matching_rule(Some(resolved), &lan), Some("rule"));
    }

    #[test]
    fn a_real_ipv6_client_is_left_alone() {
        let v6: SocketAddr = "[2001:db8::1]:51000".parse().unwrap();
        let resolved = resolve(Some(v6), &HeaderMap::new(), &[]).unwrap();

        assert_eq!(resolved, "2001:db8::1".parse::<IpAddr>().unwrap());
    }

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

    /// Several `X-Forwarded-For` lines, in the order they arrived.
    fn lines(values: &[&[u8]]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for value in values {
            h.append(
                FORWARDED_FOR,
                axum::http::HeaderValue::from_bytes(value).unwrap(),
            );
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
    fn every_line_counts_and_the_last_is_the_proxys() {
        // HAProxy's `option forwardfor`: the client's own line first, the
        // proxy's after it. Reading the first alone believed the client.
        let resolved = resolve(
            peer("10.0.0.2"),
            &lines(&[b"192.168.1.10", b"203.0.113.5"]),
            &nets(&["10.0.0.0/8"]),
        );

        assert_eq!(resolved, Some("203.0.113.5".parse().unwrap()));
    }

    #[test]
    fn a_byte_that_is_no_text_spoils_its_own_hop_only() {
        // nginx appends the real address to the client's line: one byte the
        // client sent that is not text used to make the whole line unreadable,
        // and the answer the proxy's own — allowlisted — address.
        let resolved = resolve(
            peer("10.0.0.2"),
            &lines(&[b"\xff, 203.0.113.5"]),
            &nets(&["10.0.0.0/8"]),
        );

        assert_eq!(resolved, Some("203.0.113.5".parse().unwrap()));
    }

    #[test]
    fn an_unreadable_hop_leaves_the_client_unknown_never_the_proxy() {
        let trusted = nets(&["10.0.0.0/8"]);
        for chain in [
            &b"garbage, more-garbage"[..],
            b"x",
            b"\xff",
            b"203.0.113.5, unknown",
        ] {
            let caller = caller(peer("10.0.0.2"), &lines(&[chain]), &trusted);
            assert_eq!(
                caller,
                Caller::Unreadable {
                    proxy: "10.0.0.2".parse().unwrap()
                },
                "{:?}",
                String::from_utf8_lossy(chain)
            );
            assert_eq!(caller.address(), None);
        }

        // Unknown is refused by every allowlist, the widest included.
        let everything = vec![("all".to_string(), "0.0.0.0/0".parse::<IpNet>().unwrap())];
        let unknown = resolve(peer("10.0.0.2"), &headers("x"), &trusted);
        assert_eq!(matching_rule(unknown, &everything), None);
    }

    #[test]
    fn empty_hops_carry_nothing_and_are_passed_over() {
        let resolved = resolve(
            peer("10.0.0.2"),
            &lines(&[b"", b" , 203.0.113.5 ,"]),
            &nets(&["10.0.0.0/8"]),
        );

        assert_eq!(resolved, Some("203.0.113.5".parse().unwrap()));
    }

    #[test]
    fn no_peer_is_nobody() {
        assert_eq!(caller(None, &HeaderMap::new(), &[]), Caller::Missing);
        assert_eq!(resolve(None, &headers("203.0.113.5"), &[]), None);
    }

    #[test]
    fn entries_with_ports_or_brackets_still_parse() {
        assert_eq!(
            parse_entry("203.0.113.5"),
            Some("203.0.113.5".parse().unwrap())
        );
        assert_eq!(
            parse_entry("203.0.113.5:443"),
            Some("203.0.113.5".parse().unwrap())
        );
        assert_eq!(
            parse_entry("[2001:db8::1]:443"),
            Some("2001:db8::1".parse().unwrap())
        );
        assert_eq!(
            parse_entry("2001:db8::1"),
            Some("2001:db8::1".parse().unwrap())
        );
        assert_eq!(parse_entry("not-an-ip"), None);
    }

    #[test]
    fn a_stranger_forwarding_is_told_from_a_client_calling_directly() {
        let trusted = nets(&["10.0.0.0/8"]);
        assert!(forwarded_by_stranger(
            peer("172.17.0.1"),
            &headers("203.0.113.5"),
            &trusted
        ));
        let mut real_ip = HeaderMap::new();
        real_ip.insert("x-real-ip", "203.0.113.5".parse().unwrap());
        assert!(forwarded_by_stranger(
            peer("172.17.0.1"),
            &real_ip,
            &trusted
        ));
        // Sonarr calling directly, and a declared proxy, are not strangers.
        assert!(!forwarded_by_stranger(
            peer("172.17.0.1"),
            &HeaderMap::new(),
            &trusted
        ));
        assert!(!forwarded_by_stranger(
            peer("10.0.0.2"),
            &headers("203.0.113.5"),
            &trusted
        ));
    }

    #[test]
    fn the_scheme_is_believed_from_tls_here_or_a_trusted_proxy() {
        let trusted = nets(&["10.0.0.0/8"]);
        let mut proto = HeaderMap::new();
        proto.insert(FORWARDED_PROTO, "HTTPS, http".parse().unwrap());

        assert_eq!(
            scheme(peer("10.0.0.2"), &proto, &trusted, false),
            Some("https")
        );
        // A stranger saying so is a stranger.
        assert_eq!(scheme(peer("203.0.113.5"), &proto, &trusted, false), None);
        // Nobody saying anything: unknown, not http.
        assert_eq!(
            scheme(peer("10.0.0.2"), &HeaderMap::new(), &trusted, false),
            None
        );
        assert_eq!(
            scheme(peer("203.0.113.5"), &HeaderMap::new(), &trusted, true),
            Some("https")
        );

        let mut host = HeaderMap::new();
        host.insert(FORWARDED_HOST, "films.example, inner".parse().unwrap());
        assert_eq!(
            forwarded_host(peer("10.0.0.2"), &host, &trusted).as_deref(),
            Some("films.example")
        );
        assert_eq!(forwarded_host(peer("203.0.113.5"), &host, &trusted), None);
    }

    #[test]
    fn an_ipv6_network_counts_as_one_address() {
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(bucket(a), bucket(b));
        assert_ne!(bucket(a), bucket(c));

        let mapped: IpAddr = "::ffff:192.0.2.1".parse().unwrap();
        assert_eq!(bucket(mapped), "192.0.2.1".parse::<IpAddr>().unwrap());
        let v4: IpAddr = "192.0.2.1".parse().unwrap();
        assert_eq!(bucket(v4), v4);
    }

    /// The shape the guard holds: each block with the rule that put it there.
    fn rules(entries: &[(&str, &str)]) -> Vec<(String, IpNet)> {
        entries
            .iter()
            .map(|(id, cidr)| ((*id).to_string(), cidr.parse().unwrap()))
            .collect()
    }

    #[test]
    fn allowlists_match_on_containment() {
        let allowed = rules(&[("lan", "192.168.0.0/16"), ("local", "127.0.0.1/32")]);

        assert_eq!(
            matching_rule(Some("192.168.1.50".parse().unwrap()), &allowed),
            Some("lan"),
        );
        assert_eq!(
            matching_rule(Some("127.0.0.1".parse().unwrap()), &allowed),
            Some("local"),
        );
        assert_eq!(
            matching_rule(Some("8.8.8.8".parse().unwrap()), &allowed),
            None
        );
    }

    #[test]
    fn the_narrowest_rule_wins() {
        // A container configured differently from the network around it: the
        // rule naming that one address has to beat the block containing it, or
        // its settings would never apply.
        let allowed = rules(&[("network", "172.31.0.0/24"), ("sonarr", "172.31.0.7/32")]);

        assert_eq!(
            matching_rule(Some("172.31.0.7".parse().unwrap()), &allowed),
            Some("sonarr"),
        );
        assert_eq!(
            matching_rule(Some("172.31.0.8".parse().unwrap()), &allowed),
            Some("network"),
        );
    }

    #[test]
    fn an_unresolvable_address_is_denied() {
        // Failing open here would expose the arr surfaces to anyone.
        assert_eq!(matching_rule(None, &rules(&[("all", "0.0.0.0/0")])), None);
    }
}

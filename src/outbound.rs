//! Where this server is willing to send a request of somebody else's choosing.
//!
//! Most outbound URLs here are built from a configured provider base and cannot
//! be steered. Artwork is the exception: an image URL is stored on a work, and
//! anyone with a `write` credential can put one there. The NFO export then
//! fetches it — from inside the network the server is on, which is the whole
//! value of a request somebody else picked.
//!
//! So the rule: a URL may name any ordinary host, and may not name one that only
//! means something from in here. A self-hosted image mirror on a home network is
//! a reasonable thing to own, so private ranges are allowed; the cloud metadata
//! address and this machine's own loopback are not.

use std::net::IpAddr;

/// Whether this address is one only reachable from where this server is
/// standing, and therefore not a place to follow somebody else's URL to.
///
/// Loopback is this machine — including this server's own administration API on
/// another port. Link-local is `169.254.0.0/16`, where every cloud provider
/// parks the instance metadata service that hands out credentials. The
/// unspecified address means "here" to most stacks.
pub fn is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_link_local() || v4.is_unspecified(),
        IpAddr::V6(v6) => {
            // Mapped IPv4 first, or `::ffff:127.0.0.1` would pass as an
            // ordinary global address.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_internal(IpAddr::V4(v4));
            }

            v6.is_loopback()
                || v6.is_unspecified()
                // fe80::/10, which `std` does not yet expose on stable.
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Whether a URL names an internal address *literally*, without asking a
/// resolver.
///
/// For the moment a value is accepted, where a DNS round trip would be a way to
/// make this server do work on request. [`resolves_internally`] is the check
/// that runs when the URL is actually about to be fetched.
pub fn names_internal_host(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        // Unparseable is somebody else's error to report, not this one's.
        return false;
    };

    match parsed.host() {
        Some(url::Host::Ipv4(v4)) => is_internal(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => is_internal(IpAddr::V6(v6)),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        None => false,
    }
}

/// Whether every address this URL's host resolves to is one worth refusing.
///
/// Answered just before the fetch, because a name that resolved to a public
/// address a moment ago can resolve to `127.0.0.1` now — and because a host
/// that resolves to *any* internal address is refused, a name that answers with
/// both does not get through on the strength of the good one.
pub async fn resolves_internally(url: &str) -> bool {
    if names_internal_host(url) {
        return true;
    }

    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };

    let Some(host) = parsed.host_str() else {
        return false;
    };

    let port = parsed.port_or_known_default().unwrap_or(80);

    match tokio::net::lookup_host((host, port)).await {
        Ok(addresses) => addresses.into_iter().any(|a| is_internal(a.ip())),
        // A name that does not resolve is not a place this can reach either.
        // Let the fetch fail on its own and report the real reason.
        Err(_) => false,
    }
}

/// A resolver that never answers with an internal address, for the client
/// that follows addresses somebody else chose: a picture's, a theme's.
///
/// Checked where the connection is made rather than before it, so a name
/// that answered with a public address a moment ago and with loopback now
/// gets nowhere, and so does a redirect to such a name — every hop resolves
/// through here.
#[derive(Clone, Copy, Debug, Default)]
pub struct GuardedResolver;

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .collect();
            if found.iter().any(|a| is_internal(a.ip())) {
                return Err(
                    format!("{host} resolves to an address only this server can reach").into(),
                );
            }
            Ok(Box::new(found.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The client for fetching what somebody else pointed at: resolving through
/// [`GuardedResolver`], following a few redirects, and never for long.
pub fn guarded_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!(
            "arr-metadata-server/",
            env!("CARGO_PKG_VERSION"),
            " (+",
            env!("CARGO_PKG_REPOSITORY"),
            ")"
        ))
        .dns_resolver(std::sync::Arc::new(GuardedResolver))
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_places_a_url_may_not_send_us() {
        for bad in [
            "127.0.0.1",
            "127.1.2.3",
            "::1",
            "0.0.0.0",
            "::",
            // Every cloud's instance metadata service.
            "169.254.169.254",
            "fe80::1",
            // The same loopback, wearing an IPv6 costume.
            "::ffff:127.0.0.1",
        ] {
            assert!(
                is_internal(bad.parse().unwrap()),
                "{bad} should not be reachable by request"
            );
        }
    }

    #[test]
    fn an_ordinary_host_is_still_allowed() {
        // Including a private one: a picture mirror on a home network is a
        // reasonable thing to own, and refusing it would break more than it
        // protects.
        for fine in [
            "8.8.8.8",
            "192.168.1.10",
            "10.0.0.7",
            "172.16.0.1",
            "2001:db8::1",
        ] {
            assert!(!is_internal(fine.parse().unwrap()), "{fine} is fine");
        }
    }

    #[test]
    fn a_url_naming_one_of_them_is_recognised_without_a_resolver() {
        assert!(names_internal_host(
            "http://169.254.169.254/latest/meta-data/"
        ));
        assert!(names_internal_host("http://127.0.0.1:8080/api/v1/clients"));
        assert!(names_internal_host("http://[::1]/"));
        assert!(names_internal_host("http://localhost:8080/"));
        assert!(names_internal_host("http://LocalHost./"));

        assert!(!names_internal_host(
            "https://image.tmdb.org/t/p/original/x.jpg"
        ));
        assert!(!names_internal_host("http://192.168.1.10/posters/x.jpg"));
        // Not a hostname of ours, whatever it is called.
        assert!(!names_internal_host("https://localhost.example.com/x.jpg"));
    }
}

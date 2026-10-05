//! Where this server is willing to send a request of somebody else's choosing.
//!
//! Most outbound URLs here are built from a configured provider base and cannot
//! be steered. Artwork is the exception: an image URL is stored on a work, and
//! anyone with a `write` credential can put one there. The media store and the
//! NFO export then fetch it — from inside the network the server is on, which
//! is the whole value of a request somebody else picked.
//!
//! So the rule: a URL may name any ordinary host, and may not name one that only
//! means something from in here — this machine's loopback, the link-local
//! blocks every cloud parks its metadata service in (and the addresses the
//! others park theirs at), multicast, "this network". A self-hosted image
//! mirror on a home network is a reasonable thing to own, so the private
//! ranges — IPv4's and IPv6's unique-local block, a Tailscale's or an
//! OpenWrt's — are allowed, unless the operator says otherwise:
//! `AMS_MEDIA_PRIVATE_NETWORKS=false`.
//!
//! Checked at every hop: before the request, where each connection is made —
//! the resolver — and at every redirect, whose address arrives after every
//! other check has passed, and never reaches the resolver when it is written
//! as an address.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The metadata services outside the link-local block: Alibaba Cloud's, in
/// the carrier-grade NAT range, and Azure's host endpoint, a public address
/// that answers only from inside its machines.
const METADATA_V4: [Ipv4Addr; 2] = [
    Ipv4Addr::new(100, 100, 100, 200),
    Ipv4Addr::new(168, 63, 129, 16),
];

/// AWS's instance metadata over IPv6, on Nitro instances in an IPv6 subnet:
/// inside the unique-local block, which a home network may use and is
/// allowed with the private ranges, so named on its own, and refused always.
const METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254);

/// The environment variable that keeps that client out of private networks.
pub const PRIVATE_NETWORKS_ENV: &str = "AMS_MEDIA_PRIVATE_NETWORKS";

/// How many redirects one fetch follows.
const MAX_REDIRECTS: usize = 5;

/// Why an address is refused.
const INTERNAL: &str = "an address only this server can reach";
const PRIVATE: &str = "an address in a private network, which this server is set not to fetch";

/// Whether this address is one only reachable from where this server is
/// standing, and therefore not a place to follow somebody else's URL to.
///
/// Loopback is this machine — including this server's own administration API on
/// another port. Link-local is `169.254.0.0/16` and `fe80::/10`, where most
/// cloud providers park the instance metadata service that hands out
/// credentials; the others — AWS's over IPv6, Alibaba's, Azure's — are named
/// one by one. `0.0.0.0/8` and the unspecified address mean "here" to most
/// stacks; multicast and broadcast reach every machine nearby. An IPv6
/// address that only carries an IPv4 one — mapped, NAT64, 6to4 — is judged by
/// the one it carries.
pub fn is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => internal_v4(v4),
        IpAddr::V6(v6) => internal_v6(v6) || embedded_v4(v6).is_some_and(internal_v4),
    }
}

fn internal_v4(v4: Ipv4Addr) -> bool {
    v4.octets()[0] == 0
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_multicast()
        || v4.is_broadcast()
        || METADATA_V4.contains(&v4)
}

fn internal_v6(v6: Ipv6Addr) -> bool {
    let first = v6.segments()[0];
    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        // fe80::/10, link-local, which `std` does not yet expose on stable.
        || (first & 0xffc0) == 0xfe80
        || v6 == METADATA_V6
}

/// Whether this address is in a private network: RFC 1918's three blocks,
/// the shared block carriers and Tailscale hand out, `100.64.0.0/10`, and
/// IPv6's unique-local block, `fc00::/7` — where Tailscale and OpenWrt
/// number a network — with `fec0::/10`, the site-local block it replaced. A
/// home network's, an office's, a mesh's: allowed by default, for a picture
/// mirror kept on one.
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => private_v4(v4),
        IpAddr::V6(v6) => private_v6(v6) || embedded_v4(v6).is_some_and(private_v4),
    }
}

fn private_v6(v6: Ipv6Addr) -> bool {
    let first = v6.segments()[0];
    (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfec0
}

fn private_v4(v4: Ipv4Addr) -> bool {
    let [a, b, _, _] = v4.octets();
    v4.is_private() || (a == 100 && (b & 0xc0) == 64)
}

/// The IPv4 address an IPv6 one stands for, when it is only another way of
/// writing or of reaching one: mapped (`::ffff:a.b.c.d`), compatible
/// (`::a.b.c.d`), translated (`::ffff:0:a.b.c.d`), NAT64's well-known prefix
/// (`64:ff9b::/96`) and its local-use one (`64:ff9b:1::/48`, read as a /96),
/// 6to4 (`2002::/16`) and Teredo's client (`2001::/32`, inverted). A request
/// to any of them can land on the IPv4 address inside.
fn embedded_v4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let v4 = |hi: u16, lo: u16| Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
    match v6.segments() {
        [0, 0, 0, 0, 0, 0xffff, hi, lo]
        | [0, 0, 0, 0, 0, 0, hi, lo]
        | [0, 0, 0, 0, 0xffff, 0, hi, lo]
        | [0x64, 0xff9b, 0, 0, 0, 0, hi, lo]
        | [0x64, 0xff9b, 1, _, _, _, hi, lo]
        | [0x2002, hi, lo, _, _, _, _, _] => Some(v4(hi, lo)),
        [0x2001, 0, _, _, _, _, hi, lo] => Some(v4(!hi, !lo)),
        _ => None,
    }
}

/// Whether a name is this machine's whatever it resolves to.
fn is_local_name(name: &str) -> bool {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    name == "localhost" || name.ends_with(".localhost")
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
        Some(url::Host::Domain(name)) => is_local_name(name),
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

/// What the client for addresses somebody else chose may reach: never an
/// internal address, and a private one unless the operator said not to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guard {
    /// Whether a private network may be reached.
    pub private_networks: bool,
}

impl Default for Guard {
    fn default() -> Self {
        Self {
            private_networks: true,
        }
    }
}

impl Guard {
    /// As the operator set it: `AMS_MEDIA_PRIVATE_NETWORKS`, on unless it
    /// says otherwise. Read here rather than with the rest of the
    /// configuration: this client is all it governs.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_setting(std::env::var(PRIVATE_NETWORKS_ENV).ok().as_deref())
    }

    fn from_setting(value: Option<&str>) -> anyhow::Result<Self> {
        let value = value.map(|v| v.trim().to_ascii_lowercase());
        let private_networks = match value.as_deref() {
            None | Some("" | "1" | "true" | "yes" | "on") => true,
            Some("0" | "false" | "no" | "off") => false,
            Some(other) => {
                anyhow::bail!("{PRIVATE_NETWORKS_ENV} is not a valid boolean: {other:?}")
            }
        };
        Ok(Self { private_networks })
    }

    /// Why an address is out of reach, or nothing when it is not.
    fn verdict(self, ip: IpAddr) -> Option<&'static str> {
        if is_internal(ip) {
            Some(INTERNAL)
        } else if !self.private_networks && is_private(ip) {
            Some(PRIVATE)
        } else {
            None
        }
    }

    /// Whether an address is out of reach.
    pub fn refuses(self, ip: IpAddr) -> bool {
        self.verdict(ip).is_some()
    }

    /// Why a URL is refused on what it says alone — its scheme, an address
    /// written in it, a name that is this machine's — or nothing. Any other
    /// name is the resolver's to judge, as the connection is made.
    pub fn refusal(self, url: &url::Url) -> Option<&'static str> {
        if !matches!(url.scheme(), "http" | "https") {
            return Some("not an http or https address");
        }
        match url.host() {
            Some(url::Host::Ipv4(v4)) => self.verdict(IpAddr::V4(v4)),
            Some(url::Host::Ipv6(v6)) => self.verdict(IpAddr::V6(v6)),
            Some(url::Host::Domain(name)) => is_local_name(name).then_some(INTERNAL),
            None => Some("an address without a host"),
        }
    }

    /// Whether a URL is refused, by what it says or by where its name
    /// resolves now: asked just before the fetch, and again by the resolver
    /// as the connection is made, since a name can resolve elsewhere by then.
    /// A name that resolves to any refused address is refused whole.
    pub async fn refuses_url(self, url: &str) -> bool {
        let Ok(parsed) = url::Url::parse(url) else {
            return true;
        };
        if self.refusal(&parsed).is_some() {
            return true;
        }
        let Some(url::Host::Domain(host)) = parsed.host() else {
            // An address, judged above.
            return false;
        };
        let port = parsed.port_or_known_default().unwrap_or(80);
        match tokio::net::lookup_host((host, port)).await {
            Ok(addresses) => addresses.into_iter().any(|a| self.refuses(a.ip())),
            // Let the fetch fail on its own and report the real reason.
            Err(_) => false,
        }
    }

    /// What becomes of a redirect to `next` once `hops` requests have been
    /// made: followed, or refused with the reason. An address written in the
    /// URL is judged here, because the connection to it never asks the
    /// resolver; a name is judged by the resolver.
    pub fn redirect(self, next: &url::Url, hops: usize) -> Result<(), &'static str> {
        if hops > MAX_REDIRECTS {
            return Err("too many redirects");
        }
        match self.refusal(next) {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }

    fn redirect_policy(self) -> reqwest::redirect::Policy {
        reqwest::redirect::Policy::custom(move |attempt| {
            match self.redirect(attempt.url(), attempt.previous().len()) {
                Ok(()) => attempt.follow(),
                Err(why) => attempt.error(format!("redirected to {why}")),
            }
        })
    }
}

/// A resolver that never answers with an address the [`Guard`] refuses, for
/// the client that follows addresses somebody else chose: a picture's, a
/// theme's.
///
/// Checked where the connection is made rather than before it, so a name
/// that answered with a public address a moment ago and with loopback now
/// gets nowhere, and so does a redirect to such a name — every hop to a name
/// resolves through here. A hop to an address does not, and is judged by the
/// redirect policy instead.
#[derive(Clone, Copy, Debug, Default)]
pub struct GuardedResolver {
    guard: Guard,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let guard = self.guard;
        let host = name.as_str().to_string();
        Box::pin(async move {
            if is_local_name(&host) {
                return Err(format!("{host} is {INTERNAL}").into());
            }
            let found: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .collect();
            if let Some(why) = found.iter().find_map(|a| guard.verdict(a.ip())) {
                return Err(format!("{host} resolves to {why}").into());
            }
            Ok(Box::new(found.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The client for fetching what somebody else pointed at: resolving through
/// [`GuardedResolver`], judging every redirect, going direct, and never for
/// long.
pub fn guarded_client(guard: Guard) -> anyhow::Result<reqwest::Client> {
    guarded_builder(guard).build().map_err(Into::into)
}

fn guarded_builder(guard: Guard) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(concat!(
            "arr-metadata-server/",
            env!("CARGO_PKG_VERSION"),
            " (+",
            env!("CARGO_PKG_REPOSITORY"),
            ")"
        ))
        .dns_resolver(std::sync::Arc::new(GuardedResolver { guard }))
        // A proxy resolves the name itself, out of the resolver's sight, and
        // reaches whatever it can reach from where it stands: these requests
        // go direct, whatever HTTP_PROXY or ALL_PROXY say.
        .no_proxy()
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(10))
        .redirect(guard.redirect_policy())
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
            "0.1.2.3",
            "::",
            // Every cloud's instance metadata service, and the few parked
            // elsewhere: Alibaba's, Azure's host, AWS's over IPv6.
            "169.254.169.254",
            "100.100.100.200",
            "168.63.129.16",
            "fd00:ec2::254",
            "fe80::1",
            // Multicast, broadcast.
            "ff02::1",
            "224.0.0.1",
            "255.255.255.255",
            // The same loopback and metadata, wearing IPv6 costumes: mapped,
            // compatible, translated, NAT64, local-use NAT64, 6to4, Teredo.
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::169.254.169.254",
            "::ffff:0:7f00:1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::7f00:1",
            "2002:7f00:1::1",
            "2001:0:4136:e378:8000:63bf:80ff:fffe",
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
            "100.64.0.1",
            "2001:db8::1",
            "2606:4700::1111",
            // Unique local: a home network's, a Tailscale's — private, not
            // internal, but for AWS's metadata address inside it.
            "fd12:3456:789a::1",
            "fd7a:115c:a1e0::1",
            "fec0::1",
            // A public address reached through NAT64 is a public address.
            "64:ff9b::808:808",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_internal(fine.parse().unwrap()), "{fine} is fine");
        }
    }

    #[test]
    fn a_private_network_is_known_in_every_costume() {
        for private in [
            "10.0.0.7",
            "172.31.255.1",
            "192.168.1.10",
            "100.64.0.1",
            "100.127.255.254",
            "::ffff:192.168.1.10",
            "64:ff9b::a00:1",
            "2002:c0a8:10a::1",
            "fc00::1",
            "fd12:3456:789a::1",
            "fd7a:115c:a1e0::1",
            "fec0::1",
        ] {
            assert!(is_private(private.parse().unwrap()), "{private}");
        }
        for public in ["8.8.8.8", "100.128.0.1", "172.32.0.1", "2001:db8::1"] {
            assert!(!is_private(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn a_url_naming_one_of_them_is_recognised_without_a_resolver() {
        assert!(names_internal_host(
            "http://169.254.169.254/latest/meta-data/"
        ));
        assert!(names_internal_host("http://127.0.0.1:8080/api/v1/clients"));
        assert!(names_internal_host("http://[::1]/"));
        assert!(names_internal_host("http://[::ffff:127.0.0.1]/"));
        assert!(names_internal_host("http://[fd00:ec2::254]/latest/"));
        assert!(names_internal_host("http://100.100.100.200/latest/"));
        assert!(names_internal_host("http://localhost:8080/"));
        assert!(names_internal_host("http://LocalHost./"));
        // Written as a number, in hexadecimal, in short: the same loopback.
        assert!(names_internal_host("http://2130706433/"));
        assert!(names_internal_host("http://0x7f.1/"));

        assert!(!names_internal_host(
            "https://image.tmdb.org/t/p/original/x.jpg"
        ));
        assert!(!names_internal_host("http://192.168.1.10/posters/x.jpg"));
        // Not a hostname of ours, whatever it is called.
        assert!(!names_internal_host("https://localhost.example.com/x.jpg"));
    }

    fn url(text: &str) -> url::Url {
        url::Url::parse(text).unwrap()
    }

    #[test]
    fn a_redirect_is_judged_by_where_it_points() {
        let guard = Guard::default();

        // An ordinary host, by name or by address, and a home network's.
        for fine in [
            "https://image.tmdb.org/t/p/original/a.jpg",
            "http://assets.fanart.tv/fanart/tv/1/poster.jpg",
            "http://192.168.1.10/posters/a.jpg",
            "http://[fd7a:115c:a1e0::1]/posters/a.jpg",
            "http://[2606:4700::1111]/a.jpg",
            "http://8.8.8.8/a.jpg",
        ] {
            assert_eq!(guard.redirect(&url(fine), 1), Ok(()), "{fine}");
        }

        // Written as addresses, which the resolver never sees: loopback in
        // every spelling, the metadata services, this machine by name.
        for bad in [
            "http://127.0.0.1:8479/api/v1/clients",
            "http://2130706433/",
            "http://0x7f.0.0.1/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[64:ff9b::7f00:1]/",
            "http://0.0.0.0:8479/",
            "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
            "http://[fd00:ec2::254]/latest/meta-data/",
            "http://100.100.100.200/latest/meta-data/",
            "http://[fe80::1]/",
            "http://localhost/",
            "http://api.localhost./",
        ] {
            assert_eq!(guard.redirect(&url(bad), 1), Err(INTERNAL), "{bad}");
        }

        // Somewhere that is not the web at all.
        assert!(guard.redirect(&url("file:///etc/passwd"), 1).is_err());
        assert!(guard.redirect(&url("ftp://example.com/a.jpg"), 1).is_err());

        // Five redirects, and no more.
        let fine = url("https://image.tmdb.org/a.jpg");
        assert_eq!(guard.redirect(&fine, MAX_REDIRECTS), Ok(()));
        assert!(guard.redirect(&fine, MAX_REDIRECTS + 1).is_err());
    }

    #[test]
    fn private_networks_can_be_refused_too() {
        let guard = Guard::from_setting(Some("false")).unwrap();
        assert!(!guard.private_networks);
        for bad in [
            "http://192.168.1.10/a.jpg",
            "http://10.0.0.7/a.jpg",
            "http://100.64.0.1/a.jpg",
            "http://[::ffff:172.16.0.1]/a.jpg",
            "http://[fd7a:115c:a1e0::1]/a.jpg",
            "http://[fd12:3456:789a::1]/a.jpg",
        ] {
            assert_eq!(guard.redirect(&url(bad), 1), Err(PRIVATE), "{bad}");
        }
        assert_eq!(
            guard.redirect(&url("https://image.tmdb.org/a.jpg"), 1),
            Ok(())
        );
        // Internal stays internal, the metadata inside the unique-local
        // block with it.
        for internal in ["http://169.254.169.254/", "http://[fd00:ec2::254]/"] {
            assert_eq!(
                guard.redirect(&url(internal), 1),
                Err(INTERNAL),
                "{internal}"
            );
        }
    }

    #[test]
    fn the_switch_reads_as_a_boolean() {
        for on in [None, Some(""), Some("true"), Some(" ON "), Some("1")] {
            assert!(Guard::from_setting(on).unwrap().private_networks, "{on:?}");
        }
        for off in [Some("false"), Some("No"), Some("0"), Some("off")] {
            assert!(
                !Guard::from_setting(off).unwrap().private_networks,
                "{off:?}"
            );
        }
        assert!(Guard::from_setting(Some("maybe")).is_err());
    }

    #[tokio::test]
    async fn a_url_is_refused_before_the_fetch_by_what_it_says() {
        let guard = Guard::default();
        assert!(guard.refuses_url("http://127.0.0.1/").await);
        assert!(guard.refuses_url("http://[::1]:8479/").await);
        assert!(guard.refuses_url("http://localhost/").await);
        assert!(guard.refuses_url("gopher://example.com/").await);
        assert!(guard.refuses_url("not a url").await);
        assert!(!guard.refuses_url("http://192.168.1.10/a.jpg").await);
        assert!(
            Guard::from_setting(Some("false"))
                .unwrap()
                .refuses_url("http://192.168.1.10/a.jpg")
                .await
        );
    }

    /// The client itself: a host that answers with a redirect into this
    /// machine is not followed there, whichever way the address is written,
    /// and a redirect to a name is followed to wherever the resolver lets it.
    #[tokio::test]
    async fn a_redirect_into_this_machine_is_not_followed() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let reached = Arc::new(AtomicBool::new(false));
        let server = tokio::spawn({
            let reached = reached.clone();
            async move {
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else {
                        return;
                    };
                    let mut buffer = vec![0u8; 4096];
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let to = match path.as_str() {
                        "/secret" => {
                            reached.store(true, Ordering::SeqCst);
                            None
                        }
                        "/to-loopback" => Some(format!("http://127.0.0.1:{port}/secret")),
                        "/to-v6-loopback" => Some(format!("http://[::1]:{port}/secret")),
                        "/to-mapped" => Some(format!("http://[::ffff:7f00:1]:{port}/secret")),
                        "/to-metadata" => Some("http://169.254.169.254/latest/meta-data/".into()),
                        "/to-name" => Some(format!("http://redirector.test:{port}/secret")),
                        _ => None,
                    };
                    let response = match to {
                        Some(to) => format!(
                            "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        ),
                        None => "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecret"
                            .to_string(),
                    };
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                }
            }
        });

        // The test's own name for its server, past the guard: the first hop
        // has to land somewhere, and loopback is all a test has.
        let client = guarded_builder(Guard::default())
            .resolve("redirector.test", addr)
            .build()
            .unwrap();

        for path in ["to-loopback", "to-v6-loopback", "to-mapped", "to-metadata"] {
            let error = client
                .get(format!("http://redirector.test:{port}/{path}"))
                .send()
                .await
                .expect_err(path);
            assert!(error.is_redirect(), "{path}: {error:#}");
        }
        assert!(
            !reached.load(Ordering::SeqCst),
            "nothing was fetched from this machine"
        );

        let body = client
            .get(format!("http://redirector.test:{port}/to-name"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "secret");

        server.abort();
    }
}

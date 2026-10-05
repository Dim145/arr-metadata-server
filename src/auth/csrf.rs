//! A change carried by an ambient credential must be asked from this
//! server's own pages.
//!
//! The session cookie is `SameSite=Strict`, but a *site* is a registrable
//! domain: every other application on `nas.lan`, whatever its port, and every
//! sibling under `home.example`, is the same site, and the cookie rides along
//! on what their pages send here. A plain form or a body-less `POST` needs no
//! preflight, so CORS never gets a say either. What the browser says about the
//! request settles it instead:
//!
//! * **`Sec-Fetch-Site`**, which every current browser sends and no page can
//!   set: `same-origin` (a page of this server) and `none` (somebody typed the
//!   address) go on; anything else is refused — unless the page is on an
//!   origin the operator named, in `AMS_CORS_ORIGINS` or `AMS_PUBLIC_URL`.
//! * **`Origin`**, where an older browser sends only that: it has to be this
//!   server's own — the name the request was addressed to, or a trusted
//!   proxy's `X-Forwarded-Host`, in the scheme the request came in when that
//!   is known — or a named one.
//! * **Neither**: not a browser. Sonarr, `curl`, a script — nothing a page
//!   elsewhere can make a browser send, and nothing to refuse.
//!
//! A request authenticated by an API key is not ambient: a page elsewhere
//! cannot attach one. The guard asks this only of the others.

use axum::http::{HeaderMap, Method, header};

/// What a request's own origin may be, as far as this server can tell.
#[derive(Debug, Default)]
pub struct Own<'a> {
    /// The names it was addressed to, as `host[:port]`: its `Host` — the
    /// `:authority` over HTTP/2 — and a trusted proxy's `X-Forwarded-Host`.
    pub hosts: Vec<String>,
    /// The scheme it came in, when this server knows it: TLS held here, or a
    /// trusted proxy's `X-Forwarded-Proto`.
    pub scheme: Option<&'a str>,
    /// Origins the operator named: `AMS_PUBLIC_URL`'s and `AMS_CORS_ORIGINS`.
    pub named: Vec<String>,
}

/// Whether a request may change something, or why not.
pub fn check(method: &Method, headers: &HeaderMap, own: &Own<'_>) -> Result<(), &'static str> {
    if matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    ) {
        return Ok(());
    }

    let origin = headers
        .get(header::ORIGIN)
        .map(|v| v.to_str().map(str::trim).unwrap_or("null"));
    let named = |text: &str| {
        parse_origin(text).is_some_and(|o| {
            own.named
                .iter()
                .map(String::as_str)
                .filter_map(parse_origin)
                .any(|n| n == o)
        })
    };

    match headers
        .get("sec-fetch-site")
        .map(|v| v.to_str().unwrap_or(""))
    {
        Some(site) => match site.trim().to_ascii_lowercase().as_str() {
            "same-origin" | "none" => Ok(()),
            _ if origin.is_some_and(named) => Ok(()),
            _ => Err("the request was sent by a page of another site"),
        },
        None => match origin {
            None => Ok(()),
            Some(text) if named(text) || addressed_here(text, own) => Ok(()),
            Some(_) => Err("the request's origin is not this server's"),
        },
    }
}

/// An origin, as `(scheme, host, port)`: `https://Films.example` is
/// `("https", "films.example", 443)`. Nothing for `null`, or anything that is
/// not an http(s) origin.
fn parse_origin(text: &str) -> Option<(String, String, u16)> {
    let url = url::Url::parse(text.trim()).ok()?;
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    Some((
        scheme.to_string(),
        url.host_str()?.to_ascii_lowercase(),
        url.port_or_known_default()?,
    ))
}

/// Whether the origin is the name the request was addressed to.
fn addressed_here(text: &str, own: &Own<'_>) -> bool {
    let Some((scheme, host, port)) = parse_origin(text) else {
        return false;
    };
    if own.scheme.is_some_and(|known| known != scheme) {
        return false;
    }
    // A `Host` leaves its port out where it is the scheme's own: the one
    // the request came in, or, not knowing it, the one the page names.
    let default = default_port(own.scheme.unwrap_or(&scheme));
    own.hosts.iter().any(|addressed| {
        let (name, explicit) = split_port(addressed.trim());
        name.eq_ignore_ascii_case(&host) && explicit.unwrap_or(default) == port
    })
}

fn default_port(scheme: &str) -> u16 {
    if scheme == "https" { 443 } else { 80 }
}

/// `films.example:8080` is `("films.example", Some(8080))`; `[::1]:8080` is
/// `("[::1]", Some(8080))`, its brackets kept, as a URL's host keeps them.
fn split_port(authority: &str) -> (&str, Option<u16>) {
    if authority.starts_with('[') {
        return match authority.find(']') {
            Some(end) => (
                &authority[..=end],
                authority[end + 1..]
                    .strip_prefix(':')
                    .and_then(|p| p.parse().ok()),
            ),
            None => (authority, None),
        };
    }
    match authority.rsplit_once(':') {
        Some((name, port)) => match port.parse() {
            Ok(port) => (name, Some(port)),
            Err(_) => (authority, None),
        },
        None => (authority, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        h
    }

    fn own(hosts: &[&str]) -> Own<'static> {
        Own {
            hosts: hosts.iter().map(|h| (*h).to_string()).collect(),
            scheme: None,
            named: Vec::new(),
        }
    }

    fn post(pairs: &[(&str, &str)], own: &Own<'_>) -> Result<(), &'static str> {
        check(&Method::POST, &headers(pairs), own)
    }

    #[test]
    fn reading_is_never_refused() {
        let here = own(&["ams.lan:8080"]);
        let elsewhere = [
            ("sec-fetch-site", "cross-site"),
            ("origin", "https://evil.example"),
        ];
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(check(&method, &headers(&elsewhere), &here).is_ok());
        }
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(
                check(&method, &headers(&elsewhere), &here).is_err(),
                "{method}"
            );
        }
    }

    #[test]
    fn the_browsers_own_word_settles_it() {
        let here = own(&["ams.lan:8080"]);
        assert!(post(&[("sec-fetch-site", "same-origin")], &here).is_ok());
        assert!(post(&[("sec-fetch-site", "none")], &here).is_ok());
        // Another port of the same host is the same site, and refused.
        assert!(post(&[("sec-fetch-site", "same-site")], &here).is_err());
        assert!(post(&[("sec-fetch-site", "cross-site")], &here).is_err());
        assert!(post(&[("sec-fetch-site", "something-new")], &here).is_err());
        // Same origin, said by the browser, even behind a proxy that
        // rewrote the Host: the browser compared the real one.
        let rewritten = own(&["metadata:8080"]);
        assert!(
            post(
                &[
                    ("sec-fetch-site", "same-origin"),
                    ("origin", "https://films.example")
                ],
                &rewritten
            )
            .is_ok()
        );
    }

    #[test]
    fn nothing_said_is_not_a_browser() {
        assert!(post(&[], &own(&["ams.lan:8080"])).is_ok());
        assert!(post(&[], &Own::default()).is_ok());
    }

    #[test]
    fn an_older_browser_is_judged_by_its_origin() {
        let here = own(&["ams.lan:8080"]);
        assert!(post(&[("origin", "http://ams.lan:8080")], &here).is_ok());
        assert!(post(&[("origin", "http://AMS.lan:8080")], &here).is_ok());
        // The same host on another port, another host, an opaque origin.
        assert!(post(&[("origin", "http://ams.lan:8989")], &here).is_err());
        assert!(post(&[("origin", "http://sonarr.lan:8080")], &here).is_err());
        assert!(post(&[("origin", "null")], &here).is_err());
        assert!(post(&[("origin", "file://")], &here).is_err());
    }

    #[test]
    fn a_default_port_is_the_same_port_left_out() {
        let here = own(&["films.example"]);
        assert!(post(&[("origin", "https://films.example")], &here).is_ok());
        assert!(post(&[("origin", "http://films.example")], &here).is_ok());
        assert!(post(&[("origin", "http://films.example:8080")], &here).is_err());

        let explicit = own(&["films.example:443"]);
        assert!(post(&[("origin", "https://films.example")], &explicit).is_ok());
        assert!(post(&[("origin", "http://films.example")], &explicit).is_err());

        let v6 = own(&["[::1]:8080"]);
        assert!(post(&[("origin", "http://[::1]:8080")], &v6).is_ok());
        assert!(post(&[("origin", "http://[::1]:8081")], &v6).is_err());
    }

    #[test]
    fn a_known_scheme_has_to_match() {
        let mut here = own(&["films.example"]);
        here.scheme = Some("https");
        assert!(post(&[("origin", "https://films.example")], &here).is_ok());
        // Plain http on the same name is another origin.
        assert!(post(&[("origin", "http://films.example")], &here).is_err());
    }

    #[test]
    fn a_trusted_proxys_name_counts_as_the_address() {
        let here = own(&["metadata:8080", "films.example"]);
        assert!(post(&[("origin", "https://films.example")], &here).is_ok());
    }

    #[test]
    fn origins_the_operator_named_are_let_in() {
        let mut here = own(&["metadata:8080"]);
        here.named = vec![
            "https://films.example/".into(),
            "http://dashboard.lan:3000".into(),
        ];
        // The public address, from a browser too old to say more.
        assert!(post(&[("origin", "https://films.example")], &here).is_ok());
        // A named origin calling from another site, with CORS's consent.
        assert!(
            post(
                &[
                    ("sec-fetch-site", "same-site"),
                    ("origin", "http://dashboard.lan:3000")
                ],
                &here
            )
            .is_ok()
        );
        assert!(
            post(
                &[
                    ("sec-fetch-site", "cross-site"),
                    ("origin", "http://dashboard.lan:3001")
                ],
                &here
            )
            .is_err()
        );
        // Without its origin, another site is just another site.
        assert!(post(&[("sec-fetch-site", "cross-site")], &here).is_err());
    }

    #[test]
    fn a_port_is_split_off_the_name_it_follows() {
        assert_eq!(split_port("films.example"), ("films.example", None));
        assert_eq!(
            split_port("films.example:8080"),
            ("films.example", Some(8080))
        );
        assert_eq!(split_port("[::1]:8080"), ("[::1]", Some(8080)));
        assert_eq!(split_port("[::1]"), ("[::1]", None));
        assert_eq!(split_port("host:notaport"), ("host:notaport", None));
    }
}

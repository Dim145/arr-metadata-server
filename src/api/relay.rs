//! What the relays share: a request handed on to a service in a client's
//! place, and its answer handed back.
//!
//! Four services are relayed this way — TMDB under `/3`, TheTVDB under `/v4`,
//! AniList by its name, and Sonarr's own services for whatever this server
//! does not answer itself. Each keeps its own rules for the credentials it
//! stands in for and for what it writes into an answer; what they have in
//! common is here: which headers travel, which never do, what a path may not
//! say, and the work a document is patched for.

use std::{sync::LazyLock, time::Duration};

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header};

use crate::{
    cache,
    domain::{CoverType, ExternalSource, MediaItem},
    error::AppResult,
    service,
    state::AppState,
};

/// The client the relays ask with: following no redirect — a redirect is
/// handed back to the caller as it came, since the address it names was
/// chosen by somebody else.
pub static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
});

/// Hop-by-hop headers (RFC 9110 §7.6.1), and those the HTTP stack sets
/// itself: the body is read decoded, so its encoding and length are the
/// stack's to say. This server's own mark is one too: it names the instance
/// that sent a request, not the one the answer comes from.
pub const HOP_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "content-encoding",
    "accept-encoding",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    crate::providers::radarr::LOOP_HEADER,
];

/// Query parameters that carry a key for this server, never the service's.
///
/// Every spelling of a key the guard accepts has to be here too. A client may
/// authenticate with `?apikey=`, and for a while only `?api_key=` was
/// stripped — so its credential for *this* server was relayed to TMDB, and
/// landed in TMDB's access logs on every request.
pub const OUR_PARAMS: &[&str] = &["apikey", "api_key"];

/// Whether a request was addressed to one of `hosts`.
///
/// HTTP/2 names the host in the address, HTTP/1.1 in `Host`, with its port.
/// The names are hostnames, so everything from a colon on is the port — and
/// an address in brackets is never one of them.
pub fn addressed_to(headers: &HeaderMap, uri: &Uri, hosts: &[&str]) -> bool {
    // `Host` first, then the address, as the doors' own host check reads a
    // request: a request that names one host in each must not pass the
    // check under one name and be dispatched under the other.
    let named = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|host| host.split(':').next())
        .map(str::to_string)
        .or_else(|| uri.host().map(str::to_string));
    named.is_some_and(|name| {
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        hosts.contains(&name.as_str())
    })
}

/// Whether a request was sent by an instance of this server: no client sends
/// the header, so one that carries it — whosever — has come round a loop,
/// through a resolver or a door several instances share.
pub fn looped(headers: &HeaderMap) -> bool {
    headers.contains_key(crate::providers::radarr::LOOP_HEADER)
}

/// The query asked, less the parameters named, as it was written; none when
/// nothing is left of it.
pub fn query_without(uri: &Uri, dropped: &[&str]) -> Option<String> {
    let kept: Vec<&str> = uri
        .query()?
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let name = pair.split('=').next().unwrap_or_default();
            !dropped.iter().any(|ours| name.eq_ignore_ascii_case(ours))
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join("&"))
}

/// The path and query asked, less a key for this server.
pub fn without_our_params(uri: &Uri) -> String {
    match query_without(uri, OUR_PARAMS) {
        Some(query) => format!("{}?{query}", uri.path()),
        None => uri.path().to_string(),
    }
}

/// The query asked, less the parameters named, in a fixed order: what a
/// cached document is filed under, so that two spellings of one request are
/// one entry.
///
/// Ordered by name alone: a parameter given twice keeps the order it was
/// given in, which is the order a service reads it in — the last
/// `language` wins — so `language=fr&language=de` and the other way round,
/// two answers, are two entries.
pub fn sorted_query(uri: &Uri, dropped: &[&str]) -> String {
    let mut pairs: Vec<(String, String)> = uri
        .query()
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .filter(|(name, _)| !dropped.iter().any(|d| name.eq_ignore_ascii_case(d)))
                .map(|(name, value)| (name.into_owned(), value.into_owned()))
                .collect()
        })
        .unwrap_or_default();
    // Stable: equal names stay as they came.
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

/// How long a relay keeps an answer of this status, when it keeps it at
/// all: a document for as long as its kind stays good — `path` as
/// [`cache::relay_ttl`] reads it — and a "no such thing" a few minutes.
/// Any other answer is asked again: another success than `200` — a
/// fragment of a document, "no content" — a redirect, "unchanged", a
/// refusal or an error was one caller's answer, or is gone the next minute.
pub fn kept_for(status: StatusCode, path: &str) -> Option<Duration> {
    match status {
        StatusCode::OK => Some(cache::relay_ttl(path)),
        StatusCode::NOT_FOUND => Some(Duration::from_secs(5 * 60)),
        _ => None,
    }
}

/// The caller's headers the service is asked with: those named, and nothing
/// else. Not a credential, a cookie, or a proxy's `X-Forwarded-For`.
pub fn asked_with(headers: &HeaderMap, names: &[&str]) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        if names.contains(&name.as_str()) {
            out.append(name.clone(), value.clone());
        }
    }
    out
}

/// The headers an answer keeps: all but the hop-by-hop ones, a cookie,
/// which would be set against this server — where the session cookie lives
/// — and the service's own word on cross-origin reads, which is this
/// server's to give for its own origin.
pub fn kept(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        if HOP_HEADERS.contains(&name.as_str())
            || name == header::SET_COOKIE
            || name == header::COOKIE
            || name.as_str().starts_with("access-control-")
        {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            out.append(name, value);
        }
    }
    out
}

/// The validators a patched answer may not carry: the service's described
/// the service's bytes, and a client holding them would be told "unchanged"
/// for a body it never saw the final shape of.
pub fn without_validators(headers: &mut HeaderMap) {
    for name in [
        header::ETAG,
        header::LAST_MODIFIED,
        header::CACHE_CONTROL,
        header::EXPIRES,
    ] {
        headers.remove(name);
    }
}

/// Whether any segment of `path` could be read as leaving its place: a dot
/// segment, encoded or not, or a separator hidden inside a segment.
///
/// Percent-decoded first, because `%2e%2e` and `..` mean the same thing to
/// the URL parser that builds the outgoing request and different things to a
/// naive comparison. To that parser a backslash is a slash, so
/// `list/..\account` is `account`; and `%2F` survives it, but what a
/// service's own edge makes of `..%2F` is not this server's to find out. An
/// un-decodable escape is treated as suspicious rather than harmless —
/// nothing these services address needs one.
pub fn climbs(path: &str) -> bool {
    path.split('/').any(|segment| {
        match urlencoding::decode(segment) {
            Ok(decoded) => matches!(decoded.as_ref(), "." | "..") || decoded.contains(['/', '\\']),
            // Not valid UTF-8 once decoded: not a path of theirs either.
            Err(_) => true,
        }
    })
}

pub fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
}

/// The bearer token a request carries, when it carries one.
pub fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?
        .trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// A work of this catalogue a service's document is about, when a person
/// locked something on it: what a relay writes into the answer. The
/// pictures it names are addressed as a client can fetch them.
pub async fn locked_work(
    state: &AppState,
    source: ExternalSource,
    id: &str,
) -> AppResult<Option<MediaItem>> {
    let Some(found) = crate::db::repo::item::find_id_by_external(&state.db, source, id).await?
    else {
        return Ok(None);
    };
    let Some(mut item) = service::load(state, &found).await? else {
        return Ok(None);
    };
    if item.locked_fields.is_empty() {
        return Ok(None);
    }
    state.media.for_clients(&mut item);
    Ok(Some(item))
}

/// Whether a field of the work itself is locked: `title`, `overview`,
/// `primaryPoster`, as the field registry names them.
pub fn is_locked(item: &MediaItem, field: &str) -> bool {
    let key = format!("item/{field}");
    item.locked_fields.contains(&key)
}

/// The address of the picture a person chose for a work, when they chose
/// one: the poster or the backdrop.
pub fn chosen_image(item: &MediaItem, kind: CoverType) -> Option<String> {
    let chosen = match kind {
        CoverType::Poster => item.primary_images.poster.as_deref(),
        CoverType::Fanart => item.primary_images.fanart.as_deref(),
        _ => None,
    }?;
    item.images
        .iter()
        .find(|image| image.id == chosen)
        .map(|image| image.url.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn a_request_names_its_service_by_its_host() {
        let path: Uri = "/v1/time".parse().unwrap();
        let hosts = &["services.sonarr.tv", "graphql.anilist.co"];
        assert!(addressed_to(&host("services.sonarr.tv"), &path, hosts));
        assert!(addressed_to(&host("GraphQL.AniList.co:443"), &path, hosts));
        assert!(addressed_to(&host("graphql.anilist.co."), &path, hosts));
        assert!(!addressed_to(&host("skyhook.sonarr.tv"), &path, hosts));
        assert!(!addressed_to(
            &host("graphql.anilist.co.evil"),
            &path,
            hosts
        ));
        assert!(!addressed_to(&HeaderMap::new(), &path, hosts));

        // HTTP/2 carries it in the address.
        let full: Uri = "https://graphql.anilist.co/".parse().unwrap();
        assert!(addressed_to(&HeaderMap::new(), &full, hosts));
        // Named in both, `Host` is the one read, as the doors read it.
        assert!(!addressed_to(&host("skyhook.sonarr.tv"), &full, hosts));
    }

    #[test]
    fn a_key_for_this_server_is_not_relayed_in_the_address() {
        let uri: Uri = "/v1/update/main/changes?version=4.0&apikey=ams_x&os=linux&API_KEY=y"
            .parse()
            .unwrap();
        assert_eq!(
            without_our_params(&uri),
            "/v1/update/main/changes?version=4.0&os=linux"
        );
        let bare: Uri = "/v1/time?apikey=ams_x".parse().unwrap();
        assert_eq!(without_our_params(&bare), "/v1/time");
        assert_eq!(query_without(&bare, OUR_PARAMS), None);
    }

    #[test]
    fn a_cached_document_is_filed_under_one_spelling_of_its_query() {
        let a: Uri = "/v4/series/1?page=1&meta=episodes&apikey=k"
            .parse()
            .unwrap();
        let b: Uri = "/v4/series/1?meta=episodes&page=1".parse().unwrap();
        assert_eq!(sorted_query(&a, OUR_PARAMS), sorted_query(&b, OUR_PARAMS));
        assert_eq!(sorted_query(&a, OUR_PARAMS), "meta=episodes&page=1");
        let none: Uri = "/v4/series/1".parse().unwrap();
        assert_eq!(sorted_query(&none, OUR_PARAMS), "");

        // A parameter given twice is read last-wins upstream: two orders,
        // two answers, two entries.
        let fr_last: Uri = "/3/movie/1?language=de&page=1&language=fr".parse().unwrap();
        let de_last: Uri = "/3/movie/1?language=fr&page=1&language=de".parse().unwrap();
        assert_ne!(
            sorted_query(&fr_last, OUR_PARAMS),
            sorted_query(&de_last, OUR_PARAMS)
        );
        assert_eq!(
            sorted_query(&fr_last, OUR_PARAMS),
            "language=de&language=fr&page=1"
        );
    }

    #[test]
    fn only_a_whole_document_or_a_no_such_thing_is_kept() {
        assert_eq!(
            kept_for(StatusCode::OK, "/v4/series/1"),
            Some(cache::relay_ttl("/v4/series/1"))
        );
        assert_eq!(
            kept_for(StatusCode::NOT_FOUND, "/v4/series/1"),
            Some(Duration::from_secs(5 * 60))
        );
        for status in [
            StatusCode::CREATED,
            StatusCode::NON_AUTHORITATIVE_INFORMATION,
            StatusCode::NO_CONTENT,
            StatusCode::PARTIAL_CONTENT,
            StatusCode::MOVED_PERMANENTLY,
            StatusCode::FOUND,
            StatusCode::NOT_MODIFIED,
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
        ] {
            assert_eq!(kept_for(status, "/v4/series/1"), None, "{status}");
        }
    }

    #[test]
    fn a_path_that_climbs_out_of_its_place_is_refused() {
        // The outgoing URL is built by interpolation and parsed by reqwest,
        // which resolves dot segments — so this one would have left the v3
        // API for the v4 one, carrying the operator's credentials with it.
        assert!(climbs("/3/%2e%2e/4/account"));
        // A backslash is a slash to that parser.
        assert!(climbs("/4/list/..\\account/1/lists"));
        assert!(climbs("/4/list/8136%2F..%2F..%2Faccount"));
        assert!(climbs("/3/movie/238%5C..%5C4"));
        assert!(climbs("/3/../4/account"));
        assert!(climbs("/v4/series/%2E%2E/user"));
        assert!(climbs("/v4/./series/1"));

        assert!(!climbs("/3/tv/1396"));
        assert!(!climbs("/v4/series/81189/extended"));
        assert!(!climbs("/v4/search"));
        // A dot inside a segment is just a character.
        assert!(!climbs("/3/configuration/countries.json"));
    }

    #[test]
    fn an_answer_keeps_its_headers_but_not_the_hops_or_a_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(header::SET_COOKIE, HeaderValue::from_static("a=b"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("29"));
        headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
        headers.insert(
            crate::providers::radarr::LOOP_HEADER,
            HeaderValue::from_static("another-instance"),
        );

        let kept = kept(&headers);

        assert_eq!(kept.get(header::CONTENT_TYPE).unwrap(), "application/json");
        assert_eq!(kept.get("x-ratelimit-remaining").unwrap(), "29");
        assert!(kept.get(header::CONNECTION).is_none());
        assert!(kept.get(header::SET_COOKIE).is_none());
        assert!(kept.get(crate::providers::radarr::LOOP_HEADER).is_none());
        assert!(kept.get("access-control-allow-origin").is_none());
    }

    #[test]
    fn a_request_one_of_our_instances_sent_is_a_loop() {
        let mut headers = HeaderMap::new();
        assert!(!looped(&headers));
        headers.insert(
            crate::providers::radarr::LOOP_HEADER,
            HeaderValue::from_static("another-instance"),
        );
        assert!(looped(&headers));
    }

    #[test]
    fn only_the_headers_named_travel() {
        let mut headers = HeaderMap::new();
        headers.insert(header::USER_AGENT, HeaderValue::from_static("Yamtrack"));
        headers.insert(header::COOKIE, HeaderValue::from_static("ams_session=x"));
        headers.insert("x-api-key", HeaderValue::from_static("ams_k"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("10.0.0.2"));
        let asked = asked_with(&headers, &["user-agent", "accept"]);
        assert_eq!(asked.len(), 1);
        assert_eq!(asked.get(header::USER_AGENT).unwrap(), "Yamtrack");
    }

    #[test]
    fn a_bearer_token_is_read_whatever_its_case() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer(&headers), None);
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer  their.jwt.token "),
        );
        assert_eq!(bearer(&headers).as_deref(), Some("their.jwt.token"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bearer ams_k"),
        );
        assert_eq!(bearer(&headers).as_deref(), Some("ams_k"));
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert_eq!(bearer(&headers), None);
    }

    #[test]
    fn every_spelling_of_a_key_the_guard_takes_is_stripped() {
        for name in crate::auth::middleware::API_KEY_PARAMS {
            assert!(
                OUR_PARAMS.contains(name),
                "{name} authenticates here but would be relayed"
            );
        }
    }
}

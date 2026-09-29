//! Sonarr compatibility: `services.sonarr.tv`.
//!
//! Sonarr reaches it the way it reaches Skyhook, by the name it has compiled
//! in (`NzbDrone.Common/Cloud/SonarrCloudRequestBuilder.cs`), so it lands here
//! once that name is resolved to this server. One thing it asks there is this
//! server's business: the scene-mapping list, answered with this catalogue's
//! titles added (see [`crate::service::scene`]). Everything else — its
//! updates, the daily series, the server's notices, the clock and proxy
//! checks, the MyAnimeList import's sign-in — is relayed to the real service
//! as it was asked, and its answer handed back as it came.

use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use axum::{
    Extension, Json,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{auth::Identity, service::scene, state::AppState};

/// The names Sonarr calls this service by.
pub const HOSTS: &[&str] = &["services.sonarr.tv"];

/// The most a relayed request or answer may carry. What Sonarr sends and gets
/// there is small; an update's package is downloaded from elsewhere.
const REQUEST_LIMIT: usize = 1024 * 1024;
const ANSWER_LIMIT: u64 = 16 * 1024 * 1024;

/// What a relayed request keeps of the caller's headers: what describes the
/// request. Not a credential, a cookie, or a proxy's `X-Forwarded-For` — the
/// interface's door answers `/v1/scenemapping` too, behind whatever proxy.
const ASKED_WITH: &[&str] = &[
    "user-agent",
    "accept",
    "accept-language",
    "content-type",
    "cache-control",
    "if-none-match",
    "if-modified-since",
];

/// Query parameters that carry a key for this server, never the real one's.
const OUR_PARAMS: &[&str] = &["apikey", "api_key"];

/// Hop-by-hop headers (RFC 9110 §7.6.1), and those the HTTP stack sets itself:
/// the body is read decoded, so its encoding and length are the stack's to say.
const HOP_HEADERS: &[&str] = &[
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

/// Marks a request the clients' door relays for `services.sonarr.tv`: Sonarr's,
/// whatever its path, for the guards and the counts.
#[derive(Clone, Copy, Debug)]
pub struct Relayed;

/// The client the relay asks with: following no redirect — a redirect is
/// handed back to Sonarr as it came.
static RELAY: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
});

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(scene_mapping))
}

/// Whether a request was sent by an instance of this server: Sonarr never
/// sends the header, so one that carries it — whosever — has come round a
/// loop, through a resolver or a door several instances share.
fn looped(headers: &HeaderMap) -> bool {
    headers.contains_key(crate::providers::radarr::LOOP_HEADER)
}

fn loop_refused() -> Response {
    tracing::error!(
        "services.sonarr.tv was asked by an instance of this server: it resolves back here; \
         set AMS_SONARR_SERVICES_UPSTREAM to the real service's address"
    );
    (StatusCode::LOOP_DETECTED, "loop detected").into_response()
}

/// Whether a request was addressed to `services.sonarr.tv`.
pub fn is_services_host(headers: &HeaderMap, uri: &Uri) -> bool {
    // HTTP/2 names it in the address, HTTP/1.1 in `Host`, with its port. The
    // names are hostnames, so everything from a colon on is the port — and an
    // address in brackets is never one of them.
    let named = uri.host().map(str::to_string).or_else(|| {
        headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .and_then(|host| host.split(':').next())
            .map(str::to_string)
    });
    named.is_some_and(|name| {
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        HOSTS.contains(&name.as_str())
    })
}

/// Sonarr's scene-mapping list, with this catalogue's titles added.
///
/// With `sonarr.sceneMappings` off, the real list as it is. When the real list
/// cannot be had, a `502` and nothing else: Sonarr then keeps the list it
/// holds, where this catalogue's titles alone would replace it.
#[utoipa::path(
    get, path = "/v1/scenemapping", tag = crate::api::sonarr::TAG,
    responses(
        (status = 200, description = "services.sonarr.tv's list, and a mapping for each title of this catalogue Sonarr may safely match by"),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 502, description = "The real list could not be had"),
    ),
    security(),
)]
async fn scene_mapping(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Response {
    if looped(request.headers()) {
        return loop_refused();
    }
    if !state.flag("sonarr.sceneMappings", false) {
        return relay(&state, request).await;
    }
    let language = state.language(identity.client_id(), identity.peer_id());
    match scene::answer(&state, &language).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "Sonarr's scene-mapping list could not be had; Sonarr keeps the one it holds"
            );
            (
                StatusCode::BAD_GATEWAY,
                "services.sonarr.tv's scene-mapping list could not be had",
            )
                .into_response()
        }
    }
}

/// What the clients' door answers for a path none of its routes has: relayed,
/// when it was asked of `services.sonarr.tv`; not found otherwise.
pub async fn fallback(State(state): State<AppState>, request: Request) -> Response {
    if !is_services_host(request.headers(), request.uri()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    relay(&state, request).await
}

/// A request to `services.sonarr.tv`, asked of the real one, and its answer.
async fn relay(state: &AppState, request: Request) -> Response {
    if looped(request.headers()) {
        return loop_refused();
    }
    let (parts, body) = request.into_parts();
    let url = format!(
        "{}{}",
        state.config.sonarr_services.upstream,
        without_our_params(&parts.uri)
    );

    let Ok(body) = axum::body::to_bytes(body, REQUEST_LIMIT).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };

    let mut outbound = RELAY
        .request(parts.method.clone(), &url)
        .headers(asked_with(&parts.headers))
        // This name is one this server answers on: a resolver sending it back
        // here would otherwise have it call itself.
        .header(crate::providers::radarr::LOOP_HEADER, &state.instance)
        .timeout(Duration::from_secs(60));
    if !body.is_empty() {
        outbound = outbound.body(body);
    }

    let started = Instant::now();
    let answer = outbound.send().await;
    crate::metrics::upstream(
        "sonarr_services",
        started,
        answer.as_ref().ok().map(|r| r.status()),
    );
    let answer = match answer {
        Ok(answer) => answer,
        Err(e) => {
            tracing::warn!(
                error = %crate::providers::describe_request_error(&e),
                path = parts.uri.path(),
                "services.sonarr.tv could not be reached"
            );
            return (
                StatusCode::BAD_GATEWAY,
                "services.sonarr.tv could not be reached",
            )
                .into_response();
        }
    };

    let status = answer.status();
    if status == reqwest::StatusCode::LOOP_DETECTED {
        tracing::error!(
            "services.sonarr.tv resolves to this server; set AMS_SONARR_SERVICES_UPSTREAM to \
             the real service's address"
        );
    }
    let headers = kept(answer.headers());
    let body = match crate::providers::read_body(answer, ANSWER_LIMIT).await {
        Ok(body) => body,
        Err(e) => {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "services.sonarr.tv's answer could not be read"
            );
            return (
                StatusCode::BAD_GATEWAY,
                "services.sonarr.tv's answer could not be read",
            )
                .into_response();
        }
    };

    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

/// The path and query asked, less a key for this server.
fn without_our_params(uri: &Uri) -> String {
    let path = uri.path();
    let Some(query) = uri.query() else {
        return path.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            let name = pair.split('=').next().unwrap_or_default();
            !OUR_PARAMS
                .iter()
                .any(|ours| name.eq_ignore_ascii_case(ours))
        })
        .collect();
    if kept.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

/// The caller's headers the real service is asked with.
fn asked_with(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        if ASKED_WITH.contains(&name.as_str()) {
            out.append(name.clone(), value.clone());
        }
    }
    out
}

/// The headers an answer keeps: all but the hop-by-hop ones, and a cookie,
/// which would be set against this server.
fn kept(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        if HOP_HEADERS.contains(&name.as_str())
            || name == header::SET_COOKIE
            || name == header::COOKIE
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

#[cfg(test)]
mod tests {
    use super::*;

    fn host(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn a_request_names_services_sonarr_tv_by_its_host() {
        let path: Uri = "/v1/time".parse().unwrap();
        assert!(is_services_host(&host("services.sonarr.tv"), &path));
        assert!(is_services_host(&host("Services.Sonarr.TV:443"), &path));
        assert!(is_services_host(&host("services.sonarr.tv."), &path));
        assert!(!is_services_host(&host("skyhook.sonarr.tv"), &path));
        assert!(!is_services_host(&host("services.sonarr.tv.evil"), &path));
        assert!(!is_services_host(&HeaderMap::new(), &path));

        // HTTP/2 carries it in the address.
        let full: Uri = "https://services.sonarr.tv/v1/ping".parse().unwrap();
        assert!(is_services_host(&HeaderMap::new(), &full));
    }

    #[test]
    fn a_relayed_request_is_asked_with_what_describes_it_and_nothing_of_ours() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Sonarr/4.0.20"),
        );
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(header::HOST, HeaderValue::from_static("services.sonarr.tv"));
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("ams_session=secret"),
        );
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer ams_key"),
        );
        headers.insert("x-api-key", HeaderValue::from_static("ams_key"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("192.168.1.20"));

        let asked = asked_with(&headers);

        assert_eq!(asked.get(header::USER_AGENT).unwrap(), "Sonarr/4.0.20");
        assert_eq!(asked.get(header::ACCEPT).unwrap(), "application/json");
        assert_eq!(asked.len(), 2, "{asked:?}");
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

        let kept = kept(&headers);

        assert_eq!(kept.get(header::CONTENT_TYPE).unwrap(), "application/json");
        assert!(kept.get(header::CONNECTION).is_none());
        assert!(kept.get(header::SET_COOKIE).is_none());
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
}

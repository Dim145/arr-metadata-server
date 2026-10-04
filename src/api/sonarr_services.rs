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

use std::time::{Duration, Instant};

use axum::{
    Extension, Json,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{api::relay, auth::Identity, service::scene, state::AppState};

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

/// Marks a request the clients' door relays for `services.sonarr.tv`: Sonarr's,
/// whatever its path, for the guards and the counts.
#[derive(Clone, Copy, Debug)]
pub struct Relayed;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(scene_mapping))
}

fn loop_refused() -> Response {
    // A warning, not an error: anybody past the guard can send the header.
    tracing::warn!(
        "services.sonarr.tv was asked by an instance of this server: it resolves back here; \
         set AMS_SONARR_SERVICES_UPSTREAM to the real service's address"
    );
    (StatusCode::LOOP_DETECTED, "loop detected").into_response()
}

/// Whether a request was addressed to `services.sonarr.tv`.
pub fn is_services_host(headers: &HeaderMap, uri: &Uri) -> bool {
    relay::addressed_to(headers, uri, HOSTS)
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
    if relay::looped(request.headers()) {
        return loop_refused();
    }
    if !state.flag("sonarr.sceneMappings", false) {
        return relayed(&state, request).await;
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
    relayed(&state, request).await
}

/// A request to `services.sonarr.tv`, asked of the real one, and its answer.
async fn relayed(state: &AppState, request: Request) -> Response {
    if relay::looped(request.headers()) {
        return loop_refused();
    }
    let (parts, body) = request.into_parts();
    let url = format!(
        "{}{}",
        state.config.sonarr_services.upstream,
        relay::without_our_params(&parts.uri)
    );

    let Ok(body) = axum::body::to_bytes(body, REQUEST_LIMIT).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };

    let mut outbound = relay::CLIENT
        .request(parts.method.clone(), &url)
        .headers(relay::asked_with(&parts.headers, ASKED_WITH))
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
    let headers = relay::kept(answer.headers());
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header};

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
        assert!(!is_services_host(&host("graphql.anilist.co"), &path));
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

        let asked = relay::asked_with(&headers, ASKED_WITH);

        assert_eq!(asked.get(header::USER_AGENT).unwrap(), "Sonarr/4.0.20");
        assert_eq!(asked.get(header::ACCEPT).unwrap(), "application/json");
        assert_eq!(asked.len(), 2, "{asked:?}");
    }
}

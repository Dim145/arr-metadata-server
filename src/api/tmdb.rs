//! TMDB compatibility: `/3/*`.
//!
//! This is the descendant of `the earlier TMDB relay`, with one addition that is the whole
//! point of merging the projects: responses are **patched with local edits**
//! before they are returned.
//!
//! A request is relayed upstream with this server's own TMDB credentials
//! substituted for whatever the client sent. If the path addresses a title that
//! has manual overrides stored here, those fields are rewritten in the upstream
//! document on the way back. The client sees TMDB's full response, with the
//! operator's corrections applied.

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};

use crate::{
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// Hop-by-hop headers (RFC 9110 §7.6.1) plus the ones the HTTP stack recomputes.
const HOP_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "content-encoding",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Query parameters this server controls and the client does not.
const OVERRIDDEN_PARAMS: &[&str] = &["api_key", "include_adult"];

pub fn router() -> Router<AppState> {
    Router::new().route("/3/{*path}", any(proxy))
}

async fn proxy(State(state): State<AppState>, request: Request) -> AppResult<Response> {
    if !state.config.tmdb.passthrough {
        return Err(AppError::ProviderNotConfigured);
    }

    let Some(api_key) = state.config.tmdb.api_key.clone() else {
        return Err(AppError::ProviderNotConfigured);
    };

    let (parts, body) = request.into_parts();

    let target = upstream_url(&state, &parts.uri, &api_key);

    let body_bytes = axum::body::to_bytes(body, 2 * 1024 * 1024)
        .await
        .map_err(|e| AppError::BadRequest(format!("could not read the request body: {e}")))?;

    let mut upstream = state
        .http
        .request(parts.method.clone(), &target)
        .headers(forwarded_headers(&parts.headers));

    // A v4 token authenticates by header; the query parameter is ignored then.
    if api_key.starts_with("eyJ") {
        upstream = upstream.bearer_auth(&api_key);
    }

    if !body_bytes.is_empty() {
        upstream = upstream.body(body_bytes.to_vec());
    }

    let response = upstream
        .send()
        .await
        .map_err(|e| AppError::UpstreamUnavailable(e.into()))?;

    let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = response.headers().clone();

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::UpstreamUnavailable(e.into()))?;

    // Only JSON documents for a known title are worth inspecting; images,
    // configuration and errors pass through untouched.
    if let Some(target) = patch_target(parts.uri.path()) {
        if is_json(&headers) {
            if let Ok(mut document) = serde_json::from_slice::<Value>(&bytes) {
                if patch(&state, &mut document, target).await? {
                    return Ok((status, response_headers(&headers), axum::Json(document)).into_response());
                }
            }
        }
    }

    Ok((status, response_headers(&headers), Body::from(bytes)).into_response())
}

/// Rebuild the upstream URL, replacing the parameters this server controls.
fn upstream_url(state: &AppState, uri: &Uri, api_key: &str) -> String {
    let mut pairs: Vec<(String, String)> = uri
        .query()
        .map(|q| {
            q.split('&')
                .filter(|p| !p.is_empty())
                .map(|pair| match pair.split_once('=') {
                    Some((k, v)) => (k.to_string(), v.to_string()),
                    None => (pair.to_string(), String::new()),
                })
                // The client's own TMDB key — or the key it used to authenticate
                // *here* — must never reach upstream.
                .filter(|(k, _)| !OVERRIDDEN_PARAMS.contains(&k.as_str()))
                .collect()
        })
        .unwrap_or_default();

    if !api_key.starts_with("eyJ") {
        pairs.push(("api_key".to_string(), api_key.to_string()));
    }

    pairs.push((
        "include_adult".to_string(),
        state.config.tmdb.include_adult.to_string(),
    ));

    let query = pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    format!("{}{}?{}", state.config.tmdb.upstream, uri.path(), query)
}

fn forwarded_headers(headers: &HeaderMap) -> HeaderMap {
    headers
        .iter()
        .filter(|(name, _)| !HOP_HEADERS.contains(&name.as_str()))
        // The client's Authorization is its credential for *this* server, not
        // for TMDB; forwarding it would leak it upstream.
        .filter(|(name, _)| *name != header::AUTHORIZATION)
        .filter(|(name, _)| name.as_str() != "x-api-key")
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn response_headers(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();

    for (name, value) in headers {
        if HOP_HEADERS.contains(&name.as_str()) {
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

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
}

/// A title this server may hold edits for.
#[derive(Debug, PartialEq, Eq)]
struct PatchTarget {
    kind: MediaKind,
    tmdb_id: i64,
}

/// Recognise `/3/tv/{id}` and `/3/movie/{id}` exactly.
///
/// Sub-resources (`/credits`, `/season/1`, …) are left alone: their documents do
/// not carry the fields an override addresses.
fn patch_target(path: &str) -> Option<PatchTarget> {
    let mut segments = path.trim_matches('/').split('/');

    if segments.next()? != "3" {
        return None;
    }

    let kind = match segments.next()? {
        "tv" => MediaKind::Series,
        "movie" => MediaKind::Movie,
        _ => return None,
    };

    let tmdb_id = segments.next()?.parse().ok()?;

    // Anything further means a sub-resource.
    if segments.next().is_some() {
        return None;
    }

    Some(PatchTarget { kind, tmdb_id })
}

/// Apply this server's overrides to an upstream document.
///
/// Returns whether anything changed.
async fn patch(state: &AppState, document: &mut Value, target: PatchTarget) -> AppResult<bool> {
    let source = ExternalSource::tmdb_for(target.kind);

    let Some(id) = repo::item::find_id_by_external(&state.db, source, &target.tmdb_id.to_string()).await?
    else {
        return Ok(false);
    };

    // No overrides means the upstream document is already what we would serve.
    if repo::override_field::list(&state.db, &id).await?.is_empty() {
        return Ok(false);
    }

    let Some(item) = service::load(state, &id).await? else {
        return Ok(false);
    };

    let Value::Object(map) = document else {
        return Ok(false);
    };

    let mut changed = false;
    for (key, value) in tmdb_fields(&item, target.kind) {
        map.insert(key.to_string(), value);
        changed = true;
    }

    Ok(changed)
}

/// The canonical item's fields, named and shaped as TMDB names and shapes them.
fn tmdb_fields(item: &MediaItem, kind: MediaKind) -> Vec<(&'static str, Value)> {
    let mut out: Vec<(&'static str, Value)> = Vec::new();

    push_text(&mut out, "overview", &item.overview);
    push_text(&mut out, "homepage", &item.homepage);

    match kind {
        MediaKind::Series => {
            out.push(("name", Value::String(item.title.clone())));
            push_text(&mut out, "original_name", &item.original_title);
            push_text(&mut out, "first_air_date", &item.first_aired);
            push_text(&mut out, "last_air_date", &item.last_aired);

            if let Some(status) = item.status.as_deref() {
                out.push(("status", Value::String(tmdb_series_status(status).to_string())));
            }
            if let Some(runtime) = item.runtime {
                out.push(("episode_run_time", json!([runtime])));
            }
        }
        MediaKind::Movie => {
            out.push(("title", Value::String(item.title.clone())));
            push_text(&mut out, "original_title", &item.original_title);
            push_text(&mut out, "release_date", &item.in_cinemas);

            if let Some(runtime) = item.runtime {
                out.push(("runtime", json!(runtime)));
            }
        }
    }

    // TMDB's genres are objects; ours are names. The id is not something we can
    // invent, and no client keys on it here.
    if !item.genres.is_empty() {
        let genres: Vec<Value> = item
            .genres
            .iter()
            .map(|name| json!({ "id": Value::Null, "name": name }))
            .collect();
        out.push(("genres", Value::Array(genres)));
    }

    out
}

fn push_text(out: &mut Vec<(&'static str, Value)>, key: &'static str, value: &Option<String>) {
    if let Some(v) = value {
        out.push((key, Value::String(v.clone())));
    }
}

/// Canonical status back to TMDB's vocabulary.
fn tmdb_series_status(status: &str) -> &'static str {
    match status {
        "ended" => "Ended",
        "upcoming" => "Planned",
        _ => "Returning Series",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_paths_are_recognised() {
        assert_eq!(
            patch_target("/3/tv/1396"),
            Some(PatchTarget { kind: MediaKind::Series, tmdb_id: 1396 })
        );
        assert_eq!(
            patch_target("/3/movie/329865"),
            Some(PatchTarget { kind: MediaKind::Movie, tmdb_id: 329865 })
        );
        assert_eq!(
            patch_target("/3/tv/1396/"),
            Some(PatchTarget { kind: MediaKind::Series, tmdb_id: 1396 })
        );
    }

    #[test]
    fn sub_resources_and_other_paths_are_left_alone() {
        assert_eq!(patch_target("/3/tv/1396/season/1"), None);
        assert_eq!(patch_target("/3/movie/329865/credits"), None);
        assert_eq!(patch_target("/3/search/tv"), None);
        assert_eq!(patch_target("/3/configuration"), None);
        assert_eq!(patch_target("/3/tv/not-a-number"), None);
        assert_eq!(patch_target("/3/tv"), None);
        assert_eq!(patch_target("/other/tv/1"), None);
    }

    #[test]
    fn series_fields_use_tmdbs_names() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Mon titre".into();
        item.overview = Some("Mon résumé".into());
        item.first_aired = Some("2008-01-20".into());
        item.status = Some("ended".into());
        item.runtime = Some(47);

        let fields: std::collections::HashMap<_, _> =
            tmdb_fields(&item, MediaKind::Series).into_iter().collect();

        assert_eq!(fields["name"], json!("Mon titre"));
        assert_eq!(fields["overview"], json!("Mon résumé"));
        assert_eq!(fields["first_air_date"], json!("2008-01-20"));
        assert_eq!(fields["status"], json!("Ended"));
        assert_eq!(fields["episode_run_time"], json!([47]));
        assert!(!fields.contains_key("title"));
    }

    #[test]
    fn movie_fields_use_tmdbs_names() {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "Premier Contact".into();
        item.in_cinemas = Some("2016-11-11".into());
        item.runtime = Some(116);

        let fields: std::collections::HashMap<_, _> =
            tmdb_fields(&item, MediaKind::Movie).into_iter().collect();

        assert_eq!(fields["title"], json!("Premier Contact"));
        assert_eq!(fields["release_date"], json!("2016-11-11"));
        assert_eq!(fields["runtime"], json!(116));
        assert!(!fields.contains_key("name"));
    }

    #[test]
    fn genres_are_reshaped_into_tmdbs_objects() {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.genres = vec!["Drame".into()];

        let fields: std::collections::HashMap<_, _> =
            tmdb_fields(&item, MediaKind::Movie).into_iter().collect();

        assert_eq!(fields["genres"], json!([{ "id": null, "name": "Drame" }]));
    }

    #[test]
    fn statuses_round_trip_back_to_tmdbs_vocabulary() {
        assert_eq!(tmdb_series_status("ended"), "Ended");
        assert_eq!(tmdb_series_status("continuing"), "Returning Series");
        assert_eq!(tmdb_series_status("upcoming"), "Planned");
    }
}

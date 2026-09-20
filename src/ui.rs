//! The web UI, compiled into the binary.
//!
//! One binary with no companion directory is the whole point: the container
//! image is a single static file. Assets are served with immutable caching
//! because Vite fingerprints their filenames; `index.html` never is.

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::any,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;

/// Paths that belong to the API. A request under one of these must 404 rather
/// than fall through to the UI's index page, or a mistyped endpoint would
/// answer `200 text/html` and confuse every client.
const API_PREFIXES: &[&str] = &["/api/", "/v1/", "/3/", "/health", "/ready"];

pub fn router() -> Router<crate::state::AppState> {
    Router::new().fallback(any(serve))
}

async fn serve(request: Request) -> Response {
    let path = request.uri().path();

    if API_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) {
        return not_found();
    }

    let trimmed = path.trim_start_matches('/');

    if let Some(response) = asset(trimmed) {
        return response;
    }

    // Anything else is a client-side route: hand back the app and let the
    // router in the browser resolve it.
    asset("index.html").unwrap_or_else(not_found)
}

fn asset(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;

    let mime = mime_guess::from_path(path).first_or_octet_stream();

    let cache = if path == "index.html" {
        // The entry point names the fingerprinted bundles; caching it would
        // pin a browser to an old build.
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };

    let mut response = Response::new(Body::from(file.data.into_owned()));

    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref()).ok()?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));

    Some(response)
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// Whether a URI addresses the UI rather than the API.
#[allow(dead_code)]
pub fn is_ui_path(uri: &Uri) -> bool {
    !API_PREFIXES
        .iter()
        .any(|prefix| uri.path().starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_paths_are_not_ui_paths() {
        for path in ["/api/v1/items", "/v1/tvdb/search/en", "/3/tv/1", "/health", "/ready"] {
            assert!(!is_ui_path(&path.parse::<Uri>().unwrap()), "{path}");
        }
    }

    #[test]
    fn everything_else_is_a_ui_path() {
        for path in ["/", "/catalogue", "/catalogue/abc", "/assets/index-abc.js"] {
            assert!(is_ui_path(&path.parse::<Uri>().unwrap()), "{path}");
        }
    }

    #[test]
    fn a_path_that_merely_starts_like_an_api_path_is_still_ui() {
        // "/v1abc" is not under "/v1/", so it belongs to the client router.
        assert!(is_ui_path(&"/v1abc".parse::<Uri>().unwrap()));
        assert!(is_ui_path(&"/apifoo".parse::<Uri>().unwrap()));
    }
}

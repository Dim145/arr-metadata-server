//! Conditional GET for the API: a validator on every whole answer, so a
//! client holding one is told "unchanged" instead of being sent it again.
//!
//! The tag is a digest of the bytes the handler produced, before any
//! compression, and is weak for that reason: two encodings of one answer
//! share it. A handler that sets its own tag — the TMDB relay passes TMDB's
//! through — is left alone, as is anything that is not a whole, sized body.

use axum::{
    body::{Body, HttpBody, to_bytes},
    extract::Request,
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};

/// Bodies larger than this are not fingerprinted: hashing them would cost
/// more than the round trip it saves, and an answer that large is rare.
const MAX_FINGERPRINTED: u64 = 8 * 1024 * 1024;

pub async fn conditional(request: Request, next: Next) -> Response {
    let reading = matches!(*request.method(), Method::GET | Method::HEAD);
    let held: Vec<String> = request
        .headers()
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(tags)
        .collect();

    let response = next.run(request).await;

    if !reading
        || response.status() != StatusCode::OK
        || response.headers().contains_key(header::ETAG)
    {
        return response;
    }
    let Some(size) = response.body().size_hint().exact() else {
        return response;
    };
    if size > MAX_FINGERPRINTED {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, MAX_FINGERPRINTED as usize).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(error = %e, "could not read an answer to fingerprint it");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let tag = fingerprint(&bytes);
    parts.headers.insert(
        header::ETAG,
        HeaderValue::from_str(&tag).unwrap_or_else(|_| HeaderValue::from_static("W/\"0\"")),
    );
    // Kept and asked about, unless the handler said how long it may be kept.
    if !parts.headers.contains_key(header::CACHE_CONTROL) {
        parts.headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-cache"),
        );
    }

    if held.iter().any(|h| h == "*" || matches(h, &tag)) {
        parts.status = StatusCode::NOT_MODIFIED;
        // No body: nothing to describe, and the length would be a lie on
        // one transport or the other. The encoding the answer varies by is
        // named, as the full answer names it once it is compressed.
        parts.headers.remove(header::CONTENT_LENGTH);
        parts.headers.remove(header::CONTENT_TYPE);
        parts
            .headers
            .append(header::VARY, HeaderValue::from_static("accept-encoding"));
        return Response::from_parts(parts, Body::empty());
    }
    Response::from_parts(parts, Body::from(bytes))
}

/// The tags an `If-None-Match` holds: separated by commas, but not the
/// commas inside a quoted tag.
fn tags(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' if !quoted => {
                let tag = current.trim();
                if !tag.is_empty() {
                    out.push(tag.to_string());
                }
                current.clear();
            }
            c => current.push(c),
        }
    }
    let tag = current.trim();
    if !tag.is_empty() {
        out.push(tag.to_string());
    }
    out
}

/// A weak validator over the bytes: sixteen bytes of their digest.
fn fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("W/\"{hex}\"")
}

/// The weak comparison: the tags match once the weakness marker is set aside.
fn matches(held: &str, tag: &str) -> bool {
    held.trim_start_matches("W/") == tag.trim_start_matches("W/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use tower::ServiceExt as _;

    fn app() -> Router {
        Router::new()
            .route("/answer", get(|| async { "forty-two" }))
            .route(
                "/tagged",
                get(|| async { ([(header::ETAG, "\"upstream\"")], "as it came") }),
            )
            .route(
                "/kept",
                get(|| async {
                    (
                        [(header::CACHE_CONTROL, "private, max-age=900")],
                        "for a while",
                    )
                }),
            )
            .route("/missing", get(|| async { (StatusCode::NOT_FOUND, "no") }))
            .layer(axum::middleware::from_fn(conditional))
    }

    async fn ask(method: Method, if_none_match: Option<&str>, path: &str) -> Response {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(tag) = if_none_match {
            request = request.header(header::IF_NONE_MATCH, tag);
        }
        app()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_answer_carries_a_tag_and_is_not_repeated_to_one_who_holds_it() {
        let first = ask(Method::GET, None, "/answer").await;
        assert_eq!(first.status(), StatusCode::OK);
        let tag = first.headers()[header::ETAG].to_str().unwrap().to_string();
        assert!(tag.starts_with("W/\""), "{tag}");
        assert_eq!(first.headers()[header::CACHE_CONTROL], "private, no-cache");
        let body = to_bytes(first.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"forty-two");

        let again = ask(Method::GET, Some(&tag), "/answer").await;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(again.headers()[header::ETAG].to_str().unwrap(), tag);
        assert!(again.headers().get(header::CONTENT_TYPE).is_none());
        assert!(
            again
                .headers()
                .get_all(header::VARY)
                .iter()
                .any(|v| v == "accept-encoding")
        );
        assert!(to_bytes(again.into_body(), 1024).await.unwrap().is_empty());

        // A strong form of the same tag, or a list holding it, is the same.
        let strong = tag.trim_start_matches("W/").to_string();
        assert_eq!(
            ask(Method::GET, Some(&strong), "/answer").await.status(),
            StatusCode::NOT_MODIFIED
        );
        let list = format!("\"other\", {tag}");
        assert_eq!(
            ask(Method::GET, Some(&list), "/answer").await.status(),
            StatusCode::NOT_MODIFIED
        );
        assert_eq!(
            ask(Method::GET, Some("*"), "/answer").await.status(),
            StatusCode::NOT_MODIFIED
        );
        assert_eq!(
            ask(Method::GET, Some("\"stale\""), "/answer")
                .await
                .status(),
            StatusCode::OK
        );
        // A comma inside a quoted tag is part of it, and a star inside one is
        // not the star.
        assert_eq!(
            ask(Method::GET, Some("\"a,*,b\""), "/answer")
                .await
                .status(),
            StatusCode::OK
        );
        // The head of an answer is tagged as the answer is.
        let head = ask(Method::HEAD, None, "/answer").await;
        assert_eq!(head.headers()[header::ETAG].to_str().unwrap(), tag);
    }

    #[test]
    fn tags_are_split_on_the_commas_between_them() {
        assert_eq!(tags("\"a\", W/\"b\""), vec!["\"a\"", "W/\"b\""]);
        assert_eq!(tags("\"a,b\""), vec!["\"a,b\""]);
        assert_eq!(tags(" * "), vec!["*"]);
        assert!(tags(",,").is_empty());
    }

    #[tokio::test]
    async fn a_handlers_own_tag_and_cache_directive_are_left_alone() {
        let tagged = ask(Method::GET, None, "/tagged").await;
        assert_eq!(tagged.headers()[header::ETAG], "\"upstream\"");
        assert!(tagged.headers().get(header::CACHE_CONTROL).is_none());

        let kept = ask(Method::GET, None, "/kept").await;
        assert_eq!(
            kept.headers()[header::CACHE_CONTROL],
            "private, max-age=900"
        );
        assert!(kept.headers().contains_key(header::ETAG));

        let missing = ask(Method::GET, Some("*"), "/missing").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert!(missing.headers().get(header::ETAG).is_none());
    }

    #[tokio::test]
    async fn a_write_is_never_answered_with_a_tag() {
        let app = Router::new()
            .route("/answer", axum::routing::post(|| async { "done" }))
            .layer(axum::middleware::from_fn(conditional));
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/answer")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.headers().get(header::ETAG).is_none());
    }
}

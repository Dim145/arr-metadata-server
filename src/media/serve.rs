//! Serving what is kept: `GET /media/{key}`.
//!
//! Outside every guard. A key is sixty-four hex digits of the bytes' own
//! hash, which nobody guesses and nobody lists, so a private catalogue gives
//! nothing away by it — and Sonarr, Kodi and a browser with no session all
//! fetch a poster the same way. Immutable forever, since the key names the
//! bytes. Ranges are honoured, one at a time: Safari plays no sound without.

use std::{ops::Range, time::Duration};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use object_store::GetRange;

use crate::state::AppState;

use super::file;

/// How long a bucket's own address is good for, when a reader is sent to
/// it. Longer than any download; shorter than a day's caching would be.
const PRESIGNED_FOR: Duration = Duration::from_secs(3600);

pub async fn get(
    State(state): State<AppState>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !file::valid_key(&key) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(store) = state.media.store() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // A thumbnail is looked up by its picture's key.
    let is_thumb = file::is_thumb(&key);
    let stem = if is_thumb {
        let sha = key.split('-').next().unwrap_or("");
        match state.media.keyed_by_sha(sha) {
            Some(k) => k,
            None => return StatusCode::NOT_FOUND.into_response(),
        }
    } else {
        key.clone()
    };
    let Some(known) = state.media.keyed(&stem) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // A thumbnail is asked for as `-t.jpg` whatever was made of the
    // picture: the key it is filed under, and its type, are its own.
    let (key, content_type): (String, &str) = if is_thumb {
        match file::thumb_key(&stem, known.thumb).zip(known.thumb.content_type()) {
            Some(own) => own,
            // No thumbnail was made of it: the picture itself is small enough.
            None => {
                return redirect(
                    &state.media.url_for(&stem),
                    "public, max-age=31536000, immutable",
                );
            }
        }
    } else {
        (key, &known.content_type)
    };

    let etag = format!("\"{key}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|tag| tag.trim() == etag || tag.trim() == "*")
        })
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    // Sent to the bucket itself, when that is how it is set.
    if state.text("media.serve").as_deref() == Some("redirect") {
        match store.presign(&key, PRESIGNED_FOR).await {
            Ok(Some(url)) => return redirect(url.as_str(), "private, max-age=3000"),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    key,
                    error = format_args!("{e:#}"),
                    "could not sign the bucket's address"
                );
            }
        }
    }

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(parse_range);
    let get_range = match range {
        None => None,
        Some(Some(range)) => Some(range),
        Some(None) => return StatusCode::RANGE_NOT_SATISFIABLE.into_response(),
    };

    // A range is checked against the size first: a player probes past the
    // end, and the answer to that is 416 with the size, not a store's
    // complaint.
    if let Some(asked) = &get_range {
        let size = match store.size(&key).await {
            Ok(Some(size)) => size,
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(e) => {
                tracing::warn!(
                    key,
                    error = format_args!("{e:#}"),
                    "could not size a medium"
                );
                return StatusCode::BAD_GATEWAY.into_response();
            }
        };
        if asked.as_range(size).is_err() {
            let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            if let Ok(value) = HeaderValue::from_str(&format!("bytes */{size}")) {
                response.headers_mut().insert(header::CONTENT_RANGE, value);
            }
            return response;
        }
    }

    let found = match store.get(&key, get_range.clone()).await {
        Ok(Some(found)) => found,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::warn!(
                key,
                error = format_args!("{e:#}"),
                "could not read a medium"
            );
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    let partial = get_range.is_some();
    let mut response = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, found.range.end - found.range.start)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, &etag)
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"{key}\""),
        )
        // The site's own policy is set over every answer by a layer above,
        // and forbids as much: nothing kept here is a document that runs.
        .header("cross-origin-resource-policy", "cross-origin");
    if partial {
        response = response.header(
            header::CONTENT_RANGE,
            format!(
                "bytes {}-{}/{}",
                found.range.start,
                found.range.end.saturating_sub(1),
                found.size
            ),
        );
    }
    response
        .body(Body::from_stream(found.stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn redirect(to: &str, cache: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(location) = HeaderValue::from_str(to) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    if let Ok(cache) = HeaderValue::from_str(cache) {
        response.headers_mut().insert(header::CACHE_CONTROL, cache);
    }
    response
}

/// One byte range, as a browser asks for a sound: `bytes=0-1023`,
/// `bytes=1024-` or `bytes=-500`. Several at once, or anything else, is
/// not served.
fn parse_range(header: &str) -> Option<GetRange> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    match (start.trim(), end.trim()) {
        ("", suffix) => suffix
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .map(GetRange::Suffix),
        (start, "") => start.parse::<u64>().ok().map(GetRange::Offset),
        (start, end) => {
            let (start, end) = (start.parse::<u64>().ok()?, end.parse::<u64>().ok()?);
            (start <= end).then(|| {
                GetRange::Bounded(Range {
                    start,
                    end: end + 1,
                })
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browsers_range_is_read_and_a_strange_one_refused() {
        assert!(
            matches!(parse_range("bytes=0-1023"), Some(GetRange::Bounded(r)) if r == (0..1024))
        );
        assert!(matches!(
            parse_range("bytes=1024-"),
            Some(GetRange::Offset(1024))
        ));
        assert!(matches!(
            parse_range("bytes=-500"),
            Some(GetRange::Suffix(500))
        ));
        for bad in [
            "bytes=5-2",
            "bytes=0-1,3-4",
            "items=0-1",
            "bytes=-0",
            "bytes=a-b",
            "",
        ] {
            assert!(parse_range(bad).is_none(), "{bad}");
        }
    }
}

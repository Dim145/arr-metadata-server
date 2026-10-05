//! Serving what is kept: `GET /media/{key}`.
//!
//! Outside every guard. A key is sixty-four hex digits of the bytes' own
//! hash, which nobody guesses and nobody lists, so a private catalogue gives
//! nothing away by it — and Sonarr, Kodi and a browser with no session all
//! fetch a poster the same way. Immutable forever, since the key names the
//! bytes. Ranges are honoured, one at a time: Safari plays no sound without.
//! The type a file is served as is its key's extension's, whatever anybody
//! said of it.

use std::{ops::Range, time::Duration};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use object_store::GetRange;

use crate::state::AppState;

use super::file;

/// How long a bucket's own address is good for, when a reader is sent to
/// it. Longer than any download; shorter than a day's caching would be.
const PRESIGNED_FOR: Duration = Duration::from_secs(3600);

/// How long a copy may be kept by whoever reads it: for ever, as the key
/// names the bytes.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

pub async fn get(
    State(state): State<AppState>,
    method: Method,
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
    // picture: the key it is filed under is its own.
    let key = if is_thumb {
        match file::thumb_key(&stem, known.thumb) {
            Some(own) => own,
            // No thumbnail was made of it: the picture itself is small enough.
            None => return redirect(&state.media.url_for(&stem), IMMUTABLE),
        }
    } else {
        key
    };
    let Some(content_type) = file::content_type_of(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let etag = format!("\"{key}\"");
    if none_match(&headers, &etag) {
        return not_modified(&etag);
    }

    // Sent to the bucket itself, when that is how it is set.
    if state.text("media.serve").as_deref() == Some("redirect") {
        match store.presign(&key, method.clone(), PRESIGNED_FOR).await {
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

    // What a GET would say, without a byte of the file read: ranges are a
    // GET's alone.
    if method == Method::HEAD {
        return match store.size(&key).await {
            Ok(Some(size)) => answer(content_type, &key, &etag, size, None)
                .body(Body::empty())
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(e) => {
                tracing::warn!(
                    key,
                    error = format_args!("{e:#}"),
                    "could not size a medium"
                );
                StatusCode::BAD_GATEWAY.into_response()
            }
        };
    }

    // A range this server understands, asked of this very file: anything
    // else is answered with the whole file, as RFC 9110 has it.
    let asked = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_range)
        .filter(|_| range_applies(&headers, &etag));

    // A range is checked against the size first: a player probes past the
    // end, and the answer to that is 416 with the size, not a store's
    // complaint.
    let get_range = match asked {
        None => None,
        Some(asked) => {
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
            match asked.within(size) {
                Some(range) => Some(GetRange::Bounded(range)),
                None => return unsatisfiable(size),
            }
        }
    };

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

    let partial = get_range
        .is_some()
        .then_some((found.range.clone(), found.size));
    answer(
        content_type,
        &key,
        &etag,
        found.range.end - found.range.start,
        partial,
    )
    .body(Body::from_stream(found.stream))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// The headers of a file's answer, whole or in part: `length` is what the
/// body carries, and `partial` the range it is of the whole.
fn answer(
    content_type: &str,
    key: &str,
    etag: &str,
    length: u64,
    partial: Option<(Range<u64>, u64)>,
) -> axum::http::response::Builder {
    let mut response = Response::builder()
        .status(if partial.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, length)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, IMMUTABLE)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"{key}\""),
        )
        // The site's own policy is set over every answer by a layer above,
        // and forbids as much: nothing kept here is a document that runs.
        .header("cross-origin-resource-policy", "cross-origin");
    if let Some((range, size)) = partial {
        response = response.header(
            header::CONTENT_RANGE,
            format!(
                "bytes {}-{}/{size}",
                range.start,
                range.end.saturating_sub(1)
            ),
        );
    }
    response
}

/// `304`, with what a `200` would have said of caching it (RFC 9110,
/// 15.4.5).
fn not_modified(etag: &str) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    if let Ok(etag) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
    response
}

/// `416`, with the size, so the reader knows what it may ask for.
fn unsatisfiable(size: u64) -> Response {
    let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    if let Ok(value) = HeaderValue::from_str(&format!("bytes */{size}")) {
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    response
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

/// An entity tag without the mark of a weak one: `If-None-Match` compares
/// weakly (RFC 9110, 8.8.3.2).
fn opaque(tag: &str) -> &str {
    let tag = tag.trim();
    tag.strip_prefix("W/").unwrap_or(tag)
}

/// Whether the reader has this very file already.
fn none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|tag| tag.trim() == "*" || opaque(tag) == etag)
}

/// Whether a range is to be served: always, unless `If-Range` names another
/// file than this one — or names a date, and these answers carry none to
/// compare it with. Then the whole file is (RFC 9110, 13.1.5).
fn range_applies(headers: &HeaderMap, etag: &str) -> bool {
    match headers.get(header::IF_RANGE).map(|v| v.to_str()) {
        None => true,
        // Compared strongly: a weak tag never matches.
        Some(Ok(validator)) => validator.trim() == etag,
        Some(Err(_)) => false,
    }
}

/// One byte range, as it was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    /// `bytes=first-last`, or `bytes=first-` to the end.
    From { first: u64, last: Option<u64> },
    /// `bytes=-n`: the last `n`.
    Suffix(u64),
}

impl Asked {
    /// The bytes of a file of `size` this range covers — a last position
    /// past the end is the end — or `None` when it covers none of them.
    fn within(self, size: u64) -> Option<Range<u64>> {
        match self {
            Self::From { first, last } => {
                if first >= size {
                    return None;
                }
                let end = last.map_or(size, |last| last.saturating_add(1).min(size));
                Some(first..end)
            }
            Self::Suffix(0) => None,
            Self::Suffix(_) if size == 0 => None,
            Self::Suffix(n) => Some(size.saturating_sub(n)..size),
        }
    }
}

/// One byte range, as a browser asks for a sound: `bytes=0-1023`,
/// `bytes=1024-` or `bytes=-500`. `None` for what is to be ignored, and
/// answered with the whole file: a unit other than bytes, a range that does
/// not parse, or several at once (RFC 9110, 14.2) — this server sends no
/// multipart answer.
fn parse_range(header: &str) -> Option<Asked> {
    let (unit, set) = header.split_once('=')?;
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return None;
    }
    let mut specs = set.split(',').map(str::trim).filter(|s| !s.is_empty());
    let spec = specs.next()?;
    if specs.next().is_some() {
        return None;
    }
    let (first, last) = spec.split_once('-')?;
    match (first, last) {
        ("", suffix) => position(suffix).map(Asked::Suffix),
        (first, "") => position(first).map(|first| Asked::From { first, last: None }),
        (first, last) => {
            let (first, last) = (position(first)?, position(last)?);
            (first <= last).then_some(Asked::From {
                first,
                last: Some(last),
            })
        }
    }
}

/// A byte position: digits and nothing else, and a number too large to
/// hold is past the end of anything this server keeps.
fn position(text: &str) -> Option<u64> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browsers_range_is_read() {
        assert_eq!(
            parse_range("bytes=0-1023"),
            Some(Asked::From {
                first: 0,
                last: Some(1023)
            })
        );
        assert_eq!(
            parse_range("bytes=1024-"),
            Some(Asked::From {
                first: 1024,
                last: None
            })
        );
        assert_eq!(parse_range("bytes=-500"), Some(Asked::Suffix(500)));
        // The unit is compared without regard to case, and a list may
        // carry empty elements.
        assert_eq!(
            parse_range("Bytes=5-9"),
            Some(Asked::From {
                first: 5,
                last: Some(9)
            })
        );
        assert_eq!(parse_range("bytes= 0-1 ,"), parse_range("bytes=0-1"));
        // Unsatisfiable, but understood: a 416, not the whole file.
        assert_eq!(parse_range("bytes=-0"), Some(Asked::Suffix(0)));
        // A last position too large to hold is the end.
        assert_eq!(
            parse_range("bytes=0-18446744073709551615"),
            Some(Asked::From {
                first: 0,
                last: Some(u64::MAX)
            })
        );
        assert_eq!(
            parse_range("bytes=0-99999999999999999999999"),
            Some(Asked::From {
                first: 0,
                last: Some(u64::MAX)
            })
        );
    }

    #[test]
    fn a_range_not_understood_is_ignored() {
        for ignored in [
            "items=0-1",
            "bytes=5-2",
            "bytes=0-1,3-4",
            "bytes=a-b",
            "bytes=+1-2",
            "bytes=1 -2",
            "bytes=-",
            "bytes=",
            "bytes",
            "",
        ] {
            assert_eq!(parse_range(ignored), None, "{ignored}");
        }
    }

    #[test]
    fn a_range_is_fitted_to_the_file() {
        let size = 1000;
        let from = |first, last| Asked::From { first, last };
        assert_eq!(from(0, Some(99)).within(size), Some(0..100));
        assert_eq!(from(900, Some(5000)).within(size), Some(900..1000));
        assert_eq!(from(0, Some(u64::MAX)).within(size), Some(0..1000));
        assert_eq!(from(999, None).within(size), Some(999..1000));
        assert_eq!(Asked::Suffix(100).within(size), Some(900..1000));
        assert_eq!(Asked::Suffix(5000).within(size), Some(0..1000));
        // Nothing of it: 416.
        assert_eq!(from(1000, None).within(size), None);
        assert_eq!(from(u64::MAX, Some(u64::MAX)).within(size), None);
        assert_eq!(Asked::Suffix(0).within(size), None);
        assert_eq!(Asked::Suffix(10).within(0), None);
    }

    #[test]
    fn a_copy_held_is_recognised_strongly_or_weakly() {
        let etag = "\"abc.jpg\"";
        let with = |name, value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(name, HeaderValue::from_static(value));
            headers
        };
        assert!(none_match(
            &with(header::IF_NONE_MATCH, "\"abc.jpg\""),
            etag
        ));
        assert!(none_match(
            &with(header::IF_NONE_MATCH, "W/\"abc.jpg\""),
            etag
        ));
        assert!(none_match(
            &with(header::IF_NONE_MATCH, "\"x\", W/\"abc.jpg\""),
            etag
        ));
        assert!(none_match(&with(header::IF_NONE_MATCH, "*"), etag));
        assert!(!none_match(&with(header::IF_NONE_MATCH, "\"x\""), etag));
        assert!(!none_match(&HeaderMap::new(), etag));

        let response = not_modified(etag);
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::ETAG], etag);
        assert_eq!(response.headers()[header::CACHE_CONTROL], IMMUTABLE);

        assert!(range_applies(&HeaderMap::new(), etag));
        assert!(range_applies(&with(header::IF_RANGE, "\"abc.jpg\""), etag));
        assert!(!range_applies(
            &with(header::IF_RANGE, "W/\"abc.jpg\""),
            etag
        ));
        assert!(!range_applies(&with(header::IF_RANGE, "\"x\""), etag));
        assert!(!range_applies(
            &with(header::IF_RANGE, "Sat, 04 Oct 2026 10:00:00 GMT"),
            etag
        ));

        let response = unsatisfiable(1000);
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes */1000");
    }
}

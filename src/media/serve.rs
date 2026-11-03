//! Serving what is kept: `GET /media/{key}`.
//!
//! Outside every guard. A key is sixty-four hex digits of the bytes' own
//! hash, which nobody guesses and nobody lists, so a private catalogue gives
//! nothing away by it — and Sonarr, Kodi and a browser with no session all
//! fetch a poster the same way. Immutable forever, since the key names the
//! bytes. Ranges are honoured, one at a time: Safari plays no sound without.
//! The type a file is served as is its key's extension's, whatever anybody
//! said of it.
//!
//! A copy of a picture — `<hash>-480.webp`, `<hash>-square.jpg` — is made
//! the first time it is asked for and kept: see [`super::variants`]. An
//! AVIF is the night's to make, never a request's. A WebP is made only
//! while `media.webp` is on: off, one kept already is served, and a page
//! that asks for one that is not is sent to the picture itself — for this
//! time only, as they may be turned on again.
//!
//! Making one is work, so it is held to what the rest of the server's is:
//! the caller who asks for one that is not kept is counted against the
//! requests an address may make ([`crate::auth::ratelimit::spend`]) — a
//! copy served is not counted, nor is a picture —, at most
//! [`super::COPIES_AT_ONCE`] are being made at once, and a copy waits for its
//! turn among the pictures decoded before its picture is read from the
//! store, not after.

use std::{
    net::{IpAddr, Ipv4Addr},
    ops::Range,
    sync::{Arc, LazyLock},
    time::Duration,
};

use anyhow::Context as _;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use object_store::GetRange;

use crate::{api::extract::ClientIp, db::repo, error::AppError, state::AppState};

use super::{
    file::{self, Made},
    store::Store,
    variants::{self, Format},
};

/// How long a bucket's own address is good for, when a reader is sent to
/// it. Longer than any download; shorter than a day's caching would be.
const PRESIGNED_FOR: Duration = Duration::from_secs(3600);

/// How long a copy may be kept by whoever reads it: for ever, as the key
/// names the bytes.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// What a request that is counted is called in the log: the route, never
/// the key, which is the one thing a reader needs to fetch what it names.
const LOGGED_AS: &str = "/media/{key}";

/// `GET /media/{key}`, for a caller whose address is known: what the route
/// answers. A copy this asks for that is not kept is made on its account,
/// counted against what its address may ask of the server.
///
/// What the route in `web.rs` is mounted on: [`get`] cannot be, having no
/// caller to count — and is called as it is from inside this server.
pub async fn get_for_caller(
    State(state): State<AppState>,
    method: Method,
    Path(key): Path<String>,
    ip: ClientIp,
    headers: HeaderMap,
) -> Response {
    // As the rate limiter counts it: an address by its network, and a
    // caller whose address is unknown with every other unknown one.
    let caller =
        ip.0.map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), crate::auth::ip::bucket);
    respond(&state, method, key, Some(caller), headers).await
}

/// The same, for what asks of this server from inside it — the TMDB
/// pictures' gate, for the picture a key names: nobody to count.
pub async fn get(
    State(state): State<AppState>,
    method: Method,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Response {
    respond(&state, method, key, None, headers).await
}

async fn respond(
    state: &AppState,
    method: Method,
    key: String,
    caller: Option<IpAddr>,
    headers: HeaderMap,
) -> Response {
    let Some(parsed) = file::parse_key(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(store) = state.media.store() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if matches!(parsed.made, Made::Width(_) | Made::Square) {
        return copy(state, store, &method, parsed, caller, &headers).await;
    }

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

/// Copies made in the last minute, by their key: a page's first visit asks
/// for a dozen at once, and a second reader asking for one being made waits
/// for it rather than making it again beside it.
static MADE: LazyLock<moka::future::Cache<String, Bytes>> = LazyLock::new(|| {
    moka::future::Cache::builder()
        .weigher(|_, bytes: &Bytes| u32::try_from(bytes.len()).unwrap_or(u32::MAX))
        .max_capacity(64 * 1024 * 1024)
        .time_to_live(Duration::from_secs(60))
        .build()
});

/// The copies made in the last minute that are kept: how many, and their
/// bytes.
pub fn made_kept() -> (u64, u64) {
    (MADE.entry_count(), MADE.weighted_size())
}

/// A copy of a picture kept: from the store, or made now and kept.
async fn copy(
    state: &AppState,
    store: &Store,
    method: &Method,
    asked: file::Key<'_>,
    caller: Option<IpAddr>,
    headers: &HeaderMap,
) -> Response {
    let not_found = || StatusCode::NOT_FOUND.into_response();
    // The picture it is a copy of, which must be a picture this server
    // keeps: a sound has no copies, and nothing else is made.
    let Some(original) = state.media.keyed_by_sha(asked.stem) else {
        return not_found();
    };
    let Some(known) = state.media.keyed(&original) else {
        return not_found();
    };
    if !file::content_type_of(&original).is_some_and(|kind| kind.starts_with("image/")) {
        return not_found();
    }
    let Some(format) = Format::of_ext(asked.ext) else {
        return not_found();
    };
    // Never wider than the picture: a width past its own is its own.
    let made = match asked.made {
        Made::Width(width) => match known.look.width {
            Some(own) => match variants::width_made(width, own) {
                Some(made) => Made::Width(made),
                None => return not_found(),
            },
            None if variants::WIDTHS.contains(&width) => Made::Width(width),
            None => return not_found(),
        },
        Made::Square => {
            if known
                .look
                .width
                .zip(known.look.height)
                .is_some_and(|(width, height)| height > width)
            {
                return not_found();
            }
            Made::Square
        }
        Made::Own | Made::Thumb => return not_found(),
    };
    let key = file::made_key(asked.stem, made, format.ext());

    if format == Format::Avif {
        return avif(
            state, store, method, &original, asked.stem, made, &key, caller, headers,
        )
        .await;
    }

    let etag = format!("\"{key}\"");
    if none_match(headers, &etag) {
        return not_modified(&etag);
    }
    // Sent to the bucket itself, when that is how it is set and the copy
    // is there already; one that is not is made — a WebP only while they
    // are on — and answered from here.
    if state.text("media.serve").as_deref() == Some("redirect")
        && store.exists(&key).await.unwrap_or(false)
        && let Ok(Some(url)) = store.presign(&key, method.clone(), PRESIGNED_FOR).await
    {
        return redirect(url.as_str(), "private, max-age=3000");
    }

    // The WebP copies off: one kept already is served, and none is made —
    // the page is sent to the picture itself, and asks again next time.
    if format == Format::Webp && !state.flag("media.webp", false) {
        return match store.read(&key).await {
            Ok(Some(bytes)) => whole(method, format_type(format), &key, &etag, bytes, IMMUTABLE),
            Ok(None) => redirect(&state.media.url_for(&original), "no-cache"),
            Err(e) => {
                tracing::warn!(
                    key,
                    error = format_args!("{e:#}"),
                    "could not read a medium"
                );
                redirect(&state.media.url_for(&original), "no-cache")
            }
        };
    }

    match kept_or_made(
        state, store, &original, asked.stem, made, format, &key, caller,
    )
    .await
    {
        Ok(bytes) => whole(method, format_type(format), &key, &etag, bytes, IMMUTABLE),
        // Counted against what the address may ask, and spent: told so.
        Err(Unmade::Spent(refusal)) => refusal.into_response(),
        // As many are being made as there is room for: the picture itself,
        // for now, and the copy asked for again.
        Err(Unmade::Busy) => {
            tracing::debug!(key, "too many copies are being made; the picture is sent");
            redirect(&state.media.url_for(&original), "no-store")
        }
        Err(Unmade::Failed(e)) => {
            tracing::warn!(
                key,
                error = format_args!("{e:#}"),
                "could not make a copy of a picture"
            );
            // The picture itself, whole: a page shows it rather than nothing.
            redirect(&state.media.url_for(&original), "no-store")
        }
    }
}

/// An AVIF copy: the night's task makes them, and a page is offered one
/// only where it exists. One whose file has gone — a store moved, a restore
/// onto a new machine — is answered with the WebP of the same width, or,
/// while the WebP copies are off, with the picture itself; not kept for
/// long, and forgotten, so that the next night makes it again.
#[allow(clippy::too_many_arguments)]
async fn avif(
    state: &AppState,
    store: &Store,
    method: &Method,
    original: &str,
    stem: &str,
    made: Made,
    key: &str,
    caller: Option<IpAddr>,
    headers: &HeaderMap,
) -> Response {
    let etag = format!("\"{key}\"");
    if none_match(headers, &etag) {
        return not_modified(&etag);
    }
    match store.read(key).await {
        Ok(Some(bytes)) => {
            return whole(method, "image/avif", key, &etag, bytes, IMMUTABLE);
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(
                key,
                error = format_args!("{e:#}"),
                "could not read a medium"
            );
            return StatusCode::BAD_GATEWAY.into_response();
        }
    }
    if let Made::Width(width) = made {
        state.media.drop_avif(stem, width);
        if let Err(e) = repo::asset::delete_variant(&state.db, key).await {
            tracing::debug!(key, error = %e, "could not forget a copy whose file is gone");
        }
    }
    if !state.flag("media.webp", false) {
        return redirect(&state.media.url_for(original), "no-cache");
    }
    let webp = file::made_key(stem, made, Format::Webp.ext());
    match kept_or_made(
        state,
        store,
        original,
        stem,
        made,
        Format::Webp,
        &webp,
        caller,
    )
    .await
    {
        Ok(bytes) => whole(
            method,
            "image/webp",
            &webp,
            &format!("\"{webp}\""),
            bytes,
            "no-cache",
        ),
        Err(Unmade::Spent(refusal)) => refusal.into_response(),
        Err(Unmade::Busy) => redirect(&state.media.url_for(original), "no-store"),
        Err(Unmade::Failed(_)) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Why a copy was not given.
enum Unmade {
    /// The caller has spent what its address may ask of this server.
    Spent(AppError),
    /// As many copies are being made as there are places for.
    Busy,
    /// It could not be made.
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for Unmade {
    fn from(e: anyhow::Error) -> Self {
        Self::Failed(e)
    }
}

/// Why making a copy stopped — what the several who asked for the same one
/// at once are each told.
enum Skipped {
    Busy,
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for Skipped {
    fn from(e: anyhow::Error) -> Self {
        Self::Failed(e)
    }
}

impl From<Arc<Skipped>> for Unmade {
    fn from(skipped: Arc<Skipped>) -> Self {
        match &*skipped {
            Skipped::Busy => Self::Busy,
            Skipped::Failed(e) => Self::Failed(anyhow::anyhow!("{e:#}")),
        }
    }
}

/// A copy's bytes: the store's, or made now, put there and noted — once,
/// however many ask for it at the same moment.
///
/// One that is made is work, so what asks for it is held to what the server
/// holds the rest of its work to: the caller is counted against what its
/// address may ask, once for each request that finds the copy not kept;
/// no more than [`super::COPIES_AT_ONCE`] are made at a time; and the copy
/// waits for its turn among the pictures decoded *before* its picture is
/// read from the store, so that what waits is a request and not a picture of
/// up to 25 MiB for each.
#[allow(clippy::too_many_arguments)]
async fn kept_or_made(
    state: &AppState,
    store: &Store,
    original: &str,
    stem: &str,
    made: Made,
    format: Format,
    key: &str,
    caller: Option<IpAddr>,
) -> Result<Bytes, Unmade> {
    if let Some(bytes) = store.read(key).await? {
        return Ok(bytes);
    }
    // Not kept: this request is one that makes it, or waits for one that does.
    if let Some(peer) = caller {
        crate::auth::ratelimit::spend(state, peer, LOGGED_AS)
            .await
            .map_err(Unmade::Spent)?;
    }
    MADE.try_get_with(key.to_string(), async {
        // A place among the copies being made, those waiting for a turn
        // counted: the copy's own work holds it until it is done.
        let place = state
            .media
            .copying
            .clone()
            .try_acquire_owned()
            .map_err(|_| Skipped::Busy)?;
        // The turn first, then the picture.
        let turn = variants::turn().await?;
        let bytes = store
            .read(original)
            .await?
            .context("the picture is not in the store")?;
        // What the picture is to its work: a logo is copied without loss.
        let cover = match state.media.keyed(original) {
            Some(known) => repo::asset::cover_of(&state.db, &known.origin)
                .await
                .unwrap_or(None),
            None => None,
        };
        let copy = turn
            .run(move || {
                let _place = place;
                let decoded = variants::Decoded::read(&bytes)?;
                variants::make(&decoded, made, format, cover)
            })
            .await?;
        // Filed under what was made: a width asked of a picture whose own
        // was not known may have come out narrower.
        let filed = copy.key(stem);
        let content_type = copy.content_type();
        let made_bytes = Bytes::from(copy.bytes);
        store.put(&filed, made_bytes.clone(), content_type).await?;
        if let Err(e) = repo::asset::add_variant(
            &state.db,
            &repo::asset::Variant {
                key: filed.clone(),
                stem: stem.to_string(),
                width: i32::try_from(copy.width).unwrap_or(i32::MAX),
                height: i32::try_from(copy.height).unwrap_or(i32::MAX),
                format: format.as_str().to_string(),
                bytes: i64::try_from(made_bytes.len()).unwrap_or(i64::MAX),
            },
        )
        .await
        {
            tracing::warn!(key = filed, error = %e, "could not note a copy made");
        }
        Ok::<_, Skipped>(made_bytes)
    })
    .await
    .map_err(Unmade::from)
}

fn format_type(format: Format) -> &'static str {
    match format {
        Format::Webp => "image/webp",
        Format::Avif => "image/avif",
        Format::Jpeg => "image/jpeg",
    }
}

/// A whole file's answer, from bytes in hand: nothing for a `HEAD`.
fn whole(
    method: &Method,
    content_type: &str,
    key: &str,
    etag: &str,
    bytes: Bytes,
    cache: &'static str,
) -> Response {
    let length = bytes.len() as u64;
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    let mut response = answer(content_type, key, etag, length, None)
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    // What `answer` says of caching, unless this one is to be asked again.
    if cache != IMMUTABLE {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    }
    response
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

    /// The whole server, on a database of its own, keeping its media in
    /// `dir`.
    async fn server(dir: &std::path::Path) -> AppState {
        limited_server(dir, 600).await
    }

    /// The same, an address of which may make `per_minute` requests a minute.
    async fn limited_server(dir: &std::path::Path, per_minute: u32) -> AppState {
        let mut config = crate::config::Config::from_env().expect("a configuration");
        config.security.rate_limit_per_minute = per_minute;
        config.mode = crate::config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = crate::config::MediaStorage::Filesystem;
        config.media.dir = dir.to_path_buf();
        config.clients = None;
        config.tmdb.api_key = None;
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;
        AppState::bootstrap(config).await.expect("a server")
    }

    /// A poster kept, as the worker keeps one — in the store and in the
    /// index — and no copy of it yet: its key and its hash. `shade` makes
    /// it a picture of its own, whose copies no other test makes.
    async fn kept(state: &AppState, shade: u8) -> (String, String) {
        let mut bytes = Vec::new();
        image::RgbImage::from_fn(600, 900, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, shade])
        })
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let (key, sha) = file::key_for(&bytes, "png");
        state
            .media
            .store()
            .unwrap()
            .put(&key, Bytes::from(bytes), "image/png")
            .await
            .unwrap();
        state.media.remember(
            &format!("https://images.example/{sha}.png"),
            &key,
            repo::asset::Thumb::None,
            "image/png",
        );
        (key, sha)
    }

    async fn asked(state: &AppState, key: &str) -> Response {
        get(
            State(state.clone()),
            Method::GET,
            Path(key.to_string()),
            HeaderMap::new(),
        )
        .await
    }

    async fn webp_copies(state: &AppState, sha: &str) -> Vec<(String, i32)> {
        repo::asset::variants_of(&state.db, sha)
            .await
            .unwrap()
            .into_iter()
            .filter(|copy| copy.format == "webp")
            .map(|copy| (copy.key, copy.width))
            .collect()
    }

    async fn turn(state: &AppState, key: &str, on: bool) {
        state
            .settings
            .set(
                crate::settings::Scope::Server,
                "",
                key,
                if on { "true" } else { "false" },
                None,
            )
            .await
            .unwrap();
    }

    /// While the WebP copies are off — as they are until somebody turns
    /// them on — a page that asks for one is sent to the picture itself,
    /// and asks again next time: nothing is made, and nothing noted. On,
    /// one is made, kept and noted as it is asked for; off again, the copy
    /// kept is still served, and no other is made.
    #[tokio::test]
    async fn a_webp_copy_is_made_only_while_they_are_on() {
        let dir = crate::testing::scratch_path("serve-webp");
        let state = server(&dir).await;
        let store = state.media.store().unwrap().clone();
        let (key, sha) = kept(&state, 31).await;
        let copy = format!("{sha}-480.webp");

        let answer = asked(&state, &copy).await;
        assert_eq!(answer.status(), StatusCode::FOUND);
        assert_eq!(answer.headers()[header::LOCATION], format!("/media/{key}"));
        assert_eq!(answer.headers()[header::CACHE_CONTROL], "no-cache");
        assert!(!store.exists(&copy).await.unwrap(), "nothing made");
        assert!(webp_copies(&state, &sha).await.is_empty(), "nothing noted");
        // A width no page asks for is refused all the same.
        let refused = asked(&state, &format!("{sha}-481.webp")).await;
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);

        turn(&state, "media.webp", true).await;
        let answer = asked(&state, &copy).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(answer.headers()[header::CONTENT_TYPE], "image/webp");
        assert_eq!(answer.headers()[header::CACHE_CONTROL], IMMUTABLE);
        assert!(store.exists(&copy).await.unwrap(), "made and kept");
        assert_eq!(webp_copies(&state, &sha).await, [(copy.clone(), 480)]);

        turn(&state, "media.webp", false).await;
        let answer = asked(&state, &copy).await;
        assert_eq!(answer.status(), StatusCode::OK, "the copy kept is served");
        assert_eq!(answer.headers()[header::CONTENT_TYPE], "image/webp");
        let other = format!("{sha}-240.webp");
        let answer = asked(&state, &other).await;
        assert_eq!(answer.status(), StatusCode::FOUND);
        assert_eq!(answer.headers()[header::LOCATION], format!("/media/{key}"));
        assert!(!store.exists(&other).await.unwrap());
        assert_eq!(webp_copies(&state, &sha).await.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The route is mounted on the handler that knows its caller: on [`get`],
    /// which is for what asks from inside the server, no copy made would be
    /// counted against anybody.
    #[test]
    fn the_media_route_is_mounted_on_the_handler_that_counts_its_caller() {
        let web = include_str!("../web.rs");
        let route = web
            .lines()
            .find(|line| line.contains(".route(\"/media/{key}\""))
            .expect("the route of the media kept");
        assert!(
            route.contains("crate::media::serve::get_for_caller"),
            "web.rs mounts {route:?}: no copy made would be counted against anybody — mount \
             `.route(\"/media/{{key}}\", get(crate::media::serve::get_for_caller))`"
        );
    }

    /// A copy asked for by a caller whose address is known, as the route
    /// asks it.
    async fn asked_by_a_caller(state: &AppState, key: &str) -> Response {
        get_for_caller(
            State(state.clone()),
            Method::GET,
            Path(key.to_string()),
            ClientIp::NONE,
            HeaderMap::new(),
        )
        .await
    }

    /// A caller who asks for a copy that is not kept is counted against what
    /// its address may ask — once, and for the copy that is made alone: the
    /// copy kept is served, and so is the picture, whatever has been spent.
    /// Spent, it is told so, and nothing is made.
    #[tokio::test]
    async fn a_caller_who_has_a_copy_made_is_counted_and_one_who_has_none_is_not() {
        let dir = crate::testing::scratch_path("serve-counted");
        let state = limited_server(&dir, 2).await;
        let store = state.media.store().unwrap().clone();
        turn(&state, "media.webp", true).await;
        let mut pictures = Vec::new();
        for shade in [81, 82, 83] {
            pictures.push(kept(&state, shade).await);
        }

        // Two copies are made, and counted: the quota is two.
        for (_, sha) in &pictures[..2] {
            let answer = asked_by_a_caller(&state, &format!("{sha}-480.webp")).await;
            assert_eq!(answer.status(), StatusCode::OK, "{sha}");
        }
        // The third is not made: the address has spent what it may ask.
        let third = format!("{}-480.webp", pictures[2].1);
        let answer = asked_by_a_caller(&state, &third).await;
        assert_eq!(answer.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(!store.exists(&third).await.unwrap(), "nothing was made");
        assert!(webp_copies(&state, &pictures[2].1).await.is_empty());

        // What is kept is served all the same, and so are the pictures
        // themselves: neither is counted.
        let first = format!("{}-480.webp", pictures[0].1);
        for _ in 0..3 {
            let answer = asked_by_a_caller(&state, &first).await;
            assert_eq!(answer.status(), StatusCode::OK);
            let answer = asked_by_a_caller(&state, &pictures[2].0).await;
            assert_eq!(answer.status(), StatusCode::OK);
        }
        // What asks from inside the server has no caller to count.
        let answer = asked(&state, &third).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert!(store.exists(&third).await.unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// As many copies are being made as there are places for: one more is
    /// not made — the page is sent the picture itself, for now, and the copy
    /// is made when it asks again.
    #[tokio::test]
    async fn no_more_copies_are_made_at_once_than_there_are_places_for() {
        let dir = crate::testing::scratch_path("serve-places");
        let state = server(&dir).await;
        let store = state.media.store().unwrap().clone();
        turn(&state, "media.webp", true).await;
        let (key, sha) = kept(&state, 91).await;
        let copy = format!("{sha}-480.webp");

        let places = state
            .media
            .copying
            .clone()
            .acquire_many_owned(crate::media::COPIES_AT_ONCE as u32)
            .await
            .unwrap();
        let answer = asked(&state, &copy).await;
        assert_eq!(answer.status(), StatusCode::FOUND);
        assert_eq!(answer.headers()[header::LOCATION], format!("/media/{key}"));
        assert_eq!(answer.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!store.exists(&copy).await.unwrap(), "nothing made");
        assert!(webp_copies(&state, &sha).await.is_empty(), "nothing noted");

        drop(places);
        let answer = asked(&state, &copy).await;
        assert_eq!(answer.status(), StatusCode::OK, "made once there is room");
        assert!(store.exists(&copy).await.unwrap());
        // The places are given back once a copy is made.
        assert_eq!(
            state.media.copying.available_permits(),
            crate::media::COPIES_AT_ONCE
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A copy waits for its turn among the pictures decoded before its
    /// picture is read from the store: while every turn is taken, the
    /// request is a request waiting and not a picture of up to 25 MiB held
    /// in memory with it. The picture is gone from the store here, so that
    /// a read made too soon is told: it would answer at once.
    #[tokio::test]
    async fn a_copy_waits_for_its_turn_before_its_picture_is_read() {
        let dir = crate::testing::scratch_path("serve-turn");
        let state = server(&dir).await;
        let store = state.media.store().unwrap().clone();
        turn(&state, "media.webp", true).await;
        let (key, sha) = kept(&state, 61).await;
        store.delete(&key).await.unwrap();
        let copy = format!("{sha}-480.webp");

        let turns = variants::every_turn().await;
        let asking = tokio::spawn({
            let state = state.clone();
            let copy = copy.clone();
            async move { asked(&state, &copy).await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !asking.is_finished(),
            "the picture was read before the turn was had: it is not there, and the request is over"
        );
        drop(turns);
        let answer = tokio::time::timeout(Duration::from_secs(10), asking)
            .await
            .expect("answered once there is a turn")
            .unwrap();
        // The picture is not in the store: the page is sent it all the same.
        assert_eq!(answer.status(), StatusCode::FOUND);
        assert_eq!(answer.headers()[header::CACHE_CONTROL], "no-store");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An AVIF of 480 of the picture of `sha`, noted and offered, whose
    /// file is not there: a store moved, say. Its key.
    async fn avif_without_its_file(state: &AppState, sha: &str) -> String {
        let avif = format!("{sha}-480.avif");
        repo::asset::add_variant(
            &state.db,
            &repo::asset::Variant {
                key: avif.clone(),
                stem: sha.to_string(),
                width: 480,
                height: 720,
                format: "avif".into(),
                bytes: 10,
            },
        )
        .await
        .unwrap();
        state.media.add_avif(sha, 480);
        avif
    }

    /// An AVIF whose file has gone is forgotten, and answered — while the
    /// WebP copies are off — with the picture itself, not with a WebP made
    /// for it; while they are on, with the WebP of its width, as before.
    #[tokio::test]
    async fn an_avif_gone_is_answered_with_the_picture_while_the_webp_copies_are_off() {
        let dir = crate::testing::scratch_path("serve-avif");
        let state = server(&dir).await;
        let store = state.media.store().unwrap().clone();
        let (key, sha) = kept(&state, 47).await;

        let avif = avif_without_its_file(&state, &sha).await;
        assert_eq!(state.media.keyed(&key).unwrap().look.avif, [480]);
        let answer = asked(&state, &avif).await;
        assert_eq!(answer.status(), StatusCode::FOUND);
        assert_eq!(answer.headers()[header::LOCATION], format!("/media/{key}"));
        assert_eq!(answer.headers()[header::CACHE_CONTROL], "no-cache");
        assert!(state.media.keyed(&key).unwrap().look.avif.is_empty());
        assert!(
            repo::asset::variants_of(&state.db, &sha)
                .await
                .unwrap()
                .is_empty(),
            "forgotten, and no WebP made for it"
        );
        assert!(!store.exists(&format!("{sha}-480.webp")).await.unwrap());

        turn(&state, "media.webp", true).await;
        let avif = avif_without_its_file(&state, &sha).await;
        let answer = asked(&state, &avif).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(answer.headers()[header::CONTENT_TYPE], "image/webp");
        assert_eq!(answer.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(
            webp_copies(&state, &sha).await,
            [(format!("{sha}-480.webp"), 480)]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

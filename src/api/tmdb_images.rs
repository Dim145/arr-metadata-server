//! TMDB's pictures, from this server: `GET /tmdb-images/{size}/{file}`.
//!
//! What TMDB's configuration gives its clients as the address of its
//! pictures when an administrator chose this server for them
//! (`tmdb.images = server`, [`base`]). Jellyfin, Kodi and Kometa — Plex
//! under it — and, it seems, Emby put a size and a picture's path after
//! `images.secure_base_url` and fetch the picture from there, so they fetch
//! it from here without anything changed on their side. A picture this
//! server keeps — TMDB's original, as the media store files it — is served
//! from the store, as `/media/{key}` serves it; any other is fetched from
//! image.tmdb.org at the size asked and handed on as it comes, and kept
//! nowhere. The gate answers whatever the setting says now: Kodi keeps the
//! address it was given for a month.
//!
//! Only TMDB's own sizes and the crops its site asks it for, and only names
//! of TMDB's pictures, are taken: the address asked is always TMDB's, so
//! nothing else can be had through the gate. It is guarded as the TMDB relay
//! is — switched off with it, behind the same address list — but a key
//! policy reads as the address list: a picture's address carries no key.
//!
//! A library's scan asks for pictures in bursts no relay's caller makes, so a
//! caller the guard lets in is not held to the quota every surface is held to,
//! but to one of its own, ten times as high ([`Pace`]); and the pictures
//! handed on from TMDB are so many at a time ([`crate::cache::Caches::pictures`]),
//! the others waiting for their turn.

use std::{
    net::IpAddr,
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures::StreamExt as _;
use governor::{
    Quota, RateLimiter,
    clock::{Clock as _, DefaultClock},
    state::keyed::DefaultKeyedStateStore,
};
use tokio::sync::OwnedSemaphorePermit;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    error::{AppError, AppResult, Throttle},
    state::AppState,
};

/// Where the gate is on this server: what TMDB's configuration is given as
/// the start of its pictures' addresses, a size and a path to follow.
pub const ROUTE: &str = "/tmdb-images/";

/// The setting that says where TMDB's clients fetch its pictures from:
/// `tmdb`, TMDB's own address, or `server`, this server's gate.
pub const SETTING: &str = "tmdb.images";

/// Where TMDB serves its pictures, a size and a path after it.
const TMDB_PICTURES: &str = "https://image.tmdb.org/t/p";

/// The sizes TMDB's configuration lists, every kind of picture's together:
/// posters, backgrounds, logos, profiles, stills.
const SIZES: [&str; 11] = [
    "original", "w45", "w92", "w154", "w185", "w300", "w342", "w500", "w780", "w1280", "h632",
];

/// How a crop of TMDB's site chooses its frame: the best of the picture,
/// its face, its faces.
const FRAMED_BY: [&str; 3] = ["bestv2", "face", "multi_faces"];

/// The longest side a crop may ask for: TMDB's site asks for 1920 at most.
const CROP_MAX: u32 = 3840;

/// How long a picture handed on may be kept by whoever reads it: TMDB never
/// puts other bytes under a picture's path, and keeps its own a year.
const KEPT_FOR: &str = "public, max-age=31536000, immutable";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(picture))
}

/// How many times the requests an address may make a minute
/// (`AMS_RATE_LIMIT_PER_MINUTE`) it may ask for pictures in that minute. The
/// quota every surface is held to would stop a scan halfway through a
/// library; ten times it stops nobody who is scanning one, and an address that
/// is not.
const PICTURES_PER_REQUEST: u32 = 10;

/// How many addresses are counted before those gone quiet are dropped, and the
/// fewest seconds between two such sweeps: the addresses come from whoever can
/// reach the gate, and a sweep reads every one of them.
const SWEEP_ABOVE: usize = 4096;
const SWEEP_EVERY: u64 = 30;

type Keyed = RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>;

/// What each address has asked of the gate this minute, held to a quota of its
/// own: kept in this instance's memory, as the quota of requests is when
/// nothing is shared among several.
#[derive(Clone)]
pub struct Pace {
    state: AppState,
    per_minute: u32,
    counts: Option<Arc<Keyed>>,
    /// When the addresses gone quiet were last dropped, in seconds since the
    /// epoch.
    swept: Arc<AtomicU64>,
}

impl Pace {
    /// A quota [`PICTURES_PER_REQUEST`] times the surfaces' — none, where
    /// theirs is none (`AMS_RATE_LIMIT_PER_MINUTE=0`).
    pub fn new(state: AppState) -> Self {
        let per_minute = state
            .config
            .security
            .rate_limit_per_minute
            .saturating_mul(PICTURES_PER_REQUEST);
        Self {
            state,
            per_minute,
            counts: NonZeroU32::new(per_minute)
                .map(|quota| Arc::new(RateLimiter::keyed(Quota::per_minute(quota)))),
            swept: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Drop the addresses gone quiet, once there are many and it has been a
    /// while: one count is kept for each address ever seen otherwise.
    fn sweep(&self, counts: &Keyed) {
        if counts.len() <= SWEEP_ABOVE {
            return;
        }
        let now = crate::cache::now_secs();
        let last = self.swept.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= SWEEP_EVERY
            && self
                .swept
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            counts.retain_recent();
            counts.shrink_to_fit();
        }
    }
}

/// Count a picture asked for by a caller the guard let in against its
/// address's quota of pictures: refused, saying when to come back, once that
/// is spent. Inside the guard, which counts only those it turns away
/// ([`crate::auth::middleware::guard_tmdb_images`]), and by the address it
/// resolved.
pub async fn pace(State(pace): State<Pace>, request: Request, next: Next) -> AppResult<Response> {
    if let Some(counts) = &pace.counts {
        let peer = crate::auth::ratelimit::peer_of(&pace.state, &request);
        if let Err(refused) = counts.check_key(&peer) {
            let wait = refused.wait_time_from(counts.clock().now());
            tracing::warn!(
                %peer,
                path = %crate::telemetry::redact_path(request.uri().path()),
                "rate limited"
            );
            return Err(AppError::RateLimited(Throttle::spent(
                pace.per_minute,
                Duration::from_secs(60),
                wait,
            )));
        }
        pace.sweep(counts);
    }
    Ok(next.run(request).await)
}

/// Where TMDB's configuration sends its clients for pictures, when that is
/// this server's gate: an administrator chose so (`tmdb.images = server`), a
/// media store keeps pictures here, and this server has a public address
/// (`AMS_PUBLIC_URL`) to give — the one its clients reach it at. None
/// otherwise, and TMDB's own address stands: `server` without a store or a
/// public address reads as `tmdb`. Read on each answer, so a change shows
/// at once.
pub fn base(state: &AppState) -> Option<String> {
    let chosen = state.settings.resolve(SETTING, None, None);
    if chosen.as_deref().map(str::trim) != Some("server") || !state.media.is_on() {
        return None;
    }
    let public = state.config.server.public_url.as_deref()?;
    Some(format!("{}{ROUTE}", public.trim_end_matches('/')))
}

/// One of TMDB's pictures, from this server.
///
/// A picture this server keeps — TMDB's original, in the media store — is
/// served from the store whatever the size asked, the closest this version
/// has: it makes no smaller JPEG of a picture, its copies at a width being
/// the interface's WebP and AVIF, which a TMDB client is not known to read.
/// Any other picture is fetched from image.tmdb.org at the size asked and
/// handed on as it comes, kept nowhere. Only TMDB's sizes are taken —
/// those its configuration lists, and the crops its site and Seerr ask it
/// for — and only a picture's name as TMDB files it, `.jpg` or `.png`.
#[utoipa::path(
    get, path = "/tmdb-images/{size}/{file}", tag = super::tmdb::TAG,
    operation_id = "tmdb_images_picture",
    params(
        ("size" = String, Path, description = "One of the sizes TMDB's configuration lists — `original`, `w500`, `h632`… — or a crop of TMDB's site such as `w600_and_h900_bestv2`"),
        ("file" = String, Path, description = "The picture's name, as TMDB's path gives it: `kqjL17yufvn9OVLyXYpvtyrFfak.jpg`"),
    ),
    responses(
        (status = 200, description = "The picture: the copy kept here, or TMDB's own at the size asked"),
        (status = 400, description = "Not one of TMDB's sizes, or not a picture's name as TMDB gives one"),
        (status = 403, description = "The caller's address is not on the list that guards the TMDB relay"),
        (status = 404, description = "TMDB has no such picture at that size"),
        (status = 429, description = "The caller's quota of pictures is spent for now; `Retry-After` says when to ask again"),
        (status = 502, description = "TMDB's pictures could not be reached"),
        (status = 503, description = "The TMDB relay is switched off, or as many pictures as are handed on from TMDB at once are under way (`Retry-After` says when to ask again)"),
    ),
    security(),
)]
pub async fn picture(
    State(state): State<AppState>,
    method: Method,
    Path((size, file)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    // The relay switched off is switched off, its pictures with it.
    if !state.config.tmdb.passthrough {
        return AppError::ProviderNotConfigured.into_response();
    }
    let Some(content_type) = picture_type(&size, &file) else {
        return AppError::BadRequest(
            "that is not one of TMDB's pictures at one of its sizes".into(),
        )
        .into_response();
    };

    // A picture of another source's, or one uploaded, that a document of
    // TMDB's shape written here named by its key in the store
    // (`tmdb.images = server`, [`kept_name`]): served from the store,
    // whatever the size asked, and never asked of TMDB, which files nothing
    // under such a name.
    if kept_name(&file).is_some() {
        if state.media.keyed(&file).is_none() {
            return AppError::NotFound.into_response();
        }
        return crate::media::serve::get(State(state.clone()), method, Path(file), headers).await;
    }

    // Kept here: TMDB's original, filed by the address the provider gave.
    // A file gone from the store is fetched from TMDB again.
    let origin = crate::providers::tmdb::image_url(&format!("/{file}"));
    if let Some(kept) = state.media.lookup(&origin) {
        let served = crate::media::serve::get(
            State(state.clone()),
            method.clone(),
            Path(kept.key.to_string()),
            headers.clone(),
        )
        .await;
        if served.status() != StatusCode::NOT_FOUND {
            return served;
        }
    }

    // Handed on from TMDB, a few at a time whoever asks: each is bytes read
    // from image.tmdb.org on this server's account, up to 25 MiB of them, and
    // held — with the connection to TMDB — until the caller has had the last.
    let Some(place) = state.caches.pictures.take().await else {
        return crate::api::busy();
    };
    relay(
        &state.media.http,
        TMDB_PICTURES,
        &method,
        &size,
        &file,
        content_type,
        &headers,
        crate::media::file::MAX_IMAGE_BYTES,
        place,
    )
    .await
}

/// The name the gate serves a picture this server keeps under, where it is
/// none of TMDB's: its key in the media store — sixty-four hex digits and
/// `.jpg` or `.png`, the picture's own bytes rather than a copy made of
/// them — which no name of TMDB's is. What a document of TMDB's shape
/// written here names a source's picture, or one uploaded, by when TMDB's
/// clients are sent here for pictures. None for any other key: a sound, a
/// copy at a width, a WebP.
pub fn kept_name(key: &str) -> Option<String> {
    let parsed = crate::media::file::parse_key(key)?;
    (parsed.made == crate::media::file::Made::Own && matches!(parsed.ext, "jpg" | "png"))
        .then(|| format!("/{key}"))
}

/// The type a picture is served as, when the size and the name asked are
/// TMDB's: a size its configuration lists or a crop of its site
/// ([`is_crop`]), and a picture's name as TMDB files it — letters, digits
/// and nothing that could leave the path — ending in `.jpg` or `.png`. No
/// SVG, which a few of TMDB's logos are: a document that can carry a script,
/// served from this server's origin, would run it as this server. None for
/// anything else.
fn picture_type(size: &str, file: &str) -> Option<&'static str> {
    if !(SIZES.contains(&size) || is_crop(size)) {
        return None;
    }
    let (name, ext) = file.rsplit_once('.')?;
    let named = (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !named {
        return None;
    }
    match ext {
        "jpg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        _ => None,
    }
}

/// Whether a size is a crop of TMDB's site — `w600_and_h900_bestv2`,
/// `w300_and_h450_face`, `w1920_and_h800_multi_faces`, which Seerr asks
/// TMDB for too — that TMDB serves though its configuration lists none: a
/// width, a height, and how the frame is chosen ([`FRAMED_BY`]). Its
/// approach here: a picture kept is served whole, and any other is TMDB's
/// own crop.
fn is_crop(size: &str) -> bool {
    let Some((width, rest)) = size
        .strip_prefix('w')
        .and_then(|rest| rest.split_once("_and_h"))
    else {
        return false;
    };
    let Some((height, framed)) = rest.split_once('_') else {
        return false;
    };
    side(width) && side(height) && FRAMED_BY.contains(&framed)
}

/// A side as a crop writes it: digits, no leading nought, no longer than
/// [`CROP_MAX`].
fn side(digits: &str) -> bool {
    !digits.is_empty()
        && digits.len() <= 4
        && !digits.starts_with('0')
        && digits.bytes().all(|b| b.is_ascii_digit())
        && digits.parse::<u32>().is_ok_and(|n| n <= CROP_MAX)
}

/// A picture TMDB has, asked of `from` at the size asked and handed on as
/// it comes: kept nowhere, its bytes passed through as they arrive — no
/// more of them than `most` — and served as the type its name says,
/// whatever TMDB's answer said. The caller's validators travel, so that a
/// picture it holds is answered "unchanged"; nothing else of its request
/// does. TMDB's "no such picture" — a size it does not cut that picture at
/// is one too — is a `404`; anything else that is not the picture, a `502`.
/// `place` is the one the picture takes among those handed on at once: held
/// until the last of its bytes has been read, or the caller is gone.
#[allow(clippy::too_many_arguments)]
async fn relay(
    http: &reqwest::Client,
    from: &str,
    method: &Method,
    size: &str,
    file: &str,
    content_type: &'static str,
    asked: &HeaderMap,
    most: u64,
    place: OwnedSemaphorePermit,
) -> Response {
    let url = format!("{from}/{size}/{file}");
    let head = *method == Method::HEAD;
    let mut request = if head {
        http.head(&url)
    } else {
        http.get(&url)
    };
    for name in [header::IF_NONE_MATCH, header::IF_MODIFIED_SINCE] {
        for value in asked.get_all(&name) {
            request = request.header(name.clone(), value.clone());
        }
    }

    let response = match crate::outbound::send("tmdb", request).await {
        Ok(response) => response,
        Err(e) => {
            return AppError::UpstreamUnavailable(anyhow::anyhow!(
                "TMDB's picture could not be fetched: {}",
                crate::providers::describe_request_error(&e)
            ))
            .into_response();
        }
    };

    let status = response.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        let mut answer = StatusCode::NOT_MODIFIED.into_response();
        validators_of(response.headers(), answer.headers_mut());
        answer
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(KEPT_FOR));
        return answer;
    }
    if matches!(status.as_u16(), 400 | 404 | 410) {
        return AppError::NotFound.into_response();
    }
    if status != reqwest::StatusCode::OK {
        return AppError::UpstreamUnavailable(anyhow::anyhow!(
            "TMDB answered {status} for a picture"
        ))
        .into_response();
    }
    if response
        .content_length()
        .is_some_and(|length| length > most)
    {
        return AppError::UpstreamUnavailable(anyhow::anyhow!(
            "TMDB's picture is larger than {most} bytes"
        ))
        .into_response();
    }

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(KEPT_FOR));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Read from other origins, as the media kept here are: a picture runs
    // nothing.
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("cross-origin"),
    );
    if let Ok(disposition) = HeaderValue::from_str(&format!("inline; filename=\"{file}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    validators_of(response.headers(), &mut headers);

    let body = if head {
        Body::empty()
    } else {
        // Its length when TMDB said it of the bytes as they are read: not
        // of a body the client decoded on the way.
        if let Some(length) = response.content_length() {
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
        }
        bounded(response, most, place)
    };
    (StatusCode::OK, headers, body).into_response()
}

/// TMDB's validators of a picture, which describe the bytes handed on as
/// they are: its tag and its date.
fn validators_of(from: &HeaderMap, to: &mut HeaderMap) {
    for name in [header::ETAG, header::LAST_MODIFIED] {
        if let Some(value) = from.get(&name) {
            to.insert(name, value.clone());
        }
    }
}

/// A picture's bytes as they come, cut off past `most`: a picture larger
/// than any this server keeps is not one it hands on either. The body holds
/// `place` until it is dropped, read to its end or not.
fn bounded(response: reqwest::Response, most: u64, place: OwnedSemaphorePermit) -> Body {
    let mut read: u64 = 0;
    let stream = response.bytes_stream().map(move |chunk| {
        let _held = &place;
        let chunk = chunk.map_err(std::io::Error::other)?;
        read += chunk.len() as u64;
        if read > most {
            return Err(std::io::Error::other(
                "TMDB's picture is larger than this server hands on",
            ));
        }
        Ok(chunk)
    });
    Body::from_stream(stream)
}

/// What the tests of the gate, and of the configuration that sends clients
/// to it, share: the whole server keeping its media, and the choice of
/// where TMDB's pictures are fetched from.
#[cfg(test)]
pub(crate) mod testing {
    use crate::{
        api::relay::testing::{OPERATOR_TMDB_KEY, OPERATOR_TVDB_KEY, StandIn},
        config::{self, SurfacePolicy},
        state::AppState,
    };

    /// The whole server as the relays' tests run it
    /// ([`crate::api::relay::testing::server`]) — its catalogue kept in
    /// French, TMDB keyed as an operator keys it, every service at
    /// `stand_in`, its relays guarded by a key — keeping its media in a
    /// directory of its own, and reached at `public_url`. The server, and
    /// the directory to remove.
    pub async fn server_keeping_media(
        stand_in: &StandIn,
        public_url: Option<&str>,
    ) -> (AppState, std::path::PathBuf) {
        server_keeping_media_as(stand_in, public_url, |_| {}).await
    }

    /// [`server_keeping_media`], its configuration changed by `configure`
    /// before it starts.
    pub async fn server_keeping_media_as(
        stand_in: &StandIn,
        public_url: Option<&str>,
        configure: impl FnOnce(&mut config::Config),
    ) -> (AppState, std::path::PathBuf) {
        let dir = crate::testing::scratch_path("tmdb-images");
        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.security.auth_disabled = false;
        config.security.tmdb_policy = SurfacePolicy::ApiKey;
        config.security.tvdb_policy = SurfacePolicy::ApiKey;
        config.cache.redis_url = None;
        config.media.storage = config::MediaStorage::Filesystem;
        config.media.dir = dir.clone();
        config.media.s3 = None;
        config.clients = None;
        config.server.public_url = public_url.map(str::to_string);
        config.tmdb.api_key = Some(OPERATOR_TMDB_KEY.into());
        config.tmdb.language = "fr-FR".into();
        config.tmdb.passthrough = true;
        config.tvdb.api_key = Some(OPERATOR_TVDB_KEY.into());
        config.tvdb.pin = None;
        config.tvdb.passthrough = true;
        let at = stand_in.at.clone();
        for upstream in [
            &mut config.tmdb.upstream,
            &mut config.fanart.upstream,
            &mut config.skyhook.upstream,
            &mut config.sonarr_services.upstream,
            &mut config.sonarr_services.xem_upstream,
            &mut config.radarr_metadata.upstream,
            &mut config.tvmaze.upstream,
            &mut config.anilist.upstream,
            &mut config.mal.upstream,
            &mut config.mal.jikan_upstream,
            &mut config.mal.tenrai_upstream,
            &mut config.fankai.upstream,
            &mut config.fankai_wiki.upstream,
            &mut config.imdb.datasets,
            &mut config.anime_mapping.url,
        ] {
            upstream.clone_from(&at);
        }
        config.tvdb.upstream = format!("{at}/v4");
        configure(&mut config);
        (AppState::bootstrap(config).await.expect("a server"), dir)
    }

    /// Choose where TMDB's clients fetch its pictures from: `tmdb` or
    /// `server`.
    pub async fn choose(state: &AppState, value: &str) {
        crate::api::relay::testing::choose(state, super::SETTING, value).await;
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::{Router, extract::ConnectInfo, routing::get};
    use bytes::Bytes;
    use tower::ServiceExt as _;

    use super::*;
    use crate::{
        api::relay::testing::{Asked, StandIn},
        db::repo::asset::Thumb,
    };

    #[test]
    fn only_tmdbs_sizes_and_the_names_of_its_pictures_pass() {
        for size in SIZES {
            assert_eq!(
                picture_type(size, "kqjL17yufvn9OVLyXYpvtyrFfak.jpg"),
                Some("image/jpeg"),
                "{size}"
            );
        }
        // The crops TMDB's site asks for, and Seerr after it.
        for size in [
            "w600_and_h900_bestv2",
            "w300_and_h450_face",
            "w1920_and_h800_multi_faces",
            "w1440_and_h320_multi_faces",
            "w94_and_h141_bestv2",
        ] {
            assert_eq!(picture_type(size, "a1.png"), Some("image/png"), "{size}");
        }
        assert_eq!(picture_type("w500", "x_Y-z.png"), Some("image/png"));

        for (size, file) in [
            // A size TMDB does not cut, or one written otherwise.
            ("w501", "a.jpg"),
            ("w1000", "a.jpg"),
            ("W500", "a.jpg"),
            ("", "a.jpg"),
            ("w500 ", "a.jpg"),
            ("w0500", "a.jpg"),
            ("w600_and_h900_best", "a.jpg"),
            ("w600_and_h900", "a.jpg"),
            ("w600_and_h0900_face", "a.jpg"),
            ("w99999_and_h900_face", "a.jpg"),
            ("w3841_and_h900_face", "a.jpg"),
            ("w_and_h900_face", "a.jpg"),
            ("h600_and_w900_face", "a.jpg"),
            ("w780_filter(duotone,ffffff,bababa)", "a.png"),
            ("..", "a.jpg"),
            // Not a name of TMDB's, or not a picture this server hands on.
            ("w500", "a.svg"),
            ("w500", "a.SVG"),
            ("w500", "a.JPG"),
            ("w500", "a.jpeg"),
            ("w500", "a.html"),
            ("w500", ".jpg"),
            ("w500", "a"),
            ("w500", "../a.jpg"),
            ("w500", "a/b.jpg"),
            ("w500", "a%2Fb.jpg"),
            ("w500", "a.b.jpg"),
            ("w500", "a b.jpg"),
            ("w500", "a\\b.jpg"),
            ("w500", &format!("{}.jpg", "a".repeat(65))),
        ] {
            assert_eq!(picture_type(size, file), None, "{size} {file}");
        }
    }

    /// A stand-in for image.tmdb.org: one picture at one size, with a tag;
    /// one it answers in a type it should not; one larger than the gate
    /// hands on, and one that says nothing of its length; a size it does
    /// not cut; a failure. What it was asked, kept.
    async fn pictures() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let noted = asked.clone();
        let app = Router::new()
            .route(
                "/t/p/w500/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                get(|headers: HeaderMap| async move {
                    if headers
                        .get(header::IF_NONE_MATCH)
                        .is_some_and(|tag| tag == "\"tmdb-tag\"")
                    {
                        return (StatusCode::NOT_MODIFIED, [(header::ETAG, "\"tmdb-tag\"")])
                            .into_response();
                    }
                    (
                        [
                            (header::CONTENT_TYPE, "image/jpeg"),
                            (header::ETAG, "\"tmdb-tag\""),
                        ],
                        vec![0xff_u8, 0xd8, 0xff, 0xe0, 1, 2, 3],
                    )
                        .into_response()
                }),
            )
            .route(
                "/t/p/original/sniffed.png",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/html")],
                        "<script>alert(1)</script>",
                    )
                }),
            )
            .route(
                "/t/p/original/large.jpg",
                get(|| async { vec![0_u8; 4096] }),
            )
            .route(
                "/t/p/original/streamed.jpg",
                get(|| async {
                    let chunks = futures::stream::iter(
                        (0..8).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![0_u8; 512]))),
                    );
                    Body::from_stream(chunks)
                }),
            )
            .route(
                "/t/p/w600_and_h900_bestv2/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
                get(|| async { StatusCode::BAD_REQUEST }),
            )
            .route(
                "/t/p/original/failing.jpg",
                get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
            )
            .fallback(|| async { StatusCode::NOT_FOUND })
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let noted = noted.clone();
                    async move {
                        noted.lock().unwrap().push(format!(
                            "{} {}",
                            request.method(),
                            request.uri().path()
                        ));
                        next.run(request).await
                    }
                },
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let at = format!("http://{}/t/p", listener.local_addr().expect("its address"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (at, asked)
    }

    async fn handed_on(
        from: &str,
        method: Method,
        size: &str,
        file: &str,
        asked: &[(header::HeaderName, &str)],
        most: u64,
    ) -> Response {
        let mut headers = HeaderMap::new();
        for (name, value) in asked {
            headers.insert(name.clone(), HeaderValue::from_str(value).unwrap());
        }
        let content_type = picture_type(size, file).expect("a picture of TMDB's");
        let place = crate::cache::Gate::new(1, Duration::from_millis(1))
            .take()
            .await
            .expect("a place");
        relay(
            &reqwest::Client::new(),
            from,
            &method,
            size,
            file,
            content_type,
            &headers,
            most,
            place,
        )
        .await
    }

    #[tokio::test]
    async fn a_picture_not_kept_is_handed_on_as_it_comes() {
        let (at, asked) = pictures().await;
        let most = crate::media::file::MAX_IMAGE_BYTES;
        let file = "kqjL17yufvn9OVLyXYpvtyrFfak.jpg";

        let answer = handed_on(&at, Method::GET, "w500", file, &[], most).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(answer.headers()[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(answer.headers()[header::ETAG], "\"tmdb-tag\"");
        assert_eq!(answer.headers()[header::CACHE_CONTROL], KEPT_FOR);
        assert_eq!(answer.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], &[0xff, 0xd8, 0xff, 0xe0, 1, 2, 3]);

        // Asked whether it is there, without its bytes.
        let answer = handed_on(&at, Method::HEAD, "w500", file, &[], most).await;
        assert_eq!(answer.status(), StatusCode::OK);
        let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.is_empty());

        // Held already: unchanged, said by TMDB's own tag.
        let answer = handed_on(
            &at,
            Method::GET,
            "w500",
            file,
            &[(header::IF_NONE_MATCH, "\"tmdb-tag\"")],
            most,
        )
        .await;
        assert_eq!(answer.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(answer.headers()[header::ETAG], "\"tmdb-tag\"");

        // Served as the type its name says, whatever TMDB's answer said.
        let answer = handed_on(&at, Method::GET, "original", "sniffed.png", &[], most).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(answer.headers()[header::CONTENT_TYPE], "image/png");

        // No such picture, or not at that size: TMDB's word, as a 404.
        for (size, file) in [
            ("original", "absent.jpg"),
            ("w600_and_h900_bestv2", "kqjL17yufvn9OVLyXYpvtyrFfak.jpg"),
        ] {
            let answer = handed_on(&at, Method::GET, size, file, &[], most).await;
            assert_eq!(answer.status(), StatusCode::NOT_FOUND, "{size} {file}");
        }
        // TMDB failing, or a picture larger than one this server keeps.
        let answer = handed_on(&at, Method::GET, "original", "failing.jpg", &[], most).await;
        assert_eq!(answer.status(), StatusCode::BAD_GATEWAY);
        let answer = handed_on(&at, Method::GET, "original", "large.jpg", &[], 1024).await;
        assert_eq!(answer.status(), StatusCode::BAD_GATEWAY);
        // One that said nothing of its length is cut off on the way.
        let answer = handed_on(&at, Method::GET, "original", "streamed.jpg", &[], 1024).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert!(
            axum::body::to_bytes(answer.into_body(), usize::MAX)
                .await
                .is_err()
        );

        // Each asked once, at the size and the name asked, and nothing else.
        let asked = asked.lock().unwrap().clone();
        assert_eq!(
            asked,
            [
                format!("GET /t/p/w500/{file}"),
                format!("HEAD /t/p/w500/{file}"),
                format!("GET /t/p/w500/{file}"),
                "GET /t/p/original/sniffed.png".to_string(),
                "GET /t/p/original/absent.jpg".to_string(),
                format!("GET /t/p/w600_and_h900_bestv2/{file}"),
                "GET /t/p/original/failing.jpg".to_string(),
                "GET /t/p/original/large.jpg".to_string(),
                "GET /t/p/original/streamed.jpg".to_string(),
            ]
        );
    }

    /// TMDB as the relay's stand-in answers it: nothing at all.
    fn nothing(_: &Asked) -> (StatusCode, serde_json::Value) {
        (StatusCode::NOT_FOUND, serde_json::json!({}))
    }

    /// One of TMDB's posters, kept here as the media worker keeps one: in
    /// the store, and noted in the index under the address TMDB gave. Its
    /// key and its bytes; `shade` makes them a picture of its own.
    async fn kept(state: &AppState, file: &str, shade: u8) -> (String, Vec<u8>) {
        let bytes = vec![0xff, 0xd8, 0xff, 0xe0, shade, 0x4a, 0x46, 0x49, 0x46];
        let (key, _) = crate::media::file::key_for(&bytes, "jpg");
        state
            .media
            .store()
            .expect("a store")
            .put(&key, Bytes::from(bytes.clone()), "image/jpeg")
            .await
            .expect("kept");
        state.media.remember(
            &format!("https://image.tmdb.org/t/p/original/{file}"),
            &key,
            Thumb::None,
            "image/jpeg",
        );
        (key, bytes)
    }

    async fn asked_of(state: &AppState, size: &str, file: &str) -> Response {
        picture(
            State(state.clone()),
            Method::GET,
            Path((size.to_string(), file.to_string())),
            HeaderMap::new(),
        )
        .await
    }

    #[test]
    fn a_picture_of_another_sources_is_named_by_its_own_key_alone() {
        let hash = "0123456789abcdef".repeat(4);
        for ext in ["jpg", "png"] {
            let key = format!("{hash}.{ext}");
            assert_eq!(kept_name(&key), Some(format!("/{key}")));
            // And the gate takes the name as one of a picture's.
            assert!(picture_type("w500", &key).is_some());
        }
        for key in [
            format!("{hash}.webp"),
            format!("{hash}-480.webp"),
            format!("{hash}-square.jpg"),
            format!("{hash}.m4a"),
            "kqjL17yufvn9OVLyXYpvtyrFfak.jpg".to_string(),
            format!("{}.jpg", hash.to_uppercase()),
        ] {
            assert_eq!(kept_name(&key), None, "{key}");
        }
    }

    #[tokio::test]
    async fn a_picture_of_another_sources_kept_here_is_served_by_its_key() {
        let stand_in = StandIn::start(nothing).await;
        let (state, dir) = testing::server_keeping_media(&stand_in, None).await;
        let bytes = vec![0xff, 0xd8, 0xff, 0xe0, 93, 0x4a, 0x46, 0x49, 0x46];
        let (key, _) = crate::media::file::key_for(&bytes, "jpg");
        state
            .media
            .store()
            .expect("a store")
            .put(&key, Bytes::from(bytes.clone()), "image/jpeg")
            .await
            .expect("kept");
        state.media.remember(
            "https://assets.fanart.tv/fanart/movies/1/movieposter/a.jpg",
            &key,
            Thumb::None,
            "image/jpeg",
        );
        for size in ["original", "w342"] {
            let answer = asked_of(&state, size, &key).await;
            assert_eq!(answer.status(), StatusCode::OK, "{size}");
            let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(&body[..], &bytes[..]);
        }
        // A name of the kind kept nowhere: none, and TMDB, which files
        // nothing so, is not asked.
        let missing = format!("{}.jpg", "f".repeat(64));
        let answer = asked_of(&state, "w500", &missing).await;
        assert_eq!(answer.status(), StatusCode::NOT_FOUND);
        assert!(
            stand_in
                .asked_at(&format!("/t/p/w500/{missing}"))
                .is_empty()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_picture_kept_here_is_served_from_the_store_whatever_the_size() {
        let stand_in = StandIn::start(nothing).await;
        let (state, dir) = testing::server_keeping_media(&stand_in, None).await;
        let file = "gateKeptPoster7Qx.jpg";
        let (key, bytes) = kept(&state, file, 71).await;

        for size in ["original", "w500", "w92", "w600_and_h900_bestv2"] {
            let answer = asked_of(&state, size, file).await;
            assert_eq!(answer.status(), StatusCode::OK, "{size}");
            assert_eq!(answer.headers()[header::CONTENT_TYPE], "image/jpeg");
            assert_eq!(
                answer.headers()[header::ETAG],
                format!("\"{key}\"").as_str()
            );
            let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(&body[..], &bytes[..], "{size}");
        }

        // What is not one of TMDB's pictures is refused before anything is
        // looked for.
        for (size, file) in [
            ("w501", file),
            ("w500", "../../etc/passwd"),
            ("w500", "a.svg"),
        ] {
            let answer = asked_of(&state, size, file).await;
            assert_eq!(answer.status(), StatusCode::BAD_REQUEST, "{size} {file}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One request to every surface, under its own guard, as the server
    /// mounts them, from an address: its answer.
    async fn from(state: &AppState, address: [u8; 4], uri: &str) -> Response {
        let (router, _) = crate::api::build(state.clone());
        let mut request = axum::http::Request::get(uri)
            .body(Body::empty())
            .expect("a request");
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((address, 40_000))));
        router
            .with_state(state.clone())
            .oneshot(request)
            .await
            .expect("an answer")
    }

    #[tokio::test]
    async fn the_gate_takes_the_address_list_where_the_relay_takes_a_key() {
        let stand_in = StandIn::start(nothing).await;
        let (state, dir) = testing::server_keeping_media(&stand_in, None).await;
        let file = "gateGuardedPoster3Zr.jpg";
        kept(&state, file, 72).await;
        assert_eq!(
            crate::api::relay::policy(&state, crate::config::Surface::Tmdb),
            crate::config::SurfacePolicy::ApiKey
        );

        // A picture's address carries no key: an address on the list is
        // let through without one.
        let answer = from(&state, [127, 0, 0, 1], &format!("/tmdb-images/w500/{file}")).await;
        assert_eq!(answer.status(), StatusCode::OK);
        // One off it is not.
        let answer = from(
            &state,
            [203, 0, 113, 9],
            &format!("/tmdb-images/w500/{file}"),
        )
        .await;
        assert_eq!(answer.status(), StatusCode::FORBIDDEN);
        // The relay itself still asks for a key.
        let answer = from(&state, [127, 0, 0, 1], "/3/configuration").await;
        assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);

        // Switched off with the relay.
        crate::api::relay::testing::choose(&state, "api.tmdb", "false").await;
        let answer = from(&state, [127, 0, 0, 1], &format!("/tmdb-images/w500/{file}")).await;
        assert_eq!(answer.status(), StatusCode::SERVICE_UNAVAILABLE);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_gate_lets_in_without_counting_and_counts_whom_it_turns_away() {
        let stand_in = StandIn::start(nothing).await;
        let (state, dir) = testing::server_keeping_media_as(&stand_in, None, |config| {
            config.security.rate_limit_per_minute = 2;
        })
        .await;
        let file = "gateUnlimitedPoster5Kq.jpg";
        kept(&state, file, 73).await;
        let picture = format!("/tmdb-images/w500/{file}");
        let calls = state.calls.read(crate::config::Api::Tmdb);
        let statuses = |answers: Vec<Response>| -> Vec<StatusCode> {
            answers.iter().map(Response::status).collect()
        };

        // An address the gate lets in asks for as many pictures as a scan
        // of its library takes, past the limit of its requests: it has a
        // quota of pictures of its own, ten times as high.
        let mut answers = Vec::new();
        for _ in 0..6 {
            answers.push(from(&state, [127, 0, 0, 1], &picture).await);
        }
        assert_eq!(statuses(answers), [StatusCode::OK; 6]);
        // None of them a call to TMDB's API, on the access page or in the
        // metrics.
        assert_eq!(state.calls.read(crate::config::Api::Tmdb), calls);
        assert_eq!(
            crate::metrics::SURFACES[crate::metrics::surface_of(&picture)],
            "media"
        );

        // The relay itself is held to the limit, from the same address.
        let mut answers = Vec::new();
        for _ in 0..3 {
            answers.push(from(&state, [127, 0, 0, 1], "/3/configuration").await);
        }
        assert_eq!(
            statuses(answers),
            [
                StatusCode::UNAUTHORIZED,
                StatusCode::UNAUTHORIZED,
                StatusCode::TOO_MANY_REQUESTS
            ]
        );

        // An address the gate turns away is counted, and once it has asked
        // too often, told to wait.
        let mut answers = Vec::new();
        for _ in 0..3 {
            answers.push(from(&state, [203, 0, 113, 9], &picture).await);
        }
        let waited = answers[2].headers().get(header::RETRY_AFTER).is_some();
        assert_eq!(
            statuses(answers),
            [
                StatusCode::FORBIDDEN,
                StatusCode::FORBIDDEN,
                StatusCode::TOO_MANY_REQUESTS
            ]
        );
        assert!(waited);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One request to a router already built, from an address: the count a
    /// router keeps is its own, so a test of it builds one.
    async fn routed(router: &Router, address: [u8; 4], uri: &str) -> Response {
        let mut request = axum::http::Request::get(uri)
            .body(Body::empty())
            .expect("a request");
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((address, 40_000))));
        router.clone().oneshot(request).await.expect("an answer")
    }

    /// A caller the gate lets in is counted, against a quota of pictures of
    /// its own, ten times the surfaces': told to wait once that is spent, with
    /// the quota, and with no other address the worse for it.
    #[tokio::test]
    async fn a_scan_is_held_to_a_quota_of_pictures_ten_times_the_requests() {
        let stand_in = StandIn::start(nothing).await;
        let (state, dir) = testing::server_keeping_media_as(&stand_in, None, |config| {
            config.security.rate_limit_per_minute = 1;
        })
        .await;
        let file = "gatePacedPoster9Lm.jpg";
        kept(&state, file, 74).await;
        let picture = format!("/tmdb-images/w500/{file}");
        let (router, _) = crate::api::build(state.clone());
        let router = router.with_state(state.clone());

        for n in 0..10 {
            let answer = routed(&router, [127, 0, 0, 1], &picture).await;
            assert_eq!(answer.status(), StatusCode::OK, "picture {n}");
        }
        let answer = routed(&router, [127, 0, 0, 1], &picture).await;
        assert_eq!(answer.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(answer.headers().get(header::RETRY_AFTER).is_some());
        assert_eq!(
            answer.headers()["ratelimit-policy"],
            "\"default\";q=10;w=60"
        );

        // Another address the gate lets in has its own count.
        let answer = routed(&router, [10, 0, 0, 5], &picture).await;
        assert_eq!(answer.status(), StatusCode::OK);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With limiting off, the pictures are not limited either.
    #[tokio::test]
    async fn no_quota_of_pictures_where_there_is_none_of_requests() {
        let stand_in = StandIn::start(nothing).await;
        let (state, dir) = testing::server_keeping_media_as(&stand_in, None, |config| {
            config.security.rate_limit_per_minute = 0;
        })
        .await;
        let file = "gateFreePoster2Wn.jpg";
        kept(&state, file, 75).await;
        let picture = format!("/tmdb-images/w500/{file}");
        let (router, _) = crate::api::build(state.clone());
        let router = router.with_state(state.clone());
        for _ in 0..30 {
            let answer = routed(&router, [127, 0, 0, 1], &picture).await;
            assert_eq!(answer.status(), StatusCode::OK);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pictures handed on from TMDB are so many at once, and no more: the
    /// next waits for its turn and is turned away when it does not come. One
    /// the store keeps is none of that.
    #[tokio::test]
    async fn pictures_are_handed_on_a_few_at_a_time() {
        let stand_in = StandIn::start(nothing).await;
        let (state, dir) = testing::server_keeping_media(&stand_in, None).await;
        let kept_file = "gateKeptPoster4Hs.jpg";
        kept(&state, kept_file, 76).await;
        state
            .caches
            .pictures
            .wait_at_most(Duration::from_millis(30));
        let mut held = Vec::new();
        while state.caches.pictures.free() > 0 {
            held.push(state.caches.pictures.take().await.expect("a place"));
        }

        // Not kept here, so asked of TMDB: every place is taken.
        let answer = asked_of(&state, "w500", "kqjL17yufvn9OVLyXYpvtyrFfak.jpg").await;
        assert_eq!(answer.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(answer.headers().get(header::RETRY_AFTER).is_some());

        // One kept here takes no place.
        let answer = asked_of(&state, "w500", kept_file).await;
        assert_eq!(answer.status(), StatusCode::OK);

        let _ = std::fs::remove_dir_all(&dir);
        drop(held);
    }

    /// A picture holds its place until the last of its bytes is read, and
    /// gives it back then — or when its caller goes without them.
    #[tokio::test]
    async fn a_picture_holds_its_place_until_the_last_byte() {
        let (at, _) = pictures().await;
        let gate = crate::cache::Gate::new(2, Duration::from_millis(30));
        let file = "kqjL17yufvn9OVLyXYpvtyrFfak.jpg";
        let content_type = picture_type("w500", file).expect("a picture of TMDB's");
        let relayed = async |method: Method, size: &str, file: &str| {
            let place = gate.take().await.expect("a place");
            relay(
                &reqwest::Client::new(),
                &at,
                &method,
                size,
                file,
                content_type,
                &HeaderMap::new(),
                crate::media::file::MAX_IMAGE_BYTES,
                place,
            )
            .await
        };

        let answer = relayed(Method::GET, "w500", file).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(gate.free(), 1, "held while the bytes are on their way");
        let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], &[0xff, 0xd8, 0xff, 0xe0, 1, 2, 3]);
        assert_eq!(gate.free(), 2, "given back with the last of them");

        // Gone without reading them: given back with the body.
        let answer = relayed(Method::GET, "w500", file).await;
        assert_eq!(gate.free(), 1);
        drop(answer);
        assert_eq!(gate.free(), 2);

        // Nothing to hand on, nothing held: a question, a refusal.
        let answer = relayed(Method::HEAD, "w500", file).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(gate.free(), 2);
        let answer = relayed(Method::GET, "original", "absent.jpg").await;
        assert_eq!(answer.status(), StatusCode::NOT_FOUND);
        assert_eq!(gate.free(), 2);
    }

    #[tokio::test]
    async fn the_gate_is_given_only_when_this_server_can_serve_the_pictures() {
        let stand_in = StandIn::start(nothing).await;

        // A store, a public address: the choice decides.
        let (state, dir) =
            testing::server_keeping_media(&stand_in, Some("https://ams.example/")).await;
        assert_eq!(base(&state), None, "TMDB's own until chosen");
        testing::choose(&state, "server").await;
        assert_eq!(
            base(&state).as_deref(),
            Some("https://ams.example/tmdb-images/")
        );
        testing::choose(&state, "tmdb").await;
        assert_eq!(base(&state), None);
        let _ = std::fs::remove_dir_all(&dir);

        // No public address to give: TMDB's own, whatever was chosen.
        let (state, dir) = testing::server_keeping_media(&stand_in, None).await;
        testing::choose(&state, "server").await;
        assert_eq!(base(&state), None);
        let _ = std::fs::remove_dir_all(&dir);

        // No store to keep pictures in: the same.
        let state = crate::api::relay::testing::server(&stand_in).await;
        testing::choose(&state, "server").await;
        assert!(!state.media.is_on());
        assert_eq!(base(&state), None);
    }
}

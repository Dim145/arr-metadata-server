//! The media kept, as the administration sees them: how the store stands,
//! what could not be fetched, and a work's own — with a way to put a
//! picture or a theme on a work by hand, and to take one away.

use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{
        self,
        asset::{Asset, Kind},
        audit::Action,
    },
    domain::{
        CoverType, MediaItem,
        fields::{Child, Scope},
    },
    error::{AppError, AppResult},
    media::file,
    service,
    state::AppState,
};

const TAG: &str = super::items::TAG;

/// The most an upload may weigh, whatever it is: the audio bound, and a
/// little for the form around it.
const UPLOAD_BYTES: usize = file::MAX_AUDIO_BYTES as usize + 64 * 1024;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(troubled))
        .routes(routes!(retry))
        .routes(routes!(retry_all))
        .routes(routes!(reset))
        .routes(routes!(work_media))
        .routes(routes!(by_work))
        .routes(routes!(forget))
        .merge(
            OpenApiRouter::new()
                .routes(routes!(upload))
                .layer(axum::extract::DefaultBodyLimit::max(UPLOAD_BYTES)),
        )
}

/// Where the media are kept, as `AMS_MEDIA_STORAGE` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: utoipa keys schemas on the leaf name alone, and a
// `Backend` could be anything's.
#[schema(as = MediaBackend)]
pub enum Backend {
    /// Nothing is kept: every picture stays a link to its provider.
    Off,
    /// Files under `AMS_MEDIA_DIR`, served by this server.
    Filesystem,
    /// An S3 bucket, served through this server or by the bucket itself.
    S3,
}

impl From<crate::config::MediaStorage> for Backend {
    fn from(storage: crate::config::MediaStorage) -> Self {
        match storage {
            crate::config::MediaStorage::Off => Self::Off,
            crate::config::MediaStorage::Filesystem => Self::Filesystem,
            crate::config::MediaStorage::S3 => Self::S3,
        }
    }
}

/// How the store stands.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaStatus {
    pub backend: Backend,
    /// Whether the media are fetched as works are stored.
    pub storing: bool,
    /// `proxy` or `redirect`: through this server, or sent to the bucket.
    pub serve: String,
    pub people: bool,
    pub audio: bool,
    /// Whether the served addresses can be followed from elsewhere.
    pub public_url: bool,
    pub counts: repo::asset::Counts,
    /// The copies made of the pictures kept: what each kind weighs.
    pub variants: repo::asset::VariantCounts,
    /// The pictures kept that this version has not prepared — no
    /// placeholder, no first copy — which the task `media.variants` does.
    pub unprepared: i64,
    /// Whether the night's task makes AVIF copies (`media.avif`).
    pub avif: bool,
    /// Whether a fetch or a sweep is under way.
    pub fetching: bool,
    pub sweeping: bool,
    /// Whether the pictures are being prepared, or their AVIF copies made.
    pub preparing: bool,
}

/// How the media store stands.
#[utoipa::path(
    get, path = "/media/status", tag = TAG,
    operation_id = "media_status",
    responses(
        (status = 200, body = MediaStatus),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn status(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<MediaStatus>> {
    identity.require_admin()?;
    Ok(Json(MediaStatus {
        backend: state.media.config.storage.into(),
        storing: state.flag("media.store", true),
        serve: state.text("media.serve").unwrap_or_else(|| "proxy".into()),
        people: state.flag("media.people", true),
        audio: state.flag("media.audio", true),
        public_url: state.config.server.public_url.is_some(),
        counts: repo::asset::counts(&state.db).await?,
        variants: repo::asset::variant_counts(&state.db).await?,
        unprepared: repo::asset::unprepared_count(&state.db, crate::media::variants::PREPARED)
            .await?,
        avif: state.flag("media.avif", false),
        fetching: crate::media::worker::is_storing(),
        sweeping: crate::media::worker::is_sweeping(),
        preparing: crate::jobs::media::is_busy(),
    }))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TroubledQuery {
    pub limit: Option<i64>,
}

/// What could not be fetched: given up on, or put off after a failure.
#[utoipa::path(
    get, path = "/media/troubled", tag = TAG,
    operation_id = "media_troubled",
    params(TroubledQuery),
    responses(
        (status = 200, body = Vec<Asset>),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn troubled(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<TroubledQuery>,
) -> AppResult<Json<Vec<Asset>>> {
    identity.require_admin()?;
    Ok(Json(
        repo::asset::troubled(&state.db, query.limit.unwrap_or(100)).await?,
    ))
}

/// Ask again for one that could not be fetched.
#[utoipa::path(
    post, path = "/media/assets/{id}/retry", tag = TAG,
    operation_id = "media_retry",
    params(("id" = String, Path)),
    responses(
        (status = 202, description = "In line again"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such asset, or it is stored already"),
    ),
)]
async fn retry(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    identity.require_admin()?;
    if !repo::asset::retry(&state.db, &id).await? {
        return Err(AppError::NotFound);
    }
    state.media.notify.notify_one();
    Ok(StatusCode::ACCEPTED)
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Retried {
    pub retried: u64,
}

/// Ask again for everything given up on.
#[utoipa::path(
    post, path = "/media/retry", tag = TAG,
    operation_id = "media_retry_all",
    responses(
        (status = 200, body = Retried),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn retry_all(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Retried>> {
    identity.require_admin()?;
    let retried = repo::asset::retry_troubled(&state.db).await?;
    if retried > 0 {
        state.media.notify.notify_one();
    }
    Ok(Json(Retried { retried }))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Forgotten {
    pub forgotten: u64,
}

/// Forget every copy fetched, without deleting a file — the uploads stay,
/// their row being the only record of them: for a store moved to
/// another place, or one whose files are gone. Every work points at its
/// providers again until "store everything" runs; the next sweep removes
/// whatever files no row names.
#[utoipa::path(
    post, path = "/media/reset", tag = TAG,
    operation_id = "media_reset",
    responses(
        (status = 200, body = Forgotten),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn reset(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
) -> AppResult<Json<Forgotten>> {
    identity.require_admin()?;
    // Not under a fetch, and none started until it is done: the rows a
    // fetch is marking would come back as copies the index alone knows of.
    let _fetching_held_off = crate::media::worker::hold_off_fetching(&state).await?;
    let forgotten = repo::asset::delete_fetched(&state.db).await?;
    // The index again from what is left: the uploads.
    state.media.load_index(&state.db).await?;
    state.caches.invalidate_all().await;
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::MediaRemoved,
            target: None,
            detail: Some(&format!("every copy forgotten ({forgotten} rows)")),
        },
    )
    .await;
    Ok(Json(Forgotten { forgotten }))
}

/// A work's media, and whether anything is kept at all.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkMedia {
    /// Whether a store is configured: without one, nothing can be uploaded
    /// and nothing is fetched.
    pub store: bool,
    pub media: Vec<WorkMedium>,
    /// What they come to: how many are kept, waiting, given up on, and what
    /// those kept weigh with their copies.
    pub totals: repo::asset::WorkTotals,
}

/// What is kept of one address a work points at: where its asset stands,
/// or that it has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MediumStatus {
    /// Waiting to be fetched, or to be tried again.
    Pending,
    Stored,
    /// Given up on, until somebody asks again.
    Failed,
    /// Nothing has been asked of it: the store is off, or the address is not
    /// one it fetches.
    Absent,
}

impl From<repo::asset::Status> for MediumStatus {
    fn from(status: repo::asset::Status) -> Self {
        match status {
            repo::asset::Status::Pending => Self::Pending,
            repo::asset::Status::Stored => Self::Stored,
            repo::asset::Status::Failed => Self::Failed,
        }
    }
}

/// One address a work points at, and what is kept of it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkMedium {
    /// The provider's address, or `upload:<id>`.
    pub origin: String,
    /// Where it is read from now: the copy kept, or the provider.
    pub url: String,
    pub kind: Kind,
    /// `stored`, `pending`, `failed` — or `absent`, when nothing has been
    /// asked of it: the store is off, or the address is not one it fetches.
    pub status: MediumStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempts: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded_by: Option<String>,
}

/// Every address a work points at, and what is kept of each.
#[utoipa::path(
    get, path = "/items/{id}/media", tag = TAG,
    operation_id = "media_work_media",
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, body = WorkMedia),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
    ),
)]
async fn work_media(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<WorkMedia>> {
    identity.require_write()?;
    let mut item = repo::item::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    repo::item::load_children(&state.db, &mut item).await?;
    // What an address answered is an administrator's to read; a writer is
    // told the kind of trouble, since any address can be typed in.
    let admin = identity.is_admin();

    let origins = addresses_of(&item);
    let known: HashMap<String, Asset> = repo::asset::by_origins(
        &state.db,
        &origins.iter().map(|(o, _)| o.clone()).collect::<Vec<_>>(),
    )
    .await?
    .into_iter()
    .map(|a| (a.origin.clone(), a))
    .collect();

    let media = origins
        .into_iter()
        .map(|(origin, kind)| match known.get(&origin) {
            Some(asset) => WorkMedium {
                url: state.media.localized(&origin),
                origin,
                kind: asset.kind,
                status: asset.status.into(),
                asset_id: Some(asset.id.clone()),
                bytes: asset.bytes,
                width: asset.width,
                height: asset.height,
                attempts: Some(asset.attempts),
                error: asset.error.as_deref().map(|error| {
                    if admin {
                        error.to_string()
                    } else {
                        crate::media::fetch::reason_for_writers(error).to_string()
                    }
                }),
                uploaded_by: asset.uploaded_by.clone(),
            },
            None => WorkMedium {
                url: origin.clone(),
                origin,
                kind,
                status: MediumStatus::Absent,
                asset_id: None,
                bytes: None,
                width: None,
                height: None,
                attempts: None,
                error: None,
                uploaded_by: None,
            },
        })
        .collect();
    Ok(Json(WorkMedia {
        store: state.media.is_on(),
        media,
        totals: repo::asset::work_totals(&state.db, &id).await?,
    }))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ByWorkQuery {
    /// `space` (the default): the heaviest first, copies included. `count`:
    /// the most media kept first. `failed`: the most given up on first.
    pub sort: Option<String>,
    /// How many works, fifty when absent, two hundred at the most.
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// The works, by what their media come to.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaByWork {
    pub works: Vec<repo::asset::WorkLine>,
    /// How many works point at any medium.
    pub total: i64,
}

/// The works by what their media come to: how many each keeps, how many
/// wait or were given up on, and what those kept weigh with their copies —
/// the heaviest first, or the most kept, or the most failed. A picture two
/// works share is counted for each.
#[utoipa::path(
    get, path = "/media/works", tag = TAG,
    operation_id = "media_by_work",
    params(ByWorkQuery),
    responses(
        (status = 200, body = MediaByWork),
        (status = 400, description = "An order that is none"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn by_work(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ByWorkQuery>,
) -> AppResult<Json<MediaByWork>> {
    identity.require_admin()?;
    let order = match query.sort.as_deref().map(str::trim) {
        None | Some("" | "space") => repo::asset::WorkOrder::Space,
        Some("count") => repo::asset::WorkOrder::Count,
        Some("failed") => repo::asset::WorkOrder::Failed,
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "{other} is no order: space, count or failed"
            )));
        }
    };
    let (works, total) = repo::asset::by_work(
        &state.db,
        order,
        query.limit.unwrap_or(50),
        query.offset.unwrap_or(0),
    )
    .await?;
    Ok(Json(MediaByWork { works, total }))
}

/// Delete the uploads among these addresses, once the row or the lock that
/// pointed at it is gone — unless something else still names it: a picture
/// of the work, a lock of another field — a season's poster, an episode's
/// still, a theme — or of another work. Quiet — what pointed at it is gone
/// already, and the sweep removes whatever this could not.
pub async fn forget_uploads<'a>(state: &AppState, origins: impl IntoIterator<Item = &'a str>) {
    for origin in origins {
        if !origin.starts_with("upload:") {
            continue;
        }
        let asset = match repo::asset::by_origin(&state.db, origin).await {
            Ok(Some(asset)) => asset,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(origin, error = %e, "could not look an upload up");
                continue;
            }
        };
        match remove_asset(state, &asset, Removal::UnlessNamed).await {
            Ok(true) => {}
            Ok(false) => tracing::debug!(origin, "an upload is still named elsewhere: kept"),
            Err(e) => tracing::warn!(origin, error = %e, "could not delete an upload"),
        }
    }
}

/// Every address a work's rows hold, once each, in the order the editor
/// shows them: the work's pictures, its seasons', its theme, its cast's,
/// its relations', its episodes'.
fn addresses_of(item: &MediaItem) -> Vec<(String, Kind)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut add = |url: &str, kind: Kind| {
        if seen.insert(url.to_string()) {
            out.push((url.to_string(), kind));
        }
    };
    for image in &item.images {
        add(&image.url, Kind::Image);
    }
    for season in &item.seasons {
        for image in &season.images {
            add(&image.url, Kind::Image);
        }
    }
    if let Some(theme) = &item.theme_music {
        add(theme, Kind::Audio);
    }
    for credit in &item.credits {
        if let Some(url) = &credit.image {
            add(url, Kind::Image);
        }
    }
    for relation in &item.relations {
        if let Some(url) = &relation.image {
            add(url, Kind::Image);
        }
    }
    for episode in &item.episodes {
        if let Some(url) = &episode.image {
            add(url, Kind::Image);
        }
    }
    out
}

/// What an upload became.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Uploaded {
    pub asset_id: String,
    pub origin: String,
    pub url: String,
    pub item: MediaItem,
}

/// Put a picture or a theme on a work, from a file.
///
/// A form with `file`, and: `kind` (`image`, the default, or `theme`);
/// for a picture, `coverType` (`poster`, `fanart`, `banner`, `clearlogo`,
/// `clearart`, `landscape`, `keyart`, `discart`, `characterart`) and `seasonNumber` for a
/// season's, or `episode` as `3x7` for a still. A picture becomes a hand-added one of the work's,
/// which no refresh replaces; a still or a theme locks the field it fills.
/// The file is read for what it is, not for what it is called.
#[utoipa::path(
    post, path = "/items/{id}/media", tag = TAG,
    operation_id = "media_upload",
    params(("id" = String, Path, description = "The work's identifier")),
    request_body(content_type = "multipart/form-data", description = "`file`, and `kind`, `coverType`, `seasonNumber` or `episode`"),
    responses(
        (status = 201, body = Uploaded),
        (status = 400, description = "Not a picture or a sound this server keeps, or a field it does not understand"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work, season or episode"),
        (status = 413, description = "Larger than an upload may be"),
        (status = 503, description = "No media store is configured"),
    ),
)]
async fn upload(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    mut form: Multipart,
) -> AppResult<(StatusCode, Json<Uploaded>)> {
    identity.require_write()?;
    let Some(store) = state.media.store() else {
        return Err(AppError::Disabled {
            code: "media_off",
            message: "no media store is configured; set AMS_MEDIA_STORAGE".into(),
        });
    };
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    // The form, whole: a file and a few words.
    let mut file: Option<Bytes> = None;
    let mut fields: HashMap<String, String> = HashMap::new();
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("the form could not be read: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            let bytes = field.bytes().await.map_err(|e| {
                if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    AppError::PayloadTooLarge("the file is larger than an upload may be".into())
                } else {
                    AppError::BadRequest(format!("the file could not be read: {e}"))
                }
            })?;
            file = Some(bytes);
        } else {
            let text = field
                .text()
                .await
                .map_err(|e| AppError::BadRequest(format!("{name} could not be read: {e}")))?;
            fields.insert(name, text.trim().to_string());
        }
    }
    let Some(bytes) = file.filter(|b| !b.is_empty()) else {
        return Err(AppError::BadRequest("no file was sent".into()));
    };

    let kind = match fields.get("kind").map(String::as_str).unwrap_or("image") {
        "image" => Kind::Image,
        "theme" | "audio" => Kind::Audio,
        other => {
            return Err(AppError::BadRequest(format!(
                "{other} is not a kind of upload"
            )));
        }
    };
    let limit = match kind {
        Kind::Image => file::MAX_IMAGE_BYTES,
        Kind::Audio => file::MAX_AUDIO_BYTES,
    };
    if bytes.len() as u64 > limit {
        return Err(AppError::PayloadTooLarge(format!(
            "the file is larger than the {} MiB a {} may be",
            limit / (1024 * 1024),
            kind.as_str()
        )));
    }
    let inspected = file::inspect(&bytes, kind).map_err(|e| AppError::BadRequest(e.to_string()))?;

    // Where it goes on the work, settled before anything is written.
    enum Place {
        Picture {
            cover_type: CoverType,
            season: Option<i32>,
        },
        Still {
            season: i32,
            episode: i32,
        },
        Theme,
    }
    let place = match kind {
        Kind::Audio => Place::Theme,
        Kind::Image => match fields.get("episode") {
            Some(code) => {
                let (season, episode) = code
                    .split_once(['x', 'X', 'e', 'E'])
                    .and_then(|(s, e)| {
                        Some((
                            s.trim_start_matches(['s', 'S']).parse().ok()?,
                            e.parse().ok()?,
                        ))
                    })
                    .ok_or_else(|| AppError::BadRequest("episode must be written as 3x7".into()))?;
                if !item
                    .episodes
                    .iter()
                    .any(|e| e.season_number == season && e.episode_number == episode)
                {
                    return Err(AppError::NotFound);
                }
                Place::Still { season, episode }
            }
            None => {
                let cover_type: CoverType = fields
                    .get("coverType")
                    .map(String::as_str)
                    .unwrap_or("poster")
                    .parse()
                    .unwrap_or(CoverType::Unknown);
                if cover_type == CoverType::Unknown {
                    return Err(AppError::BadRequest(
                        "coverType is not one this server keeps".into(),
                    ));
                }
                let season = fields
                    .get("seasonNumber")
                    .filter(|s| !s.is_empty())
                    .map(|s| s.parse::<i32>())
                    .transpose()
                    .map_err(|_| AppError::BadRequest("seasonNumber must be a number".into()))?;
                if let Some(n) = season
                    && !item.seasons.iter().any(|s| s.season_number == n)
                {
                    return Err(AppError::NotFound);
                }
                Place::Picture { cover_type, season }
            }
        },
    };

    let asset_id = crate::db::new_id();
    let origin = format!("upload:{asset_id}");
    let (key, sha256) = file::key_for(&bytes, inspected.ext);
    // What the picture is to the work decides the copy made of it now.
    let cover = match &place {
        Place::Picture { cover_type, .. } => Some(*cover_type),
        Place::Still { .. } | Place::Theme => None,
    };
    let kept = crate::media::worker::keep(store, key, sha256, bytes, &inspected, cover)
        .await
        .map_err(|e| AppError::Internal(e.context("could not keep the upload")))?;
    let by = identity.label();
    repo::asset::insert_upload(
        &state.db,
        &repo::asset::Upload {
            id: &asset_id,
            origin: &origin,
            kind,
            stored: kept.as_row(),
            uploaded_by: &by,
            wanted_by: &id,
        },
    )
    .await?;
    crate::media::worker::settle(store, &kept).await;
    crate::media::worker::note_copies(&state, &kept).await;
    state
        .media
        .remember(&origin, &kept.key, kept.thumb, inspected.content_type);
    state.media.describe(&kept.key, kept.look());

    // Each told to the work's journal: a picture added by hand, or the lock
    // a still or a theme fills its field with, over what was served before.
    let detail = match place {
        Place::Picture { cover_type, season } => {
            let mut image = repo::child::blank_image(cover_type, origin.clone());
            image.season_number = season;
            repo::child::add_image(&state.db, &id, &image).await?;
            super::history::write(
                &state,
                &id,
                &by,
                &[super::history::came(
                    Child::of_image(&image).to_string(),
                    super::history::IMAGE,
                    super::history::summary::image(&image),
                )],
            )
            .await;
            match season {
                Some(n) => format!("{} for season {n}", cover_type.as_str()),
                None => cover_type.as_str().to_string(),
            }
        }
        Place::Still { season, episode } => {
            super::overrides::lock_served(
                &state,
                &item,
                Scope::Episode { season, episode },
                "image",
                &serde_json::Value::String(origin.clone()),
                &by,
            )
            .await?;
            format!("still for {season}x{episode}")
        }
        Place::Theme => {
            super::overrides::lock_served(
                &state,
                &item,
                Scope::Item,
                "themeMusic",
                &serde_json::Value::String(origin.clone()),
                &by,
            )
            .await?;
            "theme".to_string()
        }
    };

    state.caches.touched(&id).await;
    state.caches.searches.invalidate_all().await;
    service::listing::after_write(&state, &id).await;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::MediaUploaded,
            target: Some(&id),
            detail: Some(&format!("{detail} ({} bytes)", kept.bytes)),
        },
    )
    .await;

    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok((
        StatusCode::CREATED,
        Json(Uploaded {
            url: state.media.url_for(&kept.key),
            asset_id,
            origin,
            item,
        }),
    ))
}

/// Take a copy away.
///
/// An upload is taken off the work, and deleted unless something else still
/// names it — another work's lock, say. A provider's copy is forgotten and
/// deleted — the work keeps pointing at the provider, and the next refresh
/// fetches it again. Only a copy of the work's own: an address its rows or
/// its locks hold, or an upload made for it.
#[utoipa::path(
    delete, path = "/items/{id}/media/{asset_id}", tag = TAG,
    operation_id = "media_forget",
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("asset_id" = String, Path),
    ),
    responses(
        (status = 204, description = "Gone"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work, or no copy of the work's"),
    ),
)]
async fn forget(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, asset_id)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    identity.require_write()?;
    let mut item = repo::item::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    repo::item::load_children(&state.db, &mut item).await?;
    let asset = repo::asset::get(&state.db, &asset_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // The work's locks that name the copy: by its address, or by the path
    // of its bytes here, as a lock written before locks were filed by their
    // origin holds it.
    let locks = repo::override_field::list(&state.db, &id).await?;
    let names_it = |locked: &&crate::domain::fields::Override| {
        crate::media::ADDRESS_FIELDS.contains(&locked.field.as_str())
            && locked
                .value
                .as_ref()
                .and_then(|v| v.as_str())
                .is_some_and(|v| v == asset.origin || state.media.unlocalize(v) == asset.origin)
    };
    let uploaded = asset.origin.starts_with("upload:");
    let the_works = (uploaded && asset.wanted_by.as_deref() == Some(id.as_str()))
        || addresses_of(&item)
            .iter()
            .any(|(origin, _)| *origin == asset.origin)
        || locks.iter().any(|locked| names_it(&locked));
    if !the_works {
        return Err(AppError::NotFound);
    }

    let removed = if uploaded {
        let by = identity.label();
        // The pictures that were the upload, as the work held them: removed,
        // each told to its journal as a picture taken off by hand.
        let pictures: Vec<_> = item
            .images
            .iter()
            .chain(item.seasons.iter().flat_map(|s| &s.images))
            .filter(|image| image.is_manual && image.url == asset.origin)
            .map(|image| {
                super::history::went(
                    Child::of_image(image).to_string(),
                    super::history::IMAGE,
                    super::history::summary::image(image),
                )
            })
            .collect();
        repo::child::remove_images_by_url(&state.db, &id, &asset.origin).await?;
        super::history::write(&state, &id, &by, &pictures).await;
        // A field that held the upload — a still, a theme, the poster the
        // work or a season was to lead with — holds nothing now, and its
        // journal says what it holds again.
        let filled: Vec<(Scope, &str)> = locks
            .iter()
            .filter(names_it)
            .filter_map(|locked| Some((locked.scope.parse::<Scope>().ok()?, locked.field.as_str())))
            .collect();
        super::overrides::lift_all(&state, &id, &filled, &by).await?;
        remove_asset(&state, &asset, Removal::UnlessNamed).await?
    } else {
        remove_asset(&state, &asset, Removal::Always).await?
    };

    state.caches.touched(&id).await;
    state.caches.searches.invalidate_all().await;
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::MediaRemoved,
            target: Some(&id),
            detail: Some(match (uploaded, removed) {
                (true, true) => "an upload",
                (true, false) => "an upload, taken off the work; named elsewhere, it is kept",
                (false, _) => "a provider's copy",
            }),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// When an asset may be deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// Whatever names it: a provider's copy, which the next refresh fetches
    /// again.
    Always,
    /// Only once nothing names it — no picture, theme or lock of any work:
    /// an upload, which is gone for good once deleted.
    UnlessNamed,
}

/// Delete an asset — its row first, then its bytes unless another row holds
/// them — and say whether it went. An upload goes through here whatever
/// asks, and asks [`repo::asset::delete_unless_referenced`], the one check
/// of whether something still names it.
pub async fn remove_asset(state: &AppState, asset: &Asset, removal: Removal) -> AppResult<bool> {
    let gone = match removal {
        Removal::UnlessNamed => repo::asset::delete_unless_referenced(&state.db, asset).await?,
        Removal::Always => {
            repo::asset::delete(&state.db, &asset.id).await?;
            true
        }
    };
    if !gone {
        return Ok(false);
    }
    state.media.forget(&asset.origin);
    // The row is gone first: a store that fails now leaves a file the
    // sweep removes, never a row that names a file that is not there.
    if let (Some(key), Some(store)) = (&asset.key, state.media.store()) {
        crate::media::worker::delete_files(state, store, key, asset.thumb).await;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, extract::FromRequest, http::Request};
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        config,
        db::repo::change::{Cause, Filter},
        domain::MediaKind,
    };

    /// A change, as these tests read it: why, where, what, what it held and
    /// what it took.
    type Line = (Cause, String, String, Option<Value>, Option<Value>);

    /// The whole server, on a database of its own, keeping its media in
    /// `dir`: what an upload needs somewhere to go.
    async fn server(dir: &std::path::Path) -> AppState {
        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = config::MediaStorage::Filesystem;
        config.media.dir = dir.to_path_buf();
        config.clients = None;
        config.tmdb.api_key = None;
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;

        AppState::bootstrap(config).await.expect("a server")
    }

    /// A picture of a few pixels, of one colour.
    fn png(red: u8) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::from_pixel(4, 6, image::Rgb([red, 30, 30]))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("a picture");
        out
    }

    /// The form an upload is sent as: these words, and the file.
    async fn uploaded(
        state: &AppState,
        id: &str,
        words: &[(&str, &str)],
        file: &[u8],
    ) -> AppResult<(StatusCode, Json<Uploaded>)> {
        let mut body = Vec::new();
        for (name, value) in words {
            body.extend_from_slice(
                format!(
                    "--edge\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        }
        body.extend_from_slice(
            b"--edge\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload\"\r\n\
              Content-Type: application/octet-stream\r\n\r\n",
        );
        body.extend_from_slice(file);
        body.extend_from_slice(b"\r\n--edge--\r\n");
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "multipart/form-data; boundary=edge")
            .body(Body::from(body))
            .expect("a request");
        let form = Multipart::from_request(request, state)
            .await
            .expect("a form");
        upload(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(id.to_string()),
            form,
        )
        .await
    }

    /// An upload that fills a field locks it, over what was served; one that
    /// is a picture is a picture added by hand: each in the work's journal,
    /// and so is taking it away again.
    #[tokio::test]
    async fn an_upload_and_its_removal_are_journaled_as_the_lock_or_the_picture_they_are() {
        let dir = std::env::temp_dir().join(format!("ams-media-{}", crate::db::new_id()));
        let state = server(&dir).await;

        let mut series = MediaItem::empty(MediaKind::Series);
        series.title = "The Wire".into();
        series.slug = "the-wire".into();
        series.external_ids.tmdb = Some(1438);
        let mut season = repo::child::blank_season(1);
        season.is_manual = false;
        let mut episode = repo::child::blank_episode(1, 1);
        episode.is_manual = false;
        episode.title = "The Target".into();
        episode.image = Some("https://images.example/still.jpg".into());
        series.seasons = vec![season];
        series.episodes = vec![episode];
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &series,
                replace_children: true,
            },
        )
        .await
        .expect("stored");

        let (_, Json(still)) = uploaded(&state, &series.id, &[("episode", "1x1")], &png(200))
            .await
            .expect("a still");
        let (_, Json(poster)) = uploaded(&state, &series.id, &[("coverType", "poster")], &png(120))
            .await
            .expect("a poster");
        for asset in [&still.asset_id, &poster.asset_id] {
            forget(
                State(state.clone()),
                Extension(Identity::Anonymous),
                ClientIp::NONE,
                Path((series.id.clone(), asset.clone())),
            )
            .await
            .expect("taken away");
        }

        let mut written = repo::change::list(&state.db, &series.id, &Filter::default(), None, 50)
            .await
            .expect("the journal")
            .entries;
        written.sort_by(|a, b| (a.at.as_str(), a.id.as_str()).cmp(&(b.at.as_str(), b.id.as_str())));
        assert!(
            written
                .iter()
                .all(|c| c.actor.as_deref() == Some("anonymous"))
        );
        let read: Vec<Line> = written
            .into_iter()
            .map(|c| (c.cause, c.scope, c.field, c.before, c.after))
            .collect();
        let provider_still = Some(json!("https://images.example/still.jpg"));
        let picture = format!("image:{}", poster.origin);
        assert_eq!(
            read,
            [
                // The still locked over the source's, and handed back to it.
                (
                    Cause::Lock,
                    "episode:1x1".to_string(),
                    "image".to_string(),
                    provider_still.clone(),
                    Some(json!(still.origin))
                ),
                (
                    Cause::Edit,
                    picture.clone(),
                    "image".to_string(),
                    None,
                    Some(json!({ "coverType": "poster" }))
                ),
                (
                    Cause::Unlock,
                    "episode:1x1".to_string(),
                    "image".to_string(),
                    Some(json!(still.origin)),
                    provider_still
                ),
                (
                    Cause::Edit,
                    picture,
                    "image".to_string(),
                    Some(json!({ "coverType": "poster" })),
                    None
                ),
            ]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod by_work_tests {
    use super::*;
    use crate::{
        db::repo::asset::{Stored, Thumb, Variant, Wanted, WorkOrder, WorkTotals},
        domain::{Image, MediaKind},
    };

    /// A film held here, pointing at these pictures.
    async fn film(state: &AppState, title: &str, urls: &[&str]) -> String {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.id = crate::db::new_id();
        item.title = title.into();
        item.created_at = crate::db::now();
        item.updated_at = crate::db::now();
        item.slug = crate::domain::make_slug(title, None);
        item.images = urls
            .iter()
            .enumerate()
            .map(|(at, url)| Image {
                id: crate::db::new_id(),
                season_number: None,
                cover_type: if at == 0 {
                    CoverType::Poster
                } else {
                    CoverType::Fanart
                },
                url: (*url).to_string(),
                language: None,
                sort_order: at as i32,
                source: None,
                is_manual: false,
            })
            .collect();
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();
        item.id
    }

    /// An address kept, of this weight and hash.
    async fn kept(state: &AppState, origin: &str, bytes: i64, sha: &str) {
        repo::asset::enqueue(
            &state.db,
            &[Wanted {
                origin,
                kind: Kind::Image,
                wanted_by: None,
            }],
        )
        .await
        .unwrap();
        let asset = repo::asset::by_origin(&state.db, origin)
            .await
            .unwrap()
            .unwrap();
        let key = format!("{sha}.jpg");
        repo::asset::mark_stored(
            &state.db,
            &asset.id,
            &Stored {
                key: &key,
                content_type: "image/jpeg",
                bytes,
                sha256: sha,
                width: Some(2),
                height: Some(3),
                thumb: Thumb::Jpeg,
                color: None,
                thumbhash: None,
                prepared: 1,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn the_works_are_ranked_by_what_their_media_weigh() {
        let state = crate::api::native::testing::server().await;
        let heat = film(
            &state,
            "Heat",
            &[
                "https://p/heat.jpg",
                "https://p/heat-bg.jpg",
                "https://p/shared.jpg",
            ],
        )
        .await;
        let ronin = film(
            &state,
            "Ronin",
            &["https://p/ronin.jpg", "https://p/shared.jpg"],
        )
        .await;
        // Nothing kept of this one: not listed.
        film(&state, "Nothing", &["https://p/nothing.jpg"]).await;

        kept(&state, "https://p/heat.jpg", 100, "aa").await;
        kept(&state, "https://p/ronin.jpg", 1_000, "bb").await;
        kept(&state, "https://p/shared.jpg", 10, "cc").await;
        repo::asset::add_variant(
            &state.db,
            &Variant {
                key: "aa-480.webp".into(),
                stem: "aa".into(),
                width: 480,
                height: 720,
                format: "webp".into(),
                bytes: 50,
            },
        )
        .await
        .unwrap();
        // Heat's background could not be fetched.
        repo::asset::enqueue(
            &state.db,
            &[Wanted {
                origin: "https://p/heat-bg.jpg",
                kind: Kind::Image,
                wanted_by: None,
            }],
        )
        .await
        .unwrap();
        let background = repo::asset::by_origin(&state.db, "https://p/heat-bg.jpg")
            .await
            .unwrap()
            .unwrap();
        repo::asset::mark_failed(&state.db, &background.id, "404", None, true)
            .await
            .unwrap();

        let Json(by_space) = by_work(
            State(state.clone()),
            Extension(Identity::Anonymous),
            Query(ByWorkQuery {
                sort: None,
                limit: None,
                offset: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(by_space.total, 2);
        assert_eq!(by_space.works[0].id, ronin);
        assert_eq!(
            by_space.works[0].totals,
            WorkTotals {
                stored: 2,
                pending: 0,
                failed: 0,
                bytes: 1_010,
                copies_bytes: 0,
            }
        );
        assert_eq!(by_space.works[1].title, "Heat");
        assert_eq!(
            by_space.works[1].totals,
            WorkTotals {
                stored: 2,
                pending: 0,
                failed: 1,
                bytes: 110,
                copies_bytes: 50,
            }
        );

        let (by_failed, _) = repo::asset::by_work(&state.db, WorkOrder::Failed, 10, 0)
            .await
            .unwrap();
        assert_eq!(by_failed[0].id, heat);
        let (second_page, total) = repo::asset::by_work(&state.db, WorkOrder::Space, 1, 1)
            .await
            .unwrap();
        assert_eq!((second_page.len(), total), (1, 2));
        assert_eq!(second_page[0].id, heat);

        // One work's own, as its media list gives it.
        assert_eq!(
            repo::asset::work_totals(&state.db, &heat).await.unwrap(),
            by_space.works[1].totals
        );
        assert_eq!(
            repo::asset::work_totals(&state.db, "nobody").await.unwrap(),
            WorkTotals::default()
        );

        // An order that is none; not for a visitor.
        assert!(
            by_work(
                State(state.clone()),
                Extension(Identity::Anonymous),
                Query(ByWorkQuery {
                    sort: Some("weight".into()),
                    limit: None,
                    offset: None,
                }),
            )
            .await
            .is_err()
        );
        assert!(
            by_work(
                State(state.clone()),
                Extension(Identity::Visitor),
                Query(ByWorkQuery {
                    sort: None,
                    limit: None,
                    offset: None,
                }),
            )
            .await
            .is_err()
        );
    }
}

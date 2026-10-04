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
        asset::{Asset, Kind, Status},
        audit::Action,
    },
    domain::{CoverType, MediaItem, fields::Scope},
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
        .routes(routes!(forget))
        .merge(
            OpenApiRouter::new()
                .routes(routes!(upload))
                .layer(axum::extract::DefaultBodyLimit::max(UPLOAD_BYTES)),
        )
}

/// How the store stands.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaStatus {
    /// `off`, `filesystem` or `s3`.
    pub backend: &'static str,
    /// Whether the media are fetched as works are stored.
    pub storing: bool,
    /// `proxy` or `redirect`: through this server, or sent to the bucket.
    pub serve: String,
    pub people: bool,
    pub audio: bool,
    /// Whether the served addresses can be followed from elsewhere.
    pub public_url: bool,
    pub counts: repo::asset::Counts,
    /// Whether a fetch or a sweep is under way.
    pub fetching: bool,
    pub sweeping: bool,
}

/// How the media store stands.
#[utoipa::path(
    get, path = "/media/status", tag = TAG,
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
        backend: match state.media.config.storage {
            crate::config::MediaStorage::Off => "off",
            crate::config::MediaStorage::Filesystem => "filesystem",
            crate::config::MediaStorage::S3 => "s3",
        },
        storing: state.flag("media.store", true),
        serve: state.text("media.serve").unwrap_or_else(|| "proxy".into()),
        people: state.flag("media.people", true),
        audio: state.flag("media.audio", true),
        public_url: state.config.server.public_url.is_some(),
        counts: repo::asset::counts(&state.db).await?,
        fetching: crate::media::worker::is_storing(),
        sweeping: crate::media::worker::is_sweeping(),
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
    // Not under a fetch: the rows it is marking would come back as copies
    // the index alone knows of.
    if crate::media::worker::is_storing() {
        return Err(AppError::Conflict(
            "media are being fetched; wait for the run to end, or stop it".into(),
        ));
    }
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
    pub status: String,
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
    require_write(&identity)?;
    let mut item = repo::item::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    repo::item::load_children(&state.db, &mut item).await?;

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
                status: asset.status.as_str().to_string(),
                asset_id: Some(asset.id.clone()),
                bytes: asset.bytes,
                width: asset.width,
                height: asset.height,
                attempts: Some(asset.attempts),
                error: asset.error.clone(),
                uploaded_by: asset.uploaded_by.clone(),
            },
            None => WorkMedium {
                url: origin.clone(),
                origin,
                kind,
                status: "absent".into(),
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
    }))
}

/// Delete the uploads among these addresses: an upload's file goes with the
/// row or the lock that pointed at it. Quiet — what pointed at it is gone
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
        if let Err(e) = remove_asset(state, &asset).await {
            tracing::warn!(origin, error = %e, "could not delete an upload");
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
/// `clearart`, `landscape`) and `seasonNumber` for a season's, or `episode`
/// as `3x7` for a still. A picture becomes a hand-added one of the work's,
/// which no refresh replaces; a still or a theme locks the field it fills.
/// The file is read for what it is, not for what it is called.
#[utoipa::path(
    post, path = "/items/{id}/media", tag = TAG,
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
    require_write(&identity)?;
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
    let kept = crate::media::worker::keep(&state, store, bytes, &inspected)
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
    state
        .media
        .remember(&origin, &kept.key, kept.thumb, inspected.content_type);

    let detail = match place {
        Place::Picture { cover_type, season } => {
            let mut image = repo::child::blank_image(cover_type, origin.clone());
            image.season_number = season;
            repo::child::add_image(&state.db, &id, &image).await?;
            match season {
                Some(n) => format!("{} for season {n}", cover_type.as_str()),
                None => cover_type.as_str().to_string(),
            }
        }
        Place::Still { season, episode } => {
            repo::override_field::set(
                &state.db,
                &id,
                Scope::Episode { season, episode },
                "image",
                Some(&serde_json::Value::String(origin.clone())),
                Some(&by),
            )
            .await?;
            format!("still for {season}x{episode}")
        }
        Place::Theme => {
            repo::override_field::set(
                &state.db,
                &id,
                Scope::Item,
                "themeMusic",
                Some(&serde_json::Value::String(origin.clone())),
                Some(&by),
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
/// An upload is taken off the work and deleted. A provider's copy is
/// forgotten and deleted — the work keeps pointing at the provider, and the
/// next refresh fetches it again.
#[utoipa::path(
    delete, path = "/items/{id}/media/{asset_id}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("asset_id" = String, Path),
    ),
    responses(
        (status = 204, description = "Gone"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work, or no such copy"),
    ),
)]
async fn forget(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, asset_id)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    require_write(&identity)?;
    if repo::item::get(&state.db, &id).await?.is_none() {
        return Err(AppError::NotFound);
    }
    let asset = repo::asset::get(&state.db, &asset_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let uploaded = asset.origin.starts_with("upload:");
    if uploaded {
        repo::child::remove_images_by_url(&state.db, &id, &asset.origin).await?;
        // A field that held the upload — a still, a theme, the poster the
        // work or a season was to lead with — holds nothing now.
        for locked in repo::override_field::list(&state.db, &id).await? {
            if matches!(
                locked.field.as_str(),
                "image" | "themeMusic" | "primaryPoster" | "primaryFanart"
            ) && locked.value.as_ref().and_then(|v| v.as_str()) == Some(asset.origin.as_str())
                && let Ok(scope) = locked.scope.parse::<Scope>()
            {
                repo::override_field::unset(&state.db, &id, scope, &locked.field).await?;
            }
        }
    }

    remove_asset(&state, &asset).await?;

    state.caches.touched(&id).await;
    state.caches.searches.invalidate_all().await;
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::MediaRemoved,
            target: Some(&id),
            detail: Some(if uploaded {
                "an upload"
            } else {
                "a provider's copy"
            }),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Delete an asset: its bytes, unless another row holds them, and its row.
pub async fn remove_asset(state: &AppState, asset: &Asset) -> AppResult<()> {
    if let (Some(key), Some(store)) = (&asset.key, state.media.store())
        && asset.status == Status::Stored
        && !repo::asset::key_shared(&state.db, key, &asset.id).await?
    {
        store.delete(key).await?;
        if let Some(thumb) = file::thumb_key(key, asset.thumb) {
            store.delete(&thumb).await?;
        }
    }
    repo::asset::delete(&state.db, &asset.id).await?;
    state.media.forget(&asset.origin);
    Ok(())
}

fn require_write(identity: &Identity) -> AppResult<()> {
    identity
        .can_write()
        .then_some(())
        .ok_or(AppError::Forbidden)
}

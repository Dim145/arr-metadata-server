//! Producing `.nfo` documents.
//!
//! See [`crate::export::nfo`] for why this exists and when you do not need it:
//! if Sonarr or Radarr manage the library, their own Kodi metadata writer
//! already puts these files beside the media, built from what this server gave
//! them.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action, job},
    domain::MediaKind,
    error::{AppError, AppResult},
    export::{artwork, nfo},
    service,
    state::AppState,
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Export";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(item_nfo))
        .routes(routes!(episode_nfo))
        .routes(routes!(export_all))
}

/// An XML response, with the filename a client should save it under.
fn xml(body: String, filename: &str) -> Response {
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/xml; charset=utf-8"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("inline; filename=\"{filename}\""))
                    .unwrap_or_else(|_| HeaderValue::from_static("inline")),
            ),
        ],
        body,
    )
        .into_response()
}

/// The `.nfo` document for a work.
///
/// Save it as `tvshow.nfo` at the series' root, or `movie.nfo` beside the film,
/// and point Plex's Personal Media agent — or Kodi, or Jellyfin — at the library.
#[utoipa::path(
    get, path = "/items/{id}/nfo", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, description = "A Kodi/XBMC .nfo document", content_type = "application/xml"),
        (status = 404, description = "No such work"),
    ),
)]
async fn item_nfo(State(state): State<AppState>, Path(id): Path<String>) -> AppResult<Response> {
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    let filename = match item.kind {
        MediaKind::Series => "tvshow.nfo",
        MediaKind::Movie => "movie.nfo",
    };

    Ok(xml(nfo::for_item(&item), filename))
}

/// The `.nfo` document for one episode.
#[utoipa::path(
    get, path = "/items/{id}/nfo/{season}/{episode}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("season" = i32, Path, description = "Season number"),
        ("episode" = i32, Path, description = "Episode number"),
    ),
    responses(
        (status = 200, description = "A Kodi/XBMC .nfo document", content_type = "application/xml"),
        (status = 404, description = "No such work or episode"),
    ),
)]
async fn episode_nfo(
    State(state): State<AppState>,
    Path((id, season, number)): Path<(String, i32, i32)>,
) -> AppResult<Response> {
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    let episode = item
        .episodes
        .iter()
        .find(|e| e.season_number == season && e.episode_number == number)
        .ok_or(AppError::NotFound)?;

    Ok(xml(
        nfo::for_episode(&item, episode),
        &format!("S{season:02}E{number:02}.nfo"),
    ))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub root: String,
    pub works: usize,
    pub episodes: usize,
    /// Pictures written. A second export leaves them alone, so this counts what
    /// was new, not what is there.
    pub images: usize,
    pub failed: usize,
}

/// Write a `.nfo` document for everything, under `AMS_NFO_EXPORT_PATH`.
///
/// The layout is `series/{slug}/tvshow.nfo`, `series/{slug}/Season NN/SNNENN.nfo`
/// and `movies/{slug}/movie.nfo`. That directory structure is this server's own:
/// nothing here can know how your library is laid out, so the files land
/// somewhere predictable for you to copy or link from.
#[utoipa::path(
    post, path = "/export/nfo", tag = TAG,
    responses(
        (status = 200, body = ExportSummary),
        (status = 403, description = "The caller may not write"),
        (status = 503, description = "AMS_NFO_EXPORT_PATH is not set"),
    ),
)]
async fn export_all(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
) -> AppResult<Json<ExportSummary>> {
    if !identity.can_write() {
        return Err(AppError::Forbidden);
    }

    let Some(root) = state.config.export.nfo_path.clone() else {
        return Err(AppError::ProviderNotConfigured);
    };

    let record = job::start(&state.db, KIND, None)
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "could not open a job run"))
        .ok();

    let outcome = write_everything(&state, &root).await;

    if let Some(record) = record {
        let closed = match &outcome {
            Ok(s) => {
                job::finish(
                    &state.db,
                    &record,
                    Some(&format!(
                        "{} works, {} episodes, {} failed",
                        s.works, s.episodes, s.failed
                    )),
                    None,
                )
                .await
            }
            Err(e) => job::finish(&state.db, &record, None, Some(&e.to_string())).await,
        };

        if let Err(e) = closed {
            tracing::warn!(error = %e, "could not close the job run");
        }
    }

    let summary = outcome.map_err(AppError::Internal)?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::NfoExported,
            target: Some(&summary.root),
            detail: Some(&format!(
                "{} works, {} episodes",
                summary.works, summary.episodes
            )),
        },
    )
    .await;

    Ok(Json(summary))
}

const KIND: &str = "export.nfo";

/// How many works one export pass handles. Enough for any realistic library,
/// and bounded so a runaway catalogue cannot fill a disk unnoticed.
const MAX_WORKS: i64 = 10_000;

async fn write_everything(
    state: &AppState,
    root: &std::path::Path,
) -> anyhow::Result<ExportSummary> {
    let items = repo::item::search(
        &state.db,
        &repo::item::Query {
            limit: MAX_WORKS,
            ..Default::default()
        },
    )
    .await?;

    let mut summary = ExportSummary {
        root: root.display().to_string(),
        works: 0,
        episodes: 0,
        images: 0,
        failed: 0,
    };

    for shallow in items {
        let Some(item) = service::load(state, &shallow.id).await? else {
            continue;
        };

        match write_one(root, &nfo::relative_path(&item), &nfo::for_item(&item)).await {
            Ok(()) => summary.works += 1,
            Err(e) => {
                tracing::warn!(id = %item.id, error = %e, "could not write the work's nfo");
                summary.failed += 1;
                continue;
            }
        }

        for episode in &item.episodes {
            let path = nfo::episode_relative_path(&item, episode);
            match write_one(root, &path, &nfo::for_episode(&item, episode)).await {
                Ok(()) => summary.episodes += 1,
                Err(e) => {
                    tracing::warn!(id = %item.id, %path, error = %e, "could not write an episode nfo");
                    summary.failed += 1;
                }
            }
        }

        if state.config.export.artwork {
            let (written, failed) = write_artwork(state, root, &item).await;
            summary.images += written;
            summary.failed += failed;
        }
    }

    tracing::info!(
        works = summary.works,
        episodes = summary.episodes,
        images = summary.images,
        failed = summary.failed,
        "nfo export finished"
    );

    Ok(summary)
}

/// Download one work's artwork into its folder.
///
/// A picture that cannot be fetched is counted and skipped: a provider serving
/// one 404 should not cost the rest of the export.
async fn write_artwork(
    state: &AppState,
    root: &std::path::Path,
    item: &crate::domain::MediaItem,
) -> (usize, usize) {
    let mut written = 0;
    let mut failed = 0;

    for download in artwork::plan(item) {
        match artwork::fetch_one(&state.http, root, &download).await {
            Ok(true) => written += 1,
            // Already on disk from an earlier export.
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(
                    id = %item.id,
                    path = %download.path,
                    error = format_args!("{e:#}"),
                    "could not write artwork"
                );
                failed += 1;
            }
        }
    }

    (written, failed)
}

/// Write one document under `root`, refusing anything that would escape it.
async fn write_one(root: &std::path::Path, relative: &str, body: &str) -> anyhow::Result<()> {
    // Slugs are generated from `make_slug`, so they cannot contain a separator
    // today. Check anyway: this joins caller-influenced text onto a filesystem
    // path, and the cost of being wrong is writing outside the export root.
    if relative
        .split('/')
        .any(|part| part == ".." || part.is_empty())
    {
        anyhow::bail!("refusing to write outside the export root: {relative}");
    }

    let target = root.join(relative);

    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tokio::fs::write(&target, body).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_path_that_would_escape_the_root_is_refused() {
        let root = std::env::temp_dir().join("ams-nfo-test");

        for bad in ["../escape.nfo", "series/../../escape.nfo", "series//x.nfo"] {
            assert!(
                write_one(&root, bad, "x").await.is_err(),
                "{bad} should have been refused"
            );
        }
    }

    #[tokio::test]
    async fn an_ordinary_path_is_written_under_the_root() {
        let root = std::env::temp_dir().join(format!("ams-nfo-{}", crate::db::new_id()));

        write_one(&root, "series/a-show-2026/tvshow.nfo", "<tvshow/>")
            .await
            .expect("write");

        let written = tokio::fs::read_to_string(root.join("series/a-show-2026/tvshow.nfo"))
            .await
            .expect("read back");
        assert_eq!(written, "<tvshow/>");

        let _ = tokio::fs::remove_dir_all(&root).await;
    }
}

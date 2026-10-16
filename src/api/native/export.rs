//! Producing `.nfo` documents, and the Kometa metadata file.
//!
//! See [`crate::export::nfo`] for why the documents exist and when you do not
//! need them: if Sonarr or Radarr manage the library, their own Kodi metadata
//! writer already puts these files beside the media, built from what this
//! server gave them. See [`crate::export::kometa`] for the other way to carry
//! a correction into Plex: a file Kometa applies.

use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action, job, list::CuratedList},
    domain::{MediaItem, MediaKind},
    error::{AppError, AppResult},
    export::{artwork, kometa, nfo},
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
        .routes(routes!(kometa_yml))
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
/// Of a work this caller may see, as its own page decides it.
#[utoipa::path(
    get, path = "/items/{id}/nfo", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, description = "A Kodi/XBMC .nfo document", content_type = "application/xml"),
        (status = 404, description = "No such work, or none this caller may see"),
    ),
)]
async fn item_nfo(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    // Switched off, or kept from this caller by the adult policy: its
    // document is kept from them as the work is.
    let item = super::visible_work(&state, &identity, &id).await?;

    let filename = match item.kind {
        MediaKind::Series => "tvshow.nfo",
        MediaKind::Movie => "movie.nfo",
    };

    // A document read elsewhere: the copies kept are named as a media
    // server can fetch them, or as their providers do.
    let mut item = item;
    state.media.for_clients(&mut item);
    let collection = collection_of(&state, &item, &mut HashMap::new()).await;
    let set = collection.as_ref().map(Named::as_set);
    Ok(xml(nfo::for_item_in(&item, set.as_ref()), filename))
}

/// A collection's name, and what it is about, as TMDB has them.
#[derive(Clone)]
struct Named {
    name: String,
    overview: Option<String>,
}

impl Named {
    fn as_set(&self) -> nfo::Collection<'_> {
        nfo::Collection {
            name: &self.name,
            overview: self.overview.as_deref(),
        }
    }
}

/// What the collection a film belongs to is called, in the catalogue's
/// language: the catalogue holds TMDB's number for it and TMDB's record is the
/// collections page's to read, which keeps it a day. `asked` is what an
/// export has asked already, so that a hundred films of one saga are one
/// question. Nothing for a film in none, and for one whose collection TMDB
/// cannot be asked about — no key, or no answer — which is a document without
/// a `<set>`, not one that fails.
async fn collection_of(
    state: &AppState,
    item: &MediaItem,
    asked: &mut HashMap<i64, Option<Named>>,
) -> Option<Named> {
    let id = item
        .collection_tmdb_id
        .filter(|_| item.kind == MediaKind::Movie)?;
    if let Some(known) = asked.get(&id) {
        return known.clone();
    }

    let language = service::language::tmdb_locale(None, &state.language(None, None));
    let record = super::recommend::named(state, id, &language).await;
    let text = |field: &str| {
        record
            .as_ref()
            .and_then(|record| record.get(field))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let named = text("name").map(|name| Named {
        name,
        overview: text("overview"),
    });

    asked.insert(id, named.clone());
    named
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
        (status = 404, description = "No such work or episode, or none this caller may see"),
    ),
)]
async fn episode_nfo(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((id, season, number)): Path<(String, i32, i32)>,
) -> AppResult<Response> {
    let item = super::visible_work(&state, &identity, &id).await?;

    let mut item = item;
    state.media.for_clients(&mut item);
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

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct KometaQuery {
    /// `movie` or `series`. A metadata file belongs to one library and a
    /// library holds one kind: films are matched by TMDB id, series by
    /// TheTVDB's.
    pub kind: Option<String>,
    /// The language of the library, `fr` or `fra`: texts locked for it, and
    /// for every language, are written; those locked for another are not.
    /// Left out, the language the catalogue is kept in.
    pub language: Option<String>,
}

/// A Kometa metadata file of the corrections locked here.
///
/// For Plex libraries run by [Kometa](https://kometa.wiki): a `metadata:` file
/// with an entry for each work that has something locked — matched by TMDB id
/// (films) or TheTVDB id (series), by IMDb id where it has not that one —
/// carrying *only* what was locked: title, sort title, original title, summary,
/// tagline, first release, content rating, studio, genres, the poster and
/// background chosen, and for a series its seasons' titles, summaries and
/// posters and its episodes' titles, summaries and dates. Kometa puts them over
/// what Plex's agent found, on every run.
///
/// Name it under `metadata_files` of the library — as a file saved from here,
/// or by this address with `?apikey=` and a key that may write, which is what
/// asking for it takes.
#[utoipa::path(
    get, path = "/export/kometa.yml", tag = TAG,
    params(KometaQuery),
    responses(
        (status = 200, description = "A Kometa metadata file", content_type = "application/yaml"),
        (status = 400, description = "kind is neither movie nor series, or language is not a language"),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn kometa_yml(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<KometaQuery>,
) -> AppResult<Response> {
    if !identity.can_write() {
        return Err(AppError::Forbidden);
    }
    let kind = match query.kind.as_deref().map(str::trim) {
        Some("movie") => MediaKind::Movie,
        Some("series") => MediaKind::Series,
        _ => {
            return Err(AppError::BadRequest(
                "kind is movie or series: a Kometa metadata file belongs to one library, which holds one of them"
                    .into(),
            ));
        }
    };
    let language = crate::api::extract::language(query.language.as_deref())?;

    // The works with something locked, of this kind: a lock is a row, so the
    // rows name them, and the store says which are films.
    let mut ids: Vec<String> = repo::override_field::all(&state.db)
        .await?
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut wanted: Vec<(String, String)> = Vec::new();
    for chunk in ids.chunks(500) {
        for work in repo::item::by_ids(&state.db, chunk).await? {
            if work.kind == kind {
                wanted.push((work.title.to_lowercase(), work.id));
            }
        }
    }
    // By title, so that a file written twice reads the same and a change to
    // it shows as one.
    wanted.sort();
    wanted.truncate(MAX_WORKS as usize);

    let mut works = Vec::with_capacity(wanted.len());
    for (_, id) in wanted {
        let Some(mut work) = service::load(&state, &id).await? else {
            continue;
        };
        if let Some(language) = &language {
            // What was locked for the library's language is what it is to say.
            service::language::lay_locks(&state, &mut work, language);
        }
        // A picture is an address Kometa — which is not this server's own
        // page — can follow.
        state.media.for_clients(&mut work);
        works.push(work);
    }

    let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let document = kometa::document(kind, &works, &day);
    let filename = match kind {
        MediaKind::Movie => "kometa-movies.yml",
        MediaKind::Series => "kometa-series.yml",
    };

    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/yaml; charset=utf-8"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            ),
            // The corrections of a catalogue are not for a cache to keep.
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            // How many works it speaks of, for a page to say so rather than
            // hand over a file with nothing in it.
            (
                header::HeaderName::from_static("x-ams-works"),
                HeaderValue::from(document.entries),
            ),
        ],
        document.text,
    )
        .into_response())
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

/// An export under way: where it writes, and the job run that records it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportStarted {
    pub root: String,
    /// The run under `export.nfo` in the jobs list. Its detail, once it is
    /// done, reads `N works, N episodes, N failed`; the settings page reads
    /// the counts out of it.
    pub job_id: Option<String>,
}

/// Whether an export is being written. One at a time: two would write the
/// same files, each over the other's.
static EXPORTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether an export is being written now.
pub fn is_exporting() -> bool {
    EXPORTING.load(std::sync::atomic::Ordering::Acquire)
}

/// Lets the next export in, however this one ends — a panic included.
struct Exporting;

impl Drop for Exporting {
    fn drop(&mut self) {
        EXPORTING.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExportQuery {
    /// A list's slug or id: write the documents of its works alone, those
    /// switched on and not for adults, as the whole export would. Left out,
    /// every work of the catalogue.
    pub list: Option<String>,
}

/// Write a `.nfo` document for everything, under `AMS_NFO_EXPORT_PATH` — or,
/// with `?list=`, for the works of one selection.
///
/// The layout is `series/{slug}/tvshow.nfo`, `series/{slug}/Season NN/SNNENN.nfo`
/// and `movies/{slug}/movie.nfo`. That directory structure is this server's own:
/// nothing here can know how your library is laid out, so the files land
/// somewhere predictable for you to copy or link from.
///
/// Answered at once, and written in the background: a library's artwork is
/// thousands of downloads — One Piece alone has a still for each of more than
/// a thousand episodes — and a request is cut off after a minute, which used
/// to stop the export wherever it had got to. The run is a job; its record
/// says when it is done and what it wrote.
#[utoipa::path(
    post, path = "/export/nfo", tag = TAG,
    params(ExportQuery),
    responses(
        (status = 202, body = ExportStarted),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such list"),
        (status = 409, description = "An export is already being written"),
        (status = 503, description = "AMS_NFO_EXPORT_PATH is not set"),
    ),
)]
async fn export_all(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Query(query): Query<ExportQuery>,
) -> AppResult<(StatusCode, Json<ExportStarted>)> {
    if !identity.can_write() {
        return Err(AppError::Forbidden);
    }

    let subset = match query
        .list
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        Some(key) => Some(
            super::lists::find(&state, key)
                .await?
                .ok_or(AppError::NotFound)?,
        ),
        None => None,
    };
    let (root, job_id) = start_export_of(&state, &identity, &ip, subset).await?;

    Ok((StatusCode::ACCEPTED, Json(ExportStarted { root, job_id })))
}

/// Begin writing the export in the background, as `identity` asked; the
/// folder it writes under, and its run. What the tasks page runs too.
pub async fn start_export(
    state: &AppState,
    identity: &Identity,
    ip: &ClientIp,
) -> AppResult<(String, Option<String>)> {
    start_export_of(state, identity, ip, None).await
}

/// [`start_export`], for the works of one selection alone where one is given.
async fn start_export_of(
    state: &AppState,
    identity: &Identity,
    ip: &ClientIp,
    subset: Option<CuratedList>,
) -> AppResult<(String, Option<String>)> {
    let Some(root) = state.config.export.nfo_path.clone() else {
        return Err(AppError::Disabled {
            code: "export_not_configured",
            message: "AMS_NFO_EXPORT_PATH is not set: there is nowhere to write the export".into(),
        });
    };

    if EXPORTING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return Err(AppError::Conflict(
            "an export is already being written; its run is in the jobs list".into(),
        ));
    }
    let exporting = Exporting;

    let by = identity.label();
    let record = job::start_by(&state.db, KIND, None, Some(&by))
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "could not open a job run"))
        .ok();

    let shown = root.display().to_string();
    let job_id = record.clone();
    let state = state.clone();
    let identity = identity.clone();
    let ip = *ip;

    tokio::spawn(async move {
        let _exporting = exporting;
        let outcome = write_everything(&state, &root, subset.as_ref()).await;

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
                Err(e) => job::finish(&state.db, &record, None, Some(&format!("{e:#}"))).await,
            };

            if let Err(e) = closed {
                tracing::warn!(error = %e, "could not close the job run");
            }
        }

        match outcome {
            Ok(summary) => {
                audit::record(
                    &state,
                    Event {
                        identity: Some(&identity),
                        ip: &ip,
                        action: Action::NfoExported,
                        target: Some(&summary.root),
                        detail: Some(&match &subset {
                            Some(list) => format!(
                                "{} works, {} episodes · the selection {}",
                                summary.works, summary.episodes, list.name
                            ),
                            None => {
                                format!("{} works, {} episodes", summary.works, summary.episodes)
                            }
                        }),
                    },
                )
                .await;
            }
            Err(e) => tracing::error!(error = format_args!("{e:#}"), "the .nfo export failed"),
        }
    });

    Ok((shown, job_id))
}

const KIND: &str = job::kinds::EXPORT_NFO;

/// How many works one export pass handles. Enough for any realistic library,
/// and bounded so a runaway catalogue cannot fill a disk unnoticed.
const MAX_WORKS: i64 = 10_000;

/// Every work an export of the catalogue holds, as the store lists them.
///
/// Paged. Asking for `MAX_WORKS` in one call got `MAX_PAGE` of them and no
/// indication of it, so a three thousand film library exported five hundred
/// and reported a clean run.
async fn whole_catalogue(state: &AppState) -> anyhow::Result<Vec<MediaItem>> {
    let mut items = Vec::new();
    let mut offset = 0i64;

    loop {
        let page = repo::item::search(
            &state.db,
            &repo::item::Query {
                limit: repo::item::MAX_PAGE,
                offset,
                ..Default::default()
            },
        )
        .await?;

        let read = page.len() as i64;
        items.extend(page);

        if read < repo::item::MAX_PAGE || items.len() as i64 >= MAX_WORKS {
            break;
        }

        offset += read;
    }

    Ok(items)
}

/// Write the documents, and the pictures beside them, of the whole catalogue
/// or — `subset` — of one selection's works.
async fn write_everything(
    state: &AppState,
    root: &std::path::Path,
    subset: Option<&CuratedList>,
) -> anyhow::Result<ExportSummary> {
    let items = match subset {
        // As the selection's own page lists them for a reader: switched on,
        // not for adults — which is what the whole export holds too.
        Some(list) => super::lists::exportable(state, list)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?,
        None => whole_catalogue(state).await?,
    };

    if items.len() as i64 >= MAX_WORKS {
        tracing::warn!(
            limit = MAX_WORKS,
            "the catalogue is larger than one export pass writes; it was cut short"
        );
    }

    let mut summary = ExportSummary {
        root: root.display().to_string(),
        works: 0,
        episodes: 0,
        images: 0,
        failed: 0,
    };

    // The collections' names, asked once each however many films of them.
    let mut collections: HashMap<i64, Option<Named>> = HashMap::new();

    for shallow in items {
        let Some(item) = service::load(state, &shallow.id).await? else {
            continue;
        };
        // The documents name the pictures as a media server can fetch them;
        // the pictures themselves are taken from the store when kept.
        let mut named = item.clone();
        state.media.for_clients(&mut named);
        let collection = collection_of(state, &named, &mut collections).await;
        let set = collection.as_ref().map(Named::as_set);

        match write_one(
            root,
            &nfo::relative_path(&named),
            &nfo::for_item_in(&named, set.as_ref()),
        )
        .await
        {
            Ok(()) => summary.works += 1,
            Err(e) => {
                tracing::warn!(id = %item.id, error = %e, "could not write the work's nfo");
                summary.failed += 1;
                continue;
            }
        }

        for episode in &named.episodes {
            let path = nfo::episode_relative_path(&named, episode);
            match write_one(root, &path, &nfo::for_episode(&named, episode)).await {
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
        match artwork::fetch_one(state, root, &download).await {
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
        // nosemgrep: rust.lang.security.temp-dir.temp-dir — a test's scratch directory, named after a fresh id: nothing security-relevant lives in it
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
        // nosemgrep: rust.lang.security.temp-dir.temp-dir — a test's scratch directory, named after a fresh id: nothing security-relevant lives in it
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

    /// A work, stored, by its kind, its title and the ids it goes by.
    async fn stored(
        state: &AppState,
        kind: MediaKind,
        title: &str,
        tmdb: Option<i64>,
        tvdb: Option<i64>,
    ) -> MediaItem {
        let mut work = MediaItem::empty(kind);
        work.title = title.into();
        work.slug = crate::domain::make_slug(title, None);
        work.external_ids.tmdb = tmdb;
        work.external_ids.tvdb = tvdb;
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &work,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        work
    }

    async fn lock(state: &AppState, work: &MediaItem, field: &str, value: serde_json::Value) {
        repo::override_field::set(
            &state.db,
            &work.id,
            crate::domain::fields::Scope::Item,
            field,
            Some(&value),
            Some("test"),
        )
        .await
        .expect("locked");
    }

    async fn answered(
        response: impl std::future::Future<Output = AppResult<Response>>,
    ) -> (axum::http::HeaderMap, String) {
        let response = response.await.expect("answered");
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read");
        (headers, String::from_utf8(bytes.to_vec()).expect("text"))
    }

    fn ask(
        state: &AppState,
        who: Identity,
        kind: Option<&str>,
        language: Option<&str>,
    ) -> impl std::future::Future<Output = AppResult<Response>> {
        kometa_yml(
            State(state.clone()),
            Extension(who),
            Query(KometaQuery {
                kind: kind.map(str::to_string),
                language: language.map(str::to_string),
            }),
        )
    }

    #[tokio::test]
    async fn the_kometa_file_is_the_locks_of_one_kind_for_whoever_may_write() {
        let state = super::super::testing::server().await;
        let heat = stored(&state, MediaKind::Movie, "Heat", Some(949), None).await;
        let ronin = stored(&state, MediaKind::Movie, "Ronin", Some(8195), None).await;
        let dark = stored(&state, MediaKind::Series, "Dark", None, Some(334_824)).await;
        // One film corrected, the other as its provider has it, a series
        // corrected: a file of films holds the first alone.
        lock(
            &state,
            &heat,
            "title",
            serde_json::json!("Heat: \"Director's cut\""),
        )
        .await;
        lock(
            &state,
            &heat,
            "overview",
            serde_json::json!("A crew.\nA cop."),
        )
        .await;
        lock(
            &state,
            &dark,
            "title",
            serde_json::json!("Dark (the series)"),
        )
        .await;
        let _ = ronin;

        let (headers, films) =
            answered(ask(&state, Identity::Anonymous, Some("movie"), None)).await;
        assert_eq!(
            headers[header::CONTENT_TYPE],
            "application/yaml; charset=utf-8"
        );
        assert_eq!(
            headers[header::CONTENT_DISPOSITION],
            "attachment; filename=\"kometa-movies.yml\""
        );
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(headers["x-ams-works"], "1");
        assert!(
            films.contains("\nmetadata:\n  \"Heat: \\\"Director's cut\\\"\":\n"),
            "{films}"
        );
        assert!(films.contains("      mapping_id: 949\n"));
        assert!(films.contains("    title: \"Heat: \\\"Director's cut\\\"\"\n"));
        assert!(films.contains("    summary: \"A crew.\\nA cop.\"\n"));
        assert!(!films.contains("Ronin") && !films.contains("Dark"));

        let (headers, series) =
            answered(ask(&state, Identity::Anonymous, Some("series"), None)).await;
        assert_eq!(headers["x-ams-works"], "1");
        assert!(series.contains("      mapping_id: 334824\n"));
        assert!(series.contains("    title: \"Dark (the series)\"\n"));
        assert!(!series.contains("Heat"));

        // A catalogue with nothing locked on a kind says so, in a file that is one.
        let none = stored(&state, MediaKind::Movie, "Alone", Some(1), None).await;
        let _ = none;
        lock_clear_all(&state, &heat).await;
        let (headers, empty) =
            answered(ask(&state, Identity::Anonymous, Some("movie"), None)).await;
        assert_eq!(headers["x-ams-works"], "0");
        assert!(empty.contains("metadata: {}\n"));

        // Not a visitor's, a member's, nor a kind that is neither.
        assert!(matches!(
            ask(&state, Identity::Visitor, Some("movie"), None).await,
            Err(AppError::Forbidden)
        ));
        for kind in [None, Some(""), Some("anime"), Some("mixed")] {
            assert!(
                matches!(
                    ask(&state, Identity::Anonymous, kind, None).await,
                    Err(AppError::BadRequest(_))
                ),
                "{kind:?}"
            );
        }
        assert!(matches!(
            ask(
                &state,
                Identity::Anonymous,
                Some("movie"),
                Some("not a language!")
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
    }

    /// Every lock on a work taken away.
    async fn lock_clear_all(state: &AppState, work: &MediaItem) {
        repo::override_field::clear(&state.db, &work.id)
            .await
            .expect("unlocked");
    }

    #[tokio::test]
    async fn a_text_locked_for_a_language_is_written_for_that_library_alone() {
        let state = super::super::testing::server().await;
        let heat = stored(&state, MediaKind::Movie, "Heat", Some(949), None).await;
        repo::override_field::set_in(
            &state.db,
            &heat.id,
            "item",
            "title",
            "fra",
            Some(&serde_json::json!("Heat (la version française)")),
            Some("test"),
        )
        .await
        .expect("locked in French");

        // Not the catalogue's language, so not what its file says...
        let (headers, stored_language) =
            answered(ask(&state, Identity::Anonymous, Some("movie"), None)).await;
        assert_eq!(headers["x-ams-works"], "0", "{stored_language}");

        // ...but what a library in French is to be told.
        let (headers, french) =
            answered(ask(&state, Identity::Anonymous, Some("movie"), Some("fr"))).await;
        assert_eq!(headers["x-ams-works"], "1", "{french}");
        assert!(french.contains("    title: \"Heat (la version française)\"\n"));
    }

    #[tokio::test]
    async fn the_documents_of_one_selection_alone_are_written() {
        use crate::db::repo::list::{ListFields, ListKind, ListMode};

        let state = super::super::testing::server().await;
        let heat = stored(&state, MediaKind::Movie, "Heat", Some(949), None).await;
        let dark = stored(&state, MediaKind::Series, "Dark", None, Some(334_824)).await;
        let ronin = stored(&state, MediaKind::Movie, "Ronin", Some(8195), None).await;
        // Switched off: no more in a selection's export than in the whole one.
        let mut off = stored(&state, MediaKind::Movie, "Switched off", Some(3), None).await;
        off.is_enabled = false;
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &off,
                replace_children: true,
            },
        )
        .await
        .expect("stored");

        let list = repo::list::create(
            &state.db,
            ListFields {
                slug: "picks",
                name: "Picks",
                description: None,
                kind: ListKind::Mixed,
                mode: ListMode::Manual,
                filter: None,
                is_public: true,
                cover: None,
            },
            Some(&[dark.id.clone(), heat.id.clone(), off.id.clone()]),
        )
        .await
        .expect("created");

        // nosemgrep: rust.lang.security.temp-dir.temp-dir — a test's scratch directory, named after a fresh id: nothing security-relevant lives in it
        let root = std::env::temp_dir().join(format!("ams-nfo-{}", crate::db::new_id()));
        let summary = write_everything(&state, &root, Some(&list))
            .await
            .expect("written");
        assert_eq!((summary.works, summary.failed), (2, 0));

        let there = |path: &str| root.join(path).exists();
        assert!(there("movies/heat/movie.nfo"));
        assert!(there("series/dark/tvshow.nfo"));
        assert!(!there("movies/ronin/movie.nfo"), "not in the selection");
        assert!(!there("movies/switched-off/movie.nfo"), "switched off");

        // The whole catalogue, for comparison, holds what is on.
        let all = write_everything(&state, &root, None)
            .await
            .expect("written");
        assert_eq!(all.works, 3);
        assert!(there("movies/ronin/movie.nfo"));
        let _ = ronin;

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn a_selection_that_is_not_there_is_not_exported() {
        let state = super::super::testing::server().await;
        let refused = export_all(
            State(state),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Query(ExportQuery {
                list: Some("no-such-selection".into()),
            }),
        )
        .await;
        assert!(matches!(refused, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn a_collection_is_asked_about_once_and_only_when_it_can_be() {
        let state = super::super::testing::server().await;
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.collection_tmdb_id = Some(8091);
        let mut asked = HashMap::new();

        // No TMDB key in a test server: no name, and the question is
        // remembered, not asked again.
        assert!(collection_of(&state, &film, &mut asked).await.is_none());
        assert!(asked.contains_key(&8091));
        // A series is in none, whatever its record says.
        film.kind = MediaKind::Series;
        assert!(
            collection_of(&state, &film, &mut HashMap::new())
                .await
                .is_none()
        );
        film.kind = MediaKind::Movie;
        film.collection_tmdb_id = None;
        assert!(
            collection_of(&state, &film, &mut HashMap::new())
                .await
                .is_none()
        );
    }
}

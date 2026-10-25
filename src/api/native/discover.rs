//! Finding a work at a provider, and pulling it in.
//!
//! The catalogue otherwise fills itself: a client asks for something, this
//! server fetches it, and it is stored. That is the right default and a poor way
//! to add one particular film — you would have to make Radarr ask for it.
//!
//! So: search the providers directly, see what they have, and take the one you
//! meant. The fetch is the same path a client's request takes, so an imported
//! work is indistinguishable from one that arrived on its own, refresh
//! schedule and all.

use axum::{
    Extension, Json,
    extract::{Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::audit::Action,
    domain::{MediaItem, MediaKind},
    error::{AppError, AppResult},
    jobs::import::{self, Line, Preview, Unread},
    providers::names,
    service,
    state::AppState,
};

pub const TAG: &str = "Discover";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(search))
        .routes(routes!(import))
        .routes(routes!(preview_batch))
        .routes(routes!(import_batch))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    /// A title, a `prefix:id` lookup (`tvdb:81189`, `imdb:tt0903747`…), or the
    /// address of the work's page at TMDB, IMDb, TheTVDB, AniList, MyAnimeList
    /// or Fankai.
    pub term: String,
    /// `series` or `movie`. Both are searched when this is absent.
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub year: Option<i32>,
    /// One source to ask, rather than the usual order: `tmdb`, `tvdb`,
    /// `skyhook` or `fankai` for series, `tmdb` or `radarr` for films. The
    /// usual order stops at the first that finds anything; this finds what
    /// that one has, even when another would have answered first.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub source: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub kind: MediaKind,
    pub title: String,
    #[schema(required)]
    pub year: Option<i32>,
    #[schema(required)]
    pub overview: Option<String>,
    #[schema(required)]
    pub poster: Option<String>,
    #[schema(required)]
    pub tmdb_id: Option<i64>,
    #[schema(required)]
    pub tvdb_id: Option<i64>,
    #[schema(required)]
    pub imdb_id: Option<String>,
    /// Fankai's id, for a Fan-Kai production.
    #[schema(required)]
    pub fankai_id: Option<i64>,
    /// Whether this server already holds it, so the interface can say "stored"
    /// rather than offering to import it twice.
    pub stored: bool,
    pub is_adult: bool,
}

/// Ask the providers what they have.
///
/// This reaches out; it is not a search of what is already stored. The results
/// are shallow on purpose — enough to recognise the right work and no more,
/// because fetching twenty in full to show a list would be twenty times the
/// work for nineteen wasted.
#[utoipa::path(
    get, path = "/discover", tag = TAG,
    operation_id = "discover_search",
    params(SearchQuery),
    responses(
        (status = 200, body = Vec<Found>),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn search(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<Vec<Found>>> {
    identity.require_write()?;

    // A page pasted whole is the lookup it stands for, and says which kind it
    // is when its address does.
    let (term, kind_of_link) = match crate::jobs::import::from_link(&query.term) {
        Some((lookup, kind)) => (lookup, kind),
        None => (query.term.trim().to_string(), None),
    };
    let term = term.as_str();
    if term.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let asked = query
        .kind
        .as_deref()
        .map(str::parse::<MediaKind>)
        .transpose()
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let wanted = kind_of_link.or(asked);

    // A lookup names its own source; one chosen beside it has nothing to add.
    let source = match query.source.as_deref().map(str::trim) {
        Some(source) if !is_lookup(term) => Some(chosen(&state, source)?),
        _ => None,
    };

    let mut found = Vec::new();

    if wanted != Some(MediaKind::Movie) {
        let hits = match source {
            Some(source) if SERIES_SOURCES.contains(&source) => {
                service::series::search_at(&state, source, term).await
            }
            Some(_) => Ok(Vec::new()),
            None => service::series::search(&state, term).await,
        };
        match hits {
            Ok(hits) => found.extend(hits.iter().map(describe)),
            Err(e) => tracing::warn!(term, ?source, error = %e, "series search failed"),
        }
    }

    if wanted != Some(MediaKind::Series) {
        let hits = match source {
            Some(source) if MOVIE_SOURCES.contains(&source) => {
                service::movie::search_at(&state, source, term, query.year).await
            }
            Some(_) => Ok(Vec::new()),
            None => service::movie::search(&state, term, query.year).await,
        };
        match hits {
            Ok(hits) => found.extend(hits.iter().map(describe)),
            Err(e) => tracing::warn!(term, ?source, error = %e, "movie search failed"),
        }
    }

    // Silence from a client means no — one that never mentions adult titles is
    // not asking for them. It cannot mean that here: this is the operator's own
    // screen, and they said what they wanted when they set `adult.mode`. Asking
    // for what the server allows is what makes the setting visible from the
    // place it is set.
    let adult = state.adult_for(
        identity.client_id(),
        identity.peer_id(),
        Some(state.adult_visible()),
    );
    found.retain(|hit| adult || !hit.is_adult);

    // Whether each is already held, asked of the store rather than inferred
    // from the result: a freshly mapped hit carries an id and a timestamp
    // because every entity does, which says nothing about it being stored.
    for hit in &mut found {
        hit.stored = held(&state, hit).await;
    }

    Ok(Json(found))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub kind: String,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub fankai_id: Option<i64>,
}

/// Fetch a work from its providers and store it.
///
/// Idempotent: importing something already held refreshes it rather than
/// duplicating it, because the fetch path keys on the external id.
#[utoipa::path(
    post, path = "/discover/import", tag = TAG,
    operation_id = "discover_import",
    request_body = ImportRequest,
    responses(
        (status = 200, body = MediaItem),
        (status = 400, description = "No usable identifier"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No provider had it"),
    ),
)]
async fn import(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<ImportRequest>,
) -> AppResult<Json<MediaItem>> {
    identity.require_write()?;

    let kind: MediaKind = request
        .kind
        .parse()
        .map_err(|e: anyhow::Error| AppError::BadRequest(e.to_string()))?;

    let found = match kind {
        MediaKind::Series => match (
            request.fankai_id,
            request.tvdb_id,
            request.tmdb_id,
            request.imdb_id.as_deref(),
        ) {
            // Fankai first: a Fan-Kai carries no other id, and one sent beside
            // it would fetch the anime it was cut from instead.
            (Some(fankai), _, _, _) => service::series::by_fankai_id(&state, fankai).await?,
            (_, Some(tvdb), _, _) => service::series::by_tvdb_id(&state, tvdb).await?,
            (_, _, Some(tmdb), _) => service::series::by_tmdb_id(&state, tmdb).await?,
            (_, _, _, Some(imdb)) => service::series::by_imdb_id(&state, imdb).await?,
            _ => return Err(AppError::BadRequest("no identifier to fetch by".into())),
        },
        MediaKind::Movie => match (request.tmdb_id, request.imdb_id.as_deref()) {
            (Some(tmdb), _) => service::movie::by_tmdb_id(&state, tmdb).await?,
            (_, Some(imdb)) => service::movie::by_imdb_id(&state, imdb).await?,
            _ => return Err(AppError::BadRequest("no identifier to fetch by".into())),
        },
    };

    let item = found.ok_or(AppError::NotFound)?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ItemImported,
            target: Some(&item.title),
            detail: Some(&format!("{} {}", kind.as_str(), item.id)),
        },
    )
    .await;

    Ok(Json(item))
}

fn describe(item: &MediaItem) -> Found {
    Found {
        kind: item.kind,
        title: item.title.clone(),
        year: item.year,
        overview: item.overview.clone(),
        poster: item
            .images
            .iter()
            .find(|image| image.cover_type == crate::domain::CoverType::Poster)
            .map(|image| image.url.clone()),
        tmdb_id: item.external_ids.tmdb,
        tvdb_id: item.external_ids.tvdb,
        imdb_id: item.external_ids.imdb.clone(),
        fankai_id: item.external_ids.fankai,
        // Filled in afterwards, against the store.
        stored: false,
        is_adult: item.is_adult,
    }
}

/// Whether this server already holds the work a hit stands for.
async fn held(state: &AppState, hit: &Found) -> bool {
    use crate::domain::ExternalSource::{Fankai, Imdb, TmdbMovie, TmdbTv, TvdbSeries};

    let lookups: [(crate::domain::ExternalSource, Option<String>); 4] = [
        (Fankai, hit.fankai_id.map(|id| id.to_string())),
        (TvdbSeries, hit.tvdb_id.map(|id| id.to_string())),
        (
            if hit.kind == MediaKind::Series {
                TmdbTv
            } else {
                TmdbMovie
            },
            hit.tmdb_id.map(|id| id.to_string()),
        ),
        (Imdb, hit.imdb_id.clone()),
    ];

    for (source, value) in lookups {
        let Some(value) = value else { continue };

        if matches!(
            crate::db::repo::item::find_id_by_external(&state.db, source, &value).await,
            Ok(Some(_))
        ) {
            return true;
        }
    }

    false
}

/// The sources a search can be sent to alone, by kind.
const SERIES_SOURCES: &[&str] = &[names::TMDB, names::TVDB, names::SKYHOOK, names::FANKAI];
const MOVIE_SOURCES: &[&str] = &[names::TMDB, names::RADARR];

/// The source asked for, as long as it is one and it is on.
fn chosen(state: &AppState, source: &str) -> AppResult<&'static str> {
    let known = SERIES_SOURCES
        .iter()
        .chain(MOVIE_SOURCES)
        .copied()
        .find(|known| *known == source)
        .ok_or_else(|| AppError::BadRequest(format!("{source} cannot be searched on its own")))?;

    if !service::gather::switched_on(state, known) {
        return Err(AppError::BadRequest(format!("{known} is switched off")));
    }
    Ok(known)
}

fn is_lookup(term: &str) -> bool {
    !matches!(
        service::ids::classify(term),
        service::ids::TermLookup::Text(_)
    )
}

// ─── a batch ─────────────────────────────────────────────────────────────────

/// Lines to set beside the catalogue.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchPreviewRequest {
    /// A work a line: an id, or its page at a provider. Two thousand at the
    /// most.
    pub lines: Vec<Line>,
    /// What a TMDB id stands for where its line does not say: `movie` or
    /// `series`.
    #[serde(default)]
    pub kind: Option<MediaKind>,
}

/// Set a batch's lines beside the catalogue before anything is imported:
/// what each names, whether it is held already, and — for the first dozen
/// seconds' worth — whether a provider has it, by its title. Nothing is
/// fetched to say so, and nothing is kept.
#[utoipa::path(
    post, path = "/items/import/preview", tag = TAG,
    operation_id = "discover_preview_batch",
    request_body = BatchPreviewRequest,
    responses(
        (status = 200, body = Preview),
        (status = 400, description = "More than two thousand lines"),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn preview_batch(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<BatchPreviewRequest>,
) -> AppResult<Json<Preview>> {
    identity.require_write()?;
    too_many(&request.lines)?;
    Ok(Json(
        import::preview(&state, &request.lines, request.kind).await?,
    ))
}

/// A batch to import.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchRequest {
    /// A work a line, as the preview reads them. Two thousand at the most.
    pub lines: Vec<Line>,
    /// What a TMDB id stands for where its line does not say.
    #[serde(default)]
    pub kind: Option<MediaKind>,
    /// A language the episode texts of each series imported are fetched in
    /// at once, besides the catalogue's own: `fr`, `pt-BR`.
    #[serde(default)]
    pub language: Option<String>,
    /// The hand-made selections the works go into, held already or
    /// imported, by id or slug. Twenty at the most.
    #[serde(default)]
    pub lists: Vec<String>,
    /// Whether the works imported are switched on; those held already keep
    /// what they are. On when absent.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// What a batch started as.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchStarted {
    /// The run, as the tasks page follows it.
    pub job_id: String,
    /// How many works it takes, each once.
    pub works: usize,
    /// How many lines named none: not an id, or a TMDB id of no kind.
    pub passed_over: usize,
}

/// Import a batch: each line's work, in order, as a run of the task
/// `import.batch` that the tasks page follows — what is held passed over,
/// what is not fetched and stored, each written down. One batch at a time.
#[utoipa::path(
    post, path = "/items/import/batch", tag = TAG,
    operation_id = "discover_import_batch",
    request_body = BatchRequest,
    responses(
        (status = 202, body = BatchStarted),
        (status = 400, description = "No line names a work, more than two thousand lines, a language that is none, or a selection that is not a hand-made one"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 409, description = "A batch is being imported already, or a backup is being restored"),
    ),
)]
async fn import_batch(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<BatchRequest>,
) -> AppResult<(StatusCode, Json<BatchStarted>)> {
    // What it starts is followed on the tasks page, which is theirs.
    identity.require_admin()?;
    too_many(&request.lines)?;

    let language = match request.language.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => Some(service::language::tag(raw).ok_or_else(|| {
            AppError::BadRequest(format!("{raw} is not a language: write it fr, de or pt-BR"))
        })?),
    };
    // Each selection is looked up before anything starts: a handful, not
    // whatever a request may carry.
    if request.lists.len() > MOST_LISTS {
        return Err(AppError::BadRequest(format!(
            "a batch goes into {MOST_LISTS} selections at the most; this one names {}",
            request.lists.len()
        )));
    }
    let mut lists = Vec::new();
    for key in &request.lists {
        lists.push(hand_made(&state, key).await?);
    }

    let mut wanted = Vec::new();
    let mut passed_over = 0;
    for line in &request.lines {
        match import::read_line(line, request.kind) {
            Ok(one) => wanted.push(one),
            Err(Unread::Nothing) => {}
            Err(Unread::Unreadable | Unread::NeedsKind) => passed_over += 1,
        }
    }
    let mut seen = std::collections::HashSet::new();
    wanted.retain(|one| seen.insert(one.clone()));
    let works = wanted.len();

    let job_id = import::start(
        &state,
        wanted,
        import::Options {
            language,
            lists,
            enabled: request.enabled.unwrap_or(true),
        },
        &identity.label(),
    )
    .await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::TaskStarted,
            target: Some(import::KIND),
            detail: Some(&format!("{works} works asked; run {job_id}")),
        },
    )
    .await;

    Ok((
        StatusCode::ACCEPTED,
        Json(BatchStarted {
            job_id,
            works,
            passed_over,
        }),
    ))
}

/// How many selections a batch may go into.
const MOST_LISTS: usize = 20;

fn too_many(lines: &[Line]) -> AppResult<()> {
    if lines.len() > import::MOST_LINES {
        return Err(AppError::BadRequest(format!(
            "a batch takes {} lines at the most; this one has {}",
            import::MOST_LINES,
            lines.len()
        )));
    }
    Ok(())
}

/// A hand-made selection, by id or slug: its id.
pub(crate) async fn hand_made(state: &AppState, key: &str) -> AppResult<String> {
    let list = match crate::db::repo::list::get(&state.db, key).await? {
        Some(list) => Some(list),
        None => crate::db::repo::list::by_slug(&state.db, key).await?,
    }
    .ok_or_else(|| AppError::BadRequest(format!("there is no selection {key}")))?;
    if list.mode != crate::db::repo::list::ListMode::Manual {
        return Err(AppError::BadRequest(format!(
            "{} is composed by a filter, and holds no works of its own",
            list.name
        )));
    }
    Ok(list.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::native::testing::server;

    fn lines(texts: &[&str]) -> Vec<Line> {
        texts
            .iter()
            .map(|text| Line {
                text: (*text).to_string(),
                ..Line::default()
            })
            .collect()
    }

    /// A film held here, by its TMDB id.
    async fn held_film(state: &AppState, tmdb: i64, title: &str) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.id = crate::db::new_id();
        item.title = title.into();
        item.external_ids.tmdb = Some(tmdb);
        item.created_at = crate::db::now();
        item.updated_at = crate::db::now();
        item.slug = crate::domain::make_slug(&item.title, None);
        crate::db::repo::item::upsert(
            &state.db,
            crate::db::repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();
        item
    }

    #[tokio::test]
    async fn a_batch_is_set_beside_the_catalogue_before_it_is_imported() {
        let state = server().await;
        let heat = held_film(&state, 949, "Heat").await;

        let Json(preview) = preview_batch(
            State(state.clone()),
            Extension(Identity::Anonymous),
            Json(BatchPreviewRequest {
                lines: lines(&[
                    "tmdb:949  # Heat (1995)",
                    "https://www.themoviedb.org/movie/949-heat",
                    "tmdb:603",
                    "# a comment",
                    "The Matrix",
                    "tvdb:81189",
                ]),
                kind: Some(MediaKind::Movie),
            }),
        )
        .await
        .expect("a preview");

        let statuses: Vec<import::Status> = preview.lines.iter().map(|l| l.status).collect();
        assert_eq!(
            statuses,
            [
                import::Status::Held,
                import::Status::Repeated,
                import::Status::Candidate,
                import::Status::Unreadable,
                import::Status::Candidate,
            ],
            "the comment is passed over without a word"
        );
        assert_eq!(
            preview.lines[0].work.as_ref().map(|w| w.id.as_str()),
            Some(heat.id.as_str())
        );
        assert_eq!(
            preview.lines[0].work.as_ref().map(|w| w.title.as_str()),
            Some("Heat")
        );
        assert_eq!(preview.counts.held, 1);
        assert_eq!(preview.counts.candidate, 2);
        // No provider to ask, here: the candidates are looked up on import.
        assert!(!preview.lines[2].checked);

        // A TMDB id of no kind is said so.
        let Json(unsure) = preview_batch(
            State(state.clone()),
            Extension(Identity::Anonymous),
            Json(BatchPreviewRequest {
                lines: lines(&["tmdb:603"]),
                kind: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(unsure.lines[0].status, import::Status::NeedsKind);

        // Too many lines.
        let long: Vec<String> = (0..=import::MOST_LINES)
            .map(|n| format!("tmdb:{n}"))
            .collect();
        let long: Vec<&str> = long.iter().map(String::as_str).collect();
        assert!(matches!(
            preview_batch(
                State(state.clone()),
                Extension(Identity::Anonymous),
                Json(BatchPreviewRequest {
                    lines: lines(&long),
                    kind: Some(MediaKind::Movie),
                }),
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
    }

    #[tokio::test]
    async fn a_batch_of_held_works_runs_through_and_fills_a_selection() {
        let state = server().await;
        let heat = held_film(&state, 949, "Heat").await;
        let ronin = held_film(&state, 8195, "Ronin").await;
        let list = crate::db::repo::list::create(
            &state.db,
            crate::db::repo::list::ListFields {
                slug: "mann",
                name: "Mann",
                description: None,
                kind: crate::db::repo::list::ListKind::Movie,
                mode: crate::db::repo::list::ListMode::Manual,
                filter: None,
                is_public: true,
                cover: None,
            },
            None,
        )
        .await
        .unwrap();

        let started = |lines: Vec<Line>, lists: Vec<String>, language: Option<String>| {
            import_batch(
                State(state.clone()),
                Extension(Identity::Anonymous),
                ClientIp::NONE,
                Json(BatchRequest {
                    lines,
                    kind: Some(MediaKind::Movie),
                    language,
                    lists,
                    enabled: None,
                }),
            )
        };

        // Nothing to import; a language that is none; a selection that is
        // none.
        assert!(matches!(
            started(lines(&["nothing"]), vec![], None).await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            started(lines(&["tmdb:949"]), vec![], Some("not a language".into())).await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            started(lines(&["tmdb:949"]), vec!["nowhere".into()], None).await,
            Err(AppError::BadRequest(_))
        ));

        let (status, Json(run)) = started(
            lines(&["tmdb:949", "tmdb:8195", "tmdb:949", "not an id"]),
            vec!["mann".into()],
            None,
        )
        .await
        .expect("started");
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(run.works, 2, "each work once");
        assert_eq!(run.passed_over, 1);

        // Followed until it ends.
        let finished = async {
            loop {
                let job = crate::db::repo::job::get(&state.db, &run.job_id)
                    .await
                    .unwrap()
                    .unwrap();
                if job.status != crate::db::repo::job::Status::Running {
                    return job;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        };
        let job = tokio::time::timeout(std::time::Duration::from_secs(20), finished)
            .await
            .expect("the run ends");
        assert_eq!(
            job.status,
            crate::db::repo::job::Status::Succeeded,
            "{job:?}"
        );
        assert_eq!(
            job.detail.as_deref(),
            Some("2 already here, 0 imported, 0 not found, 0 failed, of 2")
        );
        let entries = crate::db::repo::job::entries(&state.db, &run.job_id, 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title.as_deref(), Some("Heat"));
        // The selection holds them, in the batch's order.
        assert_eq!(
            crate::db::repo::list::member_ids(&state.db, &list.id)
                .await
                .unwrap(),
            vec![heat.id, ronin.id]
        );
    }

    #[tokio::test]
    async fn a_file_of_five_hundred_ids_is_one_run_to_its_end() {
        let state = server().await;
        // Five hundred films held here, and a file naming each: set beside
        // the catalogue whole, then one run with an entry a work, which
        // fills a selection to the most it holds.
        let mut texts = Vec::new();
        for n in 1..=500 {
            held_film(&state, 10_000 + n, &format!("Film {n}")).await;
            texts.push(format!("tmdb:{}", 10_000 + n));
        }
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        let list = crate::db::repo::list::create(
            &state.db,
            crate::db::repo::list::ListFields {
                slug: "five-hundred",
                name: "Five hundred",
                description: None,
                kind: crate::db::repo::list::ListKind::Movie,
                mode: crate::db::repo::list::ListMode::Manual,
                filter: None,
                is_public: true,
                cover: None,
            },
            None,
        )
        .await
        .unwrap();

        let Json(preview) = preview_batch(
            State(state.clone()),
            Extension(Identity::Anonymous),
            Json(BatchPreviewRequest {
                lines: lines(&texts),
                kind: Some(MediaKind::Movie),
            }),
        )
        .await
        .expect("a preview");
        assert_eq!(preview.lines.len(), 500);
        assert_eq!(preview.counts.held, 500);

        let (status, Json(run)) = import_batch(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Json(BatchRequest {
                lines: lines(&texts),
                kind: Some(MediaKind::Movie),
                language: None,
                lists: vec!["five-hundred".into()],
                enabled: None,
            }),
        )
        .await
        .expect("started");
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(run.works, 500);

        let finished = async {
            loop {
                let job = crate::db::repo::job::get(&state.db, &run.job_id)
                    .await
                    .unwrap()
                    .unwrap();
                if job.status != crate::db::repo::job::Status::Running {
                    return job;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        };
        let job = tokio::time::timeout(std::time::Duration::from_secs(120), finished)
            .await
            .expect("the run ends");
        assert_eq!(
            job.detail.as_deref(),
            Some("500 already here, 0 imported, 0 not found, 0 failed, of 500"),
            "{job:?}"
        );
        assert_eq!(
            crate::db::repo::job::entries(&state.db, &run.job_id, 1_000)
                .await
                .unwrap()
                .len(),
            500
        );
        assert_eq!(
            crate::db::repo::list::member_ids(&state.db, &list.id)
                .await
                .unwrap()
                .len(),
            500
        );
    }
}

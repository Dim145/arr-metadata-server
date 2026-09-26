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
    providers::names,
    service,
    state::AppState,
};

pub const TAG: &str = "Discover";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(search))
        .routes(routes!(import))
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
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster: Option<String>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    /// Fankai's id, for a Fan-Kai production.
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
    require_write(&identity)?;

    // A page pasted whole is the lookup it stands for, and says which kind it
    // is when its address does.
    let (term, kind_of_link) = match from_link(&query.term) {
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
    require_write(&identity)?;

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

/// The lookup a provider's page stands for: its address pasted whole, as the
/// `prefix:id` term the search already understands, and the kind of work
/// when the address says.
fn from_link(term: &str) -> Option<(String, Option<MediaKind>)> {
    let url = url::Url::parse(term.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }

    let host = url.host_str()?.to_ascii_lowercase();
    let host = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("m."))
        .unwrap_or(&host);
    let parts: Vec<&str> = url
        .path_segments()
        .map(|segments| segments.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

    // `1396-breaking-bad`, `21`: the number a slug starts with.
    let number = |segment: &str| -> Option<i64> {
        let digits: String = segment.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };

    match (host, parts.as_slice()) {
        ("themoviedb.org", ["tv", id, ..]) => {
            Some((format!("tmdb:{}", number(id)?), Some(MediaKind::Series)))
        }
        ("themoviedb.org", ["movie", id, ..]) => {
            Some((format!("tmdb:{}", number(id)?), Some(MediaKind::Movie)))
        }
        // In the reader's language too: `/fr/title/tt…`.
        ("imdb.com", ["title", id, ..]) | ("imdb.com", [_, "title", id, ..]) if is_tconst(id) => {
            Some((format!("imdb:{id}"), None))
        }
        ("anilist.co", ["anime", id, ..]) => {
            Some((format!("anilist:{}", number(id)?), Some(MediaKind::Series)))
        }
        ("myanimelist.net", ["anime", id, ..]) => {
            Some((format!("mal:{}", number(id)?), Some(MediaKind::Series)))
        }
        ("fankai.fr", ["productions", id, ..]) => {
            Some((format!("fankai:{}", number(id)?), Some(MediaKind::Series)))
        }
        // TheTVDB's pages are named by slug; only its older addresses, and the
        // ones it redirects through, carry the number.
        ("thetvdb.com", ["dereferrer", "series", id, ..]) | ("thetvdb.com", ["series", id, ..]) => {
            number(id)
                .filter(|_| id.bytes().all(|b| b.is_ascii_digit()))
                .map(|id| (format!("tvdb:{id}"), Some(MediaKind::Series)))
        }
        ("thetvdb.com", _) => url
            .query_pairs()
            .find(|(key, _)| key == "id" || key == "seriesid")
            .and_then(|(_, value)| value.parse::<i64>().ok())
            .map(|id| (format!("tvdb:{id}"), Some(MediaKind::Series))),
        _ => None,
    }
}

/// `tt0903747`: an IMDb title's id, and nothing else.
fn is_tconst(id: &str) -> bool {
    id.len() > 2 && id.starts_with("tt") && id[2..].bytes().all(|b| b.is_ascii_digit())
}

fn require_write(identity: &Identity) -> AppResult<()> {
    identity
        .can_write()
        .then_some(())
        .ok_or(AppError::Forbidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_page_is_the_lookup_it_stands_for() {
        let cases = [
            (
                "https://www.themoviedb.org/tv/1396-breaking-bad?language=fr",
                "tmdb:1396",
                Some(MediaKind::Series),
            ),
            (
                "https://www.themoviedb.org/movie/603-the-matrix",
                "tmdb:603",
                Some(MediaKind::Movie),
            ),
            (
                "https://m.imdb.com/title/tt0903747/",
                "imdb:tt0903747",
                None,
            ),
            (
                "https://www.imdb.com/fr/title/tt0903747/?ref_=nv_sr",
                "imdb:tt0903747",
                None,
            ),
            (
                "https://anilist.co/anime/21/ONE-PIECE/",
                "anilist:21",
                Some(MediaKind::Series),
            ),
            (
                "https://myanimelist.net/anime/21/One_Piece",
                "mal:21",
                Some(MediaKind::Series),
            ),
            (
                "https://fankai.fr/productions/42",
                "fankai:42",
                Some(MediaKind::Series),
            ),
            (
                "https://thetvdb.com/dereferrer/series/81189",
                "tvdb:81189",
                Some(MediaKind::Series),
            ),
            (
                "https://thetvdb.com/?tab=series&id=81189",
                "tvdb:81189",
                Some(MediaKind::Series),
            ),
        ];
        for (link, lookup, kind) in cases {
            assert_eq!(from_link(link), Some((lookup.to_string(), kind)), "{link}");
        }
    }

    #[test]
    fn anything_else_is_searched_as_typed() {
        for term in [
            "Breaking Bad",
            "tvdb:81189",
            "https://thetvdb.com/series/breaking-bad",
            "https://www.imdb.com/title/nm0000123/",
            "https://example.com/tv/1396",
            "ftp://themoviedb.org/tv/1396",
        ] {
            assert_eq!(from_link(term), None, "{term}");
        }
    }
}

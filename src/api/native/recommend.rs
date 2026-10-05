//! What goes with a work: the catalogue's own works in the same vein, TMDB's
//! suggestions for whoever maintains it, and the collections its films
//! belong to.

use std::cmp::Ordering;

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    error::{AppError, AppResult},
    providers::tmdb,
    service,
    state::AppState,
};

const TAG: &str = super::items::TAG;

/// How many works in the same vein a page shows, and how many the store is
/// asked for before they are weighed.
const SIMILAR: usize = 12;
const CANDIDATES: i64 = 80;

/// The least a work must have in common to be in the same vein at all.
const LEAST_AFFINITY: i32 = 3;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(similar))
        .routes(routes!(suggestions))
        .routes(routes!(collections))
        .routes(routes!(collection))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LanguageQuery {
    /// Titles in this language, where a translation is held.
    pub language: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Similar {
    /// The works, drawn as a list is: artwork and scores, no episodes.
    pub items: Vec<MediaItem>,
}

// ─── in the same vein ────────────────────────────────────────────────────────

/// The catalogue's own works in the same vein as one: its kind, sharing its
/// genres, keywords, network or studio, language or decade, the most alike
/// first. A film's own collection is left to the collection.
#[utoipa::path(
    get, path = "/items/{id}/similar", tag = TAG,
    params(("id" = String, Path, description = "The work's id"), LanguageQuery),
    responses(
        (status = 200, body = Similar),
        (status = 404, description = "No such work, or none this caller may see"),
    ),
)]
async fn similar(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(query): Query<LanguageQuery>,
) -> AppResult<Json<Similar>> {
    let language = crate::api::extract::language(query.language.as_deref())?;
    let item = super::visible_work(&state, &identity, &id).await?;
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);

    // The candidates: the same kind, sharing the first genre — the one a
    // provider files a work under first.
    let Some(genre) = item.genres.first().cloned() else {
        return Ok(Json(Similar { items: Vec::new() }));
    };
    let candidates = repo::item::search(
        &state.db,
        &repo::item::Query {
            kind: Some(item.kind),
            genres: vec![genre],
            include_adult: adult,
            limit: CANDIDATES,
            ..Default::default()
        },
    )
    .await?;

    let mut weighed: Vec<(i32, f64, MediaItem)> = candidates
        .into_iter()
        .filter(|other| other.id != item.id)
        .filter(|other| {
            other.collection_tmdb_id.is_none()
                || other.collection_tmdb_id != item.collection_tmdb_id
        })
        .map(|other| {
            (
                affinity(&item, &other),
                other.popularity.unwrap_or(0.0),
                other,
            )
        })
        .filter(|(score, _, _)| *score >= LEAST_AFFINITY)
        .collect();
    weighed.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal))
    });
    let mut items: Vec<MediaItem> = weighed
        .into_iter()
        .take(SIMILAR)
        .map(|(_, _, other)| other)
        .collect();
    cards(&state, &identity, &mut items, language.as_deref()).await?;
    Ok(Json(Similar { items }))
}

/// How much two works have in common, in points: a shared genre two, a
/// shared keyword three, the same network or studio two, the same original
/// language one, the same decade one.
fn affinity(work: &MediaItem, other: &MediaItem) -> i32 {
    let shared = |a: &[String], b: &[String]| {
        a.iter()
            .filter(|x| b.iter().any(|y| y.eq_ignore_ascii_case(x)))
            .count() as i32
    };
    let mut score =
        2 * shared(&work.genres, &other.genres) + 3 * shared(&work.keywords, &other.keywords);
    let same = |a: &Option<String>, b: &Option<String>| matches!((a, b), (Some(a), Some(b)) if !a.is_empty() && a.eq_ignore_ascii_case(b));
    if same(&work.network, &other.network) || same(&work.studio, &other.studio) {
        score += 2;
    }
    if same(&work.original_language, &other.original_language) {
        score += 1;
    }
    if let (Some(a), Some(b)) = (work.year, other.year)
        && a / 10 == b / 10
    {
        score += 1;
    }
    score
}

// ─── what TMDB suggests, for whoever maintains the catalogue ─────────────────

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub tmdb_id: i64,
    pub kind: MediaKind,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster: Option<String>,
    pub score: Option<f64>,
    /// The catalogue's own id, where the work is already in it.
    pub held: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Suggestions {
    pub suggestions: Vec<Suggestion>,
}

/// What TMDB recommends beside a work, for whoever maintains the catalogue
/// to import from — each marked where it is already held.
#[utoipa::path(
    get, path = "/items/{id}/suggestions", tag = TAG,
    params(("id" = String, Path, description = "The work's id")),
    responses(
        (status = 200, body = Suggestions),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
        (status = 503, description = "No TMDB key is configured"),
    ),
)]
async fn suggestions(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<Suggestions>> {
    if !identity.can_write() {
        return Err(AppError::Forbidden);
    }
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    let Some(tmdb_id) = item.external_ids.tmdb else {
        return Ok(Json(Suggestions {
            suggestions: Vec::new(),
        }));
    };
    if !state.tmdb.is_configured() {
        return Err(AppError::ProviderNotConfigured);
    }

    let key = format!("suggest:{}:{tmdb_id}", item.kind.as_str());
    let results = cached_value(&state, &key, || async {
        state
            .tmdb
            .recommendations(item.kind, tmdb_id)
            .await
            .map(|v| v.unwrap_or(Value::Array(Vec::new())))
    })
    .await?;

    let mut suggestions: Vec<Suggestion> = results
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| summary(e, item.kind))
                .collect()
        })
        .unwrap_or_default();
    // Marked where the catalogue holds them already, so the button offered
    // is the right one.
    let source = ExternalSource::tmdb_for(item.kind);
    let values: Vec<String> = suggestions.iter().map(|s| s.tmdb_id.to_string()).collect();
    let held = repo::item::held_external_ids(&state.db, source, &values).await?;
    for suggestion in &mut suggestions {
        suggestion.held = held.get(&suggestion.tmdb_id.to_string()).cloned();
    }
    Ok(Json(Suggestions { suggestions }))
}

/// One of TMDB's summaries, whichever kind it is written for.
fn summary(entry: &Value, kind: MediaKind) -> Option<Suggestion> {
    let text = |name: &str| entry.get(name).and_then(Value::as_str).map(str::to_string);
    let date = text("release_date").or_else(|| text("first_air_date"));
    Some(Suggestion {
        tmdb_id: entry.get("id").and_then(Value::as_i64)?,
        kind,
        title: text("title")
            .or_else(|| text("name"))
            .filter(|t| !t.is_empty())?,
        year: date
            .as_deref()
            .and_then(|d| d.get(..4))
            .and_then(|y| y.parse().ok()),
        overview: text("overview").filter(|o| !o.trim().is_empty()),
        poster: text("poster_path").map(|p| tmdb::image_url(&p)),
        score: entry
            .get("vote_average")
            .and_then(Value::as_f64)
            .filter(|s| *s > 0.0),
        held: None,
    })
}

// ─── collections ─────────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionCard {
    pub tmdb_id: i64,
    /// TMDB's name for it, where a key lets it be asked.
    pub name: Option<String>,
    pub poster: Option<String>,
    /// How many of its films the catalogue holds.
    pub count: i64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Collections {
    pub collections: Vec<CollectionCard>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionPart {
    pub tmdb_id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub poster: Option<String>,
    /// The catalogue's own id, where the film is in it.
    pub held: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionPage {
    pub tmdb_id: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    /// The films the catalogue holds, in release order, drawn as a list is.
    pub items: Vec<MediaItem>,
    /// Every film of the collection as TMDB lists it, marked where it is held.
    pub parts: Vec<CollectionPart>,
}

/// The collections the catalogue's films belong to, most films first.
#[utoipa::path(
    get, path = "/collections", tag = TAG,
    params(LanguageQuery),
    responses((status = 200, body = Collections)),
)]
async fn collections(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<LanguageQuery>,
) -> AppResult<Json<Collections>> {
    let asked = crate::api::extract::language(query.language.as_deref())?;
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let language = language_of(&state, &identity, asked.as_deref());
    let held = repo::item::collections(&state.db, adult).await?;
    // Named all at once — TMDB's own gate on concurrency bounds it — rather
    // than one after another, on a page every reader of the lists opens.
    let names = futures::future::join_all(
        held.iter()
            .map(|(tmdb_id, _)| named(&state, *tmdb_id, &language)),
    )
    .await;
    let collections = held
        .into_iter()
        .zip(names)
        .map(|((tmdb_id, count), named)| CollectionCard {
            tmdb_id,
            name: named
                .as_ref()
                .and_then(|n| n.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string),
            poster: named
                .as_ref()
                .and_then(|n| n.get("poster_path"))
                .and_then(Value::as_str)
                .map(tmdb::image_url),
            count,
        })
        .collect();
    Ok(Json(Collections { collections }))
}

/// One collection: what the catalogue holds of it, and what TMDB lists.
#[utoipa::path(
    get, path = "/collections/{tmdbId}", tag = TAG,
    params(("tmdbId" = i64, Path, description = "TMDB's id for the collection"), LanguageQuery),
    responses(
        (status = 200, body = CollectionPage),
        (status = 404, description = "The catalogue holds nothing of it"),
    ),
)]
async fn collection(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(tmdb_id): Path<i64>,
    Query(query): Query<LanguageQuery>,
) -> AppResult<Json<CollectionPage>> {
    let asked = crate::api::extract::language(query.language.as_deref())?;
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let mut items = repo::item::search(
        &state.db,
        &repo::item::Query {
            kind: Some(MediaKind::Movie),
            collection: Some(tmdb_id),
            include_adult: adult,
            sort: repo::item::Sort::Release,
            descending: Some(false),
            limit: 100,
            ..Default::default()
        },
    )
    .await?;
    if items.is_empty() && !identity.can_write() {
        return Err(AppError::NotFound);
    }
    cards(&state, &identity, &mut items, asked.as_deref()).await?;

    let language = language_of(&state, &identity, asked.as_deref());
    let named = named(&state, tmdb_id, &language).await;
    let field = |name: &str| {
        named
            .as_ref()
            .and_then(|n| n.get(name))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let mut parts: Vec<CollectionPart> = named
        .as_ref()
        .and_then(|n| n.get("parts"))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| summary(e, MediaKind::Movie))
                .map(|s| CollectionPart {
                    tmdb_id: s.tmdb_id,
                    title: s.title,
                    year: s.year,
                    poster: s.poster,
                    held: None,
                })
                .collect()
        })
        .unwrap_or_default();
    parts.sort_by_key(|p| p.year.unwrap_or(i32::MAX));
    for part in &mut parts {
        part.held = items
            .iter()
            .find(|i| i.external_ids.tmdb == Some(part.tmdb_id))
            .map(|i| i.id.clone());
    }

    Ok(Json(CollectionPage {
        tmdb_id,
        name: field("name"),
        overview: field("overview").filter(|o| !o.trim().is_empty()),
        poster: field("poster_path").map(|p| tmdb::image_url(&p)),
        backdrop: field("backdrop_path").map(|p| tmdb::image_url(&p)),
        items,
        parts,
    }))
}

/// TMDB's own record of a collection — name, pictures, parts — in the
/// reader's language, kept a day; nothing where no key lets it be asked or
/// TMDB has none.
async fn named(state: &AppState, tmdb_id: i64, language: &str) -> Option<Value> {
    if !state.tmdb.is_configured() {
        return None;
    }
    let key = format!("collection:{tmdb_id}:{language}");
    cached_value(state, &key, || async {
        state
            .tmdb
            .collection_raw(tmdb_id, language)
            .await
            .map(|v| v.unwrap_or(Value::Null))
    })
    .await
    .ok()
    .filter(|v| !v.is_null())
}

/// The language TMDB is asked in for a reader, and what its answer is kept
/// under: the one asked for (a tag, checked where it arrived), as far as it
/// is a language TMDB has, or the one the reader is answered in everywhere
/// else — see [`service::language::tmdb_locale`]. Not whatever was asked:
/// each new string was a key no answer was kept under, and so one call to
/// TMDB for every collection held, on every request.
fn language_of(state: &AppState, identity: &Identity, asked: Option<&str>) -> String {
    service::language::tmdb_locale(
        asked,
        &state.language(identity.client_id(), identity.peer_id()),
    )
}

// ─── the pieces ──────────────────────────────────────────────────────────────

/// Works drawn as a list is: overrides, artwork, scores and the reader's
/// language applied, and nothing a reader may not see.
async fn cards(
    state: &AppState,
    identity: &Identity,
    items: &mut [MediaItem],
    language: Option<&str>,
) -> AppResult<()> {
    service::apply_overrides(state, items).await?;
    repo::item::load_artwork(&state.db, items).await?;
    service::overlay_imdb_many(state, items).await;
    if let Some(language) = language {
        for item in items.iter_mut() {
            service::language::apply_shallow(state, item, language);
        }
    }
    for item in items.iter_mut() {
        state.media.localize(item);
        service::as_card(item);
    }
    service::redact_for_reader(identity, items);
    Ok(())
}

/// How long a fetch that failed is not tried again for the same key.
const FAILED_FOR: std::time::Duration = std::time::Duration::from_secs(300);

/// The keys whose fetch failed lately, and when. In this process only: what
/// it saves is this process's calls.
static FAILED: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<String, std::time::Instant>>,
> = std::sync::LazyLock::new(Default::default);

/// Whether the fetch for `key` failed less than [`FAILED_FOR`] ago.
fn failed_lately(key: &str) -> bool {
    FAILED
        .lock()
        .get(key)
        .is_some_and(|at| at.elapsed() < FAILED_FOR)
}

/// Remember that the fetch for `key` failed, now.
fn failed(key: &str) {
    let mut failed = FAILED.lock();
    // Bounded: the keys are as many as collections, people and languages,
    // and those long past their wait are of no use.
    if failed.len() >= 4096 {
        failed.retain(|_, at| at.elapsed() < FAILED_FOR);
    }
    failed.insert(key.to_string(), std::time::Instant::now());
}

/// A JSON value from the day-long cache, or fetched and kept.
///
/// A fetch that failed is not tried again for the same key for a few
/// minutes: with TMDB out of reach, or answering 429, the collections page
/// asked it again for every collection held on every request, and kept it
/// throttled.
pub(crate) async fn cached_value<F, Fut>(state: &AppState, key: &str, fetch: F) -> AppResult<Value>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Value>>,
{
    if let Some(bytes) = state.caches.lists.get(key).await
        && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
    {
        return Ok(value);
    }
    if failed_lately(key) {
        return Err(AppError::UpstreamUnavailable(anyhow::anyhow!(
            "asked a moment ago, to no avail"
        )));
    }
    let value = match fetch().await {
        Ok(value) => value,
        Err(e) => {
            failed(key);
            return Err(AppError::UpstreamUnavailable(e));
        }
    };
    let bytes = Bytes::from(serde_json::to_vec(&value).unwrap_or_default());
    state.caches.lists.insert(key.to_string(), bytes).await;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn work(kind: MediaKind, genres: &[&str], keywords: &[&str]) -> MediaItem {
        let mut w = MediaItem::empty(kind);
        w.genres = genres.iter().map(|g| g.to_string()).collect();
        w.keywords = keywords.iter().map(|k| k.to_string()).collect();
        w
    }

    #[test]
    fn affinity_counts_what_two_works_share() {
        let mut dark = work(
            MediaKind::Series,
            &["Drama", "Sci-Fi & Fantasy"],
            &["time travel", "small town"],
        );
        dark.network = Some("Netflix".into());
        dark.original_language = Some("de".into());
        dark.year = Some(2017);
        let mut alike = work(
            MediaKind::Series,
            &["Sci-Fi & Fantasy", "Mystery"],
            &["time travel"],
        );
        alike.network = Some("netflix".into());
        alike.original_language = Some("en".into());
        alike.year = Some(2019);
        // One genre (2), one keyword (3), the network (2), the decade (1).
        assert_eq!(affinity(&dark, &alike), 8);
        let unlike = work(MediaKind::Series, &["Comedy"], &[]);
        assert_eq!(affinity(&dark, &unlike), 0);
    }

    #[test]
    fn a_summary_is_read_whichever_kind_it_is_written_for() {
        let film = summary(
            &json!({ "id": 238, "title": "The Godfather", "release_date": "1972-03-14", "overview": "…", "poster_path": "/p.jpg", "vote_average": 8.7 }),
            MediaKind::Movie,
        )
        .unwrap();
        assert_eq!(
            (film.tmdb_id, film.year, film.score),
            (238, Some(1972), Some(8.7))
        );
        assert!(
            film.poster
                .as_deref()
                .is_some_and(|p| p.ends_with("/p.jpg"))
        );
        let series = summary(
            &json!({ "id": 1396, "name": "Breaking Bad", "first_air_date": "2008-01-20", "vote_average": 0.0 }),
            MediaKind::Series,
        )
        .unwrap();
        assert_eq!(
            (series.title.as_str(), series.year, series.score),
            ("Breaking Bad", Some(2008), None)
        );
        assert!(summary(&json!({ "id": 1, "title": "" }), MediaKind::Movie).is_none());
        assert!(summary(&json!({ "title": "no id" }), MediaKind::Movie).is_none());
    }
}

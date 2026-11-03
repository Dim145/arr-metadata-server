//! The collections the catalogue's films belong to, kept as things of their
//! own.
//!
//! A film names its collection by TMDB's id (`collection_tmdb_id`), and
//! TMDB's record of the collection — its name, its synopsis, its pictures,
//! its films — is kept once for all of them: asked again when one of its
//! films is refreshed and the record kept is [`FRESH_FOR`] old, never more
//! often, in the language the catalogue is stored in and with its texts in
//! every other TMDB has them in. Radarr, the catalogue's pages and the NFO
//! export read the record kept; one the catalogue was never told of is asked
//! for once, by whichever of them wants it first, and kept from then on —
//! one TMDB has no record of is not asked about again for a day. Its
//! pictures are kept by the media store, as a work's are.

use std::collections::HashMap;

use anyhow::Result;

use crate::{
    db::repo,
    domain::{Collection, MediaItem, MediaKind},
    providers::tmdb::map as tmdb_map,
    state::AppState,
};

/// How long a collection kept stands before a refresh of one of its films
/// asks TMDB about it again: a collection changes when a film is added to
/// it, which a week's wait does not miss by much.
pub const FRESH_FOR: chrono::TimeDelta = chrono::TimeDelta::days(7);

/// A collection, by TMDB's id: the one kept, else TMDB's record of it asked
/// for now and kept. `None` where TMDB has no such collection, or cannot be
/// asked — no key — and none is kept.
pub async fn get(state: &AppState, tmdb_id: i64) -> Result<Option<Collection>> {
    if let Some(kept) = repo::collection::by_tmdb(&state.db, tmdb_id).await? {
        return Ok(Some(kept));
    }
    fetch(state, tmdb_id).await
}

/// Several collections, by TMDB's ids, without their films: those kept, and
/// the others asked of TMDB now and kept, all at once — TMDB's own gate on
/// how many are in flight bounds it. One that cannot be had is not there.
pub async fn many(state: &AppState, tmdb_ids: &[i64]) -> HashMap<i64, Collection> {
    let mut found = match repo::collection::by_tmdb_ids(&state.db, tmdb_ids).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "could not read the collections kept"
            );
            HashMap::new()
        }
    };
    let missing: Vec<i64> = tmdb_ids
        .iter()
        .copied()
        .filter(|id| !found.contains_key(id))
        .collect();
    let fetched =
        futures::future::join_all(missing.iter().map(|id| fetch_quietly(state, *id))).await;
    for collection in fetched.into_iter().flatten() {
        found.insert(collection.tmdb_id, collection);
    }
    found
}

/// After a film is refreshed: its collection asked of TMDB again when the
/// one kept is [`FRESH_FOR`] old, or when none is kept. Quiet, and never
/// while a refresh is only previewed: the film is refreshed whether or not
/// its collection can be.
pub async fn keep_for(state: &AppState, film: &MediaItem) {
    let Some(tmdb_id) = film
        .collection_tmdb_id
        .filter(|_| film.kind == MediaKind::Movie)
    else {
        return;
    };
    if crate::service::is_previewing() || !state.tmdb.is_configured() {
        return;
    }
    let due = match repo::collection::refreshed_at(&state.db, tmdb_id).await {
        Ok(at) => is_due(at.as_deref(), chrono::Utc::now()),
        Err(e) => {
            tracing::warn!(
                tmdb_id,
                error = format_args!("{e:#}"),
                "could not read when a collection was kept"
            );
            false
        }
    };
    if due {
        fetch_quietly(state, tmdb_id).await;
    }
}

/// [`keep_for`], on its own: what a film fetched for a client is answered
/// with does not wait for its collection. Never while a refresh is only
/// previewed, which a task on its own could not tell.
pub fn keep_for_later(state: &AppState, film: &MediaItem) {
    if film.collection_tmdb_id.is_none()
        || film.kind != MediaKind::Movie
        || crate::service::is_previewing()
    {
        return;
    }
    let state = state.clone();
    let mut film_of = MediaItem::empty(MediaKind::Movie);
    film_of.collection_tmdb_id = film.collection_tmdb_id;
    tokio::spawn(async move { keep_for(&state, &film_of).await });
}

/// Whether a collection last asked about `refreshed_at` is to be asked about
/// again `now`: never asked, or [`FRESH_FOR`] ago.
fn is_due(refreshed_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    refreshed_at
        .and_then(crate::db::parse_rfc3339)
        .is_none_or(|at| now - at >= FRESH_FOR)
}

/// How long a collection TMDB could not be asked about is not asked about
/// again: with TMDB out of reach, or answering 429, a page listing the
/// collections asked it again for every collection not kept, on every
/// request, and kept it throttled.
const FAILED_FOR: std::time::Duration = std::time::Duration::from_secs(300);

/// How long a collection TMDB has no record of is not asked about again: a
/// film naming one TMDB has since dropped asked for it at every page that
/// showed the film, and at each of its refreshes. TMDB fixes the film soon
/// enough, and it is asked again then.
const ABSENT_FOR: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// The collections TMDB could not be asked about lately, and when. In this
/// process only: what it saves is this process's calls.
static FAILED: std::sync::LazyLock<parking_lot::Mutex<HashMap<i64, std::time::Instant>>> =
    std::sync::LazyLock::new(Default::default);

/// The collections TMDB said lately it has no record of, and when: a 404, or
/// a record without a name. In this process only, as [`FAILED`].
static ABSENT: std::sync::LazyLock<parking_lot::Mutex<HashMap<i64, std::time::Instant>>> =
    std::sync::LazyLock::new(Default::default);

/// Whether `tmdb_id` was noted in `noted` within `wait`.
fn noted_within(
    noted: &parking_lot::Mutex<HashMap<i64, std::time::Instant>>,
    tmdb_id: i64,
    wait: std::time::Duration,
) -> bool {
    noted
        .lock()
        .get(&tmdb_id)
        .is_some_and(|at| at.elapsed() < wait)
}

/// Note `tmdb_id` in `noted`, now: those long past `wait` go first, when the
/// list is long.
fn note(
    noted: &parking_lot::Mutex<HashMap<i64, std::time::Instant>>,
    tmdb_id: i64,
    wait: std::time::Duration,
) {
    let mut noted = noted.lock();
    if noted.len() >= 4096 {
        noted.retain(|_, at| at.elapsed() < wait);
    }
    noted.insert(tmdb_id, std::time::Instant::now());
}

/// [`fetch`], its failure said and remembered for [`FAILED_FOR`], and
/// nothing more.
async fn fetch_quietly(state: &AppState, tmdb_id: i64) -> Option<Collection> {
    if noted_within(&FAILED, tmdb_id, FAILED_FOR) {
        return None;
    }
    match fetch(state, tmdb_id).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(
                tmdb_id,
                error = format_args!("{e:#}"),
                "could not ask TMDB about a collection"
            );
            note(&FAILED, tmdb_id, FAILED_FOR);
            None
        }
    }
}

/// Ask TMDB about a collection, keep what it says, and put its pictures in
/// line for the media store: the collection as kept. One asking at a time
/// for each: Radarr refreshes a saga's films together. `None` where TMDB
/// has no such collection — not asked again for [`ABSENT_FOR`] — or cannot
/// be asked.
async fn fetch(state: &AppState, tmdb_id: i64) -> Result<Option<Collection>> {
    if !state.tmdb.is_configured() || noted_within(&ABSENT, tmdb_id, ABSENT_FOR) {
        return Ok(None);
    }
    let _fetching = crate::service::FETCHING
        .lock(&format!("collection:tmdb:{tmdb_id}"))
        .await;
    // Kept meanwhile, by whoever held the lock.
    if let Some(kept) = repo::collection::by_tmdb(&state.db, tmdb_id).await?
        && !is_due(Some(&kept.refreshed_at), chrono::Utc::now())
    {
        return Ok(Some(kept));
    }

    let Some((raw, answer)) = state.tmdb.collection(tmdb_id).await? else {
        note(&ABSENT, tmdb_id, ABSENT_FOR);
        return Ok(None);
    };
    let collection = tmdb_map::collection(tmdb_id, &answer, &state.tmdb.asked_in());
    if collection.name.is_empty() {
        note(&ABSENT, tmdb_id, ABSENT_FOR);
        return Ok(None);
    }
    // TMDB's answer itself beside it: what the TMDB relay writes a document
    // of TMDB's shape from where the columns keep nothing (`wire::tmdb`).
    repo::collection::write_with(&state.db, &collection, Some(&raw)).await?;

    let pictures: Vec<&str> = [&collection.poster, &collection.backdrop]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    state.media.enqueue_pictures(state, &pictures).await;
    repo::collection::by_tmdb(&state.db, tmdb_id).await
}

/// The language a reader is answered a collection's texts in, as the model
/// keeps languages (ISO 639-3): a TMDB locale's, or a tag's.
pub fn language_of(locale: &str) -> String {
    crate::providers::lang::iso_639_1_to_3(crate::providers::lang::base_language(locale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_collection_is_asked_about_again_a_week_after_it_was() {
        let now = chrono::Utc::now();
        let ago = |days: i64| crate::db::to_rfc3339(now - chrono::TimeDelta::days(days));
        assert!(is_due(None, now), "never asked");
        assert!(!is_due(Some(&ago(1)), now));
        assert!(!is_due(Some(&ago(6)), now));
        assert!(is_due(Some(&ago(7)), now));
        assert!(is_due(Some("not a date"), now), "unreadable is never");
    }

    #[test]
    fn a_reader_s_language_is_the_model_s() {
        assert_eq!(language_of("fr-FR"), "fra");
        assert_eq!(language_of("en"), "eng");
    }

    /// TMDB at an address that answers nothing: a collection kept is read
    /// without asking it; one never kept is asked for, and one TMDB could
    /// not be asked about is not asked about again for a while.
    #[tokio::test]
    async fn what_is_kept_is_read_and_tmdb_asked_only_for_the_rest() {
        let (state, nowhere) = crate::service::testing::server().await;
        let mut kept = Collection {
            id: String::new(),
            tmdb_id: 9_200_001,
            name: "Kept Before".into(),
            overview: None,
            poster: None,
            backdrop: None,
            language: Some("eng".into()),
            translations: Vec::new(),
            parts: Vec::new(),
            refreshed_at: String::new(),
        };
        repo::collection::write(&state.db, &kept).await.unwrap();
        kept.tmdb_id = 9_200_002;
        kept.name = "Another".into();
        repo::collection::write(&state.db, &kept).await.unwrap();

        let found = get(&state, 9_200_001).await.unwrap().expect("kept");
        assert_eq!(found.name, "Kept Before");
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.collection_tmdb_id = Some(9_200_001);
        keep_for(&state, &film).await;
        assert_eq!(nowhere.asked(), 0, "kept, and not due");

        assert!(get(&state, 9_200_003).await.is_err(), "TMDB not answering");
        assert!(nowhere.asked() > 0);

        let found = many(&state, &[9_200_001, 9_200_002, 9_200_004]).await;
        assert_eq!(found.len(), 2);
        assert!(nowhere.asked() > 0, "the one never kept asked for");
        let found = many(&state, &[9_200_001, 9_200_002, 9_200_004]).await;
        assert_eq!(found.len(), 2);
        assert_eq!(nowhere.asked(), 0, "not again so soon");

        film.collection_tmdb_id = Some(9_200_004);
        keep_for(&state, &film).await;
        assert_eq!(nowhere.asked(), 0);
        film.kind = MediaKind::Series;
        film.collection_tmdb_id = Some(9_200_005);
        keep_for(&state, &film).await;
        assert_eq!(nowhere.asked(), 0, "a series is in no collection");
    }
}

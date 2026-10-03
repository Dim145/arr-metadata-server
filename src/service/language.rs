//! Serving a work in the language a client asked for.
//!
//! The canonical entity holds one language — whatever `AMS_TMDB_LANGUAGE` was
//! set to when it was fetched. Everything else is an overlay: translations are
//! stored beside it and applied at read time, so a French request and an English
//! request are the same row seen through different text.
//!
//! Sonarr puts the language in its URL and Radarr does not, which is why this is
//! applied per request rather than baked into the stored entity.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::{
    db::repo,
    domain::{MediaItem, MediaKind},
    providers::{
        lang::{base_language, iso_639_1_to_3},
        tmdb::models::Season,
    },
    state::AppState,
};

/// Normalise whatever a client sent into the key translations are stored under.
///
/// Clients send `fr`, `fr-FR` or `fra`; all three mean the same shelf.
pub fn normalize(requested: &str) -> String {
    let base = base_language(requested.trim());

    if base.len() == 3 {
        base.to_ascii_lowercase()
    } else {
        iso_639_1_to_3(base)
    }
}

/// Whether this is the language the entity is already stored in.
fn is_default(state: &AppState, language: &str) -> bool {
    normalize(&state.language(None, None)) == language
}

/// Overlay `item` with the requested language, fetching it if we have not yet.
///
/// Absent or partial translations leave the original text in place: a client
/// showing an English title is better than one showing a blank.
///
/// Nothing is fetched for a work switched off in the catalogue: like the work
/// itself (see [`crate::service::served_as_held`]), it is given what is held
/// in the language, as [`apply_stored`] gives it, and no provider is asked.
pub async fn apply(state: &AppState, item: &mut MediaItem, requested: &str) -> Result<()> {
    let language = normalize(requested);

    if language.is_empty() || is_default(state, &language) {
        return Ok(());
    }

    // Item-level translations arrive with the work itself, so they are already
    // here. Episode text is fetched per language, on first request — and only
    // once, however many requests arrive for it together: a season's worth of
    // calls per work per language is the price, and two French Sonarrs opening
    // the same series should not pay it twice. Checked again once through, for
    // the same reason as `FETCHING`. A fetch that came to less than a whole
    // answer is tried again later, not on the next request: see `store`.
    // Never for a series switched off: it is served as it is held, in every
    // language.
    if item.kind == MediaKind::Series
        && item.is_enabled
        && !item.episodes.is_empty()
        && repo::translation::is_due(&state.db, &item.id, &language).await?
    {
        let _fetching = crate::service::FETCHING
            .lock(&format!("episodes:{}:{language}", item.id))
            .await;

        if repo::translation::is_due(&state.db, &item.id, &language).await? {
            fetch_episodes(state, item, &language).await;
        }
    }

    overlay_item(item, &language);

    if item.kind == MediaKind::Series {
        overlay_episodes(state, item, &language).await?;
    }

    Ok(())
}

/// Overlay what is already held in the requested language, and ask no
/// provider for more.
///
/// For episodes drawn from many works at once — a calendar — where [`apply`]
/// would fetch each work's missing text in turn.
pub async fn apply_stored(state: &AppState, item: &mut MediaItem, requested: &str) -> Result<()> {
    let language = normalize(requested);

    if language.is_empty() || is_default(state, &language) {
        return Ok(());
    }

    overlay_item(item, &language);

    if item.kind == MediaKind::Series && !item.episodes.is_empty() {
        overlay_episodes(state, item, &language).await?;
    }

    Ok(())
}

/// Overlay only the work's own title and overview.
///
/// For lists: [`apply`] may fetch a season's worth of episode text per work,
/// which is right for one detail view and wrong for ten search results.
pub fn apply_shallow(state: &AppState, item: &mut MediaItem, requested: &str) {
    let language = normalize(requested);

    if language.is_empty() || is_default(state, &language) {
        return;
    }

    overlay_item(item, &language);
}

/// Replace the work's own title and overview from a stored translation.
fn overlay_item(item: &mut MediaItem, language: &str) {
    let Some(translation) = item.translations.iter().find(|t| t.language == language) else {
        return;
    };

    let title = translation.title.clone().filter(|t| !t.trim().is_empty());
    let overview = translation
        .overview
        .clone()
        .filter(|o| !o.trim().is_empty());

    if let Some(title) = title
        && !locked(item, "item/title")
    {
        item.title = title;
    }

    if let Some(overview) = overview
        && !locked(item, "item/overview")
    {
        item.overview = Some(overview);
    }
}

/// Whether a person has claimed this field.
///
/// A lock is final. Translating over it would mean a work someone renamed goes
/// back to the provider's name the moment a client asks in another language —
/// exactly what locking exists to prevent. Somebody who wants a different name
/// per language locks that language's field too.
fn locked(item: &MediaItem, path: &str) -> bool {
    item.locked_fields.iter().any(|f| f == path)
}

async fn overlay_episodes(state: &AppState, item: &mut MediaItem, language: &str) -> Result<()> {
    let texts = repo::translation::for_episodes(&state.db, &item.id, language).await?;

    if texts.is_empty() {
        return Ok(());
    }

    // Collected first: checking a lock needs the item, and the loop below holds
    // a mutable borrow of its episodes.
    let locks: Vec<String> = item.locked_fields.clone();
    let is_locked = |season: i32, number: i32, field: &str| {
        locks
            .iter()
            .any(|f| f == &format!("episode:{season}x{number}/{field}"))
    };

    for episode in &mut item.episodes {
        let Some(text) = texts.get(&(episode.season_number, episode.episode_number)) else {
            continue;
        };

        if !is_locked(episode.season_number, episode.episode_number, "title")
            && let Some(title) = text.title.as_deref().filter(|t| !t.trim().is_empty())
        {
            episode.title = title.to_string();
        }

        if !is_locked(episode.season_number, episode.episode_number, "overview")
            && let Some(overview) = text.overview.as_deref().filter(|o| !o.trim().is_empty())
        {
            episode.overview = Some(overview.to_string());
        }
    }

    Ok(())
}

/// Episode text fetched in one language, and whether it is the whole answer.
#[derive(Default)]
struct Fetched {
    texts: Vec<repo::translation::EpisodeText>,
    /// A provider asked did not answer, for a season or at all, or answered
    /// with what could not be read: `texts` is what the others said, not all
    /// there is.
    degraded: bool,
}

/// TMDB's episode text, one call per season.
async fn from_tmdb(state: &AppState, item: &MediaItem, language: &str) -> Fetched {
    let Some(tmdb_id) = item.external_ids.tmdb else {
        return Fetched::default();
    };
    if !state.tmdb.is_configured() {
        return Fetched::default();
    }

    // TMDB wants the two-letter form it was given.
    let requested = two_letter(language);

    let numbers: Vec<i32> = {
        let mut n: Vec<i32> = item.episodes.iter().map(|e| e.season_number).collect();
        n.sort_unstable();
        n.dedup();
        n
    };

    let (seasons, unanswered) = state
        .tmdb
        .tv_seasons_in(tmdb_id, &numbers, &requested)
        .await;

    Fetched {
        texts: tmdb_texts(item, &seasons),
        degraded: unanswered,
    }
}

/// TMDB's text for the work's episodes, under the work's own numbers.
///
/// The regular seasons by number, as the merge pairs them. A special by the
/// TMDB id the merge gave it, because TMDB numbers its specials its own way:
/// by number alone, *Rurouni Kenshin*'s first special on TheTVDB — a 1997
/// film — was named after TMDB's first, the series' last episode. A special
/// the merge paired with none of TMDB's gets no text from it; TheTVDB is
/// asked for that one instead.
fn tmdb_texts(item: &MediaItem, seasons: &[Season]) -> Vec<repo::translation::EpisodeText> {
    let specials: HashMap<i64, i32> = item
        .episodes
        .iter()
        .filter(|e| e.season_number == 0)
        .filter_map(|e| Some((e.tmdb_id?, e.episode_number)))
        .collect();

    seasons
        .iter()
        .flat_map(|s| s.episodes.iter())
        .filter_map(|e| {
            let episode_number = if e.season_number == 0 {
                *specials.get(&e.id?)?
            } else {
                e.episode_number
            };
            Some(repo::translation::EpisodeText {
                season_number: e.season_number,
                episode_number,
                // TMDB's "Episode 3" or "Épisode 3", which names an episode by
                // TMDB's own number, is no text: taken, it replaced a real
                // title, and dropped in one language only, it let the stand-in
                // the work is stored with through to Sonarr.
                title: e.title(),
                overview: e.overview.clone().filter(|o| !o.trim().is_empty()),
            })
        })
        .collect()
}

/// Whether one of the work's episodes still has no title or no overview in
/// `texts`: what TheTVDB is asked to fill.
///
/// Counted over the work's episodes, not over what TMDB sent: a special TMDB
/// has no text for under the work's numbering — one TheTVDB alone lists, or
/// one TMDB numbers its own way — is missing too, though everything TMDB did
/// send may be complete.
fn incomplete(item: &MediaItem, texts: &[repo::translation::EpisodeText]) -> bool {
    let complete: HashSet<(i32, i32)> = texts
        .iter()
        .filter(|t| t.title.is_some() && t.overview.is_some())
        .map(|t| (t.season_number, t.episode_number))
        .collect();

    item.episodes
        .iter()
        .any(|e| !complete.contains(&(e.season_number, e.episode_number)))
}

/// Fill what TMDB left empty from TheTVDB.
///
/// TMDB serves a handful of languages well and, for the rest, has only an
/// episode's number to give, put in words of the language; TheTVDB holds
/// dozens. Asking it second means a French request is answered even when TMDB
/// has no French, without displacing TMDB's text where it exists — the same
/// fill-do-not-replace rule the merge engine uses.
async fn fill_from_tvdb(state: &AppState, item: &MediaItem, language: &str, fetched: &mut Fetched) {
    let Some(tvdb_id) = item.external_ids.tvdb else {
        return;
    };

    // Nothing missing: skip the call.
    if !incomplete(item, &fetched.texts) {
        return;
    }

    let from_tvdb = match state.tvdb.episode_texts(tvdb_id, language).await {
        Ok(found) => found,
        Err(e) => {
            tracing::debug!(id = %item.id, %language, error = format_args!("{e:#}"), "TheTVDB had no episode text");
            fetched.degraded = true;
            return;
        }
    };

    let texts = &mut fetched.texts;
    for episode in from_tvdb {
        match texts.iter_mut().find(|t| {
            t.season_number == episode.season_number && t.episode_number == episode.episode_number
        }) {
            Some(existing) => {
                if existing.title.is_none() {
                    existing.title = episode.title;
                }
                if existing.overview.is_none() {
                    existing.overview = episode.overview;
                }
            }
            None => texts.push(repo::translation::EpisodeText {
                season_number: episode.season_number,
                episode_number: episode.episode_number,
                title: episode.title,
                overview: episode.overview,
            }),
        }
    }
}

/// Fetch and store every episode's text in one language.
///
/// One provider call per season, so this happens once per work per language and
/// is then answered from the database. A failure is logged and swallowed: the
/// caller still gets the work, with what is held in the language asked.
async fn fetch_episodes(state: &AppState, item: &MediaItem, language: &str) {
    tracing::info!(id = %item.id, %language, "fetching episode translations");

    let mut fetched = from_tmdb(state, item, language).await;

    fill_from_tvdb(state, item, language, &mut fetched).await;

    store(state, item, language, fetched).await;
}

/// Store what fetching a language brought, and record how far it got.
///
/// The text held in the language is replaced outright only by a whole
/// answer: every provider asked gave one, with something in it. Any other
/// replaces only what it says, and a provider that did not answer removes
/// nothing (see [`repo::translation::put_episodes`]). The language's text was
/// deleted and the answer written in its place whatever came back: with every
/// provider out of reach, the first request in a language since the work's
/// last refresh erased the text held in it — and marked the language
/// fetched, which kept the work served in its own language until the next
/// refresh.
///
/// Only a whole answer is marked fetched, too. One with nothing in it, or
/// not all there is, is tried again once the wait a failed refresh sets is
/// over ([`crate::service::retry_after_failure`]); until then the requests in
/// the language are served what is held, rather than each waiting on the
/// providers again.
async fn store(state: &AppState, item: &MediaItem, language: &str, fetched: Fetched) {
    let Fetched { texts, degraded } = fetched;
    let said = texts
        .iter()
        .any(|t| t.title.is_some() || t.overview.is_some());
    let whole = said && !degraded;

    if let Err(e) =
        repo::translation::put_episodes(&state.db, &item.id, language, &texts, whole).await
    {
        tracing::warn!(id = %item.id, %language, error = %e, "could not store episode translations");
        return;
    }

    let recorded = if whole {
        repo::translation::mark_fetched(&state.db, &item.id, language).await
    } else {
        let retry = crate::service::retry_after_failure(Some(item));
        tracing::info!(
            id = %item.id,
            %language,
            degraded,
            %retry,
            "episode text came back incomplete or empty; what is held is served until it is \
             tried again"
        );
        repo::translation::mark_unanswered(&state.db, &item.id, language, &retry).await
    };

    if let Err(e) = recorded {
        tracing::warn!(id = %item.id, %language, error = %e, "could not record the fetch");
    }
}

/// The two-letter code for a three-letter one, for talking to TMDB.
///
/// Only the languages TMDB actually serves need to round-trip; anything else
/// passes through and TMDB falls back on its own.
pub(crate) fn two_letter(language: &str) -> String {
    for code in TWO_LETTER_CODES {
        if iso_639_1_to_3(code) == language {
            return (*code).to_string();
        }
    }

    language.to_string()
}

/// Every two-letter code the conversion table knows, so the reverse lookup does
/// not need a second table to drift out of step with the first.
const TWO_LETTER_CODES: &[&str] = &[
    "aa", "ab", "af", "ak", "am", "ar", "as", "az", "ba", "be", "bg", "bm", "bn", "bo", "br", "bs",
    "ca", "ce", "co", "cs", "cy", "da", "de", "dv", "dz", "el", "en", "eo", "es", "et", "eu", "fa",
    "ff", "fi", "fj", "fo", "fr", "fy", "ga", "gd", "gl", "gn", "gu", "ha", "he", "hi", "hr", "ht",
    "hu", "hy", "id", "ig", "is", "it", "iu", "ja", "jv", "ka", "kk", "km", "kn", "ko", "ku", "ky",
    "la", "lb", "lo", "lt", "lv", "mg", "mi", "mk", "ml", "mn", "mr", "ms", "mt", "my", "nb", "ne",
    "nl", "nn", "no", "ny", "or", "pa", "pl", "ps", "pt", "qu", "rm", "ro", "ru", "rw", "sa", "sd",
    "se", "si", "sk", "sl", "sm", "sn", "so", "sq", "sr", "ss", "st", "su", "sv", "sw", "ta", "te",
    "tg", "th", "ti", "tk", "tl", "tn", "to", "tr", "ts", "tt", "ug", "uk", "ur", "uz", "ve", "vi",
    "wo", "xh", "yi", "yo", "zh", "zu",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Episode, Translation};

    #[test]
    fn every_spelling_of_a_language_lands_on_the_same_shelf() {
        assert_eq!(normalize("fr"), "fra");
        assert_eq!(normalize("fr-FR"), "fra");
        assert_eq!(normalize("fra"), "fra");
        assert_eq!(normalize("FR"), "fra");
        assert_eq!(normalize("  pt_BR  "), "por");
        assert_eq!(normalize("en-US"), "eng");
    }

    #[test]
    fn an_unknown_code_passes_through_rather_than_being_dropped() {
        assert_eq!(normalize("xx"), "xx");
        assert_eq!(normalize("qqq"), "qqq");
    }

    #[test]
    fn three_letter_codes_map_back_for_talking_to_tmdb() {
        assert_eq!(two_letter("fra"), "fr");
        assert_eq!(two_letter("jpn"), "ja");
        assert_eq!(two_letter("por"), "pt");
        // Not in the table: hand it over and let TMDB decide.
        assert_eq!(two_letter("zzz"), "zzz");
    }

    fn item_with_translation() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Breaking Bad".into();
        item.overview = Some("A teacher and a student.".into());
        item.translations = vec![
            Translation {
                language: "fra".into(),
                title: Some("Breaking Bad".into()),
                overview: Some("Un professeur et un élève.".into()),
                is_manual: false,
            },
            Translation {
                language: "deu".into(),
                title: Some(String::new()),
                overview: None,
                is_manual: false,
            },
        ];
        item
    }

    #[test]
    fn a_translation_replaces_the_stored_text() {
        let mut item = item_with_translation();
        overlay_item(&mut item, "fra");

        assert_eq!(item.overview.as_deref(), Some("Un professeur et un élève."));
    }

    #[test]
    fn a_blank_translation_leaves_the_original_alone() {
        // Showing an English title beats showing nothing.
        let mut item = item_with_translation();
        overlay_item(&mut item, "deu");

        assert_eq!(item.title, "Breaking Bad");
        assert_eq!(item.overview.as_deref(), Some("A teacher and a student."));
    }

    #[test]
    fn a_lock_survives_being_asked_for_in_another_language() {
        // The whole point of a lock is that nothing overwrites it. A client
        // asking in French must not undo a rename.
        let mut item = item_with_translation();
        item.title = "Mon Titre".into();
        item.locked_fields = vec!["item/title".into()];

        overlay_item(&mut item, "fra");

        assert_eq!(item.title, "Mon Titre", "the lock held");
        assert_eq!(
            item.overview.as_deref(),
            Some("Un professeur et un élève."),
            "an unlocked field is still translated"
        );
    }

    #[test]
    fn locking_one_field_does_not_freeze_the_others() {
        let mut item = item_with_translation();
        item.overview = Some("Mine.".into());
        item.locked_fields = vec!["item/overview".into()];

        overlay_item(&mut item, "fra");

        assert_eq!(item.title, "Breaking Bad", "translated");
        assert_eq!(item.overview.as_deref(), Some("Mine."), "locked");
    }

    #[test]
    fn a_language_we_hold_nothing_for_changes_nothing() {
        let mut item = item_with_translation();
        overlay_item(&mut item, "jpn");

        assert_eq!(item.title, "Breaking Bad");
        assert_eq!(item.overview.as_deref(), Some("A teacher and a student."));
    }

    #[test]
    fn episodes_keep_their_numbering_when_text_is_overlaid() {
        // Only the text is translated; the numbering is the same work.
        let mut episode = Episode {
            id: "e".into(),
            season_number: 1,
            episode_number: 1,
            absolute_episode_number: Some(1),
            aired_after_season_number: None,
            aired_before_season_number: None,
            aired_before_episode_number: None,
            title: "Pilot".into(),
            overview: Some("It begins.".into()),
            air_date: Some("2008-01-20".into()),
            air_date_utc: None,
            runtime: Some(58),
            finale_type: None,
            image: None,
            tvdb_id: Some(1),
            tmdb_id: Some(2),
            rating: None,
            is_manual: false,
        };

        let text = repo::translation::EpisodeText {
            season_number: 1,
            episode_number: 1,
            title: Some("Pilote".into()),
            overview: Some("Ça commence.".into()),
        };

        if let Some(title) = text.title.as_deref() {
            episode.title = title.to_string();
        }
        if let Some(overview) = text.overview.as_deref() {
            episode.overview = Some(overview.to_string());
        }

        assert_eq!(episode.title, "Pilote");
        assert_eq!(episode.season_number, 1);
        assert_eq!(episode.runtime, Some(58));
        assert_eq!(episode.air_date.as_deref(), Some("2008-01-20"));
    }

    /// A season as TMDB answers it: each episode its number, its id, a name.
    fn tmdb_season(number: i32, episodes: &[(i32, i64, &str)]) -> Season {
        let episodes: Vec<serde_json::Value> = episodes
            .iter()
            .map(|(episode, id, name)| {
                serde_json::json!({
                    "id": id,
                    "season_number": number,
                    "episode_number": episode,
                    "name": name,
                    "overview": format!("About {name}."),
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "season_number": number,
            "episodes": episodes,
        }))
        .unwrap()
    }

    fn stored(season: i32, number: i32, tmdb_id: Option<i64>) -> Episode {
        let mut e = crate::db::repo::child::blank_episode(season, number);
        e.tmdb_id = tmdb_id;
        e
    }

    #[test]
    fn tmdbs_text_for_a_special_goes_where_the_merge_put_its_id() {
        // Rurouni Kenshin: TMDB's first special is the series' last episode,
        // which TheTVDB counts in season 3, and the 1997 film TheTVDB lists
        // first is not on TMDB at all. By number, the film was named after
        // that episode. The merge gave each special the TMDB id of the one it
        // provably is — none for the film — and the text follows the id.
        let mut item = MediaItem::empty(MediaKind::Series);
        item.episodes = vec![
            stored(0, 1, None),
            stored(0, 2, Some(703204)),
            stored(3, 32, None),
            stored(3, 33, None),
        ];
        let seasons = [
            tmdb_season(
                0,
                &[
                    (1, 1506818, "End of Wanderings"),
                    (2, 703204, "Trust & Betrayal: Act 1"),
                ],
            ),
            tmdb_season(3, &[(32, 703190, "The Elegy of Wind and Water")]),
        ];

        let texts = tmdb_texts(&item, &seasons);
        let placed: Vec<(i32, i32, Option<&str>)> = texts
            .iter()
            .map(|t| (t.season_number, t.episode_number, t.title.as_deref()))
            .collect();

        // The regular seasons go by number, as the merge pairs them.
        assert_eq!(
            placed,
            [
                (0, 2, Some("Trust & Betrayal: Act 1")),
                (3, 32, Some("The Elegy of Wind and Water")),
            ]
        );
    }

    #[test]
    fn an_episode_tmdb_has_no_text_for_is_asked_of_thetvdb() {
        // Everything TMDB sent is complete, and the film and the last episode
        // are still without text in the language asked: TMDB lists neither
        // under TheTVDB's numbers, TheTVDB lists both. Checking only what TMDB
        // sent left them in the language the work is stored in — the last
        // episode reached an English Sonarr in French.
        let mut item = MediaItem::empty(MediaKind::Series);
        item.episodes = vec![
            stored(0, 1, None),
            stored(0, 2, Some(703204)),
            stored(3, 33, None),
        ];
        let text = |season: i32, number: i32| repo::translation::EpisodeText {
            season_number: season,
            episode_number: number,
            title: Some(format!("{season}x{number}")),
            overview: Some("Complete.".into()),
        };

        assert!(incomplete(&item, &[text(0, 2)]));
        assert!(incomplete(&item, &[text(0, 1), text(0, 2)]));
        assert!(!incomplete(&item, &[text(0, 1), text(0, 2), text(3, 33)]));
    }

    #[test]
    fn tmdbs_stand_in_is_no_text_in_the_language_asked() {
        // Reincarnated as a Sword's second season, asked of TMDB in French:
        // only the first episode has a French name, the others are called by
        // their number. Taken as text, "Épisode 2" would replace a title the
        // work holds, or TheTVDB's; and with only the English stand-in
        // dropped, the French one a server set to French holds went to an
        // English Sonarr. TheTVDB is asked for the two instead.
        let mut item = MediaItem::empty(MediaKind::Series);
        item.episodes = vec![stored(2, 1, None), stored(2, 2, None), stored(2, 3, None)];
        let seasons = [tmdb_season(
            2,
            &[
                (1, 4815162, "Cette île qui flotte dans le ciel"),
                (2, 4815163, "Épisode 2"),
                (3, 4815164, "Épisode 3"),
            ],
        )];

        let texts = tmdb_texts(&item, &seasons);
        let titles: Vec<Option<&str>> = texts.iter().map(|t| t.title.as_deref()).collect();

        assert_eq!(
            titles,
            [Some("Cette île qui flotte dans le ciel"), None, None]
        );
        assert!(incomplete(&item, &texts), "TheTVDB is asked for the rest");
    }

    #[test]
    fn a_specials_stand_in_is_known_by_tmdbs_own_number() {
        // A special is placed by the TMDB id the merge gave it, under the
        // work's number, and TMDB's stand-in names it by TMDB's: TMDB's fifth
        // special, TheTVDB's first, is "Episode 5". "Episode 1" for TMDB's
        // sixth is a name somebody gave it, wherever the work counts it.
        let mut item = MediaItem::empty(MediaKind::Series);
        item.episodes = vec![stored(0, 1, Some(105)), stored(0, 2, Some(106))];
        let seasons = [tmdb_season(
            0,
            &[(5, 105, "Episode 5"), (6, 106, "Episode 1")],
        )];

        let placed: Vec<(i32, Option<String>)> = tmdb_texts(&item, &seasons)
            .into_iter()
            .map(|t| (t.episode_number, t.title))
            .collect();

        assert_eq!(placed, [(1, None), (2, Some("Episode 1".into()))]);
    }

    /// A series with one episode, whose French title is held here, as a
    /// request for it reads it.
    async fn held_in_french(state: &AppState, title: &str, ids: (i64, i64), on: bool) -> MediaItem {
        use crate::{domain::ExternalIds, service::testing::overdue};

        let (tvdb, tmdb) = ids;
        let ids = ExternalIds {
            tvdb: Some(tvdb),
            tmdb: Some(tmdb),
            ..Default::default()
        };
        let series = overdue(&state.db, MediaKind::Series, title, ids, on).await;
        repo::child::add_episode(&state.db, &series.id, &stored(1, 1, None))
            .await
            .expect("its episode");
        let french = repo::translation::EpisodeText {
            season_number: 1,
            episode_number: 1,
            title: Some("Chute libre".into()),
            overview: None,
        };
        repo::translation::put_episodes(&state.db, &series.id, "fra", &[french], true)
            .await
            .expect("its French title");

        crate::service::load(state, &series.id)
            .await
            .expect("read")
            .expect("held")
    }

    /// The series, as a request for it in French is given it.
    async fn in_french(state: &AppState, id: &str) -> MediaItem {
        let mut item = crate::service::load(state, id)
            .await
            .expect("read")
            .expect("held");
        apply(state, &mut item, "fr").await.expect("served");
        item
    }

    /// The French text held for the series' episodes: each one's numbers,
    /// title and overview, in order.
    async fn french(
        state: &AppState,
        id: &str,
    ) -> Vec<((i32, i32), Option<String>, Option<String>)> {
        let held = repo::translation::for_episodes(&state.db, id, "fra")
            .await
            .expect("read");
        let mut held: Vec<_> = held
            .into_values()
            .map(|t| ((t.season_number, t.episode_number), t.title, t.overview))
            .collect();
        held.sort();
        held
    }

    /// What is recorded of French for the series: nothing, a fetch in full
    /// (`Some(None)`), or the time it is to be tried again.
    async fn recorded(state: &AppState, id: &str) -> Option<Option<String>> {
        use crate::db::RowExt;

        let row = sqlx::query(state.db.sql(
            "SELECT retry_after FROM media_language_fetch WHERE media_id = ? AND language = 'fra'",
        ))
        .bind(id)
        .fetch_optional(state.db.pool())
        .await
        .expect("read")?;

        Some(row.opt_text("retry_after").expect("its wait"))
    }

    #[tokio::test]
    async fn a_series_switched_off_is_given_what_is_held_in_a_language_and_nothing_is_fetched() {
        let (state, nowhere) = crate::service::testing::server().await;

        // Asked for in French, whose episode text was never fetched for it.
        let mut off = held_in_french(&state, "Breaking Bad", (81189, 1396), false).await;
        apply(&state, &mut off, "fr").await.expect("served");
        assert_eq!(off.episodes[0].title, "Chute libre", "what is held");
        assert_eq!(nowhere.asked(), 0, "no provider was asked");
        assert_eq!(recorded(&state, &off.id).await, None, "no attempt");

        // Switched on, the same request has its episode text fetched.
        let mut on = held_in_french(&state, "Better Call Saul", (273181, 60059), true).await;
        apply(&state, &mut on, "fr").await.expect("served");
        assert!(nowhere.asked() > 0, "the providers were asked");
        assert!(recorded(&state, &on.id).await.is_some(), "the attempt");
    }

    #[tokio::test]
    async fn text_held_in_a_language_outlasts_a_fetch_no_provider_answers() {
        let (state, nowhere) = crate::service::testing::server().await;
        let series = held_in_french(&state, "Breaking Bad", (81189, 1396), true).await;
        let held = [((1, 1), Some("Chute libre".to_string()), None::<String>)];

        // The first request in French since its last refresh, with every
        // provider out of reach: they are asked, and what is held is served —
        // and still held.
        let first = in_french(&state, &series.id).await;
        assert!(nowhere.asked() > 0, "the providers were asked");
        assert_eq!(first.episodes[0].title, "Chute libre", "what is held");
        assert_eq!(french(&state, &series.id).await, held, "still held");

        // Nor is the language taken for fetched: it is to be tried again
        // later, and the next request is served what is held and waits on
        // nobody.
        let retry = recorded(&state, &series.id).await.flatten();
        assert!(
            retry.is_some_and(|at| at > crate::db::now()),
            "a wait, not a fetch"
        );
        let next = in_french(&state, &series.id).await;
        assert_eq!(nowhere.asked(), 0, "nobody was asked again");
        assert_eq!(next.episodes[0].title, "Chute libre");

        // Once the wait is over, the providers are asked for it again.
        let over = crate::db::to_rfc3339(chrono::Utc::now() - chrono::TimeDelta::minutes(1));
        repo::translation::mark_unanswered(&state.db, &series.id, "fra", &over)
            .await
            .expect("the wait over");
        let again = in_french(&state, &series.id).await;
        assert!(nowhere.asked() > 0, "asked again");
        assert_eq!(again.episodes[0].title, "Chute libre");
        assert_eq!(french(&state, &series.id).await, held, "and still held");
    }

    #[tokio::test]
    async fn only_a_whole_answer_replaces_what_is_held_in_a_language() {
        let (state, _) = crate::service::testing::server().await;
        let series = held_in_french(&state, "Breaking Bad", (81189, 1396), true).await;
        let text = |number: i32, title: Option<&str>, overview: Option<&str>| {
            repo::translation::EpisodeText {
                season_number: 1,
                episode_number: number,
                title: title.map(str::to_string),
                overview: overview.map(str::to_string),
            }
        };
        let some = |s: &str| Some(s.to_string());

        // A provider failed: what the others said is taken, and what they
        // left out stands — the title held, which this answer lacks. Tried
        // again later.
        let partial = Fetched {
            texts: vec![
                text(1, None, Some("Walt apprend.")),
                text(2, Some("Le Chat dans le sac"), None),
            ],
            degraded: true,
        };
        store(&state, &series, "fra", partial).await;
        assert_eq!(
            french(&state, &series.id).await,
            [
                ((1, 1), some("Chute libre"), some("Walt apprend.")),
                ((1, 2), some("Le Chat dans le sac"), None),
            ]
        );
        let retry = recorded(&state, &series.id).await.flatten();
        assert!(retry.is_some(), "tried again later");

        // Every provider answered, with nothing to say: nothing changes, and
        // it is tried again later all the same.
        let empty = Fetched {
            texts: vec![text(1, None, None)],
            degraded: false,
        };
        store(&state, &series, "fra", empty).await;
        assert_eq!(french(&state, &series.id).await.len(), 2, "nothing dropped");
        let retry = recorded(&state, &series.id).await.flatten();
        assert!(retry.is_some(), "tried again later");

        // A whole answer replaces the language's text outright, and is a
        // fetch.
        let whole = Fetched {
            texts: vec![text(2, Some("Le Chat est dans le sac"), None)],
            degraded: false,
        };
        store(&state, &series, "fra", whole).await;
        assert_eq!(
            french(&state, &series.id).await,
            [((1, 2), some("Le Chat est dans le sac"), None)]
        );
        assert_eq!(recorded(&state, &series.id).await, Some(None), "fetched");
    }
}

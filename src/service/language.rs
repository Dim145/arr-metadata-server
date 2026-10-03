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
    // the same reason as `FETCHING`.
    if item.kind == MediaKind::Series
        && !item.episodes.is_empty()
        && !repo::translation::was_fetched(&state.db, &item.id, &language).await?
    {
        let _fetching = crate::service::FETCHING
            .lock(&format!("episodes:{}:{language}", item.id))
            .await;

        if !repo::translation::was_fetched(&state.db, &item.id, &language).await? {
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

/// TMDB's episode text, one call per season.
async fn from_tmdb(
    state: &AppState,
    item: &MediaItem,
    language: &str,
) -> Vec<repo::translation::EpisodeText> {
    let Some(tmdb_id) = item.external_ids.tmdb else {
        return Vec::new();
    };
    if !state.tmdb.is_configured() {
        return Vec::new();
    }

    // TMDB wants the two-letter form it was given.
    let requested = two_letter(language);

    let numbers: Vec<i32> = {
        let mut n: Vec<i32> = item.episodes.iter().map(|e| e.season_number).collect();
        n.sort_unstable();
        n.dedup();
        n
    };

    let seasons = state
        .tmdb
        .tv_seasons_in(tmdb_id, &numbers, &requested)
        .await;

    tmdb_texts(item, &seasons)
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
                title: e
                    .name
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .filter(|t| !is_placeholder(t)),
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
/// TMDB serves a handful of languages well and returns an English placeholder
/// for the rest; TheTVDB holds dozens. Asking it second means a French request
/// is answered even when TMDB has no French, without displacing TMDB's text
/// where it exists — the same fill-do-not-replace rule the merge engine uses.
async fn fill_from_tvdb(
    state: &AppState,
    item: &MediaItem,
    language: &str,
    texts: &mut Vec<repo::translation::EpisodeText>,
) {
    let Some(tvdb_id) = item.external_ids.tvdb else {
        return;
    };

    // Nothing missing: skip the call.
    if !incomplete(item, texts) {
        return;
    }

    let from_tvdb = match state.tvdb.episode_texts(tvdb_id, language).await {
        Ok(found) => found,
        Err(e) => {
            tracing::debug!(id = %item.id, %language, error = format_args!("{e:#}"), "TheTVDB had no episode text");
            return;
        }
    };

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
/// caller still gets the work, in the language it was stored in.
async fn fetch_episodes(state: &AppState, item: &MediaItem, language: &str) {
    tracing::info!(id = %item.id, %language, "fetching episode translations");

    let mut texts = from_tmdb(state, item, language).await;

    fill_from_tvdb(state, item, language, &mut texts).await;

    if let Err(e) = repo::translation::put_episodes(&state.db, &item.id, language, &texts).await {
        tracing::warn!(id = %item.id, %language, error = %e, "could not store episode translations");
        return;
    }

    // Recorded even when nothing came back, so a language the provider has
    // nothing for is not refetched on every request.
    if let Err(e) = repo::translation::mark_fetched(&state.db, &item.id, language).await {
        tracing::warn!(id = %item.id, %language, error = %e, "could not record the fetch");
    }
}

/// Whether this is TMDB's stand-in for an episode it has no title for.
///
/// Asking for a language TMDB has not translated gets `Episode 1` back rather
/// than nothing, in English, whatever the language. Storing that would replace a
/// real title like `Pilot` with something strictly worse.
fn is_placeholder(title: &str) -> bool {
    let rest = match title.trim().strip_prefix("Episode ") {
        Some(rest) => rest,
        None => return false,
    };

    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
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
    fn tmdbs_untranslated_placeholder_is_recognised() {
        // Asking for a language TMDB has not translated returns this, in
        // English, and storing it would lose a real title.
        assert!(is_placeholder("Episode 1"));
        assert!(is_placeholder("Episode 42"));
        assert!(is_placeholder("  Episode 7  "));

        assert!(!is_placeholder("Pilot"));
        assert!(!is_placeholder("Episode of Rain"));
        assert!(!is_placeholder("Episode"));
        assert!(
            !is_placeholder("Épisode 1"),
            "a real French title is not a placeholder"
        );
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
}

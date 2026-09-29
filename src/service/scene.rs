//! Sonarr's alternate titles, with this catalogue's own added.
//!
//! Sonarr never reads a series' alternative titles from Skyhook, the real
//! one's or this server's: its `ShowResource` has no field for them, in
//! version 4 as in version 5. What it recognises a release by is the series'
//! own title and two lists it downloads every three hours: the one at
//! `services.sonarr.tv/v1/scenemapping`, and TheXEM's names. A series whose
//! releases go by its French title is therefore never recognised, however
//! many titles this catalogue holds for it.
//!
//! When `services.sonarr.tv` is resolved to this server, as
//! `skyhook.sonarr.tv` already is, this server answers for the first list:
//! the real one, passed on whole, and a mapping for each title of this
//! catalogue that is safe to add. Safe means
//!
//! - written in the Latin alphabet, as release names are;
//! - not known to Sonarr for that series already, once cleaned the way
//!   Sonarr cleans a title ([`clean_title`]);
//! - claimed by no other series anywhere Sonarr looks. A title two series
//!   answer to makes Sonarr throw `InvalidSceneMappingException` for every
//!   release by that name, and in an interactive search that failure takes
//!   the whole list of releases down with it.
//!
//! Sonarr also searches its indexers with every title of the list written in
//! Latin-1, one query each, where an indexer cannot be searched by id. A
//! title added here is searched with only when it is the series' title in the
//! caller's language: the others are recognised in release names and searched
//! under the series' own title, which Sonarr searches with anyway — thirty
//! translations of a popular show would otherwise be thirty queries an
//! episode.
//!
//! When the real list cannot be had, the answer is an error and nothing else:
//! Sonarr then keeps the list it holds. Handed this server's additions alone,
//! it would take them for the whole list and forget the rest.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization as _;
use unicode_properties::general_category::{
    GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory as _,
};

use crate::{
    db::repo,
    service::{language, series},
    state::AppState,
};

/// What each mapping this server adds says of itself, as Sonarr shows it
/// beside the title.
pub const COMMENT: &str = "arr-metadata-server";

/// The shortest cleaned title worth a mapping: shorter ones are words other
/// series' releases are named with too.
const SHORTEST: usize = 4;

/// How long a list downloaded is used before it is asked for again. Sonarr
/// asks every three hours; several Sonarr instances cost one download.
const OFFICIAL_FRESH: Duration = Duration::from_secs(60 * 60);
const XEM_FRESH: Duration = Duration::from_secs(3 * 60 * 60);

/// How long a list is still used when its source cannot be reached.
const STALE_AT_MOST: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The largest list accepted. The real one is under two megabytes.
const LIST_LIMIT: u64 = 32 * 1024 * 1024;

// ── Sonarr's title cleaning ─────────────────────────────────────────────

/// Sonarr's `CleanSeriesTitle`: what a title is compared as, when a release
/// name is matched against a series.
///
/// `%` after a number becomes `percent`; the words *a*, *à*, *an*, *the*,
/// *and*, *or* and *of* go when they stand alone neither first nor last;
/// everything that is not a letter or a digit goes; what is left is lowered
/// and its accents taken off. A title that is a number is kept as it is.
/// Ported from `NzbDrone.Core/Parser/Parser.cs`, and tested against
/// Sonarr's own cases.
pub fn clean_title(title: &str) -> String {
    if title.trim().is_empty() || title.trim().parse::<i64>().is_ok() {
        return title.to_string();
    }
    remove_accents(&normalize(&percent(title)).to_lowercase())
}

/// `\w`, as .NET reads it: a letter, a nonspacing mark, a decimal digit or a
/// connector — not `½`, `²` or `Ⅱ`, which are numbers of other kinds.
fn is_word(c: char) -> bool {
    matches!(c.general_category_group(), GeneralCategoryGroup::Letter)
        || matches!(
            c.general_category(),
            GeneralCategory::NonspacingMark
                | GeneralCategory::DecimalNumber
                | GeneralCategory::ConnectorPunctuation
        )
}

/// `\d`, as .NET reads it: a decimal digit, in any script.
fn is_digit(c: char) -> bool {
    matches!(c.general_category(), GeneralCategory::DecimalNumber)
}

/// `(?<=\b\d+)%` replaced with `percent`.
fn percent(title: &str) -> String {
    let chars: Vec<char> = title.chars().collect();
    let mut out = String::with_capacity(title.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '%' {
            let mut start = i;
            while start > 0 && is_digit(chars[start - 1]) {
                start -= 1;
            }
            if start < i && (start == 0 || !is_word(chars[start - 1])) {
                out.push_str("percent");
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// The words Sonarr drops from inside a title, in the order its expression
/// tries them.
const DROPPED: [&str; 7] = ["a", "à", "an", "the", "and", "or", "of"];

/// `((?:\b|_)(?<!^)([aà](?!$)|an|the|and|or|of)(?!$)(?:\b|_))|\W|_`,
/// case-insensitive, replaced with nothing — walked by hand, since the
/// expression looks behind and ahead.
fn normalize(title: &str) -> String {
    let chars: Vec<char> = title.chars().collect();
    let mut out = String::with_capacity(title.len());
    let mut at = 0;
    while at < chars.len() {
        if let Some(end) = dropped_word(&chars, at) {
            at = end;
            continue;
        }
        let c = chars[at];
        if is_word(c) && c != '_' {
            out.push(c);
        }
        at += 1;
    }
    out
}

/// Where a word Sonarr drops ends, when one starts at `at`.
fn dropped_word(chars: &[char], at: usize) -> Option<usize> {
    let len = chars.len();
    let boundary = |p: usize| (p > 0 && is_word(chars[p - 1])) != (p < len && is_word(chars[p]));

    // `(?:\b|_)`, then `(?<!^)`: the word starts after a boundary that is not
    // the title's start, or after an underscore.
    let mut starts = Vec::with_capacity(2);
    if boundary(at) && at != 0 {
        starts.push(at);
    }
    if chars[at] == '_' {
        starts.push(at + 1);
    }

    for start in starts {
        for word in DROPPED {
            let word: Vec<char> = word.chars().collect();
            let end = start + word.len();
            if end > len {
                continue;
            }
            let same = chars[start..end]
                .iter()
                .zip(&word)
                .all(|(c, w)| c.to_lowercase().eq(w.to_lowercase()));
            // `(?!$)`: never the last word.
            if !same || end == len {
                continue;
            }
            // `(?:\b|_)` after it.
            if boundary(end) {
                return Some(end);
            }
            if chars[end] == '_' {
                return Some(end + 1);
            }
        }
    }
    None
}

/// Sonarr's `RemoveAccent`: decomposed, its nonspacing marks dropped,
/// composed again.
fn remove_accents(text: &str) -> String {
    text.nfd()
        .filter(|c| !matches!(c.general_category(), GeneralCategory::NonspacingMark))
        .nfc()
        .collect()
}

/// Whether every letter of a title is a Latin one, and it has one at least.
pub fn is_latin(title: &str) -> bool {
    let mut letters = 0;
    for c in title
        .chars()
        .filter(|c| matches!(c.general_category_group(), GeneralCategoryGroup::Letter))
    {
        let latin = matches!(
            c as u32,
            0x41..=0x5A
                | 0x61..=0x7A
                | 0xAA
                | 0xBA
                | 0xC0..=0xD6
                | 0xD8..=0xF6
                | 0xF8..=0x24F
                | 0x1E00..=0x1EFF
                | 0x2C60..=0x2C7F
                | 0xA720..=0xA7FF
                | 0xAB30..=0xAB6F
        );
        if !latin {
            return false;
        }
        letters += 1;
    }
    letters > 0
}

/// Whether a cleaned title is specific enough to be matched by.
fn usable(clean: &str) -> bool {
    clean.chars().count() >= SHORTEST && !clean.chars().all(|c| c.is_ascii_digit())
}

// ── Which titles are added ──────────────────────────────────────────────

/// A title Sonarr already matches a series by.
#[derive(Clone, Debug)]
pub struct Claim {
    pub tvdb_id: i64,
    pub title: String,
}

/// A series of this catalogue, as the list sees it.
#[derive(Clone, Debug, Default)]
pub struct Held {
    /// The id Sonarr keeps it under.
    pub tvdb_id: i64,
    /// The title Sonarr is served for it.
    pub served: String,
    /// Its titles in the caller's language, searched with as well.
    pub searched: Vec<String>,
    /// Its other titles, to recognise a release by.
    pub other: Vec<String>,
    /// Whether it is shown at all: a hidden work gains no title, and still
    /// keeps its own from every other series.
    pub shown: bool,
}

/// A mapping this server adds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Addition {
    pub tvdb_id: i64,
    pub title: String,
    pub search_title: String,
}

impl Addition {
    /// As the list writes it. `season` -1 is every season, as Sonarr's own
    /// mappings for a whole series say.
    pub fn to_json(&self) -> Value {
        json!({
            "tvdbId": self.tvdb_id,
            "title": self.title,
            "searchTitle": self.search_title,
            "season": -1,
            "comment": COMMENT,
        })
    }
}

/// The mappings to add, from the catalogue and what Sonarr already has.
pub fn additions(held: &[Held], known: &[Claim]) -> Vec<Addition> {
    // Who answers to which cleaned title, and what each series is known by.
    let mut claimed: HashMap<String, HashSet<i64>> = HashMap::new();
    let mut known_by: HashMap<i64, HashSet<String>> = HashMap::new();
    let mut claim = |tvdb_id: i64, title: &str| {
        let clean = clean_title(title.trim());
        if clean.is_empty() {
            return;
        }
        claimed.entry(clean.clone()).or_default().insert(tvdb_id);
        known_by.entry(tvdb_id).or_default().insert(clean);
    };
    for c in known {
        claim(c.tvdb_id, &c.title);
    }
    for series in held {
        claim(series.tvdb_id, &series.served);
    }

    // Each series' titles Sonarr does not know yet, once each, those in the
    // caller's language first so a title both searched and not is searched.
    let mut candidates = Vec::new();
    for series in held {
        if !series.shown || series.served.trim().is_empty() {
            continue;
        }
        let mut seen = known_by.get(&series.tvdb_id).cloned().unwrap_or_default();
        let titles = series
            .searched
            .iter()
            .map(|t| (t, true))
            .chain(series.other.iter().map(|t| (t, false)));
        for (title, searched) in titles {
            let title = title.trim();
            if !is_latin(title) {
                continue;
            }
            let clean = clean_title(title);
            if !usable(&clean) || names_a_part(&clean, &seen) || !seen.insert(clean.clone()) {
                continue;
            }
            candidates.push((series, clean, title.to_string(), searched));
        }
    }
    for (series, clean, _, _) in &candidates {
        claimed
            .entry(clean.clone())
            .or_default()
            .insert(series.tvdb_id);
    }

    // Only what no other series answers to.
    candidates
        .into_iter()
        .filter(|(_, clean, _, _)| claimed.get(clean).is_some_and(|ids| ids.len() == 1))
        .map(|(series, _, title, searched)| Addition {
            tvdb_id: series.tvdb_id,
            search_title: if searched {
                title.clone()
            } else {
                series.served.clone()
            },
            title,
        })
        .collect()
}

/// Whether a cleaned title is one the series is known by, followed by more:
/// what names a part or a relative of it — a season, a special, a spin-off,
/// *Breaking Bad: Original Minisodes* — whose releases are not its episodes,
/// and would be taken for them.
fn names_a_part(clean: &str, known: &HashSet<String>) -> bool {
    known.iter().any(|name| {
        name.chars().count() >= SHORTEST
            && clean.len() > name.len()
            && clean.starts_with(name.as_str())
    })
}

/// The title Sonarr, which always asks in English, is served for a series
/// before anything tells it from a homonym: a locked title as it is; the
/// stored one when English is the server's language (`server`); its English
/// one otherwise, when it has one.
fn plain_title(s: &repo::scene::SeriesTitles, server: &str) -> String {
    let english = s
        .translated
        .iter()
        .find(|(code, title)| speaks(code, "en") && !title.trim().is_empty())
        .map(|(_, title)| title.clone());
    match english {
        Some(english) if !s.title_locked && !speaks(server, "en") => english,
        _ => s.title.clone(),
    }
}

/// Which series answer to each cleaned plain title, by the id Sonarr keeps
/// them under.
fn homonyms_of(catalogue: &[repo::scene::SeriesTitles], plain: &[String]) -> Homonyms {
    let mut by_title: HashMap<String, HashSet<i64>> = HashMap::new();
    for (s, title) in catalogue.iter().zip(plain) {
        if let Some(id) = series::client_id_of(s.tvdb, s.tmdb, s.fankai) {
            by_title.entry(clean_title(title)).or_default().insert(id);
        }
    }
    by_title
}

/// Whether another series than `tvdb_id` is served the plain title `plain`.
fn shares(homonyms: &Homonyms, tvdb_id: i64, plain: &str) -> bool {
    homonyms
        .get(&clean_title(plain))
        .is_some_and(|ids| ids.iter().any(|id| *id != tvdb_id))
}

/// The title Sonarr is served for a series: [`series::sonarr_title`].
fn served_title(
    s: &repo::scene::SeriesTitles,
    plain: &str,
    tvdb_id: i64,
    homonyms: &Homonyms,
) -> String {
    let homonym_year = (s.tvdb.is_none() && shares(homonyms, tvdb_id, plain))
        .then_some(s.year)
        .flatten();
    series::sonarr_title(
        plain,
        s.title_locked,
        s.title_qualifier.as_deref(),
        homonym_year,
    )
}

type Homonyms = HashMap<String, HashSet<i64>>;

/// The catalogue's plain titles, kept a minute: a whole library refreshing
/// in Sonarr is one reading of them, not one a series.
type KeptHomonyms = Option<(Instant, Arc<Homonyms>)>;

static HOMONYMS: LazyLock<tokio::sync::Mutex<KeptHomonyms>> = LazyLock::new(Default::default);
const HOMONYMS_FRESH: Duration = Duration::from_secs(60);

/// Whether another series of the catalogue than `tvdb_id` is served `plain`
/// in Sonarr — what, for a work TheTVDB has no entry for, calls for its year.
pub async fn has_homonym(state: &AppState, tvdb_id: i64, plain: &str) -> bool {
    let mut kept = HOMONYMS.lock().await;
    let fresh = kept
        .as_ref()
        .filter(|(at, _)| at.elapsed() < HOMONYMS_FRESH)
        .map(|(_, map)| map.clone());
    let map = match fresh {
        Some(map) => map,
        None => match repo::scene::series_titles(&state.db).await {
            Ok(catalogue) => {
                let server = state.language(None, None);
                let plain: Vec<String> =
                    catalogue.iter().map(|s| plain_title(s, &server)).collect();
                let map = Arc::new(homonyms_of(&catalogue, &plain));
                *kept = Some((Instant::now(), map.clone()));
                map
            }
            Err(e) => {
                tracing::warn!(
                    error = format_args!("{e:#}"),
                    "could not read the catalogue's titles"
                );
                return false;
            }
        },
    };
    shares(&map, tvdb_id, plain)
}

/// Whether a stored language code and a tag like `fr-FR` name one language.
fn speaks(code: &str, tag: &str) -> bool {
    let code = language::normalize(code);
    !code.is_empty() && code == language::normalize(tag)
}

/// The catalogue's series, as the list sees them.
///
/// Sonarr always asks in English, and is served a work's stored title when
/// English is the server's language (`server`), its English one otherwise
/// — a locked title as it is, either way.
///
/// A title is offered only in a language whose releases name it: the
/// caller's (`caller`), searched with as well when `search` allows it;
/// English; and the work's own original language, romanised. The other
/// translations — thirty of them for a popular show — would be names that
/// releases of other series outside this catalogue go by, which the lists
/// cannot tell, and an alternative title nobody gave a language is one of
/// those too.
pub fn held(
    catalogue: Vec<repo::scene::SeriesTitles>,
    server: &str,
    caller: &str,
    search: bool,
) -> Vec<Held> {
    let plain: Vec<String> = catalogue.iter().map(|s| plain_title(s, server)).collect();
    let homonyms = homonyms_of(&catalogue, &plain);
    catalogue
        .into_iter()
        .zip(plain)
        .filter_map(|(s, plain)| {
            let tvdb_id = series::client_id_of(s.tvdb, s.tmdb, s.fankai)?;
            let served = served_title(&s, &plain, tvdb_id, &homonyms);

            let original = s.original_language.clone().unwrap_or_default();
            let wanted = |code: &str| {
                speaks(code, caller)
                    || speaks(code, "en")
                    || (!original.is_empty() && speaks(code, &original))
            };
            let mut searched = Vec::new();
            let mut other = Vec::new();
            // The stored title is in the server's language.
            if search && speaks(server, caller) {
                searched.push(s.title.clone());
            } else if wanted(server) {
                other.push(s.title.clone());
            }
            if !original.is_empty() {
                other.extend(s.original_title.clone());
            }
            for (code, title) in s.translated {
                if search && speaks(&code, caller) {
                    searched.push(title);
                } else if wanted(&code) {
                    other.push(title);
                }
            }
            other.extend(
                s.alternative
                    .into_iter()
                    .filter(|(code, _)| code.as_deref().is_some_and(&wanted))
                    .map(|(_, title)| title),
            );

            Some(Held {
                tvdb_id,
                served,
                searched,
                other,
                shown: s.enabled,
            })
        })
        .collect()
}

// ── The lists, downloaded ───────────────────────────────────────────────

struct Kept<T> {
    at: Instant,
    value: Arc<T>,
}

type Slot<T> = LazyLock<tokio::sync::Mutex<HashMap<String, Kept<T>>>>;

static OFFICIAL: Slot<Vec<Value>> = LazyLock::new(Default::default);
static XEM: Slot<Vec<Claim>> = LazyLock::new(Default::default);

/// A list from where it is kept, fetched again once `fresh` has passed — and
/// the copy kept, for a while, when fetching fails.
async fn kept<T, F, Fut>(slot: &Slot<T>, key: &str, fresh: Duration, fetch: F) -> Result<Arc<T>>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    // Held across the fetch: two callers at once cost one download.
    let mut lists = slot.lock().await;
    if let Some(kept) = lists.get(key)
        && kept.at.elapsed() < fresh
    {
        return Ok(kept.value.clone());
    }
    match fetch().await {
        Ok(value) => {
            let value = Arc::new(value);
            lists.insert(
                key.to_string(),
                Kept {
                    at: Instant::now(),
                    value: value.clone(),
                },
            );
            Ok(value)
        }
        Err(e) => match lists.get(key) {
            Some(kept) if kept.at.elapsed() < STALE_AT_MOST => {
                tracing::warn!(
                    error = format_args!("{e:#}"),
                    list = key,
                    "could not download the list again; using the copy kept"
                );
                Ok(kept.value.clone())
            }
            _ => Err(e),
        },
    }
}

/// A GET to a list's source, marked as this server's so a loop shows.
async fn get(state: &AppState, provider: &str, url: &str) -> Result<Vec<u8>> {
    let started = Instant::now();
    let response = state
        .http
        .get(url)
        .header(crate::providers::radarr::LOOP_HEADER, &state.instance)
        .timeout(Duration::from_secs(30))
        .send()
        .await;
    crate::metrics::upstream(
        provider,
        started,
        response.as_ref().ok().map(|r| r.status()),
    );
    let response = response.with_context(|| format!("could not reach {url}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::LOOP_DETECTED {
        bail!(
            "{url} resolves to this server; set AMS_SONARR_SERVICES_UPSTREAM or \
             AMS_THEXEM_UPSTREAM to the real service's address"
        );
    }
    if !status.is_success() {
        bail!("{url} answered {status}");
    }
    crate::providers::read_body(response, LIST_LIMIT).await
}

/// Sonarr's own list, as `services.sonarr.tv` has it.
pub async fn official(state: &AppState) -> Result<Arc<Vec<Value>>> {
    let url = format!("{}/v1/scenemapping", state.config.sonarr_services.upstream);
    kept(&OFFICIAL, &url.clone(), OFFICIAL_FRESH, || async move {
        let body = get(state, "sonarr_services", &url).await?;
        let list: Vec<Value> =
            serde_json::from_slice(&body).context("the scene-mapping list is not a JSON list")?;
        if list.is_empty() {
            bail!("the scene-mapping list came back empty");
        }
        Ok(list)
    })
    .await
}

/// TheXEM's names, as Sonarr downloads them.
pub async fn xem(state: &AppState) -> Result<Arc<Vec<Claim>>> {
    let url = format!(
        "{}/map/allNames?origin=tvdb&seasonNumbers=1",
        state.config.sonarr_services.xem_upstream
    );
    kept(&XEM, &url.clone(), XEM_FRESH, || async move {
        let body = get(state, "thexem", &url).await?;
        let claims = xem_claims(&body)?;
        // Sonarr keeps the names it holds when TheXEM hands it none, so an
        // empty answer is not what Sonarr has: the copy kept is.
        if claims.is_empty() {
            bail!("TheXEM answered with no names");
        }
        Ok(claims)
    })
    .await
}

/// The names in TheXEM's answer: `{"result": "success", "data": {"<tvdb
/// id>": [{"<name>": <season>}, …]}}`.
pub fn xem_claims(body: &[u8]) -> Result<Vec<Claim>> {
    let answer: Value = serde_json::from_slice(body).context("TheXEM answered something else")?;
    if answer.get("result").and_then(Value::as_str) != Some("success") {
        bail!("TheXEM did not answer with success");
    }
    let Some(data) = answer.get("data").and_then(Value::as_object) else {
        bail!("TheXEM's answer holds no names");
    };
    let mut claims = Vec::new();
    for (id, names) in data {
        let Ok(tvdb_id) = id.parse::<i64>() else {
            continue;
        };
        for name in names.as_array().into_iter().flatten() {
            match name {
                Value::String(title) => claims.push(Claim {
                    tvdb_id,
                    title: title.clone(),
                }),
                Value::Object(titles) => claims.extend(titles.keys().map(|title| Claim {
                    tvdb_id,
                    title: title.clone(),
                })),
                _ => {}
            }
        }
    }
    Ok(claims)
}

/// The titles of Sonarr's own list.
pub fn official_claims(list: &[Value]) -> Vec<Claim> {
    list.iter()
        .filter_map(|entry| {
            Some(Claim {
                tvdb_id: entry.get("tvdbId")?.as_i64()?,
                title: entry.get("title")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// The list Sonarr is answered with: the real one, and what this catalogue
/// adds to it. An error when the real one cannot be had.
pub async fn answer(state: &AppState, language: &str) -> Result<Vec<Value>> {
    let official = official(state).await?;
    let mut list = official.as_ref().clone();

    // Without TheXEM's names, a title added could be one of them for
    // another series; the real list goes as it is rather than risk it.
    let xem = match xem(state).await {
        Ok(xem) => xem,
        Err(e) => {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "TheXEM's names could not be read; Sonarr's list is passed on without this \
                 catalogue's titles"
            );
            return Ok(list);
        }
    };

    let search = state.flag("sonarr.sceneMappingSearch", true);
    let catalogue = repo::scene::series_titles(&state.db).await?;
    let held = held(catalogue, &state.language(None, None), language, search);
    let mut known = official_claims(&official);
    known.extend(xem.iter().cloned());

    let added = additions(&held, &known);
    tracing::debug!(added = added.len(), "scene mappings added to Sonarr's list");
    list.extend(added.iter().map(Addition::to_json));
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sonarr's own cases, `NzbDrone.Core.Test/ParserTests/NormalizeSeriesTitleFixture.cs`.
    #[test]
    fn titles_clean_as_sonarr_cleans_them() {
        for (dirty, clean) in [
            ("Series", "series"),
            ("Series (2009)", "series2009"),
            ("Series.2010", "series2010"),
            ("Series_and_Title_Sonarr", "seriestitlesonarr"),
            ("CaPitAl", "capital"),
            ("peri.od", "period"),
            ("this.^&%^**$%@#$!That", "thisthat"),
            ("test/test", "testtest"),
            ("90210", "90210"),
            ("24", "24"),
            ("Test: Something à Deux", "testsomethingdeux"),
            ("Parler à", "parlera"),
            ("The Series", "theseries"),
            (
                "The Series Show With Sonarr Dev",
                "theseriesshowwithsonarrdev",
            ),
            ("The.Series.Show", "theseriesshow"),
            ("Series Title A", "seriestitlea"),
            ("3%", "3percent"),
            (
                "Series Top & 100% Coding Developers",
                "seriestop100percentcodingdevelopers",
            ),
            (
                "Series Title What's Your F@%king Deal?!",
                "seriestitlewhatsyourfkingdeal",
            ),
        ] {
            assert_eq!(clean_title(dirty), clean, "{dirty}");
        }

        for word in ["the", "and", "or", "an", "of"] {
            for format in ["word.{}.word", "word {} word", "word-{}-word"] {
                let dirty = format.replace("{}", word);
                assert_eq!(clean_title(&dirty), "wordword", "{dirty}");
            }
            for format in ["word.word.{}", "word-word-{}", "word-word {}"] {
                let dirty = format.replace("{}", word);
                assert_eq!(clean_title(&dirty), format!("wordword{word}"), "{dirty}");
            }
        }
        for word in ["the", "and", "or", "a", "an", "of"] {
            for format in [
                "word.{}word",
                "word {}word",
                "word-{}word",
                "word{}.word",
                "word{}-word",
            ] {
                let dirty = format.replace("{}", word);
                assert_eq!(clean_title(&dirty), format!("word{word}word"), "{dirty}");
            }
            for format in ["{}.word.word", "{}-word-word", "{} word word"] {
                let dirty = format.replace("{}", word);
                assert_eq!(clean_title(&dirty), format!("{word}wordword"), "{dirty}");
            }
        }
        for format in ["word.a.word", "word a word", "word-a-word"] {
            assert_eq!(clean_title(format), "wordword", "{format}");
        }
    }

    /// .NET's `\w` holds letters, nonspacing marks, decimal digits and
    /// connectors, and nothing else: `½`, `²` and `Ⅱ` are dropped like
    /// punctuation, and a full-width digit is a digit.
    #[test]
    fn numbers_of_other_kinds_are_dropped_as_sonarr_drops_them() {
        assert_eq!(clean_title("Ranma ½"), "ranma");
        assert_eq!(clean_title("Overlord Ⅱ"), "overlord");
        assert_eq!(clean_title("Kaguya-sama²"), "kaguyasama");
        assert_eq!(clean_title("３%"), "３percent");
        assert_eq!(clean_title("Pokémon"), "pokemon");
    }

    #[test]
    fn a_title_in_any_language_cleans_to_what_sonarr_compares() {
        assert_eq!(
            clean_title("Presque mariés, loin d'être amoureux."),
            "presquemariesloindetreamoureux"
        );
        assert_eq!(
            clean_title("Fuufu Ijou, Koibito Miman."),
            clean_title("Fuufu Ijou Koibito Miman")
        );
        assert_eq!(
            clean_title("Fūfu Ijō, Koibito Miman."),
            "fufuijokoibitomiman"
        );
    }

    #[test]
    fn only_latin_titles_are_kept() {
        assert!(is_latin("Presque mariés, loin d'être amoureux."));
        assert!(is_latin("Hơn Vợ Chồng Dưới Tình Nhân"));
        assert!(is_latin("Fūfu Ijō, Koibito Miman."));
        assert!(!is_latin("夫婦以上、恋人未満。"));
        assert!(!is_latin("Больше чем пара, меньше чем любовники"));
        assert!(!is_latin("1899"));
    }

    fn series(tvdb_id: i64, served: &str, searched: &[&str], other: &[&str]) -> Held {
        Held {
            tvdb_id,
            served: served.into(),
            searched: searched.iter().map(|t| t.to_string()).collect(),
            other: other.iter().map(|t| t.to_string()).collect(),
            shown: true,
        }
    }

    fn claim(tvdb_id: i64, title: &str) -> Claim {
        Claim {
            tvdb_id,
            title: title.into(),
        }
    }

    #[test]
    fn a_series_gains_the_titles_sonarr_does_not_know() {
        let held = [series(
            412806,
            "More than a Married Couple, but Not Lovers",
            &["Presque mariés, loin d'être amoureux"],
            &[
                "More than a Married Couple, but Not Lovers.",
                "Presque mariés, loin d'être amoureux.",
                "Fuufu Ijou Koibito Miman",
                "Fuukoi",
                "夫婦以上、恋人未満。",
                "More than a Couple, Less than Lovers.",
            ],
        )];
        // What TheXEM gives Sonarr for it already.
        let known = [
            claim(412806, "Fuufu Ijou, Koibito Miman"),
            claim(412806, "FuuKoi"),
        ];

        let added = additions(&held, &known);

        assert_eq!(
            added,
            [
                Addition {
                    tvdb_id: 412806,
                    title: "Presque mariés, loin d'être amoureux".into(),
                    search_title: "Presque mariés, loin d'être amoureux".into(),
                },
                // Recognised, and searched with under the series' own title.
                Addition {
                    tvdb_id: 412806,
                    title: "More than a Couple, Less than Lovers.".into(),
                    search_title: "More than a Married Couple, but Not Lovers".into(),
                },
            ]
        );
    }

    #[test]
    fn a_title_another_series_answers_to_is_never_added() {
        // Sonarr throws on a release name two series answer to, and in an
        // interactive search that takes every release down with it.
        let held = [
            series(1, "Anime", &[], &["Shared Title", "Only Mine"]),
            series(2, "Live Action", &[], &["Shared Title"]),
            series(3, "Other", &[], &["Listed Elsewhere", "The Other Main"]),
            series(4, "The Other Main", &[], &[]),
        ];
        let known = [claim(99, "Listed Elsewhere")];

        let added = additions(&held, &known);

        let titles: Vec<(i64, &str)> = added
            .iter()
            .map(|a| (a.tvdb_id, a.title.as_str()))
            .collect();
        assert_eq!(titles, [(1, "Only Mine")]);
    }

    #[test]
    fn short_and_numeric_titles_are_left_out() {
        let held = [series(1, "Main Title", &[], &["Ça", "1899", "Abc", "Abcd"])];
        let added = additions(&held, &[]);
        let titles: Vec<&str> = added.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, ["Abcd"]);
    }

    fn stored(title: &str, locked: bool, translated: &[(&str, &str)]) -> repo::scene::SeriesTitles {
        repo::scene::SeriesTitles {
            title: title.into(),
            title_locked: locked,
            enabled: true,
            tvdb: Some(412806),
            translated: translated
                .iter()
                .map(|(code, title)| (code.to_string(), title.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn sonarr_is_taken_to_know_a_series_by_its_english_title() {
        // Stored in French: Sonarr, which asks in English, is served the
        // English one, and the French one is worth adding.
        let held = held(
            vec![stored(
                "Presque mariés, loin d'être amoureux",
                false,
                &[
                    ("eng", "More than a Married Couple, but Not Lovers"),
                    ("fra", "Presque mariés, loin d'être amoureux"),
                ],
            )],
            "fr-FR",
            "fr-FR",
            true,
        );
        assert_eq!(held[0].served, "More than a Married Couple, but Not Lovers");
        assert!(!held[0].searched.is_empty());
        assert!(
            held[0]
                .searched
                .iter()
                .all(|t| t == "Presque mariés, loin d'être amoureux"),
            "{:?}",
            held[0].searched
        );

        // A locked title is served as it is, in English too.
        let held = held_locked();
        assert_eq!(held[0].served, "Mon titre");
    }

    fn held_locked() -> Vec<Held> {
        held(
            vec![stored("Mon titre", true, &[("eng", "English title")])],
            "fr-FR",
            "fr-FR",
            true,
        )
    }

    #[test]
    fn with_searching_off_nothing_added_is_searched_with() {
        let held = held(
            vec![stored("Main", false, &[("fra", "Titre en français")])],
            "fr-FR",
            "fr-FR",
            false,
        );
        assert!(held[0].searched.is_empty());
        let added = additions(&held, &[]);
        assert!(added.iter().all(|a| a.search_title == "Main"), "{added:?}");
    }

    #[test]
    fn only_titles_in_a_language_releases_use_are_offered() {
        let mut work = stored(
            "More than a Married Couple, but Not Lovers",
            false,
            &[
                ("fra", "Presque mariés, loin d'être amoureux"),
                ("ces", "Víc než manželé"),
                ("eng", "More than a Married Couple, but Not Lovers"),
            ],
        );
        work.original_language = Some("jpn".into());
        work.original_title = Some("夫婦以上、恋人未満。".into());
        work.alternative = vec![
            (Some("jpn".into()), "Fūfu Ijō, Koibito Miman.".into()),
            (
                Some("eng".into()),
                "More than a Couple, Less than Lovers.".into(),
            ),
            (
                Some("por".into()),
                "Mais Que Um Casal, Menos Que Amantes.".into(),
            ),
            (None, "Presque mariés, loin d'être amoureux.".into()),
        ];

        let held = held(vec![work], "en-US", "fr-FR", true);

        assert_eq!(held[0].searched, ["Presque mariés, loin d'être amoureux"]);
        let other = &held[0].other;
        assert!(
            other.contains(&"Fūfu Ijō, Koibito Miman.".to_string()),
            "{other:?}"
        );
        assert!(other.contains(&"More than a Couple, Less than Lovers.".to_string()));
        assert!(
            !other
                .iter()
                .any(|t| t.starts_with("Víc") || t.starts_with("Mais")),
            "{other:?}"
        );
        assert!(!other.contains(&"Presque mariés, loin d'être amoureux.".to_string()));
    }

    #[test]
    fn with_english_the_servers_language_sonarr_is_served_the_stored_title() {
        // No overlay then: Sonarr's English is the language works are kept in.
        let held = held(
            vec![stored(
                "Stored Title",
                false,
                &[("eng", "Another English Title")],
            )],
            "en-US",
            "en-US",
            true,
        );
        assert_eq!(held[0].served, "Stored Title");
        let added = additions(&held, &[]);
        assert_eq!(added.len(), 1, "{added:?}");
        assert_eq!(added[0].title, "Another English Title");
    }

    #[test]
    fn a_hidden_work_gains_no_title_and_keeps_its_own() {
        let mut hidden = series(1, "Its Own Name", &[], &["Shared Name"]);
        hidden.shown = false;
        let held = [
            hidden,
            series(2, "Shown", &[], &["Its Own Name", "Mine Alone"]),
        ];
        let added = additions(&held, &[]);
        let titles: Vec<(i64, &str)> = added
            .iter()
            .map(|a| (a.tvdb_id, a.title.as_str()))
            .collect();
        assert_eq!(titles, [(2, "Mine Alone")]);
    }

    #[test]
    fn a_title_naming_a_part_of_the_series_is_left_out() {
        // A release of the minisodes is not an episode of the series.
        let held = [series(
            81189,
            "Breaking Bad",
            &["Breaking Bad"],
            &["Breaking Bad: Original Minisodes", "BrBa", "Perníkový táta"],
        )];
        let added = additions(&held, &[]);
        let titles: Vec<&str> = added.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, ["BrBa", "Perníkový táta"]);

        // Nor is the first book's title, which is only the first season's.
        let held = [series(
            433637,
            "Harry Potter",
            &[],
            &[
                "Harry Potter and the Philosopher's Stone",
                "Harry Potter à l'école des sorciers",
            ],
        )];
        assert!(additions(&held, &[]).is_empty());
    }

    #[test]
    fn each_homonym_is_known_by_what_tells_it_apart_in_sonarr() {
        let mut recent = stored("Rurouni Kenshin", false, &[]);
        recent.tvdb = Some(413578);
        recent.title_qualifier = Some("2023".into());
        recent.year = Some(2023);
        let mut first = stored("Rurouni Kenshin", false, &[]);
        first.tvdb = Some(70863);
        first.year = Some(1996);
        let mut tmdb_only = stored("Rurouni Kenshin", false, &[]);
        tmdb_only.tvdb = None;
        tmdb_only.tmdb = Some(99);
        tmdb_only.year = Some(2010);

        let held = held(vec![recent, first, tmdb_only], "en-US", "en-US", true);

        let served: Vec<&str> = held.iter().map(|h| h.served.as_str()).collect();
        assert_eq!(
            served,
            [
                "Rurouni Kenshin (2023)",
                "Rurouni Kenshin",
                "Rurouni Kenshin (2010)"
            ]
        );
    }

    #[test]
    fn a_series_sonarr_cannot_address_has_no_mapping() {
        let mut work = stored("Nameless", false, &[]);
        work.tvdb = None;
        assert!(held(vec![work], "en", "en", true).is_empty());
    }

    #[test]
    fn thexems_names_are_read_as_sonarr_reads_them() {
        let body = br#"{"result":"success","data":{"412806":[{"Fuufu Ijou, Koibito Miman":-1},{"FuuKoi":-1}],"x":[{"y":1}]}}"#;
        let claims = xem_claims(body).unwrap();
        let titles: Vec<(i64, &str)> = claims
            .iter()
            .map(|c| (c.tvdb_id, c.title.as_str()))
            .collect();
        assert_eq!(
            titles,
            [(412806, "Fuufu Ijou, Koibito Miman"), (412806, "FuuKoi")]
        );

        assert!(xem_claims(br#"{"result":"failure","data":{}}"#).is_err());
    }

    #[test]
    fn a_mapping_reads_as_sonarrs_own() {
        let json = Addition {
            tvdb_id: 1,
            title: "Titre".into(),
            search_title: "Title".into(),
        }
        .to_json();
        assert_eq!(json["tvdbId"], 1);
        assert_eq!(json["title"], "Titre");
        assert_eq!(json["searchTitle"], "Title");
        assert_eq!(json["season"], -1);
        assert_eq!(json["comment"], COMMENT);
    }
}

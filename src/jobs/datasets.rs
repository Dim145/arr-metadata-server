//! Lists downloaded whole, on a schedule, and asked locally.
//!
//! Two of the further sources are not asked per request at all. The anime
//! identifier list says which AniList and MyAnimeList entries a series or a
//! film is; IMDb's ratings are a file IMDb republishes every day. Both are
//! fetched here, kept in the database, and read from there — one download a
//! week or a day, however many works are asked about in between.
//!
//! Each runs only while a source that needs it is switched on, and each is
//! recorded as a job run so an operator can see it happened and why it failed.

use std::{
    collections::HashSet,
    io::{BufRead as _, Read as _},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    db::{
        now, parse_rfc3339,
        repo::{self, anime::Entry, imdb::Rating, import::Import, job},
    },
    providers::read_body,
    state::AppState,
};

/// The names imports are recorded under in `data_import`.
pub const ANIME: &str = "anime-lists";
pub const IMDB: &str = "imdb-ratings";

/// How often each list is fetched again. The anime list moves by a handful of
/// entries a week; IMDb republishes its ratings daily.
const ANIME_EVERY: chrono::Duration = chrono::Duration::days(7);
const IMDB_EVERY: chrono::Duration = chrono::Duration::days(1);

/// How soon IMDb's ratings are fetched again for works added since the last
/// import. Only the works held here are kept from the file, so a work added an
/// hour after the import would otherwise wait the rest of the day for its
/// rating — and on a fresh install, that is every work there is.
const IMDB_CATCH_UP: chrono::Duration = chrono::Duration::hours(1);

/// How long a failed import waits before it is tried again.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

/// How often the scheduler looks. Short, so a source switched on in the
/// interface has what it needs within a minute rather than at the next day.
const TICK: Duration = Duration::from_secs(60);

/// The largest each list is accepted at: a few times its size today — the
/// anime list is seven and a half megabytes, IMDb's ratings eight compressed
/// and thirty unpacked — so a mirror that has broken, or been broken into,
/// costs a refusal rather than the machine's memory.
const MAX_ANIME_LIST: u64 = 32 * 1024 * 1024;
const MAX_RATINGS_FILE: u64 = 64 * 1024 * 1024;
const MAX_RATINGS_UNPACKED: u64 = 256 * 1024 * 1024;

/// A line of IMDb's ratings file is some thirty bytes.
const MAX_RATINGS_LINE: u64 = 4096;

/// Whether anything needs the anime identifier list.
pub fn anime_wanted(state: &AppState) -> bool {
    state.flag("anilist.enabled", false) || state.flag("mal.enabled", false)
}

/// Run the scheduler until the process shuts down.
pub async fn run(state: AppState) {
    // Let the server finish starting before the first download.
    tokio::time::sleep(Duration::from_secs(20)).await;

    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut anime_failed: Option<tokio::time::Instant> = None;
    let mut imdb_failed: Option<tokio::time::Instant> = None;

    loop {
        ticker.tick().await;

        // Asked again once the lock is held: an import somebody asked for by
        // hand may have been what it waited on, and has just done the work.
        if anime_wanted(&state) && retry_allowed(anime_failed) && anime_due(&state).await {
            let _importing = IMPORTING.lock().await;
            if anime_due(&state).await {
                let ok = record(&state, job::kinds::IMPORT_ANIME, import_anime(&state))
                    .await
                    .is_ok();
                anime_failed = (!ok).then(tokio::time::Instant::now);
            }
        }

        if state.flag("imdb.enabled", false) && retry_allowed(imdb_failed) && imdb_due(&state).await
        {
            let _importing = IMPORTING.lock().await;
            if imdb_due(&state).await {
                let ok = record(&state, job::kinds::IMPORT_IMDB, import_imdb(&state))
                    .await
                    .is_ok();
                imdb_failed = (!ok).then(tokio::time::Instant::now);
            }
        }
    }
}

fn retry_allowed(failed: Option<tokio::time::Instant>) -> bool {
    failed.is_none_or(|at| at.elapsed() >= RETRY_AFTER)
}

/// The last import of a list, `Some(None)` if there has been none; `None` when
/// the database could not say, which is never a reason to download.
async fn last_import(state: &AppState, name: &str) -> Option<Option<Import>> {
    match repo::import::get(&state.db, name).await {
        Ok(last) => Some(last),
        Err(e) => {
            tracing::warn!(
                name,
                error = format_args!("{e:#}"),
                "could not read when a list was last imported"
            );
            None
        }
    }
}

/// Whether the anime list has never been imported, or not for a week.
async fn anime_due(state: &AppState) -> bool {
    last_import(state, ANIME)
        .await
        .is_some_and(|last| due(last.as_ref(), ANIME_EVERY))
}

/// Whether a list has never been imported, or not for `every`.
fn due(last: Option<&Import>, every: chrono::Duration) -> bool {
    last.and_then(|l| parse_rfc3339(&l.imported_at))
        .is_none_or(|at| chrono::Utc::now() - at >= every)
}

/// Whether IMDb's ratings are due: once a day, and sooner — though not more
/// than hourly — when works have been added since they were last read.
async fn imdb_due(state: &AppState) -> bool {
    let Some(last) = last_import(state, IMDB).await else {
        return false;
    };
    let Some(last) = last else {
        return true;
    };

    if due(Some(&last), IMDB_EVERY) {
        return true;
    }

    due(Some(&last), IMDB_CATCH_UP)
        && repo::imdb::added_since(&state.db, &last.imported_at)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    error = format_args!("{e:#}"),
                    "could not tell whether works were added"
                );
                false
            })
}

/// One import at a time, whether the scheduler or an operator asked for it:
/// two writers replacing the same table at once would each wait out the other.
static IMPORTING: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// What asking for an import now came to.
pub enum Asked {
    Imported(String),
    /// Another import is running; this one was not started.
    Busy,
    /// No source that needs this list is switched on.
    NotWanted,
    Unknown,
    Failed(anyhow::Error),
}

/// Import one list now, rather than when the schedule next says.
///
/// For an operator who has just switched a source on, or who suspects a list
/// is stale. Recorded as a job like any other import.
pub async fn import_now(state: &AppState, name: &str) -> Asked {
    let wanted = match name {
        ANIME => anime_wanted(state),
        IMDB => state.flag("imdb.enabled", false),
        _ => return Asked::Unknown,
    };
    if !wanted {
        return Asked::NotWanted;
    }

    let Ok(_importing) = IMPORTING.try_lock() else {
        return Asked::Busy;
    };

    let outcome = match name {
        ANIME => record(state, job::kinds::IMPORT_ANIME, import_anime(state)).await,
        _ => record(state, job::kinds::IMPORT_IMDB, import_imdb(state)).await,
    };

    match outcome {
        Ok(summary) => Asked::Imported(summary),
        Err(e) => Asked::Failed(e),
    }
}

/// Run one import as a job, so it shows in the interface.
async fn record(
    state: &AppState,
    kind: &str,
    work: impl Future<Output = Result<String>>,
) -> Result<String> {
    let run = match job::start(&state.db, kind, None).await {
        Ok(id) => Some(id),
        Err(e) => {
            // Losing the record must not stop the work.
            tracing::warn!(error = %e, "could not open a job run");
            None
        }
    };

    let outcome = work.await;

    if let Some(run) = run {
        let closed = match &outcome {
            Ok(summary) => job::finish(&state.db, &run, Some(summary), None).await,
            Err(e) => job::finish(&state.db, &run, None, Some(&format!("{e:#}"))).await,
        };
        if let Err(e) = closed {
            tracing::warn!(error = %e, "could not close the job run");
        }
    }

    match &outcome {
        Ok(summary) => tracing::info!(kind, summary, "list imported"),
        Err(e) => tracing::warn!(
            kind,
            error = format_args!("{e:#}"),
            "list import failed; the previous one stays"
        ),
    }

    outcome
}

async fn download(state: &AppState, url: &str, limit: u64) -> Result<Vec<u8>> {
    let response = state
        .http
        .get(url)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;

    let status = response.status();
    if !status.is_success() {
        bail!("{url} answered {status}");
    }

    read_body(response, limit)
        .await
        .with_context(|| format!("could not read {url}"))
}

// ─── the anime identifier list ───────────────────────────────────────────────

async fn import_anime(state: &AppState) -> Result<String> {
    let started = now();
    let body = download(state, &state.config.anime_mapping.url, MAX_ANIME_LIST).await?;
    let entries = tokio::task::spawn_blocking(move || parse_anime(&body)).await??;

    // An empty answer is a broken mirror, not a list with nothing in it; one
    // far shorter than the last is a format this server no longer reads —
    // elements it cannot read are skipped whole. The list only ever grows, and
    // taking either would forget what this server knew until next week.
    if entries.is_empty() {
        bail!("the list had no usable entries");
    }
    if let Some(previous) = repo::import::get(&state.db, ANIME).await?
        && (entries.len() as i64) < previous.row_count / 2
    {
        bail!(
            "the list has {} usable entries against {} last time; keeping the previous one",
            entries.len(),
            previous.row_count
        );
    }

    repo::anime::replace_all(&state.db, &entries).await?;
    repo::import::record(&state.db, ANIME, entries.len() as i64, &started).await?;

    Ok(format!("{} entries", entries.len()))
}

/// One element of the Fribb list, as far as this server reads it.
#[derive(Debug, Deserialize)]
struct AnimeRecord {
    #[serde(rename = "type")]
    kind: Option<String>,
    mal_id: Option<i64>,
    anilist_id: Option<i64>,
    tvdb_id: Option<i64>,
    season: Option<TvdbPlace>,
    episode_offset: Option<TvdbPlace>,
    /// `{"tv": 1429}` or `{"movie": [372058]}`. Read loosely: the project
    /// changed this field's shape once already.
    themoviedb_id: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TvdbPlace {
    tvdb: Option<i32>,
}

/// The rows this server keeps from the list.
///
/// An entry with neither an AniList nor a MyAnimeList id has nothing to look
/// up, and one with neither a TheTVDB series nor a TMDB film has nothing to be
/// looked up from. An entry filed under several TMDB films becomes a row for
/// each. An element that does not read is skipped rather than failing the list.
fn parse_anime(body: &[u8]) -> Result<Vec<Entry>> {
    let elements: Vec<serde_json::Value> =
        serde_json::from_slice(body).context("the anime list is not a JSON array")?;

    let mut entries = Vec::new();

    for element in elements {
        let Ok(record) = serde_json::from_value::<AnimeRecord>(element) else {
            continue;
        };

        if record.mal_id.is_none() && record.anilist_id.is_none() {
            continue;
        }

        let movies: Vec<i64> = record
            .themoviedb_id
            .as_ref()
            .and_then(|v| v.get("movie"))
            .and_then(|v| v.as_array())
            .map(|ids| ids.iter().filter_map(serde_json::Value::as_i64).collect())
            .unwrap_or_default();

        if record.tvdb_id.is_none() && movies.is_empty() {
            continue;
        }

        let entry = Entry {
            mal_id: record.mal_id,
            anilist_id: record.anilist_id,
            tvdb_id: record.tvdb_id,
            tvdb_season: record.season.and_then(|s| s.tvdb),
            tvdb_offset: record.episode_offset.and_then(|o| o.tvdb),
            tmdb_movie: None,
            kind: record.kind,
        };

        if movies.is_empty() {
            entries.push(entry);
        } else {
            entries.extend(movies.into_iter().map(|id| Entry {
                tmdb_movie: Some(id),
                ..entry.clone()
            }));
        }
    }

    Ok(entries)
}

// ─── IMDb's ratings ──────────────────────────────────────────────────────────

async fn import_imdb(state: &AppState) -> Result<String> {
    // Before the ids are read: a work stored while this runs is newer than the
    // import, so the catch-up sees it.
    let started = now();
    let wanted: HashSet<String> = repo::imdb::wanted(&state.db).await?.into_iter().collect();

    let url = format!(
        "{}/title.ratings.tsv.gz",
        state.config.imdb.datasets.trim_end_matches('/')
    );
    let body = download(state, &url, MAX_RATINGS_FILE).await?;
    let wanted_count = wanted.len();
    let ratings = tokio::task::spawn_blocking(move || parse_ratings(&body, &wanted)).await??;

    // IMDb rates nearly every title it lists. A file that rates almost none of
    // the works held here was misread, and taking it would wipe their ratings.
    if wanted_count >= 50 && ratings.len() * 4 < wanted_count {
        bail!(
            "only {} of {wanted_count} works were rated; keeping the previous ratings",
            ratings.len()
        );
    }

    repo::imdb::replace_all(&state.db, &ratings).await?;
    repo::import::record(&state.db, IMDB, ratings.len() as i64, &started).await?;

    // A cached work carries the rating it was loaded with.
    state.caches.invalidate_all().await;

    Ok(format!("{} of {wanted_count} works rated", ratings.len()))
}

/// The ratings in IMDb's `title.ratings.tsv.gz` for the titles in `wanted`.
///
/// Three tab-separated columns under a header: `tconst`, `averageRating`,
/// `numVotes`. Read a line at a time from the compressed file, so the million
/// and a half titles never sit in memory at once.
///
/// Bounded twice over, because eight compressed megabytes can unpack to any
/// size at all: a line may not run past `MAX_RATINGS_LINE`, and the whole file
/// may not unpack past `MAX_RATINGS_UNPACKED`.
fn parse_ratings(gz: &[u8], wanted: &HashSet<String>) -> Result<Vec<Rating>> {
    let mut reader = std::io::BufReader::new(
        flate2::read::MultiGzDecoder::new(gz).take(MAX_RATINGS_UNPACKED + 1),
    );
    let mut buffer = Vec::new();
    let mut header = true;
    let mut ratings = Vec::new();

    loop {
        buffer.clear();
        let read = (&mut reader)
            .take(MAX_RATINGS_LINE + 1)
            .read_until(b'\n', &mut buffer)
            .context("IMDb's ratings file could not be decompressed")?;

        if read == 0 {
            break;
        }
        if buffer.len() as u64 > MAX_RATINGS_LINE {
            bail!("IMDb's ratings file has a line longer than any it should");
        }
        if reader.get_ref().limit() == 0 {
            bail!("IMDb's ratings file unpacks to more than {MAX_RATINGS_UNPACKED} bytes");
        }

        let line = std::str::from_utf8(&buffer)
            .context("IMDb's ratings file is not text")?
            .trim_end_matches(['\n', '\r']);

        if std::mem::take(&mut header) {
            if !line.starts_with("tconst\taverageRating\tnumVotes") {
                bail!("IMDb's ratings file does not start with the header it should: {line:?}");
            }
            continue;
        }

        let mut columns = line.split('\t');

        let (Some(tconst), Some(average), Some(votes)) =
            (columns.next(), columns.next(), columns.next())
        else {
            continue;
        };

        if !wanted.contains(tconst) {
            continue;
        }

        let (Ok(rating), Ok(votes)) = (average.parse::<f64>(), votes.parse::<i64>()) else {
            continue;
        };

        ratings.push(Rating {
            tconst: tconst.to_string(),
            rating,
            votes,
        });
    }

    if header {
        bail!("IMDb's ratings file is empty");
    }

    Ok(ratings)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn the_anime_list_becomes_rows_for_what_can_be_looked_up() {
        let body = br#"[
            {"type": "TV", "anidb_id": 9541, "anilist_id": 16498, "mal_id": 16498,
             "themoviedb_id": {"tv": 1429}, "tvdb_id": 267440, "season": {"tvdb": 1, "tmdb": 1}},
            {"type": "TV", "anilist_id": 104578, "mal_id": 38524, "tvdb_id": 267440,
             "season": {"tvdb": 3}, "episode_offset": {"tvdb": 12}},
            {"type": "MOVIE", "anilist_id": 21519, "mal_id": 32281,
             "themoviedb_id": {"movie": [372058]}},
            {"type": "MOVIE", "mal_id": 460, "tvdb_id": 81797, "season": {"tvdb": 0},
             "themoviedb_id": {"movie": [23446, 23447]}},
            {"type": "TV", "anidb_id": 1, "tvdb_id": 5},
            {"type": "TV", "mal_id": 7},
            {"type": "TV", "mal_id": "not a number", "tvdb_id": 8}
        ]"#;

        let rows = parse_anime(body).unwrap();

        assert_eq!(rows.len(), 5, "{rows:#?}");

        assert_eq!(rows[0].tvdb_id, Some(267440));
        assert_eq!(rows[0].tvdb_season, Some(1));
        assert_eq!(
            rows[0].tmdb_movie, None,
            "a series' TMDB id is not a film's"
        );

        assert_eq!(rows[1].tvdb_offset, Some(12));

        assert_eq!(rows[2].tmdb_movie, Some(372058));
        assert_eq!(rows[2].tvdb_id, None);

        // A film TheTVDB files under a series keeps both, once per TMDB id.
        assert_eq!(rows[3].tmdb_movie, Some(23446));
        assert_eq!(rows[4].tmdb_movie, Some(23447));
        assert_eq!(rows[4].tvdb_id, Some(81797));
    }

    #[test]
    fn a_list_that_is_not_json_is_refused() {
        assert!(parse_anime(b"<html>rate limited</html>").is_err());
    }

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn only_the_wanted_ratings_are_kept() {
        let file = gzip(
            "tconst\taverageRating\tnumVotes\n\
             tt0903747\t9.5\t2300000\n\
             tt0000001\t5.7\t2100\n\
             tt2560140\t9.1\t750000\n\
             tt9999999\tnot-a-number\t3\n",
        );
        let wanted: HashSet<String> = ["tt0903747", "tt2560140", "tt9999999"]
            .into_iter()
            .map(String::from)
            .collect();

        let ratings = parse_ratings(&file, &wanted).unwrap();

        assert_eq!(
            ratings,
            vec![
                Rating {
                    tconst: "tt0903747".into(),
                    rating: 9.5,
                    votes: 2_300_000
                },
                Rating {
                    tconst: "tt2560140".into(),
                    rating: 9.1,
                    votes: 750_000
                },
            ]
        );
    }

    #[test]
    fn a_ratings_file_that_unpacks_into_a_monster_is_refused() {
        // Compresses to almost nothing; would unpack into one line of any size.
        let file = gzip(&format!(
            "tconst\taverageRating\tnumVotes\n{}",
            "a".repeat(100_000)
        ));
        let error = parse_ratings(&file, &HashSet::new()).unwrap_err();

        assert!(
            error.to_string().contains("longer than any it should"),
            "{error:#}"
        );
    }

    #[test]
    fn a_ratings_file_without_its_header_is_refused() {
        let file = gzip("<html>blocked</html>\n");
        assert!(parse_ratings(&file, &HashSet::new()).is_err());
        assert!(parse_ratings(b"not gzip at all", &HashSet::new()).is_err());
    }
}

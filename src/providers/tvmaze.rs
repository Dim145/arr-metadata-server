//! TVmaze: the moment each episode aired.
//!
//! TheTVDB keeps one broadcast time per series and Skyhook applies it to every
//! episode, so a show that changed time slot is wrong for everything before the
//! change — *The Big Bang Theory*'s third season, Mondays at 21:30, reaches
//! Sonarr an hour and a half early. TVmaze keeps a time per episode, in the
//! network's timezone, and publishes the resulting instant as `airstamp`.
//! Sonarr does not search for an episode before that instant, so it is the
//! field worth fetching this source for.
//!
//! No key: TVmaze's API keys open its user API — a premium member's own marks,
//! follows and votes — and change nothing of the public one read here, which
//! is limited per address. Twenty calls in ten seconds: one per series once its
//! TVmaze id is known, three the first time. The data is CC BY-SA, which asks
//! for a link back to TVmaze wherever it is shown — the interface carries one
//! when this source is on.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    config,
    db::{new_id, now},
    domain::{Episode, ExternalIds, MediaItem, MediaKind},
    providers::{PATIENCE, Pacer, plain_text},
};

/// How long TVmaze is given for an answer while a client waits on this
/// server: it is cut at `AMS_REQUEST_TIMEOUT`, sixty seconds by default, and
/// the other sources take their time as well.
const ANSWER: Duration = Duration::from_secs(20);

/// How long it is given in a background task, which nobody waits on. What
/// its cache does not hold — the first lookup of a series, a little-asked
/// list of episodes — its servers take fifteen to twenty seconds over, now
/// and then, whatever the pace of the calls: they answer in the end.
const ANSWER_UNHURRIED: Duration = Duration::from_secs(60);

/// The pause before asking again, in a background task, for what TVmaze never
/// answered: by then it has often finished, and the answer comes from its
/// cache. As TVmaze asks of its clients, a pause and a second try rather than
/// a failure.
const AGAIN_AFTER: Duration = Duration::from_secs(2);

pub struct TvmazeClient {
    http: reqwest::Client,
    base: String,
    pacer: Pacer,
    gate: crate::providers::Gate,
    answer: Duration,
    answer_unhurried: Duration,
    again_after: Duration,
}

impl TvmazeClient {
    pub fn new(http: reqwest::Client, cfg: &config::Tvmaze) -> Self {
        Self {
            http,
            base: cfg.upstream.clone(),
            // Twenty calls in ten seconds, spaced rather than bunched.
            pacer: Pacer::new(Duration::from_millis(500)),
            // Spaced already; this is for its `Retry-After`.
            gate: crate::providers::Gate::new("tvmaze", "TVmaze", 4),
            answer: ANSWER,
            answer_unhurried: ANSWER_UNHURRIED,
            again_after: AGAIN_AFTER,
        }
    }

    /// A series and its episodes, by TVmaze id when it is already known and by
    /// TheTVDB id otherwise. Returns the raw answer alongside the parsed one.
    ///
    /// Known, it is one request: the show with its episodes embedded. Unknown,
    /// it is three — the lookup, the redirect it answers with (which drops any
    /// query, `embed` included), and the episodes — and the pacer is charged
    /// for all three.
    pub async fn series(
        &self,
        tvdb_id: Option<i64>,
        tvmaze_id: Option<i64>,
    ) -> Result<Option<(Value, MediaItem)>> {
        if let Some(id) = tvmaze_id
            && let Some(mut show) = self
                .fetch(
                    &format!("{}/shows/{id}", self.base),
                    &[("embed", "episodes")],
                    1,
                )
                .await?
        {
            let episodes = show
                .as_object_mut()
                .and_then(|o| o.remove("_embedded"))
                .and_then(|mut embedded| embedded.get_mut("episodes").map(Value::take))
                .unwrap_or(Value::Array(Vec::new()));

            let parsed = Show::deserialize(&show)
                .context("TVmaze returned a show this server could not read")?;

            // An id stored from another provider's answer is checked against
            // the one this fetch is for; a mismatch is looked up afresh.
            // Its own TVmaze id is enough; with a TheTVDB id too, both must agree.
            if tvdb_id.is_none_or(|tvdb| parsed.externals.thetvdb.is_none_or(|t| t == tvdb)) {
                return Self::parsed(show, parsed, episodes).map(Some);
            }
        }

        let Some(tvdb_id) = tvdb_id else {
            return Ok(None);
        };
        let tvdb = tvdb_id.to_string();
        let Some(show) = self
            .fetch(
                &format!("{}/lookup/shows", self.base),
                &[("thetvdb", tvdb.as_str())],
                2,
            )
            .await?
        else {
            return Ok(None);
        };

        let parsed = Show::deserialize(&show)
            .context("TVmaze returned a show this server could not read")?;

        let episodes = self
            .fetch(
                &format!("{}/shows/{}/episodes", self.base, parsed.id),
                &[],
                1,
            )
            .await?
            .unwrap_or(Value::Array(Vec::new()));

        Self::parsed(show, parsed, episodes).map(Some)
    }

    fn parsed(show: Value, parsed: Show, episodes: Value) -> Result<(Value, MediaItem)> {
        let listed = Vec::<EpisodeRecord>::deserialize(&episodes)
            .context("TVmaze returned episodes this server could not read")?;

        let item = to_item(&parsed, &listed);
        Ok((json!({ "show": show, "episodes": episodes }), item))
    }

    /// One request, or `calls` of them when TVmaze is known to answer with a
    /// redirect the client follows. In a background task it is given longer,
    /// and asked once more when it never answered (see [`ANSWER_UNHURRIED`]).
    async fn fetch(&self, url: &str, query: &[(&str, &str)], calls: u32) -> Result<Option<Value>> {
        let unhurried = crate::providers::is_unhurried();
        let wait = if unhurried {
            self.answer_unhurried
        } else {
            self.answer
        };
        let mut again = unhurried;

        let (response, _permit) = loop {
            if !self.pacer.turn(calls, PATIENCE).await {
                anyhow::bail!("TVmaze's queue is full; this fetch goes without it");
            }
            match self
                .gate
                .send(|| self.http.get(url).query(query).timeout(wait))
                .await
            {
                Ok(answered) => break answered,
                Err(e) if again && crate::providers::timed_out(&e) => {
                    tracing::debug!(url, "TVmaze did not answer in time; asking once more");
                    again = false;
                    tokio::time::sleep(self.again_after).await;
                }
                Err(e) => anyhow::bail!("TVmaze request failed: {url}: {e}"),
            }
        };

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            anyhow::bail!("TVmaze's rate limit was reached");
        }

        let status = response.status();
        if !status.is_success() {
            let reason = crate::providers::error_text(response).await;
            anyhow::bail!("TVmaze returned {status} for {url}: {reason}");
        }

        crate::providers::read_json(response)
            .await
            .map(Some)
            .with_context(|| format!("TVmaze returned a body this server could not read: {url}"))
    }
}

// ─── the wire ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Show {
    id: i64,
    #[serde(default)]
    name: String,
    summary: Option<String>,
    runtime: Option<i32>,
    #[serde(rename = "averageRuntime")]
    average_runtime: Option<i32>,
    #[serde(default)]
    externals: Externals,
}

#[derive(Debug, Default, Deserialize)]
struct Externals {
    thetvdb: Option<i64>,
    imdb: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EpisodeRecord {
    season: Option<i32>,
    /// Absent for a special, which TVmaze files under a season without a number.
    number: Option<i32>,
    name: Option<String>,
    summary: Option<String>,
    airdate: Option<String>,
    /// Blank when TVmaze does not know the time — typically a streaming
    /// release — and then `airstamp` is noon UTC on the date, not a moment.
    airtime: Option<String>,
    airstamp: Option<String>,
    runtime: Option<i32>,
    image: Option<ImageRecord>,
}

#[derive(Debug, Deserialize)]
struct ImageRecord {
    original: Option<String>,
}

// ─── mapping ─────────────────────────────────────────────────────────────────

fn to_item(show: &Show, episodes: &[EpisodeRecord]) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Series);

    item.title = show.name.clone();
    item.overview = show
        .summary
        .as_deref()
        .map(plain_text)
        .filter(|s| !s.is_empty());
    item.runtime = show.average_runtime.or(show.runtime).filter(|r| *r > 0);
    item.external_ids = ExternalIds {
        tvmaze: Some(show.id),
        tvdb: show.externals.thetvdb,
        imdb: show.externals.imdb.clone().filter(|s| !s.is_empty()),
        ..Default::default()
    };
    item.updated_at = now();

    // A special has no number here, and TheTVDB files specials under season 0
    // with numbers of its own: there is no honest way to line the two up, so
    // specials are left to the providers that number them.
    item.episodes = episodes
        .iter()
        .filter_map(|record| {
            let (Some(season), Some(number)) = (record.season, record.number) else {
                return None;
            };

            Some(Episode {
                id: new_id(),
                season_number: season,
                episode_number: number,
                absolute_episode_number: None,
                aired_after_season_number: None,
                aired_before_season_number: None,
                aired_before_episode_number: None,
                title: record.name.clone().unwrap_or_default(),
                overview: record
                    .summary
                    .as_deref()
                    .map(plain_text)
                    .filter(|s| !s.is_empty()),
                air_date: record.airdate.clone().filter(|d| !d.is_empty()),
                air_date_utc: record
                    .airtime
                    .as_deref()
                    .filter(|t| !t.trim().is_empty())
                    .and(record.airstamp.as_deref())
                    .and_then(instant),
                runtime: record.runtime.filter(|r| *r > 0),
                finale_type: None,
                image: record.image.as_ref().and_then(|i| i.original.clone()),
                tvdb_id: None,
                tmdb_id: None,
                rating: None,
                is_manual: false,
            })
        })
        .collect();

    item
}

/// `2008-01-21T03:00:00+00:00` as the `…Z` form every other source uses.
///
/// Parsed rather than trusted: an airstamp that is not a timestamp is dropped,
/// because this field decides when Sonarr goes looking.
fn instant(airstamp: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(airstamp.trim())
        .ok()
        .map(|t| {
            t.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TVmaze that keeps its first caller waiting without a word, then
    /// answers the show with its episodes; and how many came.
    async fn silent_then_answering() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = asked.clone();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((mut connection, _)) = listener.accept().await {
                let first = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
                let mut request = [0u8; 4096];
                let _ = connection.read(&mut request).await;
                if first {
                    held.push(connection);
                    continue;
                }
                let body = json!({
                    "id": 169,
                    "name": "Breaking Bad",
                    "externals": { "thetvdb": 81189 },
                    "_embedded": { "episodes": [] }
                })
                .to_string();
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = connection.write_all(answer.as_bytes()).await;
                let _ = connection.shutdown().await;
            }
        });
        (at, asked)
    }

    /// The client, given a fraction of a second where TVmaze is given tens.
    fn impatient(at: &str) -> TvmazeClient {
        let mut client = TvmazeClient::new(
            reqwest::Client::new(),
            &config::Tvmaze {
                upstream: at.to_string(),
                enabled: true,
            },
        );
        client.answer = Duration::from_millis(300);
        client.answer_unhurried = Duration::from_millis(300);
        client.again_after = Duration::from_millis(10);
        client
    }

    #[tokio::test]
    async fn in_a_background_task_what_tvmaze_never_answered_is_asked_once_more() {
        let (at, asked) = silent_then_answering().await;
        let tvmaze = impatient(&at);

        let (_, item) = crate::providers::unhurried(tvmaze.series(Some(81189), Some(169)))
            .await
            .expect("answered the second time")
            .expect("the show");
        assert_eq!(item.title, "Breaking Bad");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_client_waiting_is_not_kept_for_a_second_try() {
        let (at, asked) = silent_then_answering().await;
        let tvmaze = impatient(&at);

        let failed = tvmaze
            .series(Some(81189), Some(169))
            .await
            .expect_err("never answered");
        assert!(format!("{failed:#}").contains("timed out"), "{failed:#}");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    fn fixture() -> (Show, Vec<EpisodeRecord>) {
        let show: Show = serde_json::from_value(json!({
            "id": 169,
            "name": "Breaking Bad",
            "summary": "<p><b>Breaking Bad</b> follows a chemistry teacher.</p>",
            "runtime": 60,
            "averageRuntime": 60,
            "externals": { "tvrage": 18164, "thetvdb": 81189, "imdb": "tt0903747" }
        }))
        .unwrap();

        let episodes: Vec<EpisodeRecord> = serde_json::from_value(json!([
            {
                "season": 1, "number": 1, "name": "Pilot",
                "summary": "<p>A teacher learns he is ill.</p>",
                "airdate": "2008-01-20", "airtime": "22:00",
                "airstamp": "2008-01-21T03:00:00+00:00", "runtime": 60,
                "image": { "medium": "m.jpg", "original": "o.jpg" }
            },
            {
                "season": 5, "number": null, "name": "El Camino",
                "airdate": "2019-10-11", "airstamp": "2019-10-12T02:00:00+00:00"
            },
            {
                "season": 1, "number": 2, "name": "Cat's in the Bag...",
                "airdate": "2008-01-27", "airtime": "22:00", "airstamp": "not a time"
            },
            {
                "season": 1, "number": 3, "name": "...And the Bag's in the River",
                "airdate": "2008-02-10", "airtime": "", "airstamp": "2008-02-10T12:00:00+00:00"
            }
        ]))
        .unwrap();

        (show, episodes)
    }

    #[test]
    fn an_episode_carries_the_instant_it_aired() {
        let (show, episodes) = fixture();
        let item = to_item(&show, &episodes);

        let pilot = &item.episodes[0];
        assert_eq!(pilot.air_date.as_deref(), Some("2008-01-20"));
        assert_eq!(pilot.air_date_utc.as_deref(), Some("2008-01-21T03:00:00Z"));
        assert_eq!(
            pilot.overview.as_deref(),
            Some("A teacher learns he is ill.")
        );
        assert_eq!(pilot.image.as_deref(), Some("o.jpg"));
    }

    #[test]
    fn a_special_without_a_number_is_left_out() {
        let (show, episodes) = fixture();
        let item = to_item(&show, &episodes);

        assert!(item.episodes.iter().all(|e| e.title != "El Camino"));
        assert_eq!(item.episodes.len(), 3);
    }

    #[test]
    fn an_airstamp_that_is_not_a_time_is_dropped() {
        let (show, episodes) = fixture();
        let item = to_item(&show, &episodes);

        let second = item
            .episodes
            .iter()
            .find(|e| e.episode_number == 2)
            .unwrap();
        assert_eq!(second.air_date.as_deref(), Some("2008-01-27"));
        assert_eq!(second.air_date_utc, None);
    }

    #[test]
    fn a_time_tvmaze_does_not_know_is_not_passed_off_as_one() {
        // Every Stranger Things episode reads airtime "" and airstamp noon UTC:
        // a placeholder that would otherwise replace the real release time.
        let (show, episodes) = fixture();
        let item = to_item(&show, &episodes);

        let third = item
            .episodes
            .iter()
            .find(|e| e.episode_number == 3)
            .unwrap();
        assert_eq!(third.air_date.as_deref(), Some("2008-02-10"));
        assert_eq!(third.air_date_utc, None);
    }

    #[test]
    fn the_show_brings_its_identifiers() {
        let (show, episodes) = fixture();
        let item = to_item(&show, &episodes);

        assert_eq!(item.external_ids.tvmaze, Some(169));
        assert_eq!(item.external_ids.tvdb, Some(81189));
        assert_eq!(item.external_ids.imdb.as_deref(), Some("tt0903747"));
        assert_eq!(
            item.overview.as_deref(),
            Some("Breaking Bad follows a chemistry teacher.")
        );
    }

    #[test]
    fn an_offset_airstamp_is_converted_to_utc() {
        assert_eq!(
            instant("2024-03-10T21:30:00-04:00").as_deref(),
            Some("2024-03-11T01:30:00Z")
        );
        assert_eq!(instant("garbage"), None);
    }
}

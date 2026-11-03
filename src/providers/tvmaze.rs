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
//!
//! Its index of every series it lists is read here too, for the copy each
//! instance keeps of it (`tvmaze.index`, see `jobs::tvmaze_index`): its pages
//! once, then the series it says changed. Through the same pacer as the
//! series fetched one by one, so that the two together keep to TVmaze's limit.

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

/// How long a reading of the index waits for one answer. A page is a third of
/// a megabyte, and nobody waits on the reading.
const INDEX_ANSWER: Duration = Duration::from_secs(60);

/// How many times one request of a reading of the index is made before the
/// reading gives up: TVmaze asking to be left alone, failing, or not
/// answering in time.
const INDEX_TRIES: u32 = 6;

/// The longest pause between two of those tries.
const INDEX_PAUSE: Duration = Duration::from_secs(60);

/// The most one answer of the index may weigh: a page is a third of a
/// megabyte, the list of the series changed in a month two thirds of one.
const MAX_INDEX_ANSWER: u64 = 16 * 1024 * 1024;

/// How far back TVmaze's list of the series it changed goes: the last day,
/// week or month.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Since {
    Day,
    Week,
    Month,
}

impl Since {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }
}

/// A series as TVmaze's index lists it, as far as this server keeps it: what
/// names it and places it, its ids elsewhere, and how popular TVmaze says it
/// is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexShow {
    /// TVmaze's id.
    pub id: i64,
    pub name: String,
    /// TVmaze's `type`: `Scripted`, `Animation`, `Reality`…
    pub kind: Option<String>,
    /// Named in English, as TVmaze writes it: `English`, `Japanese`.
    pub language: Option<String>,
    /// `Running`, `Ended`, `To Be Determined`, `In Development`.
    pub status: Option<String>,
    /// When it first aired, and when it last did: `2008-01-20`.
    pub premiered: Option<String>,
    pub ended: Option<String>,
    /// Its network, else the web channel it streams on.
    pub network: Option<String>,
    /// That one's country, ISO 3166-1 alpha-2.
    pub country: Option<String>,
    /// Whether `network` is a web channel.
    pub web: bool,
    pub tvdb_id: Option<i64>,
    /// `tt0903747`.
    pub imdb_id: Option<String>,
    pub tvrage_id: Option<i64>,
    /// How popular TVmaze says it is, from 0 to 100.
    pub weight: Option<i32>,
    /// When TVmaze last changed it, in seconds since 1970.
    pub updated: Option<i64>,
}

/// One page of TVmaze's index, as read.
#[derive(Debug, Default)]
pub struct IndexPage {
    /// The series read.
    pub shows: Vec<IndexShow>,
    /// How many of its entries could not be read.
    pub unread: usize,
    /// The ids of those, where their id could be: series TVmaze still lists,
    /// which a reading of the whole index does not forget.
    pub unread_ids: Vec<i64>,
}

/// One series of the index, read alone.
#[derive(Debug, PartialEq)]
pub enum Indexed {
    Listed(Box<IndexShow>),
    /// TVmaze gave something for it that does not read.
    Unreadable,
    /// TVmaze no longer lists it: 404.
    Gone,
}

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

    // ─── the index ───────────────────────────────────────────────────────────

    /// One page of TVmaze's index — the series it still lists of those it
    /// numbered from 250 × `page` to 250 × `page` + 249 — with what of it
    /// could not be read; `None` past the last page, which TVmaze answers
    /// 404.
    pub async fn index_page(&self, page: u32) -> Result<Option<IndexPage>> {
        let url = format!("{}/shows", self.base);
        let page = page.to_string();
        let Some(body) = self.fetch_index(&url, &[("page", page.as_str())]).await? else {
            return Ok(None);
        };
        let entries: Vec<Value> = serde_json::from_slice(&body)
            .with_context(|| format!("TVmaze's index page {page} is not a list"))?;

        // An entry that does not read is passed over rather than costing the
        // page: the other series on it are no less TVmaze's. Its id, where
        // that reads, says TVmaze still lists it.
        let mut read = IndexPage {
            shows: Vec::with_capacity(entries.len()),
            ..IndexPage::default()
        };
        for entry in &entries {
            match Listed::deserialize(entry) {
                Ok(show) => read.shows.push(show.indexed()),
                Err(_) => {
                    read.unread += 1;
                    read.unread_ids
                        .extend(entry.get("id").and_then(Value::as_i64));
                }
            }
        }
        Ok(Some(read))
    }

    /// The series TVmaze changed within `since`, each with when it last
    /// changed, in seconds since 1970.
    pub async fn changed(&self, since: Since) -> Result<Vec<(i64, i64)>> {
        let url = format!("{}/updates/shows", self.base);
        let body = self
            .fetch_index(&url, &[("since", since.as_str())])
            .await?
            .ok_or_else(|| anyhow::anyhow!("TVmaze has no list of the series it changed"))?;
        let listed: std::collections::HashMap<String, Value> = serde_json::from_slice(&body)
            .context("TVmaze's list of the series it changed is not one")?;

        let mut changed: Vec<(i64, i64)> = listed
            .into_iter()
            .filter_map(|(id, at)| Some((id.parse().ok()?, at.as_i64()?)))
            .collect();
        changed.sort_unstable();
        Ok(changed)
    }

    /// One series of the index, as TVmaze has it now: gone once TVmaze no
    /// longer lists it, and unreadable — passed over, not the reading's end
    /// — when what it gives for it does not read.
    pub async fn indexed(&self, id: i64) -> Result<Indexed> {
        let url = format!("{}/shows/{id}", self.base);
        let Some(body) = self.fetch_index(&url, &[]).await? else {
            return Ok(Indexed::Gone);
        };
        Ok(match serde_json::from_slice::<Listed>(&body) {
            Ok(show) => Indexed::Listed(Box::new(show.indexed())),
            Err(e) => {
                tracing::debug!(id, error = %e, "TVmaze gave a series this server could not read");
                Indexed::Unreadable
            }
        })
    }

    /// One request of a reading of the index, its body whole, or `None` for a
    /// 404. Nobody waits on the reading: its turn among every request to
    /// TVmaze — the series fetched one by one included — is waited for however
    /// long the queue, and a request TVmaze asked to be left alone over, failed
    /// or never answered is made again after a pause, a few times: for as long
    /// as its `Retry-After` asks, else longer each time.
    async fn fetch_index(&self, url: &str, query: &[(&str, &str)]) -> Result<Option<Vec<u8>>> {
        let mut tries = 0;
        loop {
            tries += 1;
            let pause = (self.again_after * 2u32.saturating_pow(tries - 1)).min(INDEX_PAUSE);
            if !self.pacer.turn(1, Duration::from_secs(3600)).await {
                anyhow::bail!("TVmaze's queue is an hour long; the reading of its index stops");
            }
            let sent = self
                .gate
                .send(|| self.http.get(url).query(query).timeout(INDEX_ANSWER))
                .await;
            let (response, permit) = match sent {
                Ok(answered) => answered,
                Err(e) if tries < INDEX_TRIES => {
                    let wait = crate::providers::closed_for(&e).unwrap_or(pause);
                    tracing::debug!(
                        url,
                        error = format_args!("{e:#}"),
                        "TVmaze's index was not read; asking again"
                    );
                    tokio::time::sleep(wait).await;
                    continue;
                }
                Err(e) => anyhow::bail!("TVmaze request failed: {url}: {e:#}"),
            };

            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            // A 429 has closed its door for as long as it asked: waited out.
            if (status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
                && tries < INDEX_TRIES
            {
                drop(permit);
                let wait = self.gate.quiet_for().unwrap_or(pause);
                tracing::debug!(url, %status, "TVmaze did not give its index; asking again");
                tokio::time::sleep(wait).await;
                continue;
            }
            if !status.is_success() {
                let reason = crate::providers::error_text(response).await;
                anyhow::bail!("TVmaze returned {status} for {url}: {reason}");
            }
            let body = crate::providers::read_body(response, MAX_INDEX_ANSWER)
                .await
                .with_context(|| format!("TVmaze's answer could not be read: {url}"));
            drop(permit);
            return body.map(Some);
        }
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

/// A series as the index reads it: the same document as [`Show`], read for
/// other fields, and apart from it so that the series fetched one by one are
/// read as they always were.
#[derive(Debug, Deserialize)]
struct Listed {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    language: Option<String>,
    status: Option<String>,
    premiered: Option<String>,
    ended: Option<String>,
    network: Option<Channel>,
    #[serde(rename = "webChannel")]
    web_channel: Option<Channel>,
    #[serde(default)]
    externals: ListedExternals,
    /// A whole number from 0 to 100; read as any number, and rounded.
    weight: Option<f64>,
    updated: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct ListedExternals {
    thetvdb: Option<i64>,
    imdb: Option<String>,
    tvrage: Option<i64>,
}

/// A network, or a web channel.
#[derive(Debug, Deserialize)]
struct Channel {
    name: Option<String>,
    country: Option<Country>,
}

#[derive(Debug, Deserialize)]
struct Country {
    code: Option<String>,
}

/// A text TVmaze gives, unless it is blank.
fn given(text: Option<&String>) -> Option<String> {
    text.map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// A channel by its name, when it has one.
fn named(channel: Option<&Channel>) -> Option<(String, &Channel)> {
    channel.and_then(|c| given(c.name.as_ref()).map(|name| (name, c)))
}

impl Listed {
    /// The series as the index keeps it: its network, else the web channel it
    /// streams on, each by its name.
    fn indexed(&self) -> IndexShow {
        let placed = named(self.network.as_ref())
            .map(|(name, channel)| (name, channel, false))
            .or_else(|| {
                named(self.web_channel.as_ref()).map(|(name, channel)| (name, channel, true))
            });

        let mut show = IndexShow {
            id: self.id,
            name: self.name.trim().to_string(),
            kind: given(self.kind.as_ref()),
            language: given(self.language.as_ref()),
            status: given(self.status.as_ref()),
            premiered: given(self.premiered.as_ref()),
            ended: given(self.ended.as_ref()),
            tvdb_id: self.externals.thetvdb.filter(|id| *id > 0),
            imdb_id: given(self.externals.imdb.as_ref()),
            tvrage_id: self.externals.tvrage.filter(|id| *id > 0),
            weight: self
                .weight
                .filter(|w| w.is_finite())
                .map(|w| w.round().clamp(0.0, 100.0) as i32),
            updated: self.updated,
            ..IndexShow::default()
        };
        if let Some((name, channel, web)) = placed {
            show.network = Some(name);
            show.country = channel_country(channel);
            show.web = web;
        }
        show
    }
}

/// A channel's country, as two upper-case letters.
fn channel_country(channel: &Channel) -> Option<String> {
    channel
        .country
        .as_ref()
        .and_then(|c| given(c.code.as_ref()))
        .filter(|code| code.len() == 2 && code.chars().all(|c| c.is_ascii_alphabetic()))
        .map(|code| code.to_ascii_uppercase())
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

    #[test]
    fn the_index_keeps_a_series_network_else_its_web_channel_and_nothing_blank() {
        let listed: Listed = serde_json::from_value(json!({
            "id": 82,
            "name": " Game of Thrones ",
            "type": "Scripted",
            "language": "",
            "status": "Ended",
            "premiered": "2011-04-17",
            "ended": "2019-05-19",
            "network": { "id": 8, "name": "HBO", "country": { "code": "us" } },
            "webChannel": { "id": 329, "name": "Max", "country": null },
            "externals": { "tvrage": 24493, "thetvdb": 121361, "imdb": "tt0944947" },
            "weight": 99.6,
            "updated": 1_704_794_122,
            "summary": "<p>Not kept.</p>"
        }))
        .unwrap();
        let show = listed.indexed();
        assert_eq!(show.name, "Game of Thrones");
        assert_eq!(show.language, None, "blank");
        assert_eq!(
            (show.network.as_deref(), show.country.as_deref(), show.web),
            (Some("HBO"), Some("US"), false)
        );
        assert_eq!(
            (show.tvdb_id, show.tvrage_id),
            (Some(121_361), Some(24_493))
        );
        assert_eq!(show.weight, Some(100));
        assert_eq!(show.updated, Some(1_704_794_122));

        // No network: the web channel, of no country.
        let streamed: Listed = serde_json::from_value(json!({
            "id": 83, "name": "Streamed", "network": null,
            "webChannel": { "name": "Lanternlight+", "country": null },
            "externals": { "thetvdb": 0 }, "weight": null
        }))
        .unwrap();
        let show = streamed.indexed();
        assert_eq!(
            (show.network.as_deref(), show.country, show.web),
            (Some("Lanternlight+"), None, true)
        );
        assert_eq!((show.tvdb_id, show.weight), (None, None));
    }
}

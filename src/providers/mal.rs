//! MyAnimeList: the largest anime audience's score, and the titles it files.
//!
//! Three ways in. MyAnimeList's own API, when an operator has registered a
//! client id for it (`AMS_MAL_CLIENT_ID`) — authoritative and current. Jikan
//! otherwise: an unofficial mirror that needs no key, but serves what it last
//! managed to scrape, which can be months old, and answers 504 when
//! MyAnimeList turns it away. Tenrai behind either, while `mal.fallback` is
//! on: another mirror, written to Jikan's schema — Jikan has said it will
//! close. Each way in describes the work the same.
//!
//! The next way in is asked when one does not answer — no connection, an
//! error of its own, a rate limit that outlasts the wait — and that one is
//! passed over for a few minutes. A 404 is an answer, that there is no such
//! entry, and asks no other.
//!
//! Like AniList this is never numbering — one entry per season or cour — and
//! the entry asked for is the one the anime identifier list names.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::{
    config,
    db::{new_id, now},
    domain::{CoverType, ExternalIds, Image, MediaItem, MediaKind, Rating},
    providers::{Gate, PATIENCE, Pacer, alternative_title},
};

/// What the official API is asked to include; it returns only `id`, `title`
/// and `main_picture` otherwise.
const FIELDS: &str = "id,title,main_picture,alternative_titles,start_date,synopsis,mean,\
                      num_scoring_users,genres,rating,studios";

/// How long a way in that did not answer is passed over for.
const PASSED_OVER_FOR: Duration = Duration::from_secs(5 * 60);

/// One of the ways into MyAnimeList.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Base {
    /// MyAnimeList's own API, with the operator's client id.
    Official,
    Jikan,
    Tenrai,
}

impl Base {
    fn label(self) -> &'static str {
        match self {
            Self::Official => "MyAnimeList's API",
            Self::Jikan => "Jikan",
            Self::Tenrai => "Tenrai",
        }
    }
}

/// How MyAnimeList is reached, as this process last found it.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Reach {
    /// The ways in, in the order they are tried.
    pub bases: Vec<BaseReach>,
    /// The first not passed over, which is asked next; none when every one
    /// failed to answer lately.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serving: Option<Base>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BaseReach {
    pub base: Base,
    /// When it last failed to answer, while it is passed over for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<String>,
}

/// One way in: where it is, how often it may be asked, and whether it
/// answered lately.
struct Way {
    base: Base,
    url: String,
    pacer: Pacer,
    /// One door each: Jikan asking to be left alone is no reason to leave
    /// Tenrai alone too.
    gate: Gate,
    /// When it last failed to answer: the instant counted from, and the time
    /// a reader is told.
    failed: parking_lot::Mutex<Option<(Instant, String)>>,
}

impl Way {
    fn new(base: Base, url: &str, gap: Duration) -> Self {
        Self {
            base,
            url: url.trim_end_matches('/').to_string(),
            pacer: Pacer::new(gap),
            // Spaced already; this is for its `Retry-After`.
            gate: Gate::new("mal", base.label(), 4),
            failed: parking_lot::Mutex::new(None),
        }
    }

    /// When it failed to answer, if that was less than [`PASSED_OVER_FOR`]
    /// ago.
    fn failed_lately(&self) -> Option<String> {
        self.failed
            .lock()
            .as_ref()
            .filter(|(at, _)| at.elapsed() < PASSED_OVER_FOR)
            .map(|(_, when)| when.clone())
    }
}

/// What one way in made of a request for an entry.
enum Asked {
    /// The entry, as it wrote it.
    Entry(Value),
    /// Its word that there is no such entry.
    Missing,
    /// Nothing to go on this once — its queue was full, or what it said does
    /// not read — and no reason to pass it over: the next is asked.
    Skipped(String),
    /// No answer: the next is asked, and this one passed over for a while.
    Unanswered(String),
}

pub struct MalClient {
    /// One that follows a redirect on the same host only: the client id goes
    /// in a header of MyAnimeList's own, which reqwest does not know to drop
    /// on the way to another.
    http: reqwest::Client,
    client_id: Option<String>,
    official: Way,
    jikan: Way,
    tenrai: Way,
}

impl MalClient {
    pub fn new(http: reqwest::Client, cfg: &config::Mal) -> Self {
        Self {
            http,
            client_id: cfg.client_id.clone().filter(|id| !id.trim().is_empty()),
            // No published limit; this is being polite.
            official: Way::new(Base::Official, &cfg.upstream, Duration::from_millis(500)),
            // Three a second and sixty a minute. The minute binds.
            jikan: Way::new(
                Base::Jikan,
                &cfg.jikan_upstream,
                Duration::from_millis(1_100),
            ),
            // Four a second and a hundred and twenty a minute: kept under the
            // minute's.
            tenrai: Way::new(
                Base::Tenrai,
                &cfg.tenrai_upstream,
                Duration::from_millis(600),
            ),
        }
    }

    /// Whether the official API is the way in tried first, for the interface.
    pub fn uses_official_api(&self) -> bool {
        self.client_id.is_some()
    }

    /// The ways in, in the order they are tried: the official API with a
    /// client id, Jikan without — and, while `fallback` is on, those behind.
    fn ways(&self, fallback: bool) -> Vec<&Way> {
        let mut ways = Vec::with_capacity(3);
        if self.client_id.is_some() {
            ways.push(&self.official);
        }
        if self.client_id.is_none() || fallback {
            ways.push(&self.jikan);
        }
        if fallback {
            ways.push(&self.tenrai);
        }
        ways
    }

    /// How MyAnimeList is reached now, with `fallback` as the settings have
    /// it.
    pub fn reach(&self, fallback: bool) -> Reach {
        let bases: Vec<BaseReach> = self
            .ways(fallback)
            .into_iter()
            .map(|way| BaseReach {
                base: way.base,
                failed_at: way.failed_lately(),
            })
            .collect();
        let serving = bases
            .iter()
            .find(|reach| reach.failed_at.is_none())
            .map(|reach| reach.base);

        Reach { bases, serving }
    }

    /// One MyAnimeList entry, as a contribution to the work of `kind`. Returns
    /// the raw answer alongside the parsed one.
    ///
    /// The ways in are asked in turn until one answers, those behind the
    /// first only while `fallback` is on. One passed over is not asked, so a
    /// way in that is down costs one fetch its wait every few minutes rather
    /// than every fetch.
    pub async fn anime(
        &self,
        id: i64,
        kind: MediaKind,
        fallback: bool,
    ) -> Result<Option<(Value, MediaItem)>> {
        let mut unanswered = Vec::new();

        for way in self.ways(fallback) {
            let label = way.base.label();
            if way.failed_lately().is_some() {
                unanswered.push(format!("{label} did not answer a moment ago"));
                continue;
            }

            let raw = match self.ask(way, id).await {
                Asked::Entry(raw) => raw,
                Asked::Missing => return Ok(None),
                Asked::Skipped(why) => {
                    unanswered.push(why);
                    continue;
                }
                Asked::Unanswered(why) => {
                    tracing::warn!(
                        mal_id = id,
                        error = %why,
                        "a way into MyAnimeList did not answer; it is passed over for a few minutes"
                    );
                    *way.failed.lock() = Some((Instant::now(), now()));
                    unanswered.push(why);
                    continue;
                }
            };

            match read(way.base, &raw) {
                Ok(anime) => return Ok(Some((raw, to_item(&anime, kind)))),
                Err(e) => unanswered.push(format!("{e:#}")),
            }
        }

        anyhow::bail!(
            "no way into MyAnimeList answered: {}",
            unanswered.join("; ")
        )
    }

    /// One way in, asked for one entry.
    async fn ask(&self, way: &Way, id: i64) -> Asked {
        let label = way.base.label();
        if !way.pacer.turn(1, PATIENCE).await {
            return Asked::Skipped(format!(
                "{label}'s queue is full; this fetch goes without it"
            ));
        }

        let request = || {
            let request = self
                .http
                .get(format!("{}/anime/{id}", way.url))
                .timeout(Duration::from_secs(20));
            match (way.base, &self.client_id) {
                (Base::Official, Some(client_id)) => request
                    .query(&[("fields", FIELDS)])
                    .header("X-MAL-CLIENT-ID", client_id),
                _ => request,
            }
        };

        let (response, _permit) = match way.gate.send(request).await {
            Ok(sent) => sent,
            Err(e) => return Asked::Unanswered(format!("{label} request failed: {e}")),
        };

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            // Its word that there is no such entry is written as JSON, as its
            // other errors are; a page of the server's own is the way in
            // itself gone, or not where it is looked for.
            let body =
                crate::providers::read_body(response, crate::providers::ERROR_BODY_BYTES as u64)
                    .await
                    .unwrap_or_default();
            return match serde_json::from_slice::<Value>(&body) {
                Ok(Value::Object(_)) => Asked::Missing,
                _ => Asked::Unanswered(format!("{label} answered 404 without a word of the entry")),
            };
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Asked::Unanswered(format!("{label}'s rate limit was reached"));
        }
        if status == reqwest::StatusCode::GATEWAY_TIMEOUT && way.base == Base::Jikan {
            // Jikan's 504 is about this one entry — not in its cache, and
            // MyAnimeList not answering for it just now — so Jikan is not
            // passed over for the others: the next way in is asked for this
            // entry alone.
            return Asked::Skipped(
                "Jikan could not reach MyAnimeList and had nothing cached for this entry"
                    .to_string(),
            );
        }
        if !status.is_success() {
            let reason = crate::providers::error_text(response).await;
            return Asked::Unanswered(format!("{label} returned {status}: {reason}"));
        }

        let body =
            match crate::providers::read_body(response, crate::providers::MAX_BODY_BYTES).await {
                Ok(body) => body,
                Err(e) => return Asked::Unanswered(format!("{label}'s answer was cut short: {e}")),
            };
        match serde_json::from_slice(&body) {
            Ok(raw) => Asked::Entry(raw),
            Err(_) => Asked::Skipped(format!(
                "{label} returned a body this server could not read"
            )),
        }
    }
}

/// An entry as one way in writes it: the official API its own way, Jikan and
/// Tenrai as Jikan does.
fn read(base: Base, raw: &Value) -> Result<Anime> {
    Ok(match base {
        Base::Official => Official::deserialize(raw)
            .context("MyAnimeList returned an entry this server could not read")?
            .into(),
        Base::Jikan | Base::Tenrai => Jikan::deserialize(raw.get("data").unwrap_or(&Value::Null))
            .with_context(|| {
                format!(
                    "{} returned an entry this server could not read",
                    base.label()
                )
            })?
            .into(),
    })
}

// ─── the wire ────────────────────────────────────────────────────────────────

/// An entry either way in describes, in the form this server reads.
#[derive(Debug, Default)]
struct Anime {
    id: i64,
    title: Option<String>,
    english: Option<String>,
    japanese: Option<String>,
    synonyms: Vec<String>,
    /// Titles in other languages, by the country TMDB would file them under.
    localised: Vec<(String, &'static str)>,
    synopsis: Option<String>,
    score: Option<f64>,
    scored_by: Option<i64>,
    /// MyAnimeList's own age rating, `rx` being the one for pornography.
    adult: bool,
    genres: Vec<String>,
    studio: Option<String>,
    picture: Option<String>,
    year: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct Official {
    id: i64,
    title: Option<String>,
    main_picture: Option<Picture>,
    alternative_titles: Option<OfficialTitles>,
    start_date: Option<String>,
    synopsis: Option<String>,
    mean: Option<f64>,
    num_scoring_users: Option<i64>,
    #[serde(default)]
    genres: Vec<Named>,
    rating: Option<String>,
    #[serde(default)]
    studios: Vec<Named>,
}

#[derive(Debug, Deserialize)]
struct Picture {
    large: Option<String>,
    medium: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OfficialTitles {
    #[serde(default)]
    synonyms: Vec<String>,
    en: Option<String>,
    ja: Option<String>,
}

/// A genre or a studio. Tenrai writes `null` for what it does not know.
#[derive(Debug, Deserialize)]
struct Named {
    name: Option<String>,
}

impl From<Official> for Anime {
    fn from(o: Official) -> Self {
        let titles = o.alternative_titles;

        Self {
            id: o.id,
            title: o.title,
            english: titles.as_ref().and_then(|t| t.en.clone()),
            japanese: titles.as_ref().and_then(|t| t.ja.clone()),
            synonyms: titles.map(|t| t.synonyms).unwrap_or_default(),
            localised: Vec::new(),
            synopsis: o.synopsis,
            score: o.mean,
            scored_by: o.num_scoring_users,
            adult: o.rating.as_deref() == Some("rx"),
            genres: o.genres.into_iter().filter_map(|g| g.name).collect(),
            studio: o.studios.into_iter().find_map(|s| s.name),
            picture: o.main_picture.and_then(|p| p.large.or(p.medium)),
            year: o
                .start_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Jikan {
    mal_id: i64,
    title: Option<String>,
    title_english: Option<String>,
    title_japanese: Option<String>,
    #[serde(default)]
    title_synonyms: Vec<String>,
    #[serde(default)]
    titles: Vec<JikanTitle>,
    synopsis: Option<String>,
    score: Option<f64>,
    scored_by: Option<i64>,
    rating: Option<String>,
    #[serde(default)]
    genres: Vec<Named>,
    #[serde(default)]
    studios: Vec<Named>,
    images: Option<JikanImages>,
    /// The year of the season it premiered in, which a film does not have.
    year: Option<i32>,
    aired: Option<JikanAired>,
}

#[derive(Debug, Deserialize)]
struct JikanAired {
    /// `2001-07-20T00:00:00+00:00`.
    from: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JikanTitle {
    #[serde(rename = "type")]
    kind: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JikanImages {
    jpg: Option<JikanImage>,
}

#[derive(Debug, Deserialize)]
struct JikanImage {
    large_image_url: Option<String>,
    image_url: Option<String>,
}

impl From<Jikan> for Anime {
    fn from(j: Jikan) -> Self {
        // The languages Jikan names, as TMDB's countries. Anything else it
        // lists is left out rather than filed under a guess.
        let country = |language: &str| match language {
            "French" => Some("fra"),
            "German" => Some("deu"),
            "Spanish" => Some("esp"),
            "Italian" => Some("ita"),
            "Portuguese" => Some("prt"),
            "Korean" => Some("kor"),
            "Chinese" => Some("chn"),
            _ => None,
        };

        Self {
            id: j.mal_id,
            title: j.title,
            english: j.title_english,
            japanese: j.title_japanese,
            synonyms: j.title_synonyms,
            localised: j
                .titles
                .into_iter()
                .filter_map(|t| Some((t.title?, country(t.kind.as_deref()?)?)))
                .collect(),
            synopsis: j.synopsis,
            score: j.score,
            scored_by: j.scored_by,
            adult: j.rating.as_deref().is_some_and(|r| r.starts_with("Rx")),
            genres: j.genres.into_iter().filter_map(|g| g.name).collect(),
            studio: j.studios.into_iter().find_map(|s| s.name),
            picture: j
                .images
                .and_then(|i| i.jpg)
                .and_then(|jpg| jpg.large_image_url.or(jpg.image_url)),
            year: j.year.or_else(|| {
                j.aired
                    .and_then(|a| a.from)
                    .and_then(|from| from.get(..4).and_then(|y| y.parse().ok()))
            }),
        }
    }
}

// ─── mapping ─────────────────────────────────────────────────────────────────

/// MyAnimeList's synopses end with a credit to whoever wrote them, which is
/// theirs to show and not a sentence of the synopsis.
fn synopsis(text: &str) -> Option<String> {
    let trimmed = text
        .trim()
        .trim_end_matches("[Written by MAL Rewrite]")
        .trim();

    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn to_item(anime: &Anime, kind: MediaKind) -> MediaItem {
    let mut item = MediaItem::empty(kind);

    let text = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };

    item.title = text(&anime.english)
        .or_else(|| text(&anime.title))
        .unwrap_or_default();
    item.original_title = text(&anime.japanese);
    item.overview = anime.synopsis.as_deref().and_then(synopsis);
    item.year = anime.year;
    item.studio = text(&anime.studio);
    item.genres = anime.genres.clone();
    item.is_adult = anime.adult;

    item.external_ids = ExternalIds {
        mal: vec![anime.id],
        ..Default::default()
    };

    if let Some(score) = anime.score.filter(|s| *s > 0.0) {
        item.ratings = vec![Rating {
            source: "mal".to_string(),
            value: Some(score),
            votes: anime.scored_by.filter(|n| *n > 0),
            rating_type: Some("user".to_string()),
        }];
    }

    // MyAnimeList's main title is the romaji one.
    item.alternative_titles = [
        (anime.title.as_deref(), "Romaji", Some("jpn")),
        (anime.japanese.as_deref(), "Native", Some("jpn")),
        (anime.english.as_deref(), "English", None),
    ]
    .into_iter()
    .chain(
        anime
            .synonyms
            .iter()
            .map(|s| (Some(s.as_str()), "Synonym", None)),
    )
    .chain(
        anime
            .localised
            .iter()
            .map(|(title, country)| (Some(title.as_str()), "Localised", Some(*country))),
    )
    .filter_map(|(title, title_type, language)| alternative_title(title?, title_type, language))
    .collect();

    if let Some(url) = anime.picture.clone().filter(|u| u.starts_with("https://")) {
        item.images.push(Image {
            id: new_id(),
            season_number: None,
            cover_type: CoverType::Poster,
            url,
            language: None,
            sort_order: 100,
            source: Some("mal".to_string()),
            is_manual: false,
        });
    }

    item.updated_at = now();
    item
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn from_jikan() -> Anime {
        serde_json::from_value::<Jikan>(json!({
            "mal_id": 16498,
            "title": "Shingeki no Kyojin",
            "title_english": "Attack on Titan",
            "title_japanese": "進撃の巨人",
            "title_synonyms": ["AoT", "SnK"],
            "titles": [
                { "type": "Default", "title": "Shingeki no Kyojin" },
                { "type": "French", "title": "L'Attaque des Titans" },
                { "type": "Klingon", "title": "nothing" }
            ],
            "synopsis": "Centuries ago, mankind was slaughtered to near extinction.\n\n[Written by MAL Rewrite]",
            "score": 8.57,
            "scored_by": 3089461,
            "rating": "R - 17+ (violence & profanity)",
            "genres": [{ "name": "Action" }],
            "studios": [{ "name": "Wit Studio" }],
            "images": { "jpg": { "large_image_url": "https://cdn.myanimelist.net/images/anime/10/47347l.jpg" } },
            "year": 2013
        }))
        .unwrap()
        .into()
    }

    #[test]
    fn jikan_gives_the_score_and_how_many_gave_it() {
        let item = to_item(&from_jikan(), MediaKind::Series);
        let rating = item.rating("mal").unwrap();

        assert_eq!(rating.value, Some(8.57));
        assert_eq!(rating.votes, Some(3_089_461));
        assert!(!item.is_adult);
    }

    #[test]
    fn the_credit_line_is_not_part_of_the_synopsis() {
        let item = to_item(&from_jikan(), MediaKind::Series);
        assert_eq!(
            item.overview.as_deref(),
            Some("Centuries ago, mankind was slaughtered to near extinction.")
        );
    }

    #[test]
    fn titles_in_named_languages_are_filed_under_their_country() {
        let item = to_item(&from_jikan(), MediaKind::Movie);

        let french = item
            .alternative_titles
            .iter()
            .find(|t| t.title == "L'Attaque des Titans")
            .unwrap();
        assert_eq!(french.language.as_deref(), Some("fra"));
        assert!(item.alternative_titles.iter().all(|t| t.title != "nothing"));
    }

    #[test]
    fn the_official_api_describes_the_same_work() {
        let anime: Anime = serde_json::from_value::<Official>(json!({
            "id": 16498,
            "title": "Shingeki no Kyojin",
            "main_picture": { "medium": "https://m.jpg", "large": "https://l.jpg" },
            "alternative_titles": { "synonyms": ["AoT"], "en": "Attack on Titan", "ja": "進撃の巨人" },
            "start_date": "2013-04-07",
            "synopsis": "Centuries ago.",
            "mean": 8.55,
            "num_scoring_users": 2800000,
            "genres": [{ "id": 1, "name": "Action" }],
            "rating": "r",
            "studios": [{ "id": 858, "name": "Wit Studio" }]
        }))
        .unwrap()
        .into();

        let item = to_item(&anime, MediaKind::Series);

        assert_eq!(item.rating("mal").and_then(|r| r.votes), Some(2_800_000));
        assert_eq!(item.year, Some(2013));
        assert_eq!(item.images[0].url, "https://l.jpg");
        assert_eq!(item.title, "Attack on Titan");
    }

    #[test]
    fn a_film_has_its_year_through_jikan_too() {
        // Jikan's `year` is the premiere season's, and a film has none.
        let film: Anime = serde_json::from_value::<Jikan>(json!({
            "mal_id": 199, "title": "Sen to Chihiro no Kamikakushi", "type": "Movie",
            "year": null, "aired": { "from": "2001-07-20T00:00:00+00:00" }
        }))
        .unwrap()
        .into();

        assert_eq!(to_item(&film, MediaKind::Movie).year, Some(2001));
    }

    #[test]
    fn rx_is_adult_either_way_in() {
        let mut jikan = from_jikan();
        jikan.adult = false;
        assert!(!to_item(&jikan, MediaKind::Movie).is_adult);

        let official: Anime = serde_json::from_value::<Official>(json!({
            "id": 1, "title": "x", "rating": "rx"
        }))
        .unwrap()
        .into();
        assert!(to_item(&official, MediaKind::Movie).is_adult);

        let jikan: Anime = serde_json::from_value::<Jikan>(json!({
            "mal_id": 1, "title": "x", "rating": "Rx - Hentai"
        }))
        .unwrap()
        .into();
        assert!(to_item(&jikan, MediaKind::Movie).is_adult);
    }

    // ─── the ways in ─────────────────────────────────────────────────────────

    type Lines = std::sync::Arc<parking_lot::Mutex<Vec<String>>>;

    /// A server answering every request with `answer`, and the request line
    /// of each.
    async fn answering(answer: String) -> (String, Lines) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let asked = Lines::default();
        let seen = asked.clone();
        tokio::spawn(async move {
            while let Ok((mut connection, _)) = listener.accept().await {
                let mut request = vec![0u8; 8192];
                let read = connection.read(&mut request).await.unwrap_or(0);
                let line = String::from_utf8_lossy(&request[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                seen.lock().push(line);
                let _ = connection.write_all(answer.as_bytes()).await;
                let _ = connection.shutdown().await;
            }
        });
        (at, asked)
    }

    fn reply(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// Attack on Titan as Tenrai writes it: Jikan's schema, a nought for the
    /// score it does not know, a `null` for a name.
    fn tenrais_entry() -> String {
        reply(
            "200 OK",
            "Content-Type: application/json\r\n",
            &json!({ "data": {
                "mal_id": 16498,
                "title": "Shingeki no Kyojin",
                "title_english": "Attack on Titan",
                "titles": [{ "type": null, "title": "AoT" }, { "type": "French", "title": "L'Attaque des Titans" }],
                "score": 0,
                "scored_by": 0,
                "genres": [{ "name": null }, { "name": "Action" }],
                "studios": [],
                "images": {},
                "year": 2013
            }})
            .to_string(),
        )
    }

    fn json_error(status: &str) -> String {
        reply(
            status,
            "Content-Type: application/json\r\n",
            r#"{"status":404,"type":"BadResponseException","message":"Resource does not exist"}"#,
        )
    }

    fn client(official: &str, jikan: &str, tenrai: &str, client_id: Option<&str>) -> MalClient {
        MalClient::new(
            reqwest::Client::new(),
            &config::Mal {
                upstream: official.into(),
                jikan_upstream: jikan.into(),
                tenrai_upstream: tenrai.into(),
                client_id: client_id.map(String::from),
                enabled: true,
            },
        )
    }

    fn reached(mal: &MalClient, fallback: bool) -> (Vec<(Base, bool)>, Option<Base>) {
        let reach = mal.reach(fallback);
        (
            reach
                .bases
                .iter()
                .map(|b| (b.base, b.failed_at.is_some()))
                .collect(),
            reach.serving,
        )
    }

    #[tokio::test]
    async fn when_jikan_does_not_answer_tenrai_does_and_jikan_is_passed_over() {
        // A rate limit that outlasts the one wait the door allows.
        let (jikan, to_jikan) =
            answering(reply("429 Too Many Requests", "Retry-After: 1\r\n", "")).await;
        let (tenrai, to_tenrai) = answering(tenrais_entry()).await;
        let mal = client(&jikan, &jikan, &tenrai, None);

        let (raw, item) = mal
            .anime(16498, MediaKind::Series, true)
            .await
            .expect("answered")
            .expect("found");
        assert_eq!(raw["data"]["mal_id"], 16498);
        assert_eq!(item.title, "Attack on Titan");
        assert_eq!(item.genres, ["Action"]);
        // Tenrai's nought is no score.
        assert!(item.rating("mal").is_none());
        assert!(
            item.alternative_titles
                .iter()
                .any(|t| t.language.as_deref() == Some("fra"))
        );
        assert_eq!(to_jikan.lock().len(), 2, "asked again once, after the wait");
        assert!(to_tenrai.lock()[0].starts_with("GET /anime/16498 "));

        assert_eq!(
            reached(&mal, true),
            (
                vec![(Base::Jikan, true), (Base::Tenrai, false)],
                Some(Base::Tenrai)
            )
        );

        // The next fetch goes straight to Tenrai.
        mal.anime(16498, MediaKind::Series, true)
            .await
            .expect("answered")
            .expect("found");
        assert_eq!(to_jikan.lock().len(), 2, "Jikan is passed over");
        assert_eq!(to_tenrai.lock().len(), 2);
    }

    #[tokio::test]
    async fn a_404_is_an_answer_and_asks_no_other() {
        let (jikan, _) = answering(json_error("404 Not Found")).await;
        let (tenrai, to_tenrai) = answering(tenrais_entry()).await;
        let mal = client(&jikan, &jikan, &tenrai, None);

        assert!(
            mal.anime(1, MediaKind::Series, true)
                .await
                .expect("answered")
                .is_none()
        );
        assert!(to_tenrai.lock().is_empty());
        assert_eq!(reached(&mal, true).1, Some(Base::Jikan));

        // A 404 that is a page of the server's own is no word of the entry:
        // the way in is not where it is looked for.
        let (gone, _) = answering(reply(
            "404 Not Found",
            "Content-Type: text/html\r\n",
            "<html>Not Found</html>",
        ))
        .await;
        let mal = client(&gone, &gone, &tenrai, None);
        assert!(
            mal.anime(16498, MediaKind::Series, true)
                .await
                .expect("answered")
                .is_some()
        );
        assert_eq!(to_tenrai.lock().len(), 1);
    }

    #[tokio::test]
    async fn with_the_fallback_off_jikan_alone_is_asked_and_its_failure_kept() {
        let (jikan, to_jikan) = answering(reply("503 Service Unavailable", "", "")).await;
        let (tenrai, to_tenrai) = answering(tenrais_entry()).await;
        let mal = client(&jikan, &jikan, &tenrai, None);

        let first = mal
            .anime(16498, MediaKind::Series, false)
            .await
            .unwrap_err();
        assert!(format!("{first:#}").contains("503"), "{first:#}");
        assert_eq!(reached(&mal, false), (vec![(Base::Jikan, true)], None));

        // Not asked again for a few minutes: the fetches meanwhile fail with
        // the reason, at once.
        let second = mal
            .anime(16498, MediaKind::Series, false)
            .await
            .unwrap_err();
        assert!(
            format!("{second:#}").contains("did not answer a moment ago"),
            "{second:#}"
        );
        assert_eq!(to_jikan.lock().len(), 1);
        assert!(to_tenrai.lock().is_empty());
    }

    #[tokio::test]
    async fn with_a_client_id_the_official_api_is_asked_first_then_jikan() {
        let (official, to_official) =
            answering(reply("500 Internal Server Error", "", "down")).await;
        let (jikan, to_jikan) = answering(tenrais_entry()).await;
        let (tenrai, to_tenrai) = answering(tenrais_entry()).await;
        let mal = client(&official, &jikan, &tenrai, Some("an id"));

        assert_eq!(
            reached(&mal, true).1,
            Some(Base::Official),
            "first, while it answers"
        );
        let (_, item) = mal
            .anime(16498, MediaKind::Series, true)
            .await
            .expect("answered")
            .expect("found");
        assert_eq!(item.title, "Attack on Titan");
        assert!(to_official.lock()[0].contains("fields="));
        assert_eq!(to_jikan.lock().len(), 1);
        assert!(to_tenrai.lock().is_empty());
        assert_eq!(
            reached(&mal, true),
            (
                vec![
                    (Base::Official, true),
                    (Base::Jikan, false),
                    (Base::Tenrai, false)
                ],
                Some(Base::Jikan)
            )
        );

        // Without the fallback, the official API alone, as before.
        assert_eq!(reached(&mal, false).0, [(Base::Official, true)]);
    }

    #[tokio::test]
    async fn when_no_way_in_answers_it_is_said() {
        let (jikan, _) = answering(reply("502 Bad Gateway", "", "")).await;
        // Nothing listens there.
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let mal = client(&jikan, &jikan, &closed, None);

        let failed = mal.anime(16498, MediaKind::Series, true).await.unwrap_err();
        let said = format!("{failed:#}");
        assert!(said.contains("Jikan returned 502"), "{said}");
        assert!(said.contains("Tenrai request failed"), "{said}");
        assert_eq!(
            reached(&mal, true),
            (vec![(Base::Jikan, true), (Base::Tenrai, true)], None)
        );
    }
}

//! MyAnimeList: the largest anime audience's score, and the titles it files.
//!
//! Two ways in. MyAnimeList's own API, when an operator has registered a
//! client id for it (`AMS_MAL_CLIENT_ID`) — authoritative and current. Jikan
//! otherwise: an unofficial mirror that needs no key, but serves what it last
//! managed to scrape, which can be months old, and answers 504 when
//! MyAnimeList turns it away. Either way the work is described the same.
//!
//! Like AniList this is never numbering — one entry per season or cour — and
//! the entry asked for is the one the anime identifier list names.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    config,
    db::{new_id, now},
    domain::{CoverType, ExternalIds, Image, MediaItem, MediaKind, Rating},
    providers::{PATIENCE, Pacer, alternative_title},
};

/// What the official API is asked to include; it returns only `id`, `title`
/// and `main_picture` otherwise.
const FIELDS: &str = "id,title,main_picture,alternative_titles,start_date,synopsis,mean,\
                      num_scoring_users,genres,rating,studios";

pub struct MalClient {
    http: reqwest::Client,
    official: String,
    jikan: String,
    client_id: Option<String>,
    pacer: Pacer,
}

impl MalClient {
    pub fn new(http: reqwest::Client, cfg: &config::Mal) -> Self {
        let client_id = cfg.client_id.clone().filter(|id| !id.trim().is_empty());

        Self {
            http,
            official: cfg.upstream.trim_end_matches('/').to_string(),
            jikan: cfg.jikan_upstream.trim_end_matches('/').to_string(),
            pacer: Pacer::new(std::time::Duration::from_millis(if client_id.is_some() {
                // No published limit; this is being polite.
                500
            } else {
                // Jikan: three a second and sixty a minute. The minute binds.
                1_100
            })),
            client_id,
        }
    }

    /// Which of the two ways in this client uses, for the interface.
    pub fn uses_official_api(&self) -> bool {
        self.client_id.is_some()
    }

    /// One MyAnimeList entry, as a contribution to the work of `kind`. Returns
    /// the raw answer alongside the parsed one.
    pub async fn anime(&self, id: i64, kind: MediaKind) -> Result<Option<(Value, MediaItem)>> {
        if !self.pacer.turn(1, PATIENCE).await {
            anyhow::bail!("MyAnimeList's queue is full; this fetch goes without it");
        }

        let request = match &self.client_id {
            Some(client_id) => self
                .http
                .get(format!("{}/anime/{id}", self.official))
                .query(&[("fields", FIELDS)])
                .header("X-MAL-CLIENT-ID", client_id),
            None => self.http.get(format!("{}/anime/{id}", self.jikan)),
        };

        let started = std::time::Instant::now();
        let response = request
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await;
        crate::metrics::upstream("mal", started, response.as_ref().ok().map(|r| r.status()));
        let response = response.context("MyAnimeList request failed")?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            anyhow::bail!("MyAnimeList's rate limit was reached");
        }
        if status == reqwest::StatusCode::GATEWAY_TIMEOUT && self.client_id.is_none() {
            anyhow::bail!("Jikan could not reach MyAnimeList and had nothing cached");
        }
        if !status.is_success() {
            anyhow::bail!("MyAnimeList returned {status}");
        }

        let raw = crate::providers::read_json(response)
            .await
            .context("MyAnimeList returned a body this server could not read")?;

        let anime = if self.client_id.is_some() {
            Official::deserialize(&raw)
                .context("MyAnimeList returned an entry this server could not read")?
                .into()
        } else {
            Jikan::deserialize(raw.get("data").unwrap_or(&Value::Null))
                .context("Jikan returned an entry this server could not read")?
                .into()
        };

        Ok(Some((raw, to_item(&anime, kind))))
    }
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

#[derive(Debug, Deserialize)]
struct Named {
    name: String,
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
            genres: o.genres.into_iter().map(|g| g.name).collect(),
            studio: o.studios.into_iter().next().map(|s| s.name),
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
    kind: String,
    title: String,
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
                .filter_map(|t| country(&t.kind).map(|c| (t.title, c)))
                .collect(),
            synopsis: j.synopsis,
            score: j.score,
            scored_by: j.scored_by,
            adult: j.rating.as_deref().is_some_and(|r| r.starts_with("Rx")),
            genres: j.genres.into_iter().map(|g| g.name).collect(),
            studio: j.studios.into_iter().next().map(|s| s.name),
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
}

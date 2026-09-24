//! AniList: what the anime community scores a work, and every name it goes by.
//!
//! Three things nothing else here supplies as well. A score from an audience
//! that watches anime, with the size of that audience behind it. The romaji,
//! native and English titles, and the synonyms people search by — which Radarr
//! matches release names against for an anime film. And an adult flag that is
//! set on what TMDB often leaves unmarked.
//!
//! Never numbering. AniList has one entry per season or cour, so it can say
//! nothing about which episode is which; the entry asked for is the one the
//! anime identifier list says a series or a film *is* — see
//! [`crate::jobs::datasets`].
//!
//! No key. Ninety calls a minute normally, thirty at the time of writing while
//! AniList runs degraded; one call per work here, spaced to the lower figure.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    config,
    db::{new_id, now},
    domain::{CoverType, ExternalIds, Image, MediaItem, MediaKind, Rating},
    providers::{PATIENCE, Pacer, alternative_title, plain_text},
};

const QUERY: &str = "query ($id: Int) {
  Media(id: $id, type: ANIME) {
    id idMal
    title { romaji english native }
    synonyms
    description(asHtml: false)
    averageScore
    stats { scoreDistribution { score amount } }
    genres
    tags { name rank isMediaSpoiler isGeneralSpoiler }
    studios(isMain: true) { nodes { name } }
    isAdult
    coverImage { extraLarge }
    startDate { year }
  }
}";

/// Tags ranked below this are ones a minority of voters agreed with.
const TAG_RANK: i32 = 60;

pub struct AnilistClient {
    http: reqwest::Client,
    endpoint: String,
    pacer: Pacer,
}

impl AnilistClient {
    pub fn new(http: reqwest::Client, cfg: &config::Anilist) -> Self {
        Self {
            http,
            endpoint: cfg.upstream.clone(),
            // Thirty a minute, AniList's current ceiling.
            pacer: Pacer::new(std::time::Duration::from_millis(2_100)),
        }
    }

    /// One AniList entry, as a contribution to the work of `kind`. Returns the
    /// raw answer alongside the parsed one.
    pub async fn media(&self, id: i64, kind: MediaKind) -> Result<Option<(Value, MediaItem)>> {
        if !self.pacer.turn(1, PATIENCE).await {
            anyhow::bail!("AniList's queue is full; this fetch goes without it");
        }

        let response = self
            .http
            .post(&self.endpoint)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&json!({ "query": QUERY, "variables": { "id": id } }))
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .context("AniList request failed")?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            anyhow::bail!("AniList's rate limit was reached");
        }
        if !status.is_success() {
            anyhow::bail!("AniList returned {status}");
        }

        let raw = crate::providers::read_json(response)
            .await
            .context("AniList returned a body this server could not read")?;

        let Some(media) = raw.pointer("/data/Media").filter(|m| !m.is_null()) else {
            // GraphQL reports its errors in the body, with a 200 as often as not.
            if let Some(errors) = raw.get("errors") {
                anyhow::bail!("AniList refused the query: {errors}");
            }
            return Ok(None);
        };

        let media = Media::deserialize(media)
            .context("AniList returned an entry this server could not read")?;

        let item = to_item(&media, kind);
        Ok(Some((raw, item)))
    }
}

// ─── the wire ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Media {
    id: i64,
    id_mal: Option<i64>,
    #[serde(default)]
    title: Titles,
    #[serde(default)]
    synonyms: Vec<String>,
    description: Option<String>,
    average_score: Option<i32>,
    stats: Option<Stats>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    tags: Vec<Tag>,
    studios: Option<Studios>,
    #[serde(default)]
    is_adult: bool,
    cover_image: Option<Cover>,
    start_date: Option<Date>,
}

#[derive(Debug, Default, Deserialize)]
struct Titles {
    romaji: Option<String>,
    english: Option<String>,
    native: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stats {
    #[serde(default)]
    score_distribution: Vec<Bucket>,
}

#[derive(Debug, Deserialize)]
struct Bucket {
    amount: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tag {
    name: String,
    rank: Option<i32>,
    #[serde(default)]
    is_media_spoiler: bool,
    #[serde(default)]
    is_general_spoiler: bool,
}

#[derive(Debug, Deserialize)]
struct Studios {
    #[serde(default)]
    nodes: Vec<Named>,
}

#[derive(Debug, Deserialize)]
struct Named {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cover {
    extra_large: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Date {
    year: Option<i32>,
}

// ─── mapping ─────────────────────────────────────────────────────────────────

fn to_item(media: &Media, kind: MediaKind) -> MediaItem {
    let mut item = MediaItem::empty(kind);

    let text = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };

    item.title = text(&media.title.english)
        .or_else(|| text(&media.title.romaji))
        .unwrap_or_default();
    item.original_title = text(&media.title.native);
    item.overview = media
        .description
        .as_deref()
        .map(plain_text)
        .filter(|s| !s.is_empty());
    item.year = media.start_date.as_ref().and_then(|d| d.year);
    item.studio = media
        .studios
        .as_ref()
        .and_then(|s| s.nodes.first())
        .map(|n| n.name.clone());
    item.genres = media.genres.clone();
    item.keywords = media
        .tags
        .iter()
        .filter(|t| !t.is_media_spoiler && !t.is_general_spoiler)
        .filter(|t| t.rank.unwrap_or(0) >= TAG_RANK)
        .map(|t| t.name.clone())
        .collect();
    item.is_adult = media.is_adult;

    item.external_ids = ExternalIds {
        anilist: vec![media.id],
        mal: media.id_mal.into_iter().collect(),
        ..Default::default()
    };

    // A mean out of a hundred, and no count of its own: the distribution's
    // buckets add up to everyone who scored it.
    if let Some(score) = media.average_score.filter(|s| *s > 0) {
        let votes: i64 = media
            .stats
            .iter()
            .flat_map(|s| &s.score_distribution)
            .filter_map(|b| b.amount)
            .sum();

        item.ratings = vec![Rating {
            source: "anilist".to_string(),
            value: Some(f64::from(score) / 10.0),
            votes: (votes > 0).then_some(votes),
            rating_type: Some("user".to_string()),
        }];
    }

    // Filed the way TMDB files them — a country, not a language — so the same
    // title from both is recognised as one.
    item.alternative_titles = [
        (media.title.romaji.as_deref(), "Romaji", Some("jpn")),
        (media.title.native.as_deref(), "Native", Some("jpn")),
        (media.title.english.as_deref(), "English", None),
    ]
    .into_iter()
    .chain(
        media
            .synonyms
            .iter()
            .map(|s| (Some(s.as_str()), "Synonym", None)),
    )
    .filter_map(|(title, title_type, language)| alternative_title(title?, title_type, language))
    .collect();

    if let Some(url) = media
        .cover_image
        .as_ref()
        .and_then(|c| c.extra_large.clone())
        .filter(|u| u.starts_with("https://"))
    {
        item.images.push(Image {
            id: new_id(),
            season_number: None,
            cover_type: CoverType::Poster,
            url,
            language: None,
            // Behind every poster a provider of record chose.
            sort_order: 100,
            source: Some("anilist".to_string()),
            is_manual: false,
        });
    }

    item.updated_at = now();
    item
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attack_on_titan() -> Media {
        serde_json::from_value(json!({
            "id": 16498,
            "idMal": 16498,
            "title": { "romaji": "Shingeki no Kyojin", "english": "Attack on Titan", "native": "進撃の巨人" },
            "synonyms": ["SnK", "AoT", " "],
            "description": "Several hundred years ago, humans were nearly exterminated by titans.<br><br>\n(Source: Kodansha)",
            "averageScore": 85,
            "stats": { "scoreDistribution": [
                { "score": 10, "amount": 3175 }, { "score": 90, "amount": 196044 }, { "score": 100, "amount": 187249 }
            ] },
            "genres": ["Action", "Drama"],
            "tags": [
                { "name": "Kaiju", "rank": 93, "isMediaSpoiler": false, "isGeneralSpoiler": false },
                { "name": "Twist", "rank": 90, "isMediaSpoiler": true, "isGeneralSpoiler": false },
                { "name": "Rare", "rank": 20, "isMediaSpoiler": false, "isGeneralSpoiler": false }
            ],
            "studios": { "nodes": [{ "name": "WIT STUDIO" }] },
            "isAdult": false,
            "coverImage": { "extraLarge": "https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/bx16498.jpg" },
            "startDate": { "year": 2013 }
        }))
        .unwrap()
    }

    #[test]
    fn the_score_is_out_of_ten_with_everyone_who_scored_it() {
        let item = to_item(&attack_on_titan(), MediaKind::Series);
        let rating = item.rating("anilist").unwrap();

        assert_eq!(rating.value, Some(8.5));
        assert_eq!(rating.votes, Some(3175 + 196_044 + 187_249));
    }

    #[test]
    fn every_name_it_goes_by_becomes_an_alternative_title() {
        let item = to_item(&attack_on_titan(), MediaKind::Movie);
        let titles: Vec<&str> = item
            .alternative_titles
            .iter()
            .map(|t| t.title.as_str())
            .collect();

        assert_eq!(
            titles,
            [
                "Shingeki no Kyojin",
                "進撃の巨人",
                "Attack on Titan",
                "SnK",
                "AoT"
            ],
            "a blank synonym is not a title"
        );
        assert_eq!(item.original_title.as_deref(), Some("進撃の巨人"));
    }

    #[test]
    fn spoilers_and_fringe_tags_are_not_keywords() {
        let item = to_item(&attack_on_titan(), MediaKind::Series);
        assert_eq!(item.keywords, ["Kaiju"]);
    }

    #[test]
    fn the_synopsis_is_text_and_the_cover_is_a_poster() {
        let item = to_item(&attack_on_titan(), MediaKind::Series);

        assert_eq!(
            item.overview.as_deref(),
            Some(
                "Several hundred years ago, humans were nearly exterminated by titans.\n(Source: Kodansha)"
            )
        );
        assert_eq!(item.images.len(), 1);
        assert_eq!(item.images[0].cover_type, CoverType::Poster);
        assert_eq!(item.studio.as_deref(), Some("WIT STUDIO"));
        assert_eq!(item.external_ids.mal, [16498]);
    }

    #[test]
    fn an_unscored_entry_has_no_rating() {
        let mut media = attack_on_titan();
        media.average_score = None;
        assert!(to_item(&media, MediaKind::Series).ratings.is_empty());
    }
}

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

use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    config,
    db::{new_id, now},
    domain::{CoverType, ExternalIds, Image, MediaItem, MediaKind, Rating, Relation},
    providers::{PATIENCE, Pacer, alternative_title, plain_text},
};

/// One entry, whole. `{scores}` is where the [`SCORES`] go: see [`scored`].
///
/// Relations in their third version, which adds `SAME_UNIVERSE` to the second.
const QUERY: &str = "query ($id: Int) {
  Media(id: $id, type: ANIME) {
    id idMal format
    title { romaji english native }
    synonyms
    description(asHtml: false)
    {scores}
    stats { scoreDistribution { score amount } }
    genres
    tags { name rank isMediaSpoiler isGeneralSpoiler }
    studios(isMain: true) { nodes { name } }
    isAdult
    coverImage { extraLarge }
    startDate { year }
    relations {
      edges {
        relationType(version: 3)
        node {
          id idMal type format isAdult
          title { romaji english native }
          startDate { year }
          coverImage { extraLarge large }
        }
      }
    }
  }
}";

/// Just enough of an entry to show it as another work's relation: the entry a
/// Fan-Kai was cut from, say.
const ENTRY: &str = "query ($id: Int) {
  Media(id: $id, type: ANIME) {
    id idMal type format isAdult
    title { romaji english native }
    startDate { year }
    coverImage { extraLarge large }
  }
}";

/// The scores [`QUERY`] asks for, in the order they are read: `weightedScore`,
/// and `averageScore`, which AniList has since deprecated for it — the same
/// figure under its former name, read when the first is missing, and asked
/// for as long as AniList still knows it.
const SCORES: [&str; 2] = ["weightedScore", "averageScore"];

/// Tags ranked below this are ones a minority of voters agreed with.
const TAG_RANK: i32 = 60;

pub struct AnilistClient {
    http: reqwest::Client,
    endpoint: String,
    pacer: Pacer,
    /// This process, named on every call: `graphql.anilist.co` is a name
    /// this server answers on too, and a resolver that sends it here would
    /// have it ask itself. See [`crate::providers::radarr::LOOP_HEADER`].
    instance: String,
    gate: crate::providers::Gate,
    /// The [`SCORES`] AniList refused a query for, a bit each by their place
    /// there: a field it no longer knows is not asked for again.
    dropped: AtomicU8,
}

impl AnilistClient {
    pub fn new(http: reqwest::Client, cfg: &config::Anilist, instance: String) -> Self {
        Self {
            http,
            endpoint: cfg.upstream.clone(),
            // Thirty a minute, AniList's current ceiling.
            pacer: Pacer::new(std::time::Duration::from_millis(2_100)),
            instance,
            // Spaced already; this is for its `Retry-After`.
            gate: crate::providers::Gate::new("anilist", "AniList", 4),
            dropped: AtomicU8::new(0),
        }
    }

    /// AniList's door here: the few requests in flight at once, and the
    /// pause its 429 asks for. The relay goes through it too
    /// ([`Gate::send_now`](crate::providers::Gate::send_now)), so that one
    /// pause holds for both — but not through the spacing, which keeps this
    /// server's own fetches, one per work, under AniList's limit.
    pub fn gate(&self) -> &crate::providers::Gate {
        &self.gate
    }

    /// One AniList entry, as a contribution to the work of `kind`. Returns the
    /// raw answer alongside the parsed one.
    ///
    /// A query refused because AniList no longer knows one of the [`SCORES`]
    /// is sent again without it, and so is every one after: a deprecated
    /// field going away must cost the figure it carried, not the entry.
    pub async fn media(&self, id: i64, kind: MediaKind) -> Result<Option<(Value, MediaItem)>> {
        let raw = loop {
            let dropped = self.dropped.load(Ordering::Relaxed);
            let error = match self.ask(&scored(dropped), id).await {
                Ok(Some(raw)) => break raw,
                Ok(None) => return Ok(None),
                Err(error) => error,
            };
            // One this query did not ask for is not one to leave out.
            let Some(unknown) = error
                .downcast_ref::<UnknownScore>()
                .filter(|u| dropped & u.bit() == 0)
            else {
                return Err(error);
            };
            if self.dropped.fetch_or(unknown.bit(), Ordering::Relaxed) & unknown.bit() == 0 {
                tracing::warn!(
                    field = SCORES[unknown.0],
                    "AniList no longer knows this score; it is not asked for again"
                );
            }
        };

        let media = Media::deserialize(&raw["data"]["Media"])
            .context("AniList returned an entry this server could not read")?;

        let item = to_item(&media, kind);
        Ok(Some((raw, item)))
    }

    /// One AniList entry as another work's relation of `relation_type`, with
    /// its title, year and cover — one small query, for a work that is not
    /// AniList's own but points at one of its entries.
    pub async fn entry(&self, id: i64, relation_type: &str) -> Result<Option<Relation>> {
        let Some(raw) = self.ask(ENTRY, id).await? else {
            return Ok(None);
        };

        let node = Node::deserialize(&raw["data"]["Media"])
            .context("AniList returned an entry this server could not read")?;

        Ok(relation(&node, relation_type.to_string()))
    }

    /// One query about one entry: the answer, when it names one.
    async fn ask(&self, query: &str, id: i64) -> Result<Option<Value>> {
        if !self.pacer.turn(1, PATIENCE).await {
            anyhow::bail!("AniList's queue is full; this fetch goes without it");
        }

        let body = json!({ "query": query, "variables": { "id": id } });
        let (response, _permit) = self
            .gate
            .send(|| {
                self.http
                    .post(&self.endpoint)
                    .header(reqwest::header::ACCEPT, "application/json")
                    .header(crate::providers::radarr::LOOP_HEADER, &self.instance)
                    .json(&body)
                    .timeout(std::time::Duration::from_secs(20))
            })
            .await
            .map_err(|e| anyhow::anyhow!("AniList request failed: {e}"))?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            anyhow::bail!("AniList's rate limit was reached");
        }
        if status == reqwest::StatusCode::BAD_REQUEST {
            // A query GraphQL would not run, with its reasons.
            let body =
                crate::providers::read_body(response, crate::providers::ERROR_BODY_BYTES as u64)
                    .await
                    .unwrap_or_default();
            if let Some(unknown) = serde_json::from_slice(&body)
                .ok()
                .and_then(|said: Value| unknown_score(&said))
            {
                return Err(unknown.into());
            }
            let reason = String::from_utf8_lossy(&body)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            anyhow::bail!(
                "AniList returned {status}: {}",
                crate::providers::clip(&reason, crate::providers::MESSAGE_CHARS)
            );
        }
        if !status.is_success() {
            let reason = crate::providers::error_text(response).await;
            anyhow::bail!("AniList returned {status}: {reason}");
        }

        let raw = crate::providers::read_json(response)
            .await
            .context("AniList returned a body this server could not read")?;

        if raw.pointer("/data/Media").is_none_or(Value::is_null) {
            if let Some(unknown) = unknown_score(&raw) {
                return Err(unknown.into());
            }
            // GraphQL reports its errors in the body, with a 200 as often as not.
            if let Some(errors) = raw.get("errors") {
                anyhow::bail!(
                    "AniList refused the query: {}",
                    crate::providers::clip(&errors.to_string(), crate::providers::MESSAGE_CHARS)
                );
            }
            return Ok(None);
        }

        Ok(Some(raw))
    }
}

/// [`QUERY`], asking for the [`SCORES`] whose bits are not in `dropped`.
fn scored(dropped: u8) -> String {
    let scores: Vec<&str> = SCORES
        .iter()
        .enumerate()
        .filter(|(at, _)| dropped & (1 << at) == 0)
        .map(|(_, field)| *field)
        .collect();
    QUERY.replace("{scores}", &scores.join(" "))
}

/// AniList refusing a query because it no longer knows one of the
/// [`SCORES`]: the field's place there.
#[derive(Debug)]
struct UnknownScore(usize);

impl UnknownScore {
    /// The field's bit in [`AnilistClient::dropped`].
    fn bit(&self) -> u8 {
        1 << self.0
    }
}

impl std::fmt::Display for UnknownScore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AniList no longer knows the field {}", SCORES[self.0])
    }
}

impl std::error::Error for UnknownScore {}

/// The score a query was refused for, when GraphQL's errors in `answer` say
/// AniList does not know one of the [`SCORES`] — in its own words, `Cannot
/// query field "averageScore" on type "Media".`, whatever it suggests after.
fn unknown_score(answer: &Value) -> Option<UnknownScore> {
    answer
        .get("errors")?
        .as_array()?
        .iter()
        .filter_map(|error| error.get("message")?.as_str())
        .filter_map(|message| {
            let (field, rest) = message
                .split_once("Cannot query field \"")?
                .1
                .split_once('"')?;
            rest.trim_start()
                .starts_with("on type \"Media\"")
                .then_some(field)
        })
        .find_map(|field| SCORES.iter().position(|score| *score == field))
        .map(UnknownScore)
}

// ─── the wire ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Media {
    id: i64,
    id_mal: Option<i64>,
    /// `TV`, `TV_SHORT`, `MOVIE`, `SPECIAL`, `OVA`, `ONA`, `MUSIC`.
    format: Option<String>,
    #[serde(default)]
    title: Titles,
    #[serde(default)]
    synonyms: Vec<String>,
    description: Option<String>,
    /// Out of a hundred. An `Int` in AniList's schema, read as a number: a
    /// change of type must not make the whole entry unreadable.
    weighted_score: Option<f64>,
    /// The same figure under its former name: see [`SCORES`].
    average_score: Option<f64>,
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
    relations: Option<Relations>,
}

#[derive(Debug, Deserialize)]
struct Relations {
    #[serde(default)]
    edges: Vec<Edge>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Edge {
    relation_type: Option<String>,
    node: Option<Node>,
}

/// The entry at the other end of a relation, as much of it as is shown.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    id: i64,
    id_mal: Option<i64>,
    /// `ANIME` or `MANGA`.
    #[serde(rename = "type")]
    medium: Option<String>,
    format: Option<String>,
    #[serde(default)]
    is_adult: bool,
    #[serde(default)]
    title: Titles,
    start_date: Option<Date>,
    cover_image: Option<Cover>,
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
    large: Option<String>,
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
    // What the entry is, in AniList's own words: the interface names them.
    item.format = text(&media.format).map(|f| f.to_ascii_uppercase());

    item.external_ids = ExternalIds {
        anilist: vec![media.id],
        mal: media.id_mal.into_iter().collect(),
        ..Default::default()
    };

    // A mean out of a hundred — the weighted one, or under its former name
    // when that is missing — and no count of its own: the distribution's
    // buckets add up to everyone who scored it.
    if let Some(score) = [media.weighted_score, media.average_score]
        .into_iter()
        .flatten()
        .find(|s| *s > 0.0)
    {
        // Saturating: the counts are a stranger's numbers, and a sum past
        // `i64` panics in a debug build and wraps negative in a release one.
        let votes: i64 = media
            .stats
            .iter()
            .flat_map(|s| &s.score_distribution)
            .filter_map(|b| b.amount)
            .filter(|amount| *amount > 0)
            .fold(0i64, i64::saturating_add);

        item.ratings = vec![Rating {
            source: "anilist".to_string(),
            value: Some(score / 10.0),
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

    item.relations = relations(media);

    item.updated_at = now();
    item
}

/// The entries AniList files beside this one: prequels and sequels first, then
/// the stories beside the main one, then what it was drawn from and the rest —
/// each kind in order of year. An edge with no entry behind it is nothing.
///
/// A kind is kept as AniList names it, the third version's `SAME_UNIVERSE`
/// too: the interface names the kinds it knows and calls the others related.
fn relations(media: &Media) -> Vec<Relation> {
    let mut related: Vec<Relation> = media
        .relations
        .iter()
        .flat_map(|r| &r.edges)
        .filter_map(|edge| relation(edge.node.as_ref()?, edge.relation_type.clone()?))
        .collect();
    related.sort_by_key(|r| {
        (
            relation_rank(&r.relation_type),
            r.year.unwrap_or(i32::MAX),
            r.external_id,
        )
    });
    for (index, relation) in related.iter_mut().enumerate() {
        relation.sort_order = i32::try_from(index).unwrap_or(i32::MAX);
    }
    related
}

/// The entry at the other end of a relation, as the relation shows it.
fn relation(node: &Node, relation_type: String) -> Option<Relation> {
    let title = [&node.title.english, &node.title.romaji, &node.title.native]
        .into_iter()
        .find_map(|t| t.as_deref().map(str::trim).filter(|t| !t.is_empty()))?
        .to_string();

    Some(Relation {
        id: String::new(),
        relation_type,
        source: "anilist".to_string(),
        external_id: node.id,
        mal_id: node.id_mal,
        title,
        medium: node
            .medium
            .as_deref()
            .unwrap_or("ANIME")
            .to_ascii_lowercase(),
        format: node.format.clone(),
        year: node.start_date.as_ref().and_then(|d| d.year),
        image: node
            .cover_image
            .as_ref()
            .and_then(|c| c.extra_large.clone().or_else(|| c.large.clone()))
            .filter(|u| u.starts_with("https://")),
        is_adult: node.is_adult,
        work_id: None,
        sort_order: 0,
    })
}

/// Where a kind of relation stands: the story's own line first, and a work
/// that only shares its world just before the rest — a long franchise has many.
fn relation_rank(relation_type: &str) -> u8 {
    match relation_type {
        "PREQUEL" => 0,
        "SEQUEL" => 1,
        "PARENT" => 2,
        "SIDE_STORY" => 3,
        "SPIN_OFF" => 4,
        "ALTERNATIVE" => 5,
        "SUMMARY" => 6,
        "COMPILATION" => 7,
        "CONTAINS" => 8,
        "SOURCE" => 9,
        "ADAPTATION" => 10,
        "CHARACTER" => 11,
        "SAME_UNIVERSE" => 12,
        _ => 13,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;

    fn attack_on_titan() -> Media {
        serde_json::from_value(json!({
            "id": 16498,
            "idMal": 16498,
            "title": { "romaji": "Shingeki no Kyojin", "english": "Attack on Titan", "native": "進撃の巨人" },
            "synonyms": ["SnK", "AoT", " "],
            "description": "Several hundred years ago, humans were nearly exterminated by titans.<br><br>\n(Source: Kodansha)",
            "weightedScore": 85,
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
            "startDate": { "year": 2013 },
            "relations": { "edges": [
                { "relationType": "SEQUEL", "node": { "id": 20958, "idMal": 25777, "type": "ANIME", "format": "TV",
                  "title": { "romaji": "Shingeki no Kyojin 2", "english": "Attack on Titan Season 2", "native": "進撃の巨人 Season 2" },
                  "startDate": { "year": 2017 },
                  "coverImage": { "extraLarge": "https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/bx20958.jpg" } } },
                { "relationType": "SOURCE", "node": { "id": 53390, "idMal": 23390, "type": "MANGA", "format": "MANGA", "isAdult": true,
                  "title": { "romaji": "Shingeki no Kyojin", "english": "Attack on Titan", "native": "進撃の巨人" },
                  "startDate": { "year": 2009 },
                  "coverImage": { "large": "https://s4.anilist.co/file/anilistcdn/media/manga/cover/medium/bx53390.jpg" } } },
                { "relationType": "PREQUEL", "node": { "id": 20811, "idMal": 25781, "type": "ANIME", "format": "OVA",
                  "title": { "romaji": "Shingeki no Kyojin: Kuinaki Sentaku", "english": "Attack on Titan: No Regrets", "native": null },
                  "startDate": { "year": 2014 },
                  "coverImage": { "extraLarge": "http://insecure.example/x.jpg" } } },
                { "relationType": "SAME_UNIVERSE", "node": { "id": 990001, "type": "ANIME", "format": "ONA",
                  "title": { "romaji": "Kabe no Soto no Monogatari", "english": null, "native": null },
                  "startDate": { "year": 2016 } } },
                { "relationType": "OTHER", "node": null }
            ] }
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
    fn the_works_filed_beside_it_are_kept_nearest_first() {
        let item = to_item(&attack_on_titan(), MediaKind::Series);
        let related: Vec<(&str, i64, &str)> = item
            .relations
            .iter()
            .map(|r| (r.relation_type.as_str(), r.external_id, r.medium.as_str()))
            .collect();
        assert_eq!(
            related,
            [
                ("PREQUEL", 20811, "anime"),
                ("SEQUEL", 20958, "anime"),
                ("SOURCE", 53390, "manga"),
                // The third version's kind, kept as AniList names it.
                ("SAME_UNIVERSE", 990_001, "anime")
            ],
            "an edge with no entry behind it is nothing to keep"
        );
        assert_eq!(item.relations[3].title, "Kabe no Soto no Monogatari");
        assert_eq!(item.relations[3].mal_id, None);
        let sequel = &item.relations[1];
        assert_eq!(sequel.title, "Attack on Titan Season 2");
        assert_eq!(sequel.year, Some(2017));
        assert_eq!(sequel.mal_id, Some(25777));
        assert_eq!(sequel.format.as_deref(), Some("TV"));
        assert!(
            sequel
                .image
                .as_deref()
                .is_some_and(|u| u.contains("bx20958"))
        );
        // The manga's cover comes in the one size AniList gives for it.
        assert!(
            item.relations[2]
                .image
                .as_deref()
                .is_some_and(|u| u.contains("bx53390"))
        );
        // Not over plain http.
        assert_eq!(item.relations[0].image, None);
        // As AniList flags each entry.
        assert!(item.relations[2].is_adult);
        assert!(!item.relations[1].is_adult);
        assert_eq!(
            item.relations
                .iter()
                .map(|r| r.sort_order)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn a_work_that_shares_the_world_comes_after_one_that_shares_a_character() {
        let mut kinds = [
            "OTHER",
            "SAME_UNIVERSE",
            "CHARACTER",
            "A_KIND_TO_COME",
            "SOURCE",
            "SEQUEL",
        ];
        kinds.sort_by_key(|kind| relation_rank(kind));
        assert_eq!(
            kinds,
            [
                "SEQUEL",
                "SOURCE",
                "CHARACTER",
                "SAME_UNIVERSE",
                "OTHER",
                "A_KIND_TO_COME"
            ]
        );
    }

    #[test]
    fn the_entry_s_format_is_kept() {
        let mut media = attack_on_titan();
        media.format = Some("tv".into());
        assert_eq!(
            to_item(&media, MediaKind::Series).format.as_deref(),
            Some("TV")
        );
        media.format = Some("OVA".into());
        assert_eq!(
            to_item(&media, MediaKind::Series).format.as_deref(),
            Some("OVA")
        );
        media.format = None;
        assert_eq!(to_item(&media, MediaKind::Series).format, None);
    }

    /// TMDB answers first, with keywords of its own: taken whole, they were
    /// all a work kept, and AniList's tags went.
    #[test]
    fn its_tags_stay_beside_tmdb_s_keywords() {
        use crate::merge::{Contribution, combine};

        let mut media = attack_on_titan();
        media.format = Some("TV".into());
        media.tags.push(Tag {
            name: "Military".into(),
            rank: Some(80),
            is_media_spoiler: false,
            is_general_spoiler: false,
        });
        let mut tmdb = MediaItem::empty(MediaKind::Series);
        tmdb.title = "Attack on Titan".into();
        tmdb.keywords = vec!["based on manga".into(), "kaiju".into()];

        let merged = combine(
            vec![
                Contribution {
                    provider: "tmdb".into(),
                    item: tmdb,
                },
                Contribution {
                    provider: "anilist".into(),
                    item: to_item(&media, MediaKind::Series),
                },
            ],
            &["tmdb".to_string(), "anilist".to_string()],
        )
        .expect("merged");

        // Each once, whatever its case, the first spelling kept.
        assert_eq!(merged.keywords, ["based on manga", "kaiju", "Military"]);
        assert_eq!(merged.format.as_deref(), Some("TV"));
    }

    #[test]
    fn an_unscored_entry_has_no_rating() {
        let mut media = attack_on_titan();
        media.weighted_score = None;
        media.average_score = None;
        assert!(to_item(&media, MediaKind::Series).ratings.is_empty());
    }

    #[test]
    fn the_weighted_score_is_read_and_its_former_name_stands_in_for_it() {
        let score = |media: &Media| {
            to_item(media, MediaKind::Series)
                .rating("anilist")
                .and_then(|r| r.value)
        };
        let mut media = attack_on_titan();
        media.weighted_score = Some(86.0);
        media.average_score = Some(85.0);
        assert_eq!(score(&media), Some(8.6));

        media.weighted_score = None;
        assert_eq!(score(&media), Some(8.5), "missing, the former name answers");
        media.weighted_score = Some(0.0);
        assert_eq!(score(&media), Some(8.5), "nought is no score");
    }

    #[test]
    fn both_scores_and_the_third_version_of_relations_are_asked_for() {
        let query = scored(0);
        assert!(query.contains("weightedScore averageScore"), "{query}");
        assert!(query.contains("relationType(version: 3)"), "{query}");
        assert!(!query.contains("{scores}"), "{query}");

        let without = scored(UnknownScore(1).bit());
        assert!(without.contains("weightedScore"), "{without}");
        assert!(!without.contains("averageScore"), "{without}");

        // Neither known any more: the entry is still asked for.
        let neither = scored(UnknownScore(0).bit() | UnknownScore(1).bit());
        assert!(!neither.contains("weightedScore") && !neither.contains("averageScore"));
        assert!(neither.contains("relations"), "{neither}");
    }

    #[test]
    fn a_score_anilist_no_longer_knows_is_told_from_any_other_refusal() {
        // GraphQL's words, with the field it suggests in its place.
        let refused = json!({ "errors": [{
            "message": "Cannot query field \"averageScore\" on type \"Media\". Did you mean \"weightedScore\"?",
            "status": 400,
            "locations": [{ "line": 7, "column": 5 }]
        }], "data": null });
        assert_eq!(unknown_score(&refused).map(|u| u.0), Some(1));

        for message in [
            "Cannot query field \"popularity\" on type \"Media\".",
            "Cannot query field \"averageScore\" on type \"MediaTrend\".",
            "Not Found.",
        ] {
            let said = json!({ "errors": [{ "message": message }], "data": null });
            assert!(unknown_score(&said).is_none(), "{message}");
        }
        assert!(unknown_score(&json!({ "data": { "Media": null } })).is_none());
    }

    /// An AniList that no longer knows `averageScore`: a query asking for it
    /// is refused as GraphQL refuses one, with a 400, and the others are
    /// answered with Attack on Titan. And the queries it was sent.
    async fn without_the_former_score() -> (String, Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = asked.clone();
        tokio::spawn(async move {
            while let Ok((mut connection, _)) = listener.accept().await {
                // The head, then as much body as it announces.
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                let body = loop {
                    let read = connection.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        break Vec::new();
                    }
                    request.extend_from_slice(&chunk[..read]);
                    let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let length = String::from_utf8_lossy(&request[..end])
                        .to_ascii_lowercase()
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:")?.trim().parse().ok())
                        .unwrap_or(0usize);
                    if request.len() >= end + 4 + length {
                        break request[end + 4..end + 4 + length].to_vec();
                    }
                };
                let query = serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|b| b["query"].as_str().map(String::from))
                    .unwrap_or_default();
                let (status, answer) = if query.contains("averageScore") {
                    (
                        "400 Bad Request",
                        json!({ "errors": [{
                            "message": "Cannot query field \"averageScore\" on type \"Media\". Did you mean \"weightedScore\"?",
                            "status": 400,
                            "locations": [{ "line": 7, "column": 5 }]
                        }], "data": null }),
                    )
                } else {
                    (
                        "200 OK",
                        json!({ "data": { "Media": {
                            "id": 16498,
                            "title": { "romaji": "Shingeki no Kyojin" },
                            "weightedScore": 86
                        } } }),
                    )
                };
                seen.lock().push(query);
                let answer = answer.to_string();
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{answer}",
                    answer.len()
                );
                let _ = connection.write_all(reply.as_bytes()).await;
                let _ = connection.shutdown().await;
            }
        });
        (at, asked)
    }

    #[tokio::test]
    async fn a_score_anilist_no_longer_knows_costs_one_refusal_and_never_the_entry() {
        let (at, asked) = without_the_former_score().await;
        let mut anilist = AnilistClient::new(
            reqwest::Client::new(),
            &config::Anilist {
                upstream: at,
                enabled: true,
                passthrough: false,
            },
            "a test".into(),
        );
        // The stand-in has no limit to keep under.
        anilist.pacer = Pacer::new(std::time::Duration::ZERO);

        let (_, item) = anilist
            .media(16498, MediaKind::Series)
            .await
            .unwrap()
            .expect("the entry, asked again without the field");
        assert_eq!(item.rating("anilist").and_then(|r| r.value), Some(8.6));
        assert!(
            anilist
                .media(16498, MediaKind::Series)
                .await
                .unwrap()
                .is_some()
        );

        let asked = asked.lock().clone();
        assert_eq!(asked.len(), 3, "refused once, and only once: {asked:#?}");
        assert!(asked[0].contains("weightedScore averageScore"));
        for query in &asked[1..] {
            assert!(query.contains("weightedScore"), "{query}");
            assert!(!query.contains("averageScore"), "{query}");
        }
    }
}

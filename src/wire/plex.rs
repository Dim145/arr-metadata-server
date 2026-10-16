//! Plex's format for a metadata provider.
//!
//! Plex Media Server 1.43 and later take a library's metadata from a
//! *metadata provider*: an HTTP API a person adds by its address, under
//! Settings › Metadata Agents › Add Provider, which Plex then asks to match
//! its files and to describe what they matched. The shapes here are the ones
//! Plex documents — developer.plex.tv/pms, *Metadata Providers* — and answers
//! with in its reference provider, `plexinc/tmdb-example-provider`
//! (`docs/MediaProvider.md`, `docs/Metadata.md`, `docs/API Endpoints.md`,
//! `src/models/*.ts`, `src/mappers/TMDBMapper.ts`): a list is named in
//! PascalCase — `Metadata`, `Image`, `Guid` — and everything else in
//! camelCase. Where a field is shaped by one of them, that source is named
//! beside it.
//!
//! Written only: Plex is a client of this server, never one of its sources.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    ops::Range,
};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::{
    domain::{
        CoverType, CreditType, Episode, EpisodeOrder, Image, MediaItem, MediaKind, PrimaryImages,
        Season, genre_parts, ids::normalize_imdb_id, lead_images,
    },
    service::{
        language,
        orders::{self, AIRED},
    },
};

/// The provider's identifier, and the scheme of every GUID it hands out:
/// what Plex files a matched item under. Plex asks a custom provider's to
/// begin with `tv.plex.agents.custom.` and to go on in ASCII letters, digits
/// and dots (developer.plex.tv/pms, Metadata Providers › MediaProvider).
/// Fixed for good: every item Plex matched holds it, and another would
/// orphan them all.
pub const IDENTIFIER: &str = "tv.plex.agents.custom.arrmetadataserver";

/// The name Plex lists the provider under.
pub const TITLE: &str = "arr-metadata-server";

/// Where the provider's two features answer, under the address a person
/// gave Plex: the reference provider's paths (`src/constants.ts`,
/// `API_PATHS`), which Plex appends to that address.
pub const METADATA_PATH: &str = "/library/metadata";
pub const MATCH_PATH: &str = "/library/metadata/matches";

/// Plex's numbers for the kinds of item (`src/models/MediaProvider.ts`,
/// `MetadataType`).
pub const MOVIE: u8 = 1;
pub const SHOW: u8 = 2;
pub const SEASON: u8 = 3;
pub const EPISODE: u8 = 4;

// ─── the manifest ────────────────────────────────────────────────────────────

/// What the provider's root answers (`src/models/MediaProvider.ts`,
/// `MediaProviderResponse`).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexProviderResponse {
    #[serde(rename = "MediaProvider")]
    pub media_provider: PlexProvider,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexProvider {
    pub identifier: String,
    pub title: String,
    pub version: String,
    /// The kinds of item it describes, each under the provider's scheme.
    #[serde(rename = "Types")]
    pub types: Vec<PlexType>,
    /// What it can be asked, and at which path.
    #[serde(rename = "Feature")]
    pub features: Vec<PlexFeature>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexType {
    /// 1 a film, 2 a series, 3 a season, 4 an episode.
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(rename = "Scheme")]
    pub schemes: Vec<PlexScheme>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexScheme {
    pub scheme: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexFeature {
    /// `metadata` or `match`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The path the feature answers at, under the provider's own address.
    pub key: String,
}

/// The provider as Plex is told of it at its root: films, and series with
/// their seasons and episodes, all under the one scheme, and the two
/// features Plex requires of every provider. Plex advises one kind of item
/// per provider, so that two providers can be put together in one agent;
/// it does not require it, and one address that serves both kinds of
/// library is what a person running one catalogue wants.
pub fn provider() -> PlexProviderResponse {
    let schemes = || {
        vec![PlexScheme {
            scheme: IDENTIFIER.to_string(),
        }]
    };
    PlexProviderResponse {
        media_provider: PlexProvider {
            identifier: IDENTIFIER.to_string(),
            title: TITLE.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            types: [MOVIE, SHOW, SEASON, EPISODE]
                .into_iter()
                .map(|kind| PlexType {
                    kind,
                    schemes: schemes(),
                })
                .collect(),
            features: vec![
                PlexFeature {
                    kind: "metadata".to_string(),
                    key: METADATA_PATH.to_string(),
                },
                PlexFeature {
                    kind: "match".to_string(),
                    key: MATCH_PATH.to_string(),
                },
            ],
        },
    }
}

// ─── the answers ─────────────────────────────────────────────────────────────

/// What every metadata and match route answers: the items in a
/// `MediaContainer` (`src/models/Metadata.ts`, `MetadataResponse`).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexMetadataResponse {
    #[serde(rename = "MediaContainer")]
    pub media_container: PlexContainer,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlexContainer {
    /// Where the page starts among all there are, from 0.
    pub offset: usize,
    /// How many there are in all; `size` is how many are in this answer.
    pub total_size: usize,
    pub identifier: String,
    pub size: usize,
    #[serde(rename = "Metadata")]
    pub metadata: Vec<PlexMetadata>,
}

/// What `/images` answers (`docs/API Endpoints.md`, Images).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexImagesResponse {
    #[serde(rename = "MediaContainer")]
    pub media_container: PlexImageContainer,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlexImageContainer {
    pub offset: usize,
    pub total_size: usize,
    pub identifier: String,
    pub size: usize,
    #[serde(rename = "Image")]
    pub images: Vec<PlexImage>,
}

/// A film, a series, a season or an episode, as Plex reads one
/// (`src/models/Metadata.ts`; developer.plex.tv/pms, Metadata Providers ›
/// Metadata). Only the fields with a value are written.
#[derive(Clone, Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlexMetadata {
    pub rating_key: String,
    /// The path the item is asked at, under the provider's address: a
    /// series' and a season's are those of their children, as Plex's own
    /// examples write them (`docs/Metadata.md`).
    pub key: String,
    /// `{scheme}://{type}/{ratingKey}`.
    pub guid: String,
    /// `movie`, `show`, `season` or `episode`.
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    /// `YYYY-MM-DD`; empty where no day is known, as the reference
    /// provider leaves it (`TMDBMapper.ts`: `air_date || ''`).
    pub originally_available_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    /// The United States' rating plain, `PG-13`; any other country's after
    /// its code, `de/12`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_sort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_adult: Option<bool>,
    /// In milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio: Option<String>,
    /// The address of the work's theme music.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_rating_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_guid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_thumb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_art: Option<String>,
    /// A season's number, or an episode's within its season.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_rating_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_guid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_thumb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_art: Option<String>,
    /// An episode's season.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index: Option<i32>,

    #[serde(rename = "Image", skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<PlexImage>,
    #[serde(rename = "Genre", skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<PlexTag>,
    /// The other providers' ids for the item, `imdb://tt0133093`: what Plex
    /// passes on to the other providers of an agent.
    #[serde(rename = "Guid", skip_serializing_if = "Vec::is_empty")]
    pub guids: Vec<PlexGuid>,
    #[serde(rename = "Role", skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<PlexPerson>,
    #[serde(rename = "Director", skip_serializing_if = "Vec::is_empty")]
    pub directors: Vec<PlexPerson>,
    #[serde(rename = "Producer", skip_serializing_if = "Vec::is_empty")]
    pub producers: Vec<PlexPerson>,
    #[serde(rename = "Writer", skip_serializing_if = "Vec::is_empty")]
    pub writers: Vec<PlexPerson>,
    #[serde(rename = "Studio", skip_serializing_if = "Vec::is_empty")]
    pub studios: Vec<PlexTag>,
    #[serde(rename = "Network", skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<PlexTag>,
    #[serde(rename = "Rating", skip_serializing_if = "Vec::is_empty")]
    pub ratings: Vec<PlexRating>,
    /// The orders a series' episodes may be asked in, by `episodeOrder`.
    #[serde(rename = "SeasonType", skip_serializing_if = "Vec::is_empty")]
    pub season_types: Vec<PlexSeasonType>,
    /// A series' seasons, or a season's episodes, when asked for with
    /// `includeChildren=1`.
    #[serde(rename = "Children", skip_serializing_if = "Option::is_none")]
    pub children: Option<PlexChildren>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexChildren {
    pub size: usize,
    /// An item holds its children and a child is an item: the loop would
    /// have the documentation's schemas collected for ever, so it is cut here.
    #[schema(no_recursion)]
    #[serde(rename = "Metadata")]
    pub metadata: Vec<PlexMetadata>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexImage {
    /// `coverPoster`, `background`, `clearLogo` or `snapshot`.
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexTag {
    pub tag: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexGuid {
    pub id: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexPerson {
    pub tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
    /// The character, or the job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Where they come in the cast, from 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<usize>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexRating {
    /// `imdb://image.rating` or `themoviedb://image.rating`.
    pub image: String,
    /// `audience`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Out of ten.
    pub value: f64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlexSeasonType {
    /// What Plex sends back as `episodeOrder`.
    pub id: String,
    pub source: String,
    pub tag: String,
    pub title: String,
}

/// A page of items as Plex reads one.
pub fn container(metadata: Vec<PlexMetadata>, offset: usize, total: usize) -> PlexMetadataResponse {
    PlexMetadataResponse {
        media_container: PlexContainer {
            offset,
            total_size: total,
            identifier: IDENTIFIER.to_string(),
            size: metadata.len(),
            metadata,
        },
    }
}

/// An item's pictures as `/images` answers them.
pub fn image_container(images: Vec<PlexImage>) -> PlexImagesResponse {
    PlexImagesResponse {
        media_container: PlexImageContainer {
            offset: 0,
            total_size: images.len(),
            identifier: IDENTIFIER.to_string(),
            size: images.len(),
            images,
        },
    }
}

// ─── what Plex asks ──────────────────────────────────────────────────────────

/// What Plex sends to be matched (developer.plex.tv/pms, Metadata Providers
/// › Match; the reference provider's `MatchRequest`). Read leniently: a
/// number may come as text, and a value of a shape this server cannot read
/// is taken for one not sent, rather than failing the match.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct PlexMatchRequest {
    /// 1 a film, 2 a series, 3 a season, 4 an episode.
    #[serde(rename = "type", deserialize_with = "lenient_number")]
    pub kind: Option<i64>,
    /// A film's or a series' title.
    #[serde(deserialize_with = "lenient_text")]
    pub title: Option<String>,
    /// A season's series.
    #[serde(deserialize_with = "lenient_text")]
    pub parent_title: Option<String>,
    /// An episode's series.
    #[serde(deserialize_with = "lenient_text")]
    pub grandparent_title: Option<String>,
    #[serde(deserialize_with = "lenient_number")]
    pub year: Option<i64>,
    /// One id of the item: this provider's GUID for it, or another's,
    /// `imdb://tt0133093`.
    #[serde(deserialize_with = "lenient_text")]
    pub guid: Option<String>,
    /// Every id Plex knows the item by, as Plex has said it will send them.
    #[serde(deserialize_with = "lenient_guids")]
    pub guids: Vec<String>,
    #[serde(deserialize_with = "lenient_text")]
    pub parent_guid: Option<String>,
    #[serde(deserialize_with = "lenient_guids")]
    pub parent_guids: Vec<String>,
    #[serde(deserialize_with = "lenient_text")]
    pub grandparent_guid: Option<String>,
    #[serde(deserialize_with = "lenient_guids")]
    pub grandparent_guids: Vec<String>,
    /// A season's number, or an episode's within its season.
    #[serde(deserialize_with = "lenient_number")]
    pub index: Option<i64>,
    /// An episode's season.
    #[serde(deserialize_with = "lenient_number")]
    pub parent_index: Option<i64>,
    /// The day an episode aired, `YYYY-MM-DD`. Plex sends a file's name
    /// too, which a provider that does not see the files has no use for.
    #[serde(deserialize_with = "lenient_text")]
    pub date: Option<String>,
    /// 1 for every likely match, best first — a person choosing one in
    /// Plex's *Fix Match* — rather than the best alone.
    #[serde(deserialize_with = "lenient_flag")]
    pub manual: Option<bool>,
    #[serde(deserialize_with = "lenient_flag")]
    pub include_adult: Option<bool>,
    #[serde(deserialize_with = "lenient_flag")]
    pub include_children: Option<bool>,
    /// The order to number episodes by: a series' `SeasonType` id. Plex's
    /// documentation spells it `episodeOrdering` in one place too.
    #[serde(alias = "episodeOrdering", deserialize_with = "lenient_text")]
    pub episode_order: Option<String>,
}

fn lenient_number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && f.abs() < 1e15)
                .map(|f| f as i64)
        }),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

fn lenient_flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::Bool(b) => Some(b),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0),
        Value::String(s) => flag(&s),
        _ => None,
    })
}

fn lenient_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// A list of GUIDs as strings, or as `{ "id": … }` the way Plex writes its
/// own `Guid`; a lone string is a list of one.
fn lenient_guids<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    let text = |value: &Value| match value {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Object(map) => map.get("id").and_then(Value::as_str).map(str::to_string),
        _ => None,
    };
    Ok(match Value::deserialize(deserializer)? {
        Value::Array(items) => items.iter().filter_map(text).collect(),
        other => text(&other).into_iter().collect(),
    })
}

/// A flag as Plex writes one in a query or a body: `1`, `0`, `true`,
/// `false`.
pub fn flag(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

// ─── keys and GUIDs ──────────────────────────────────────────────────────────

/// How a work is named in a ratingKey: by the provider id it is best known
/// by, so that a key Plex holds names the same work after the catalogue lost
/// it and fetched it again, or after a new database — or by the catalogue's
/// own id, for a work with none, which only an entry made by hand can be.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Work {
    Tmdb(i64),
    Tvdb(i64),
    Imdb(String),
    Fankai(i64),
    Held(String),
}

impl Work {
    /// The name a work is given: a film by TMDB's id, then IMDb's; a series
    /// by TheTVDB's — which settles its numbering — then TMDB's, then
    /// Fankai's; either by the catalogue's own when it has none of those.
    pub fn of(item: &MediaItem) -> Self {
        let ids = &item.external_ids;
        let named = match item.kind {
            MediaKind::Movie => ids
                .tmdb
                .map(Self::Tmdb)
                .or_else(|| ids.imdb.clone().map(Self::Imdb)),
            MediaKind::Series => ids
                .tvdb
                .map(Self::Tvdb)
                .or_else(|| ids.tmdb.map(Self::Tmdb))
                .or_else(|| ids.fankai.map(Self::Fankai)),
        };
        named.unwrap_or_else(|| Self::Held(item.id.clone()))
    }

    fn parse(raw: &str) -> Option<Self> {
        let (namespace, value) = raw.split_once('-')?;
        match namespace {
            "tmdb" => positive(value).map(Self::Tmdb),
            "tvdb" => positive(value).map(Self::Tvdb),
            "fankai" => positive(value).map(Self::Fankai),
            "imdb" => value
                .strip_prefix("tt")
                .filter(|digits| digits_only(digits, 12))
                .map(|_| Self::Imdb(value.to_string())),
            // The catalogue's ids are UUIDs: hexadecimal and hyphens, none
            // of which a season's or an episode's mark can be taken for.
            "id" => ((1..=64).contains(&value.len())
                && value.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
            .then(|| Self::Held(value.to_string())),
            _ => None,
        }
    }
}

impl fmt::Display for Work {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tmdb(id) => write!(f, "tmdb-{id}"),
            Self::Tvdb(id) => write!(f, "tvdb-{id}"),
            Self::Imdb(id) => write!(f, "imdb-{id}"),
            Self::Fankai(id) => write!(f, "fankai-{id}"),
            Self::Held(id) => write!(f, "id-{id}"),
        }
    }
}

/// What a ratingKey names: a film, a series, or one of a series' seasons or
/// episodes as one of its orders numbers them — the aired one when the key
/// names none. Plex takes a ratingKey in ASCII letters, digits, `-` and `_`,
/// and makes the item's GUID of it, `{scheme}://{type}/{ratingKey}`
/// (developer.plex.tv/pms, Metadata Providers › Metadata); it is all a
/// refresh asks again by, so it says everything the answer needs.
///
/// `movie-tmdb-603`, `movie-id-…` for an entry made by hand,
/// `show-tvdb-81189`, `season-tvdb-81189-s1`, `episode-tvdb-81189-s1e3`, and
/// in an order of TheTVDB's, `season-tvdb-81189-s1-dvd`,
/// `episode-tvdb-81189-s1e3-dvd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Movie(Work),
    Show(Work),
    Season {
        show: Work,
        number: i32,
        order: Option<String>,
    },
    Episode {
        show: Work,
        season: i32,
        number: i32,
        order: Option<String>,
    },
}

impl Key {
    pub fn parse(raw: &str) -> Option<Self> {
        let (kind, rest) = raw.trim().split_once('-')?;
        match kind {
            "movie" => Work::parse(rest)
                .filter(|work| matches!(work, Work::Tmdb(_) | Work::Imdb(_) | Work::Held(_)))
                .map(Self::Movie),
            "show" => Work::parse(rest)
                .filter(|work| !matches!(work, Work::Imdb(_)))
                .map(Self::Show),
            "season" => {
                let (show, mark, order) = placed(rest)?;
                let number = mark
                    .strip_prefix('s')
                    .filter(|n| digits_only(n, 4))?
                    .parse()
                    .ok()?;
                Some(Self::Season {
                    show,
                    number,
                    order,
                })
            }
            "episode" => {
                let (show, mark, order) = placed(rest)?;
                let (season, number) = mark.strip_prefix('s')?.split_once('e')?;
                if !digits_only(season, 4) || !digits_only(number, 5) {
                    return None;
                }
                Some(Self::Episode {
                    show,
                    season: season.parse().ok()?,
                    number: number.parse().ok()?,
                    order,
                })
            }
            _ => None,
        }
    }

    /// `movie`, `show`, `season` or `episode`: the type the GUID names.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Movie(_) => "movie",
            Self::Show(_) => "show",
            Self::Season { .. } => "season",
            Self::Episode { .. } => "episode",
        }
    }

    /// The work the key belongs to: the film, or the series.
    pub fn work(&self) -> &Work {
        match self {
            Self::Movie(work) | Self::Show(work) => work,
            Self::Season { show, .. } | Self::Episode { show, .. } => show,
        }
    }

    pub fn guid(&self) -> String {
        format!("{IDENTIFIER}://{}/{self}", self.kind())
    }

    /// One of this provider's GUIDs, read back.
    pub fn from_guid(guid: &str) -> Option<Self> {
        let rest = guid.trim().strip_prefix(IDENTIFIER)?.strip_prefix("://")?;
        let (kind, key) = rest.split_once('/')?;
        let key = Self::parse(key)?;
        (key.kind() == kind).then_some(key)
    }

    /// Where Plex asks for the item: a series and a season by their
    /// children, as Plex's examples address them (`docs/Metadata.md`).
    pub fn path(&self) -> String {
        match self {
            Self::Show(_) | Self::Season { .. } => format!("{METADATA_PATH}/{self}/children"),
            _ => self.plain_path(),
        }
    }

    /// Where the item is, as its children name their parent:
    /// `parentKey`, `grandparentKey` (`TMDBMapper.ts`, `constructMetadataKey`).
    pub fn plain_path(&self) -> String {
        format!("{METADATA_PATH}/{self}")
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let order = |order: &Option<String>| {
            order
                .as_deref()
                .map(|kind| format!("-{kind}"))
                .unwrap_or_default()
        };
        match self {
            Self::Movie(work) => write!(f, "movie-{work}"),
            Self::Show(work) => write!(f, "show-{work}"),
            Self::Season {
                show,
                number,
                order: kind,
            } => write!(f, "season-{show}-s{number}{}", order(kind)),
            Self::Episode {
                show,
                season,
                number,
                order: kind,
            } => write!(f, "episode-{show}-s{season}e{number}{}", order(kind)),
        }
    }
}

/// A season's or an episode's key, after its kind: the series, the mark of
/// the place — `s1`, `s1e3` — and the order, when one is named after it.
/// Read from the right, since the catalogue's own ids hold hyphens.
fn placed(rest: &str) -> Option<(Work, &str, Option<String>)> {
    let is_mark = |part: &str| {
        part.strip_prefix('s')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit() || b == b'e'))
    };
    let (head, last) = rest.rsplit_once('-')?;
    let (work, mark, order) = if is_mark(last) {
        (head, last, None)
    } else {
        let (work, mark) = head.rsplit_once('-')?;
        if !is_mark(mark) || !is_order(last) {
            return None;
        }
        (work, mark, Some(last.to_string()))
    };
    let order = order.filter(|kind| kind != AIRED);
    Some((Work::parse(work)?, mark, order))
}

/// Whether a kind of order can be written into a key: lower-case letters and
/// digits, as TheTVDB names its orders — `dvd`, `altdvd`, `alttwo`.
pub fn is_order(kind: &str) -> bool {
    (1..=32).contains(&kind.len())
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn positive(text: &str) -> Option<i64> {
    digits_only(text, 18)
        .then(|| text.parse().ok())
        .flatten()
        .filter(|n: &i64| *n > 0)
}

fn digits_only(text: &str, most: usize) -> bool {
    (1..=most).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
}

/// An id Plex gives for what it asks to match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hint {
    /// One of this provider's own GUIDs: an item Plex matched here before.
    Own(Key),
    Imdb(String),
    Tmdb(i64),
    Tvdb(i64),
}

impl Hint {
    /// One of Plex's GUIDs, read: this provider's own, or another's id —
    /// `imdb://tt0133093`, `tmdb://603`, `tvdb://81189` as Plex writes them
    /// now (`docs/Metadata.md`, `Guid`), and
    /// `com.plexapp.agents.imdb://tt0133093?lang=en`,
    /// `com.plexapp.agents.themoviedb://603`,
    /// `com.plexapp.agents.thetvdb://81189/1/3` as its retired agents wrote
    /// them, which an item they matched still carries. Plex's own,
    /// `plex://movie/…`, names nothing this server knows.
    pub fn parse(guid: &str) -> Option<Self> {
        let guid = guid.trim();
        if let Some(key) = Key::from_guid(guid) {
            return Some(Self::Own(key));
        }
        let (scheme, rest) = guid.split_once("://")?;
        let source = scheme.rsplit('.').next()?.to_ascii_lowercase();
        let id = rest.split(['?', '/']).next()?.trim();
        match source.as_str() {
            "imdb" => normalize_imdb_id(id).map(Self::Imdb),
            "tmdb" | "themoviedb" => positive(id).map(Self::Tmdb),
            "tvdb" | "thetvdb" => positive(id).map(Self::Tvdb),
            _ => None,
        }
    }
}

// ─── drawing an answer ───────────────────────────────────────────────────────

/// What an answer is drawn for.
pub struct Drawn<'a> {
    /// Where this server is reached — its public address, or the host the
    /// request came to — for the addresses of the pictures it keeps itself:
    /// Plex fetches every picture on its own, and a path alone leads it
    /// nowhere.
    pub origin: &'a str,
    /// Three letters: the language of the few words this server writes
    /// itself, a season's or an episode's name where nobody gave one.
    pub language: &'a str,
    /// ISO 3166-1 alpha-2: the country whose rating, and whose release day,
    /// a work is given.
    pub country: Option<&'a str>,
    /// Whether the text is served in the language the catalogue is stored
    /// in. Only then is the tagline, which no translation holds, said.
    pub stored_language: bool,
}

/// A film, as Plex reads one.
pub fn movie(item: &MediaItem, work: &Work, drawn: &Drawn<'_>) -> PlexMetadata {
    let key = Key::Movie(work.clone());
    let pictures = pictures(&item.images, &item.primary_images, &item.title, drawn);
    let released = release_day(item, drawn.country);
    let people = people(item, drawn);

    PlexMetadata {
        rating_key: key.to_string(),
        key: key.path(),
        guid: key.guid(),
        kind: key.kind().to_string(),
        title: item.title.clone(),
        year: item.year.or_else(|| year_of(released.as_deref())),
        originally_available_at: released.unwrap_or_default(),
        thumb: pictures.thumb,
        art: pictures.art,
        images: pictures.images,
        content_rating: content_rating(item, drawn.country),
        original_title: original_title(item),
        title_sort: text(item.sort_title.as_deref()),
        summary: text(item.overview.as_deref()),
        is_adult: Some(item.is_adult),
        duration: milliseconds(item.runtime),
        tagline: drawn
            .stored_language
            .then(|| text(item.tagline.as_deref()))
            .flatten(),
        studio: text(item.studio.as_deref()),
        theme: theme(item, drawn),
        genres: genres(item),
        guids: guids(
            item.external_ids.imdb.as_deref(),
            item.external_ids.tmdb,
            item.external_ids.tvdb,
        ),
        roles: people.roles,
        directors: people.directors,
        producers: people.producers,
        writers: people.writers,
        studios: studios(item),
        ratings: ratings(item),
        ..Default::default()
    }
}

/// A series' seasons and episodes as one order lays them out.
pub struct Layout<'a> {
    /// The order's kind, as the keys of its seasons and episodes carry it;
    /// none for the aired order.
    pub order: Option<String>,
    /// Whether the seasons are another order's rather than the work's own:
    /// such a season holds what the order places in it, and has no name or
    /// overview of its own here — TheTVDB keeps them, the catalogue does not.
    pub placed: bool,
    pub seasons: Vec<Laid<'a>>,
}

/// One season of a [`Layout`].
pub struct Laid<'a> {
    pub number: i32,
    /// The aired season of this number: the season itself in the aired
    /// order; in another, the one that lends it its pictures.
    pub season: Option<&'a Season>,
    /// Its episodes, by their number in the order.
    pub episodes: Vec<(i32, &'a Episode)>,
}

impl<'a> Layout<'a> {
    /// The work laid out in the order a client named by kind: its own for
    /// the aired order, or when none is named; the one of that kind the
    /// catalogue keeps for it otherwise — and not a season at all where it
    /// keeps none, or one in name only (see [`orders::in_use`]): Plex asks
    /// that no season be given for an order a series has not
    /// (developer.plex.tv/pms, Metadata Providers › `episodeOrder`).
    pub fn asked(item: &'a MediaItem, orders: &'a [EpisodeOrder], kind: Option<&str>) -> Self {
        let kind = kind
            .map(|k| k.trim().to_ascii_lowercase())
            .filter(|k| !k.is_empty() && k != AIRED);
        let Some(kind) = kind else {
            return Self::aired(item);
        };
        match orders::in_use(item, orders, &kind) {
            Some(order) if is_order(&kind) => Self::placed(item, order, Some(kind)),
            _ => Self {
                order: None,
                placed: true,
                seasons: Vec::new(),
            },
        }
    }

    /// The seasons and episodes as they aired: every season number once,
    /// whatever the seasons were merged from, and a season the episodes name
    /// that no season row does listed too, bare.
    fn aired(item: &'a MediaItem) -> Self {
        let mut seasons: BTreeMap<i32, Laid<'a>> = BTreeMap::new();
        for season in &item.seasons {
            seasons.entry(season.season_number).or_insert_with(|| Laid {
                number: season.season_number,
                season: Some(season),
                episodes: Vec::new(),
            });
        }
        for episode in &item.episodes {
            seasons
                .entry(episode.season_number)
                .or_insert_with(|| Laid {
                    number: episode.season_number,
                    season: None,
                    episodes: Vec::new(),
                })
                .episodes
                .push((episode.episode_number, episode));
        }
        Self::sorted(None, false, seasons)
    }

    /// The same episodes as another order places them: by TVDB id, under
    /// that order's seasons and numbers — where they are is worked out once,
    /// for every client, in [`orders::placements`].
    fn placed(item: &'a MediaItem, order: &'a EpisodeOrder, kind: Option<String>) -> Self {
        let mut aired: HashMap<i32, &'a Season> = HashMap::new();
        for season in &item.seasons {
            aired.entry(season.season_number).or_insert(season);
        }
        let mut seasons: BTreeMap<i32, Laid<'a>> = BTreeMap::new();
        for (place, episode) in orders::placements(item, order) {
            seasons
                .entry(place.season_number)
                .or_insert_with(|| Laid {
                    number: place.season_number,
                    season: aired.get(&place.season_number).copied(),
                    episodes: Vec::new(),
                })
                .episodes
                .push((place.episode_number, episode));
        }
        Self::sorted(kind, true, seasons)
    }

    fn sorted(order: Option<String>, placed: bool, seasons: BTreeMap<i32, Laid<'a>>) -> Self {
        let mut seasons: Vec<Laid<'a>> = seasons.into_values().collect();
        for laid in &mut seasons {
            laid.episodes.sort_by_key(|(number, _)| *number);
        }
        Self {
            order,
            placed,
            seasons,
        }
    }

    pub fn season(&self, number: i32) -> Option<&Laid<'a>> {
        self.seasons.iter().find(|laid| laid.number == number)
    }
}

impl Laid<'_> {
    pub fn episode(&self, number: i32) -> Option<&Episode> {
        self.episodes
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, episode)| *episode)
    }
}

/// What a child says of its parent, and a season of itself.
#[derive(Clone)]
struct Line {
    rating_key: String,
    /// Where the item is asked for.
    path: String,
    /// Where its children say it is.
    plain: String,
    guid: String,
    title: String,
    thumb: Option<String>,
    art: Option<String>,
}

/// A series, ready to be drawn as Plex reads it: itself, its seasons, and
/// their episodes, all laid out in the one order.
pub struct Series<'a> {
    item: &'a MediaItem,
    work: &'a Work,
    layout: &'a Layout<'a>,
    drawn: &'a Drawn<'a>,
    words: Words,
    rating: Option<String>,
    show: Line,
    pictures: Pictures,
}

impl<'a> Series<'a> {
    pub fn new(
        item: &'a MediaItem,
        work: &'a Work,
        layout: &'a Layout<'a>,
        drawn: &'a Drawn<'a>,
    ) -> Self {
        let key = Key::Show(work.clone());
        let pictures = pictures(&item.images, &item.primary_images, &item.title, drawn);
        Self {
            item,
            work,
            layout,
            drawn,
            words: words(drawn.language),
            rating: content_rating(item, drawn.country),
            show: Line {
                rating_key: key.to_string(),
                path: key.path(),
                plain: key.plain_path(),
                guid: key.guid(),
                title: item.title.clone(),
                thumb: pictures.thumb.clone(),
                art: pictures.art.clone(),
            },
            pictures,
        }
    }

    /// The series, with the orders it may be asked in as `SeasonType` —
    /// given the catalogue's `orders` for it — and with its seasons as
    /// `Children` when asked.
    pub fn show(&self, orders: &[EpisodeOrder], with_children: bool) -> PlexMetadata {
        let item = self.item;
        let key = Key::Show(self.work.clone());
        let aired = day(item.first_aired.as_deref());
        let people = people(item, self.drawn);

        PlexMetadata {
            rating_key: key.to_string(),
            key: key.path(),
            guid: key.guid(),
            kind: key.kind().to_string(),
            title: item.title.clone(),
            year: item.year.or_else(|| year_of(aired.as_deref())),
            originally_available_at: aired.unwrap_or_default(),
            thumb: self.pictures.thumb.clone(),
            art: self.pictures.art.clone(),
            images: self.pictures.images.clone(),
            content_rating: self.rating.clone(),
            original_title: original_title(item),
            title_sort: text(item.sort_title.as_deref()),
            summary: text(item.overview.as_deref()),
            is_adult: Some(item.is_adult),
            duration: milliseconds(item.runtime),
            tagline: self
                .drawn
                .stored_language
                .then(|| text(item.tagline.as_deref()))
                .flatten(),
            studio: text(item.studio.as_deref()),
            theme: theme(item, self.drawn),
            genres: genres(item),
            guids: guids(
                item.external_ids.imdb.as_deref(),
                item.external_ids.tmdb,
                item.external_ids.tvdb,
            ),
            roles: people.roles,
            directors: people.directors,
            producers: people.producers,
            writers: people.writers,
            studios: studios(item),
            networks: text(item.network.as_deref())
                .map(|tag| vec![PlexTag { tag }])
                .unwrap_or_default(),
            ratings: ratings(item),
            season_types: season_types(item, orders),
            children: with_children.then(|| {
                let metadata = self.seasons(0..self.layout.seasons.len());
                PlexChildren {
                    size: metadata.len(),
                    metadata,
                }
            }),
            ..Default::default()
        }
    }

    pub fn season_count(&self) -> usize {
        self.layout.seasons.len()
    }

    /// The seasons `range` covers, without their episodes.
    pub fn seasons(&self, range: Range<usize>) -> Vec<PlexMetadata> {
        self.layout
            .seasons
            .get(range)
            .unwrap_or_default()
            .iter()
            .map(|laid| self.season(laid, false))
            .collect()
    }

    /// A season, with its episodes as `Children` when asked.
    pub fn season(&self, laid: &Laid<'_>, with_children: bool) -> PlexMetadata {
        let line = self.season_line(laid);
        // Its own in the aired order; another order's has none here.
        let own = laid.season.filter(|_| !self.layout.placed);
        let day = own
            .and_then(|season| day(season.air_date.as_deref()))
            .or_else(|| first_day(laid));
        let pictures = laid
            .season
            .map(|season| {
                pictures(
                    &season.images,
                    &season.primary_images,
                    &line.title,
                    self.drawn,
                )
            })
            .unwrap_or_default();

        PlexMetadata {
            rating_key: line.rating_key.clone(),
            key: line.path.clone(),
            guid: line.guid.clone(),
            kind: "season".to_string(),
            title: line.title.clone(),
            year: year_of(day.as_deref()),
            originally_available_at: day.unwrap_or_default(),
            thumb: pictures.thumb,
            art: self.show.art.clone(),
            images: pictures.images,
            content_rating: self.rating.clone(),
            summary: own.and_then(|season| text(season.overview.as_deref())),
            parent_rating_key: Some(self.show.rating_key.clone()),
            parent_key: Some(self.show.plain.clone()),
            parent_guid: Some(self.show.guid.clone()),
            parent_type: Some("show".to_string()),
            parent_title: Some(self.show.title.clone()),
            parent_thumb: self.show.thumb.clone(),
            parent_art: self.show.art.clone(),
            index: Some(laid.number),
            guids: own
                .map(|season| guids(None, season.tmdb_id, season.tvdb_id))
                .unwrap_or_default(),
            children: with_children.then(|| {
                let metadata = self.episodes_of(laid, 0..laid.episodes.len());
                PlexChildren {
                    size: metadata.len(),
                    metadata,
                }
            }),
            ..Default::default()
        }
    }

    /// The episodes of a season that `range` covers.
    pub fn episodes_of(&self, laid: &Laid<'_>, range: Range<usize>) -> Vec<PlexMetadata> {
        let line = self.season_line(laid);
        laid.episodes
            .get(range)
            .unwrap_or_default()
            .iter()
            .map(|(number, episode)| self.episode_in(laid, &line, *number, episode))
            .collect()
    }

    /// An episode, numbered as its season of the layout numbers it.
    pub fn episode(&self, laid: &Laid<'_>, number: i32, episode: &Episode) -> PlexMetadata {
        self.episode_in(laid, &self.season_line(laid), number, episode)
    }

    /// How many episodes the series has in the layout, every season counted.
    pub fn episode_count(&self) -> usize {
        self.layout
            .seasons
            .iter()
            .map(|laid| laid.episodes.len())
            .sum()
    }

    /// The episodes of the whole series, season after season — its
    /// grandchildren — that `range` covers; only theirs are drawn.
    pub fn episodes(&self, range: Range<usize>) -> Vec<PlexMetadata> {
        let mut drawn = Vec::with_capacity(range.len());
        let mut before = 0;
        for laid in &self.layout.seasons {
            let count = laid.episodes.len();
            let from = range.start.max(before);
            let to = range.end.min(before + count);
            if from < to {
                drawn.extend(self.episodes_of(laid, from - before..to - before));
            }
            before += count;
            if before >= range.end {
                break;
            }
        }
        drawn
    }

    fn episode_in(
        &self,
        laid: &Laid<'_>,
        season: &Line,
        number: i32,
        episode: &Episode,
    ) -> PlexMetadata {
        let key = Key::Episode {
            show: self.work.clone(),
            season: laid.number,
            number,
            order: self.layout.order.clone(),
        };
        let title = text(Some(&episode.title))
            .unwrap_or_else(|| format!("{} {number}", self.words.episode));
        let aired = day(episode.air_date.as_deref());
        let thumb = episode
            .image
            .as_deref()
            .map(|url| absolute(self.drawn.origin, url));

        PlexMetadata {
            rating_key: key.to_string(),
            key: key.path(),
            guid: key.guid(),
            kind: key.kind().to_string(),
            year: year_of(aired.as_deref()),
            originally_available_at: aired.unwrap_or_default(),
            summary: text(episode.overview.as_deref()),
            images: thumb
                .iter()
                .map(|url| PlexImage {
                    kind: "snapshot".to_string(),
                    url: url.clone(),
                    alt: Some(title.clone()),
                })
                .collect(),
            thumb,
            title,
            duration: milliseconds(episode.runtime.or(self.item.runtime)),
            content_rating: self.rating.clone(),
            index: Some(number),
            parent_index: Some(laid.number),
            parent_rating_key: Some(season.rating_key.clone()),
            parent_key: Some(season.plain.clone()),
            parent_guid: Some(season.guid.clone()),
            parent_type: Some("season".to_string()),
            parent_title: Some(season.title.clone()),
            parent_thumb: season.thumb.clone(),
            parent_art: self.show.art.clone(),
            grandparent_rating_key: Some(self.show.rating_key.clone()),
            grandparent_key: Some(self.show.plain.clone()),
            grandparent_guid: Some(self.show.guid.clone()),
            grandparent_type: Some("show".to_string()),
            grandparent_title: Some(self.show.title.clone()),
            grandparent_thumb: self.show.thumb.clone(),
            grandparent_art: self.show.art.clone(),
            guids: guids(None, episode.tmdb_id, episode.tvdb_id),
            ..Default::default()
        }
    }

    fn season_line(&self, laid: &Laid<'_>) -> Line {
        let key = Key::Season {
            show: self.work.clone(),
            number: laid.number,
            order: self.layout.order.clone(),
        };
        // A season's own name, where it has one beyond its number — the
        // test Sonarr's surface applies (`own_season_title`) — else the
        // words for it in the language asked: TMDB's "Season 2" was stored
        // in the catalogue's language, whichever Plex is in.
        let named = laid
            .season
            .filter(|_| !self.layout.placed)
            .and_then(|season| {
                super::sonarr_v2::own_season_title(season.title.as_deref(), laid.number)
            });
        let title = named.unwrap_or_else(|| self.words.season(laid.number));
        let thumb = laid.season.and_then(|season| {
            lead_images(&season.images, &season.primary_images)
                .into_iter()
                .find(|image| image.cover_type == CoverType::Poster)
                .map(|image| absolute(self.drawn.origin, &image.url))
        });
        Line {
            rating_key: key.to_string(),
            path: key.path(),
            plain: key.plain_path(),
            guid: key.guid(),
            title,
            thumb,
            art: self.show.art.clone(),
        }
    }
}

/// An item's pictures as `/images` lists them: of each kind, the one the
/// item leads with first — a person's choice, else one added by hand, else
/// the first a source offered — then the rest, in the language asked, then
/// in none, then the others.
pub fn images_of(
    images: &[Image],
    chosen: &PrimaryImages,
    alt: &str,
    drawn: &Drawn<'_>,
) -> Vec<PlexImage> {
    let leads: HashSet<&str> = lead_images(images, chosen)
        .into_iter()
        .map(|image| image.id.as_str())
        .collect();
    let mut listed: Vec<&Image> = images
        .iter()
        .filter(|image| image_kind(image.cover_type).is_some())
        .collect();
    listed.sort_by_key(|image| {
        let spoken = match image.language.as_deref().map(language::normalize) {
            Some(language) if language == drawn.language => 0,
            None => 1,
            Some(_) => 2,
        };
        (
            image.cover_type.priority(),
            !leads.contains(image.id.as_str()),
            spoken,
        )
    });
    listed
        .into_iter()
        .filter_map(|image| plex_image(image, alt, drawn))
        .collect()
}

/// A still, the one picture an episode has.
pub fn snapshot(episode: &Episode, alt: &str, drawn: &Drawn<'_>) -> Vec<PlexImage> {
    episode
        .image
        .as_deref()
        .map(|url| PlexImage {
            kind: "snapshot".to_string(),
            url: absolute(drawn.origin, url),
            alt: Some(alt.to_string()),
        })
        .into_iter()
        .collect()
}

// ─── the parts ───────────────────────────────────────────────────────────────

/// What an item shows of its pictures: the poster as `thumb`, the
/// background as `art`, and one of each kind Plex has a word for as `Image`.
#[derive(Clone, Default)]
struct Pictures {
    thumb: Option<String>,
    art: Option<String>,
    images: Vec<PlexImage>,
}

fn pictures(images: &[Image], chosen: &PrimaryImages, alt: &str, drawn: &Drawn<'_>) -> Pictures {
    let leads = lead_images(images, chosen);
    let address = |kind: CoverType| {
        leads
            .iter()
            .find(|image| image.cover_type == kind)
            .map(|image| absolute(drawn.origin, &image.url))
    };
    Pictures {
        thumb: address(CoverType::Poster),
        art: address(CoverType::Fanart),
        images: leads
            .iter()
            .filter_map(|image| plex_image(image, alt, drawn))
            .collect(),
    }
}

fn plex_image(image: &Image, alt: &str, drawn: &Drawn<'_>) -> Option<PlexImage> {
    Some(PlexImage {
        kind: image_kind(image.cover_type)?.to_string(),
        url: absolute(drawn.origin, &image.url),
        alt: Some(alt.to_string()),
    })
}

/// Plex's word for a kind of picture (`src/models/Metadata.ts`,
/// `Image.type`). The kinds it has none for — banners, clear art,
/// landscapes — are not sent.
fn image_kind(kind: CoverType) -> Option<&'static str> {
    match kind {
        CoverType::Poster => Some("coverPoster"),
        CoverType::Fanart => Some("background"),
        CoverType::Clearlogo => Some("clearLogo"),
        CoverType::Screenshot => Some("snapshot"),
        _ => None,
    }
}

/// An address Plex can follow: one of this server's own pictures, kept and
/// served under `/media/`, under the address it is reached at; any other as
/// it is.
fn absolute(origin: &str, url: &str) -> String {
    if url.starts_with('/') && !url.starts_with("//") {
        format!("{}{url}", origin.trim_end_matches('/'))
    } else {
        url.to_string()
    }
}

fn theme(item: &MediaItem, drawn: &Drawn<'_>) -> Option<String> {
    text(item.theme_music.as_deref()).map(|url| absolute(drawn.origin, &url))
}

/// The cast and the crew, in the order the catalogue keeps them: the actors
/// numbered from 1, as Plex orders a cast; a crew member's job as their role.
struct People {
    roles: Vec<PlexPerson>,
    directors: Vec<PlexPerson>,
    producers: Vec<PlexPerson>,
    writers: Vec<PlexPerson>,
}

fn people(item: &MediaItem, drawn: &Drawn<'_>) -> People {
    let mut credits: Vec<_> = item.credits.iter().collect();
    credits.sort_by_key(|credit| credit.sort_order);

    let mut people = People {
        roles: Vec::new(),
        directors: Vec::new(),
        producers: Vec::new(),
        writers: Vec::new(),
    };
    for credit in credits {
        let Some(tag) = text(Some(&credit.person_name)) else {
            continue;
        };
        let person = |order: Option<usize>| PlexPerson {
            tag: tag.clone(),
            thumb: credit
                .image
                .as_deref()
                .map(|url| absolute(drawn.origin, url)),
            role: text(credit.character_name.as_deref()),
            order,
        };
        match credit.credit_type {
            CreditType::Actor => {
                let order = people.roles.len() + 1;
                people.roles.push(person(Some(order)));
            }
            CreditType::Director => people.directors.push(person(None)),
            CreditType::Producer => people.producers.push(person(None)),
            CreditType::Writer => people.writers.push(person(None)),
            // A series' guest stars are no part of its cast, and the
            // catalogue keeps no episode's own.
            CreditType::Guest => {}
        }
    }
    people
}

/// The genres, each once: TMDB's combined ones as their parts, so a film's
/// "Action" and a series' "Action & Adventure" are one genre in Plex.
fn genres(item: &MediaItem) -> Vec<PlexTag> {
    let mut seen = HashSet::new();
    item.genres
        .iter()
        .flat_map(|genre| genre_parts(genre))
        .filter(|genre| seen.insert(genre.to_lowercase()))
        .map(|tag| PlexTag { tag })
        .collect()
}

/// The companies that made it, or its studio where only that is known.
fn studios(item: &MediaItem) -> Vec<PlexTag> {
    let mut named: Vec<String> = item
        .production_companies
        .iter()
        .filter_map(|company| text(Some(company)))
        .collect();
    if named.is_empty() {
        named.extend(text(item.studio.as_deref()));
    }
    named.into_iter().map(|tag| PlexTag { tag }).collect()
}

/// The other providers' ids, as Plex writes them (`docs/Metadata.md`,
/// `Guid`): IMDb's, TMDB's, TheTVDB's.
fn guids(imdb: Option<&str>, tmdb: Option<i64>, tvdb: Option<i64>) -> Vec<PlexGuid> {
    imdb.map(|id| format!("imdb://{id}"))
        .into_iter()
        .chain(tmdb.map(|id| format!("tmdb://{id}")))
        .chain(tvdb.map(|id| format!("tvdb://{id}")))
        .map(|id| PlexGuid { id })
        .collect()
}

/// The marks Plex can show, IMDb's and TMDB's, under the only pictures it
/// takes for them (`docs/Metadata.md`, `Rating`: no other is supported).
/// Rotten Tomatoes' are left out: Plex takes them as `ripe` or `upright`
/// alone, which would show a rotten score as fresh.
fn ratings(item: &MediaItem) -> Vec<PlexRating> {
    item.ratings
        .iter()
        .filter_map(|rating| {
            let image = match rating.source.as_str() {
                "imdb" => "imdb://image.rating",
                "tmdb" => "themoviedb://image.rating",
                _ => return None,
            };
            let value = rating.value.filter(|v| *v > 0.0 && *v <= 10.0)?;
            Some(PlexRating {
                image: image.to_string(),
                kind: "audience".to_string(),
                value,
            })
        })
        .collect()
}

/// The orders a series may be asked in, the aired one first, then each
/// other order the catalogue keeps that places an episode the work still
/// has — the same list Sonarr v5 is offered.
fn season_types(item: &MediaItem, orders: &[EpisodeOrder]) -> Vec<PlexSeasonType> {
    let ids = &item.external_ids;
    let source = if ids.tvdb.is_some() {
        "tvdb"
    } else if ids.tmdb.is_some() {
        "tmdb"
    } else if ids.fankai.is_some() {
        "fankai"
    } else {
        "manual"
    };
    let mut types = vec![PlexSeasonType {
        id: AIRED.to_string(),
        source: source.to_string(),
        tag: "Aired".to_string(),
        title: "Aired order".to_string(),
    }];
    for order in orders {
        let id = order.kind.to_ascii_lowercase();
        if !is_order(&id) || id == AIRED || orders::placements(item, order).is_empty() {
            continue;
        }
        let tag = match id.as_str() {
            "dvd" => "DVD".to_string(),
            "absolute" => "Absolute".to_string(),
            "alternate" => "Alternate".to_string(),
            "regional" => "Regional".to_string(),
            "altdvd" => "Alternate DVD".to_string(),
            other => super::sonarr::capitalize(other),
        };
        types.push(PlexSeasonType {
            title: format!("TheTVDB ({tag} order)"),
            id,
            source: "tvdb".to_string(),
            tag,
        });
    }
    types
}

/// The rating a work carries in the country asked, as Plex writes a
/// country's (`docs/Metadata.md`, `contentRating`): the United States' plain,
/// `PG-13`; any other's after its code in lower case, `de/12`. The one the
/// catalogue holds, under its own country, where the country asked has none.
fn content_rating(item: &MediaItem, country: Option<&str>) -> Option<String> {
    let found = country.and_then(|c| {
        item.certification_in(c)
            .map(|rating| (c.to_string(), rating.to_string()))
    });
    let (country, rating) = match found {
        Some(found) => found,
        None => (
            item.content_rating_country
                .clone()
                .unwrap_or_else(|| "US".to_string()),
            text(item.content_rating.as_deref())?,
        ),
    };
    Some(if country.eq_ignore_ascii_case("US") {
        rating
    } else {
        format!("{}/{rating}", country.to_ascii_lowercase())
    })
}

/// The day a film came out in the country asked — in cinemas, else online,
/// else on disc — or, where its releases there are not known, the day the
/// catalogue holds.
///
/// A date somebody fixed by hand is the film's in every country, as the site
/// reads a film's days by country (`in_country`, in `api/native/browse.rs`):
/// it stands where the country's releases would give one, and no day they
/// give comes in its place.
fn release_day(item: &MediaItem, country: Option<&str>) -> Option<String> {
    let own = || {
        day(item.in_cinemas.as_deref())
            .or_else(|| day(item.digital_release.as_deref()))
            .or_else(|| day(item.physical_release.as_deref()))
    };
    let Some(country) = country else {
        return own();
    };

    let there = item.days_in(country);
    // One kind's day: what was fixed, if it was; else what the country gives.
    let of_kind = |field: &str, fixed: &Option<String>, given: Option<String>| {
        if item.is_locked(field) {
            day(fixed.as_deref())
        } else {
            given
        }
    };
    of_kind("inCinemas", &item.in_cinemas, there.cinema)
        .or_else(|| of_kind("digitalRelease", &item.digital_release, there.digital))
        .or_else(|| of_kind("physicalRelease", &item.physical_release, there.physical))
        .or_else(own)
}

/// The day an episode of the season first aired.
fn first_day(laid: &Laid<'_>) -> Option<String> {
    laid.episodes
        .iter()
        .filter_map(|(_, episode)| day(episode.air_date.as_deref()))
        .min()
}

/// A day as Plex takes one, `YYYY-MM-DD`, from a day or an instant.
fn day(value: Option<&str>) -> Option<String> {
    super::sonarr::sonarr_day(value)
}

fn year_of(day: Option<&str>) -> Option<i32> {
    day?.get(0..4)?.parse().ok()
}

fn milliseconds(minutes: Option<i32>) -> Option<i64> {
    minutes.filter(|m| *m > 0).map(|m| i64::from(m) * 60 * 1000)
}

/// The title in the work's own language, where it is another than the one
/// served.
fn original_title(item: &MediaItem) -> Option<String> {
    text(item.original_title.as_deref()).filter(|original| *original != item.title)
}

fn text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// The words this server writes where nobody named a season or an episode,
/// in the language asked among the few it knows, else in English.
struct Words {
    season: &'static str,
    specials: &'static str,
    episode: &'static str,
}

impl Words {
    fn season(&self, number: i32) -> String {
        if number == 0 {
            self.specials.to_string()
        } else {
            format!("{} {number}", self.season)
        }
    }
}

fn words(language: &str) -> Words {
    let (season, specials, episode) = match language {
        "fra" => ("Saison", "Épisodes spéciaux", "Épisode"),
        "deu" => ("Staffel", "Specials", "Folge"),
        "spa" => ("Temporada", "Especiales", "Episodio"),
        "ita" => ("Stagione", "Speciali", "Episodio"),
        "por" => ("Temporada", "Especiais", "Episódio"),
        "nld" => ("Seizoen", "Specials", "Aflevering"),
        _ => ("Season", "Specials", "Episode"),
    };
    Words {
        season,
        specials,
        episode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::repo::child::{blank_episode, blank_season},
        domain::{Credit, ExternalIds, PlacedEpisode, Rating},
    };

    fn drawn(language: &str) -> Drawn<'_> {
        Drawn {
            origin: "http://ams.lan:8479",
            language,
            country: None,
            stored_language: true,
        }
    }

    fn image(id: &str, kind: CoverType, url: &str, language: Option<&str>) -> Image {
        Image {
            id: id.into(),
            season_number: None,
            cover_type: kind,
            url: url.into(),
            language: language.map(str::to_string),
            sort_order: 0,
            source: None,
            is_manual: false,
        }
    }

    fn series() -> MediaItem {
        let mut it = MediaItem::empty(MediaKind::Series);
        it.title = "Rurouni Kenshin".into();
        it.year = Some(2023);
        it.first_aired = Some("2023-07-07".into());
        it.external_ids = ExternalIds {
            tvdb: Some(416802),
            tmdb: Some(209867),
            ..Default::default()
        };
        let mut first = blank_season(1);
        first.title = Some("東京編".into());
        first.tvdb_id = Some(1971);
        let mut second = blank_season(2);
        second.title = Some("Season 2".into());
        it.seasons = vec![first, second];
        it.episodes = [
            (1, 1, 1001, "2023-07-07"),
            (1, 2, 1002, "2023-07-14"),
            (2, 1, 2001, "2024-10-04"),
        ]
        .into_iter()
        .map(|(season, number, tvdb_id, aired)| {
            let mut episode = blank_episode(season, number);
            episode.title = format!("Episode {tvdb_id}");
            episode.tvdb_id = Some(tvdb_id);
            episode.air_date = Some(aired.into());
            episode
        })
        .collect();
        it
    }

    fn dvd() -> Vec<EpisodeOrder> {
        let place = |tvdb_id, season_number, episode_number| PlacedEpisode {
            tvdb_id,
            season_number,
            episode_number,
            absolute_number: None,
        };
        vec![EpisodeOrder {
            kind: "dvd".into(),
            episodes: vec![place(1002, 1, 1), place(1001, 1, 2), place(2001, 2, 1)],
        }]
    }

    #[test]
    fn a_key_says_what_it_names_and_reads_back() {
        let id = "0192f8a4-1234-7abc-8def-0123456789ab";
        for raw in [
            "movie-tmdb-603".to_string(),
            "movie-imdb-tt0133093".to_string(),
            format!("movie-id-{id}"),
            "show-tvdb-81189".to_string(),
            "show-fankai-12".to_string(),
            "season-tvdb-81189-s0".to_string(),
            format!("season-id-{id}-s2-dvd"),
            "episode-tmdb-1396-s1e3".to_string(),
            format!("episode-id-{id}-s10e120-absolute"),
        ] {
            let key = Key::parse(&raw).unwrap_or_else(|| panic!("{raw} is a key"));
            assert_eq!(key.to_string(), raw);
            assert_eq!(Key::from_guid(&key.guid()), Some(key));
        }
        assert_eq!(
            Key::parse("season-tvdb-81189-s1-dvd"),
            Some(Key::Season {
                show: Work::Tvdb(81189),
                number: 1,
                order: Some("dvd".into())
            })
        );
        assert_eq!(
            Key::parse("episode-tvdb-81189-s1e3"),
            Some(Key::Episode {
                show: Work::Tvdb(81189),
                season: 1,
                number: 3,
                order: None
            })
        );
        // The aired order is the one a key names by naming none.
        assert_eq!(
            Key::parse("season-tvdb-81189-s1-official"),
            Key::parse("season-tvdb-81189-s1")
        );
        assert_eq!(
            Key::Movie(Work::Tmdb(603)).guid(),
            "tv.plex.agents.custom.arrmetadataserver://movie/movie-tmdb-603"
        );
    }

    #[test]
    fn what_is_no_key_of_this_provider_is_not_read_as_one() {
        for raw in [
            "",
            "movie",
            "movie-tmdb-",
            "movie-tmdb-0",
            "movie-tmdb--3",
            "movie-tvdb-81189",
            "show-imdb-tt0903747",
            "season-tvdb-81189",
            "season-tvdb-81189-s",
            "season-tvdb-81189-s1-DVD",
            "season-tvdb-81189-s1-dvd-x",
            "episode-tvdb-81189-s1",
            "episode-tvdb-81189-s1e",
            "movie-id-../../etc",
            "movie-id-zz",
            "5d7768244de0ee001fcc7fed",
        ] {
            assert_eq!(Key::parse(raw), None, "{raw}");
        }
        // Another scheme's GUID, or this one's of another type.
        assert_eq!(
            Key::from_guid("plex://movie/5d7768244de0ee001fcc7fed"),
            None
        );
        assert_eq!(
            Key::from_guid("tv.plex.agents.custom.arrmetadataserver://show/movie-tmdb-603"),
            None
        );
    }

    #[test]
    fn plex_s_guids_are_read_for_the_ids_they_carry() {
        assert_eq!(
            Hint::parse("imdb://tt0133093"),
            Some(Hint::Imdb("tt0133093".into()))
        );
        assert_eq!(Hint::parse("tmdb://603"), Some(Hint::Tmdb(603)));
        assert_eq!(Hint::parse("tvdb://81189"), Some(Hint::Tvdb(81189)));
        // As the retired agents wrote them, which a library matched by one
        // still carries.
        assert_eq!(
            Hint::parse("com.plexapp.agents.imdb://tt0133093?lang=en"),
            Some(Hint::Imdb("tt0133093".into()))
        );
        assert_eq!(
            Hint::parse("com.plexapp.agents.themoviedb://603?lang=en"),
            Some(Hint::Tmdb(603))
        );
        assert_eq!(
            Hint::parse("com.plexapp.agents.thetvdb://81189/1/3?lang=en"),
            Some(Hint::Tvdb(81189))
        );
        assert_eq!(
            Hint::parse(&Key::Show(Work::Tvdb(81189)).guid()),
            Some(Hint::Own(Key::Show(Work::Tvdb(81189))))
        );
        assert_eq!(Hint::parse("plex://movie/5d7768244de0ee001fcc7fed"), None);
        assert_eq!(Hint::parse("tmdb://abc"), None);
    }

    #[test]
    fn the_manifest_is_shaped_as_plex_reads_it() {
        let sent = serde_json::to_value(provider()).unwrap();
        let provider = &sent["MediaProvider"];
        assert_eq!(provider["identifier"], IDENTIFIER);
        assert_eq!(provider["title"], TITLE);
        let types: Vec<i64> = provider["Types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["type"].as_i64().unwrap())
            .collect();
        assert_eq!(types, [1, 2, 3, 4]);
        assert_eq!(provider["Types"][0]["Scheme"][0]["scheme"], IDENTIFIER);
        assert_eq!(provider["Feature"][0]["type"], "metadata");
        assert_eq!(provider["Feature"][0]["key"], "/library/metadata");
        assert_eq!(provider["Feature"][1]["type"], "match");
        assert_eq!(provider["Feature"][1]["key"], "/library/metadata/matches");
    }

    #[test]
    fn a_film_is_drawn_with_its_ids_its_pictures_and_its_country_s_rating() {
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.title = "The Matrix".into();
        film.original_title = Some("The Matrix".into());
        film.year = Some(1999);
        film.in_cinemas = Some("1999-03-31T00:00:00Z".into());
        film.runtime = Some(136);
        film.content_rating = Some("12".into());
        film.content_rating_country = Some("FR".into());
        film.genres = vec!["Action".into(), "Science Fiction".into(), "Action".into()];
        film.external_ids = ExternalIds {
            tmdb: Some(603),
            imdb: Some("tt0133093".into()),
            ..Default::default()
        };
        film.images = vec![
            image(
                "p1",
                CoverType::Poster,
                "https://image.tmdb.org/p1.jpg",
                Some("en"),
            ),
            image("p2", CoverType::Poster, "/media/abc.jpg", None),
            image("b1", CoverType::Banner, "https://x/banner.jpg", None),
        ];
        // The poster a person chose leads, and it is one this server keeps.
        film.primary_images.poster = Some("p2".into());
        film.ratings = vec![
            Rating {
                source: "imdb".into(),
                value: Some(8.7),
                votes: Some(2_000_000),
                rating_type: None,
            },
            Rating {
                source: "metacritic".into(),
                value: Some(7.3),
                votes: None,
                rating_type: None,
            },
        ];
        film.credits = vec![Credit {
            id: "c1".into(),
            credit_type: CreditType::Actor,
            person_name: "Keanu Reeves".into(),
            character_name: Some("Neo".into()),
            image: Some("/media/keanu.jpg".into()),
            tmdb_person_id: Some(6384),
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
        }];

        let drawn = drawn("eng");
        let sent = serde_json::to_value(movie(&film, &Work::of(&film), &drawn)).unwrap();
        assert_eq!(sent["ratingKey"], "movie-tmdb-603");
        assert_eq!(sent["key"], "/library/metadata/movie-tmdb-603");
        assert_eq!(sent["type"], "movie");
        assert_eq!(sent["originallyAvailableAt"], "1999-03-31");
        assert_eq!(sent["year"], 1999);
        assert_eq!(sent["duration"], 136 * 60 * 1000);
        assert_eq!(sent["contentRating"], "fr/12");
        assert!(sent.get("originalTitle").is_none(), "{sent}");
        assert_eq!(sent["thumb"], "http://ams.lan:8479/media/abc.jpg");
        assert_eq!(sent["Image"].as_array().unwrap().len(), 1);
        assert_eq!(sent["Image"][0]["type"], "coverPoster");
        assert_eq!(sent["Genre"].as_array().unwrap().len(), 2);
        assert_eq!(sent["Guid"][0]["id"], "imdb://tt0133093");
        assert_eq!(sent["Guid"][1]["id"], "tmdb://603");
        assert_eq!(sent["Rating"].as_array().unwrap().len(), 1);
        assert_eq!(sent["Rating"][0]["image"], "imdb://image.rating");
        assert_eq!(sent["Role"][0]["tag"], "Keanu Reeves");
        assert_eq!(sent["Role"][0]["role"], "Neo");
        assert_eq!(sent["Role"][0]["order"], 1);
        assert_eq!(
            sent["Role"][0]["thumb"],
            "http://ams.lan:8479/media/keanu.jpg"
        );

        // Every picture for `/images`: the chosen poster first, the banner
        // Plex has no word for left out.
        let listed = images_of(&film.images, &film.primary_images, &film.title, &drawn);
        let urls: Vec<&str> = listed.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "http://ams.lan:8479/media/abc.jpg",
                "https://image.tmdb.org/p1.jpg"
            ]
        );

        // A rating of the United States' is written plain.
        film.content_rating_country = Some("US".into());
        film.content_rating = Some("R".into());
        assert_eq!(content_rating(&film, None).as_deref(), Some("R"));
    }

    #[test]
    fn a_release_day_fixed_by_hand_is_the_film_s_in_every_country() {
        use crate::domain::{CountryRelease, ReleaseKind};

        let release = |country: &str, kind: ReleaseKind, date: &str| CountryRelease {
            country: country.into(),
            release_type: Some(kind),
            date: Some(date.into()),
            certification: None,
            note: None,
            source: None,
        };
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.title = "The Matrix".into();
        film.external_ids.tmdb = Some(603);
        film.in_cinemas = Some("1999-03-31T00:00:00Z".into());
        film.digital_release = Some("2000-06-01".into());
        film.releases = vec![
            release("FR", ReleaseKind::Theatrical, "1999-06-23"),
            release("FR", ReleaseKind::Digital, "2001-03-01"),
        ];
        let drawn_in = |film: &MediaItem, country: &'static str| {
            let drawn = Drawn {
                country: Some(country),
                ..drawn("eng")
            };
            let sent = serde_json::to_value(movie(film, &Work::of(film), &drawn)).unwrap();
            sent["originallyAvailableAt"].as_str().unwrap().to_string()
        };

        // Nothing fixed: the country's day where its releases give one, the
        // catalogue's where they do not, and when no country is asked.
        assert_eq!(
            release_day(&film, Some("FR")).as_deref(),
            Some("1999-06-23")
        );
        assert_eq!(
            release_day(&film, Some("DE")).as_deref(),
            Some("1999-03-31")
        );
        assert_eq!(release_day(&film, None).as_deref(), Some("1999-03-31"));
        assert_eq!(drawn_in(&film, "FR"), "1999-06-23");

        // The day in cinemas fixed by hand is the film's, whichever country
        // is asked: what France's releases say does not come in its place.
        film.in_cinemas = Some("1999-04-01".into());
        film.locked_fields = vec!["item/inCinemas".into()];
        assert_eq!(
            release_day(&film, Some("FR")).as_deref(),
            Some("1999-04-01")
        );
        assert_eq!(
            release_day(&film, Some("DE")).as_deref(),
            Some("1999-04-01")
        );
        assert_eq!(release_day(&film, None).as_deref(), Some("1999-04-01"));
        assert_eq!(drawn_in(&film, "FR"), "1999-04-01");

        // It holds the date it names and no other: the digital day fixed
        // leaves the country's day in cinemas first.
        film.locked_fields = vec!["item/digitalRelease".into()];
        assert_eq!(
            release_day(&film, Some("FR")).as_deref(),
            Some("1999-06-23")
        );

        // Fixed as none, it is none: the next kind the country gives stands,
        // and the day the catalogue holds does not come back.
        film.in_cinemas = None;
        film.locked_fields = vec!["item/inCinemas".into()];
        assert_eq!(
            release_day(&film, Some("FR")).as_deref(),
            Some("2001-03-01")
        );
    }

    #[test]
    fn a_series_lays_its_seasons_out_in_the_order_asked() {
        let it = series();
        let orders = dvd();
        let drawn = drawn("fra");
        let work = Work::of(&it);
        assert_eq!(work, Work::Tvdb(416802));

        let aired = Layout::asked(&it, &orders, None);
        let show = Series::new(&it, &work, &aired, &drawn).show(&orders, true);
        let sent = serde_json::to_value(&show).unwrap();
        assert_eq!(sent["ratingKey"], "show-tvdb-416802");
        assert_eq!(sent["key"], "/library/metadata/show-tvdb-416802/children");
        let types: Vec<&str> = sent["SeasonType"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(types, ["official", "dvd"]);
        assert_eq!(sent["Children"]["size"], 2);
        // A season's own name stands; one that only says its number is
        // said in the language asked.
        assert_eq!(sent["Children"]["Metadata"][0]["title"], "東京編");
        assert_eq!(sent["Children"]["Metadata"][1]["title"], "Saison 2");
        assert_eq!(
            sent["Children"]["Metadata"][0]["Guid"][0]["id"],
            "tvdb://1971"
        );
        assert_eq!(
            sent["Children"]["Metadata"][0]["parentKey"],
            "/library/metadata/show-tvdb-416802"
        );

        // In the DVD order, the first two episodes change places, under keys
        // of that order's.
        let placed = Layout::asked(&it, &orders, Some("dvd"));
        let series = Series::new(&it, &work, &placed, &drawn);
        let first = placed.season(1).unwrap();
        let season = serde_json::to_value(series.season(first, true)).unwrap();
        assert_eq!(season["ratingKey"], "season-tvdb-416802-s1-dvd");
        assert_eq!(season["title"], "Saison 1");
        let episodes: Vec<(&str, i64)> = season["Children"]["Metadata"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["title"].as_str().unwrap(), e["index"].as_i64().unwrap()))
            .collect();
        assert_eq!(episodes, [("Episode 1002", 1), ("Episode 1001", 2)]);
        assert_eq!(
            season["Children"]["Metadata"][0]["ratingKey"],
            "episode-tvdb-416802-s1e1-dvd"
        );
        assert_eq!(
            season["Children"]["Metadata"][0]["grandparentGuid"],
            "tv.plex.agents.custom.arrmetadataserver://show/show-tvdb-416802"
        );
        assert_eq!(series.episode_count(), 3);
        let across: Vec<String> = series
            .episodes(1..3)
            .into_iter()
            .map(|e| e.rating_key)
            .collect();
        assert_eq!(
            across,
            [
                "episode-tvdb-416802-s1e2-dvd",
                "episode-tvdb-416802-s2e1-dvd"
            ]
        );

        // An order the work has not: no season at all, as Plex asks. The
        // aired one, by its id or by none.
        assert!(
            Layout::asked(&it, &orders, Some("absolute"))
                .seasons
                .is_empty()
        );
        let official = Layout::asked(&it, &orders, Some("official"));
        assert!(!official.placed);
        assert_eq!(official.order, None);
        assert_eq!(
            official.season(1).unwrap().episode(1).unwrap().tvdb_id,
            Some(1001)
        );
    }

    #[test]
    fn match_requests_are_read_leniently() {
        let read: PlexMatchRequest = serde_json::from_value(serde_json::json!({
            "type": "2",
            "title": "Rurouni Kenshin",
            "year": 2023.0,
            "guids": ["tvdb://416802", { "id": "imdb://tt21207408" }],
            "manual": "1",
            "includeChildren": 1,
            "episodeOrdering": "dvd",
            "index": null,
            "includeElements": "Metadata,Children"
        }))
        .unwrap();
        assert_eq!(read.kind, Some(2));
        assert_eq!(read.year, Some(2023));
        assert_eq!(read.guids, ["tvdb://416802", "imdb://tt21207408"]);
        assert_eq!(read.manual, Some(true));
        assert_eq!(read.include_children, Some(true));
        assert_eq!(read.episode_order.as_deref(), Some("dvd"));
        assert_eq!(read.index, None);
    }
}

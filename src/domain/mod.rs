//! The canonical model.
//!
//! Every provider is mapped *into* these types, and every compatibility surface
//! is rendered *out of* them. Nothing downstream of [`crate::merge`] ever sees a
//! provider-shaped struct.

pub mod fields;
pub mod ids;

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

pub use ids::{ExternalIds, ExternalSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Series,
    Movie,
}

impl MediaKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Series => "series",
            Self::Movie => "movie",
        }
    }
}

impl fmt::Display for MediaKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MediaKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "series" | "tv" | "show" => Ok(Self::Series),
            "movie" | "film" => Ok(Self::Movie),
            other => anyhow::bail!("unknown media kind: {other}"),
        }
    }
}

/// A work, with whatever children the caller asked to load.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub id: String,
    pub kind: MediaKind,
    pub slug: String,
    pub title: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_aired: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_aired: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_cinemas: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digital_release: Option<String>,
    /// Local broadcast time, `HH:MM`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_time: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailer_youtube_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub popularity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_tmdb_id: Option<i64>,

    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,

    pub external_ids: ExternalIds,

    pub is_manual: bool,
    pub is_enabled: bool,

    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_error: Option<String>,

    // ── children, empty unless explicitly loaded ─────────────────────────────
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seasons: Vec<Season>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub episodes: Vec<Episode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credits: Vec<Credit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternative_titles: Vec<AlternativeTitle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ratings: Vec<Rating>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub translations: Vec<Translation>,

    /// Fields carrying a manual override, as `scope/field` paths. The web UI
    /// renders a lock next to each of these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked_fields: Vec<String>,
}

impl MediaItem {
    /// A blank work of the given kind, ready to be filled by a mapper.
    pub fn empty(kind: MediaKind) -> Self {
        let now = crate::db::now();
        Self {
            id: crate::db::new_id(),
            kind,
            slug: String::new(),
            title: String::new(),
            sort_title: None,
            original_title: None,
            overview: None,
            status: None,
            original_language: None,
            original_country: None,
            runtime: None,
            year: None,
            first_aired: None,
            last_aired: None,
            in_cinemas: None,
            physical_release: None,
            digital_release: None,
            air_time: None,
            network: None,
            studio: None,
            content_rating: None,
            homepage: None,
            trailer_youtube_id: None,
            popularity: None,
            collection_tmdb_id: None,
            genres: Vec::new(),
            keywords: Vec::new(),
            external_ids: ExternalIds::default(),
            is_manual: false,
            is_enabled: true,
            created_at: now.clone(),
            updated_at: now,
            refreshed_at: None,
            refresh_after: None,
            refresh_error: None,
            seasons: Vec::new(),
            episodes: Vec::new(),
            images: Vec::new(),
            credits: Vec::new(),
            alternative_titles: Vec::new(),
            ratings: Vec::new(),
            translations: Vec::new(),
            locked_fields: Vec::new(),
        }
    }

    pub fn rating(&self, source: &str) -> Option<&Rating> {
        self.ratings.iter().find(|r| r.source == source)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Season {
    pub id: String,
    pub season_number: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    pub is_manual: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub id: String,
    pub season_number: i32,
    pub episode_number: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub absolute_episode_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aired_after_season_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aired_before_season_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aired_before_episode_number: Option<i32>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date_utc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finale_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<RatingValue>,
    pub is_manual: bool,
}

/// Cover types, using Sonarr and Radarr's spelling so the compatibility
/// surfaces can emit them without a translation table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoverType {
    Poster,
    Banner,
    Fanart,
    Clearlogo,
    Screenshot,
    Headshot,
    Unknown,
}

impl CoverType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Poster => "poster",
            Self::Banner => "banner",
            Self::Fanart => "fanart",
            Self::Clearlogo => "clearlogo",
            Self::Screenshot => "screenshot",
            Self::Headshot => "headshot",
            Self::Unknown => "unknown",
        }
    }
}

impl FromStr for CoverType {
    type Err = std::convert::Infallible;

    /// Unrecognised types degrade to [`CoverType::Unknown`] rather than failing:
    /// a provider inventing a new cover type must not break a whole response.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "poster" => Self::Poster,
            "banner" => Self::Banner,
            "fanart" | "backdrop" => Self::Fanart,
            "clearlogo" | "logo" => Self::Clearlogo,
            "screenshot" | "still" => Self::Screenshot,
            "headshot" | "profile" => Self::Headshot,
            _ => Self::Unknown,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_number: Option<i32>,
    pub cover_type: CoverType,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub sort_order: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub is_manual: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CreditType {
    Actor,
    Director,
    Writer,
    Producer,
    Guest,
}

impl CreditType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Actor => "actor",
            Self::Director => "director",
            Self::Writer => "writer",
            Self::Producer => "producer",
            Self::Guest => "guest",
        }
    }
}

impl FromStr for CreditType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "director" => Self::Director,
            "writer" => Self::Writer,
            "producer" => Self::Producer,
            "guest" => Self::Guest,
            _ => Self::Actor,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credit {
    pub id: String,
    pub credit_type: CreditType,
    pub person_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_person_id: Option<i64>,
    pub sort_order: i32,
    pub is_manual: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlternativeTitle {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub is_manual: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatingValue {
    pub value: f64,
    pub votes: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rating {
    /// `tmdb`, `imdb`, `metacritic`, `rottenTomatoes`, `trakt`, …
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub votes: Option<i64>,
    /// `user` or `critic`, following Radarr's vocabulary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Translation {
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub is_manual: bool,
}

/// Build the URL-safe slug a work is addressed by.
///
/// Sonarr and Radarr both key their UI routes on this, so it must stay stable
/// for a given (title, year) pair.
pub fn make_slug(title: &str, year: Option<i32>) -> String {
    let base = slug::slugify(title);
    let base = if base.is_empty() {
        "untitled".to_string()
    } else {
        base
    };

    match year {
        Some(y) => format!("{base}-{y}"),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_stable_and_url_safe() {
        assert_eq!(make_slug("Breaking Bad", Some(2008)), "breaking-bad-2008");
        assert_eq!(make_slug("WALL·E", Some(2008)), "wall-e-2008");
        assert_eq!(make_slug("Amélie", None), "amelie");
        assert_eq!(make_slug("", Some(1999)), "untitled-1999");
    }

    #[test]
    fn unknown_cover_types_degrade_instead_of_failing() {
        assert_eq!("poster".parse::<CoverType>().unwrap(), CoverType::Poster);
        assert_eq!("backdrop".parse::<CoverType>().unwrap(), CoverType::Fanart);
        assert_eq!("sideways".parse::<CoverType>().unwrap(), CoverType::Unknown);
    }

    #[test]
    fn media_kind_accepts_the_spellings_clients_use() {
        assert_eq!("tv".parse::<MediaKind>().unwrap(), MediaKind::Series);
        assert_eq!("Movie".parse::<MediaKind>().unwrap(), MediaKind::Movie);
        assert!("album".parse::<MediaKind>().is_err());
    }
}

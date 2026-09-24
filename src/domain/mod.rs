//! The canonical model.
//!
//! Every provider is mapped *into* these types, and every compatibility surface
//! is rendered *out of* them. Nothing downstream of [`crate::merge`] ever sees a
//! provider-shaped struct.

pub mod fields;
pub mod ids;

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub use ids::{ExternalIds, ExternalSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
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
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
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
    /// ISO 3166-1 alpha-2, uppercase — the form Radarr matches against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_rating_country: Option<String>,
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
    /// What the provider that supplied this called it. Serving a catalogue
    /// means being able to decide per request who sees it.
    #[serde(default)]
    pub is_adult: bool,

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
            content_rating_country: None,
            homepage: None,
            trailer_youtube_id: None,
            popularity: None,
            collection_tmdb_id: None,
            genres: Vec::new(),
            keywords: Vec::new(),
            external_ids: ExternalIds::default(),
            is_manual: false,
            is_enabled: true,
            is_adult: false,
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

    /// The one rating to give where a client takes only one.
    ///
    /// IMDb's when there is one: it is what Skyhook serves, so it is what
    /// Sonarr has always shown for a series. Otherwise the one with the most
    /// votes behind it — TMDB's for most works, MyAnimeList's for most anime —
    /// and the first listed of those tied.
    ///
    /// Only marks out of ten. TheTVDB's popularity figure used to be stored as
    /// a rating; a figure in the millions is not one, whatever it is filed as.
    /// The interface picks by the same rule: see `ratingsOf` in `lib/media.ts`.
    pub fn headline_rating(&self) -> Option<&Rating> {
        let real = || {
            self.ratings
                .iter()
                .filter(|r| r.value.is_some_and(|v| v > 0.0 && v <= 10.0))
        };

        real()
            .find(|r| r.source == "imdb")
            .or_else(|| real().min_by_key(|r| std::cmp::Reverse(r.votes.unwrap_or(0))))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
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

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum CoverType {
    Poster,
    Banner,
    Fanart,
    Clearlogo,
    /// Character artwork on a transparent background. Kodi and Jellyfin use it;
    /// Sonarr and Radarr map anything they do not know to `Unknown`, which is
    /// harmless — they simply ignore it.
    Clearart,
    /// Wide 16:9 artwork, `<thumb aspect="landscape">` in a Kodi document.
    Landscape,
    Screenshot,
    Headshot,
    Unknown,
}

impl CoverType {
    /// Display order. Clients look images up by type, but anything that takes
    /// `images[0]` should get the poster, not whichever type sorts first.
    pub const fn priority(self) -> u8 {
        match self {
            Self::Poster => 0,
            Self::Fanart => 1,
            Self::Banner => 2,
            Self::Clearlogo => 3,
            Self::Clearart => 4,
            Self::Landscape => 5,
            Self::Screenshot => 6,
            Self::Headshot => 7,
            Self::Unknown => 8,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Poster => "poster",
            Self::Banner => "banner",
            Self::Fanart => "fanart",
            Self::Clearlogo => "clearlogo",
            Self::Clearart => "clearart",
            Self::Landscape => "landscape",
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
            "clearart" | "art" => Self::Clearart,
            "landscape" | "thumb" => Self::Landscape,
            "screenshot" | "still" => Self::Screenshot,
            "headshot" | "profile" => Self::Headshot,
            _ => Self::Unknown,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
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
    /// TMDB's identifier for *this role*, distinct from the person's id. Radarr
    /// requires it and rejects a credit without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credit_tmdb_id: Option<String>,
    pub sort_order: i32,
    pub is_manual: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RatingValue {
    pub value: f64,
    pub votes: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
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

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
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
/// A calendar date as a UTC timestamp, or nothing if it is not a date.
///
/// Sonarr parses `airDateUtc` as a `DateTime`, and this used to be a `format!`
/// over whatever string the provider put in its date field — so `"2008"`,
/// `"TBA"` and an already-complete timestamp each came out as something no
/// parser accepts (`"TBAT00:00:00Z"`). An unusable date is better absent.
///
/// The time of day is a known simplification: providers give a broadcast date
/// in the network's own timezone and the broadcast time separately, so midnight
/// UTC is a placeholder rather than a claim.
pub fn midnight_utc(date: &str) -> Option<String> {
    let date = date.trim();

    // Already a timestamp: keep it rather than stamping it twice.
    if date.contains('T') {
        return crate::db::parse_rfc3339(date).map(|_| date.to_string());
    }

    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(|d| format!("{d}T00:00:00Z"))
}

/// The genres one of a work's stands for, as a list files them.
///
/// TMDB gives series and films different genres for the same thing: a series
/// is "Action & Adventure", a film "Action" and "Adventure"; a series "Sci-Fi
/// & Fantasy", a film "Science Fiction" and "Fantasy". In a catalogue of both,
/// "Action" found no series at all, and the genres on offer named everything
/// twice. TMDB's three combined genres are listed as their parts, in the two
/// languages this interface speaks; anything else is left as it is, a genre
/// somebody typed — "Sword & Sorcery" — included.
pub fn genre_parts(genre: &str) -> Vec<String> {
    let genre = genre.trim();
    let parts: &[&str] = match genre {
        "" => &[],
        "Action & Adventure" => &["Action", "Adventure"],
        "Sci-Fi & Fantasy" => &["Science Fiction", "Fantasy"],
        "War & Politics" => &["War", "Politics"],
        "Action & Aventure" => &["Action", "Aventure"],
        "Science-Fiction & Fantastique" => &["Science-Fiction", "Fantastique"],
        "Guerre & Politique" => &["Guerre", "Politique"],
        other => return vec![other.to_string()],
    };
    parts.iter().map(|p| p.to_string()).collect()
}

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

    #[test]
    fn a_series_genre_is_listed_as_the_film_genres_it_stands_for() {
        assert_eq!(genre_parts("Action & Adventure"), ["Action", "Adventure"]);
        assert_eq!(
            genre_parts("Sci-Fi & Fantasy"),
            ["Science Fiction", "Fantasy"]
        );
        assert_eq!(genre_parts("Action & Aventure"), ["Action", "Aventure"]);
        assert_eq!(
            genre_parts("Science-Fiction & Fantastique"),
            ["Science-Fiction", "Fantastique"]
        );
        assert_eq!(genre_parts(" Drama "), ["Drama"]);
        assert!(genre_parts("  ").is_empty());
        // Only TMDB's own combinations: one typed by hand stays whole.
        assert_eq!(genre_parts("Sword & Sorcery"), ["Sword & Sorcery"]);
    }

    #[test]
    fn a_broadcast_date_that_is_not_a_date_produces_nothing() {
        // Sonarr parses this field as a timestamp. It used to be built by
        // sticking `T00:00:00Z` on whatever the provider sent, so a year, a
        // `TBA` or an already-complete timestamp each came out unparseable.
        assert_eq!(
            midnight_utc("2008-01-20").as_deref(),
            Some("2008-01-20T00:00:00Z")
        );
        assert_eq!(
            midnight_utc("  2008-01-20 ").as_deref(),
            Some("2008-01-20T00:00:00Z")
        );
        assert_eq!(
            midnight_utc("2008-01-20T21:00:00Z").as_deref(),
            Some("2008-01-20T21:00:00Z"),
            "a timestamp is not stamped a second time"
        );

        assert_eq!(midnight_utc("2008"), None);
        assert_eq!(midnight_utc("TBA"), None);
        assert_eq!(midnight_utc(""), None);
        assert_eq!(midnight_utc("2008-13-45"), None);
    }

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
    fn posters_lead_the_image_order() {
        let mut types = vec![
            CoverType::Clearlogo,
            CoverType::Unknown,
            CoverType::Landscape,
            CoverType::Fanart,
            CoverType::Poster,
            CoverType::Banner,
        ];
        types.sort_by_key(|t| t.priority());

        assert_eq!(
            types,
            vec![
                CoverType::Poster,
                CoverType::Fanart,
                CoverType::Banner,
                CoverType::Clearlogo,
                CoverType::Landscape,
                CoverType::Unknown,
            ]
        );
    }

    #[test]
    fn media_kind_accepts_the_spellings_clients_use() {
        assert_eq!("tv".parse::<MediaKind>().unwrap(), MediaKind::Series);
        assert_eq!("Movie".parse::<MediaKind>().unwrap(), MediaKind::Movie);
        assert!("album".parse::<MediaKind>().is_err());
    }
}

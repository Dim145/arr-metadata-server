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
    /// Where the work's theme music is, as its provider keeps it: an address,
    /// like an image's. Fankai keeps one for every Fan-Kai.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme_music: Option<String>,
    /// What TheTVDB adds to the series' name to tell it from another of the
    /// same name — `2023`, `US` — where the title, as TMDB names it, has
    /// none. The Sonarr surface gives it back, as Skyhook does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_qualifier: Option<String>,
    /// The line the work is sold on, under its title — *Why are they here?* —
    /// as TMDB has it in the language the catalogue is stored in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    /// What AniList calls the entry the work is: `TV`, `TV_SHORT`, `MOVIE`,
    /// `SPECIAL`, `OVA`, `ONA` or `MUSIC`. A series or a film says too little
    /// of an anime: an OVA is filed here as one or the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub popularity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_tmdb_id: Option<i64>,

    // Always written; the defaults only let a reader do without them.
    #[serde(default)]
    #[schema(required)]
    pub genres: Vec<String>,
    #[serde(default)]
    #[schema(required)]
    pub keywords: Vec<String>,
    /// Every country the work was made in, ISO 3166-1 alpha-2 in upper case
    /// — `US`, `GB` — in the order TMDB lists them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub production_countries: Vec<String>,
    /// Every company that made it, in the order TMDB lists them. The studio
    /// is the first of them, or the one an anime source names instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub production_companies: Vec<String>,

    pub external_ids: ExternalIds,

    pub is_manual: bool,
    pub is_enabled: bool,
    /// What the provider that supplied this called it. Serving a catalogue
    /// means being able to decide per request who sees it.
    #[serde(default)]
    #[schema(required)]
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
    /// The other works a provider files beside this one — its sequels,
    /// prequels, side stories — with the work here that each is, when the
    /// catalogue holds it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
    /// When the work came out in each country, and the rating it carries
    /// there: a country's, whole, from the first source that has any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub releases: Vec<CountryRelease>,

    /// Fields carrying a manual override, as `scope/field` paths. The web UI
    /// renders a lock next to each of these. A text locked in another
    /// language than the one served is `scope/field@fra`; a lock on a child
    /// the sources gave is named as [`fields::Child`] names it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked_fields: Vec<String>,
    /// The poster and the background a person chose to lead with, by image
    /// id: what every client that keeps one image a kind is given. Set from
    /// the locks when the work is read; none by default.
    #[serde(default, skip_serializing_if = "PrimaryImages::is_empty")]
    pub primary_images: PrimaryImages,
    /// The texts a person locked, each in one language or in every one: what
    /// the language the work is served in lays over it. Set from the locks
    /// when the work is read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked_texts: Vec<fields::LockedText>,
    /// What the sources gave and a person hid: kept apart for whoever
    /// maintains the catalogue, served to nobody.
    #[serde(default, skip_serializing_if = "fields::Hidden::is_empty")]
    pub hidden: fields::Hidden,
}

/// The images a person chose, by id among the work's own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrimaryImages {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanart: Option<String>,
}

impl PrimaryImages {
    pub fn is_empty(&self) -> bool {
        self.poster.is_none() && self.fanart.is_none()
    }

    /// The image chosen for a kind, when one was.
    pub fn of(&self, kind: CoverType) -> Option<&str> {
        match kind {
            CoverType::Poster => self.poster.as_deref(),
            CoverType::Fanart => self.fanart.as_deref(),
            _ => None,
        }
    }
}

/// One image a kind, in the order the kinds come: the one a person chose,
/// else one added by hand, else the first the sources offered. What a client
/// that keeps a single poster, a single background, is to be given — Sonarr
/// and Radarr write every image of a kind to one file, so the last they are
/// sent is the one they show.
pub fn lead_images<'a>(
    images: impl IntoIterator<Item = &'a Image>,
    chosen: &PrimaryImages,
) -> Vec<&'a Image> {
    let mut by_kind: Vec<(CoverType, Vec<&'a Image>)> = Vec::new();
    for image in images {
        match by_kind
            .iter_mut()
            .find(|(kind, _)| *kind == image.cover_type)
        {
            Some((_, of_kind)) => of_kind.push(image),
            None => by_kind.push((image.cover_type, vec![image])),
        }
    }
    by_kind
        .into_iter()
        .filter_map(|(kind, of_kind)| {
            chosen
                .of(kind)
                .and_then(|id| of_kind.iter().find(|i| i.id == id))
                .or_else(|| of_kind.iter().find(|i| i.is_manual))
                .or_else(|| of_kind.first())
                .copied()
        })
        .collect()
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
            theme_music: None,
            title_qualifier: None,
            tagline: None,
            format: None,
            popularity: None,
            collection_tmdb_id: None,
            genres: Vec::new(),
            keywords: Vec::new(),
            production_countries: Vec::new(),
            production_companies: Vec::new(),
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
            relations: Vec::new(),
            releases: Vec::new(),
            locked_fields: Vec::new(),
            primary_images: PrimaryImages::default(),
            locked_texts: Vec::new(),
            hidden: fields::Hidden::default(),
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

    /// The rating the work carries in a country, by its ISO 3166-1 alpha-2
    /// code: the one its record holds when that is the country's — a person
    /// may have locked it — else the first its releases there give, in
    /// cinemas before anything else.
    pub fn certification_in(&self, country: &str) -> Option<&str> {
        let blank = |s: &&str| s.trim().is_empty();
        if self
            .content_rating_country
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case(country))
            && let Some(own) = self.content_rating.as_deref().filter(|s| !blank(s))
        {
            return Some(own.trim());
        }
        // Of two of the same kind, the earlier: the first run's rating, not
        // a re-release's.
        self.releases
            .iter()
            .filter(|r| r.country.eq_ignore_ascii_case(country))
            .filter_map(|r| {
                let certification = r.certification.as_deref().filter(|s| !blank(s))?;
                let day = r.date.as_deref().unwrap_or("~");
                Some(((ReleaseKind::rank(r.release_type), day), certification))
            })
            .min_by_key(|(order, _)| *order)
            .map(|(_, certification)| certification.trim())
    }

    /// One rating a country, by country code in order: what Radarr is sent,
    /// to pick its own country's from. The record's own counts for its
    /// country, as [`Self::certification_in`] says.
    pub fn certifications(&self) -> Vec<(String, String)> {
        let mut countries: Vec<String> = self
            .releases
            .iter()
            .map(|r| r.country.to_ascii_uppercase())
            .chain(
                self.content_rating_country
                    .as_deref()
                    .map(str::to_ascii_uppercase),
            )
            .filter(|c| !c.trim().is_empty())
            .collect();
        countries.sort_unstable();
        countries.dedup();
        countries
            .into_iter()
            .filter_map(|country| {
                let certification = self.certification_in(&country)?.to_string();
                Some((country, certification))
            })
            .collect()
    }

    /// The first day the work came out in a country in each of the three
    /// ways Radarr keeps — in cinemas, digitally, on disc — as its releases
    /// there say. The earliest of each: a re-release does not move a film
    /// forward, here any more than in the record's own dates.
    pub fn days_in(&self, country: &str) -> ReleaseDays {
        let mut days = ReleaseDays::default();
        for release in self
            .releases
            .iter()
            .filter(|r| r.country.eq_ignore_ascii_case(country))
        {
            let Some(day) = release
                .date
                .as_deref()
                .and_then(|d| d.trim().get(..10))
                .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
            else {
                continue;
            };
            let slot = match release.release_type {
                Some(kind) if kind.in_cinemas() => &mut days.cinema,
                Some(ReleaseKind::Digital) => &mut days.digital,
                Some(ReleaseKind::Physical) => &mut days.physical,
                _ => continue,
            };
            if slot.as_deref().is_none_or(|kept| day < kept) {
                *slot = Some(day.to_string());
            }
        }
        days
    }

    /// Whether somebody fixed this field of the work itself by hand: `item/`
    /// and its name as the registry spells it — `inCinemas`, `title` — is
    /// among its locked fields. A lock is final: what a provider, or a
    /// country's releases, say of the field is not put in its place.
    pub fn is_locked(&self, field: &str) -> bool {
        self.locked_fields
            .iter()
            .any(|locked| locked.strip_prefix("item/") == Some(field))
    }
}

/// A film's first day in a country in each of the ways Radarr keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseDays {
    pub cinema: Option<String>,
    pub digital: Option<String>,
    pub physical: Option<String>,
}

/// How a work came out in a country, as TMDB tells its releases apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum ReleaseKind {
    /// A first showing — a festival, a gala — before it is out.
    Premiere,
    /// In a few cinemas.
    Limited,
    /// In cinemas.
    Theatrical,
    /// To buy or rent online.
    Digital,
    /// On disc.
    Physical,
    /// On television.
    Tv,
}

impl ReleaseKind {
    /// TMDB's code for it: 1 a premiere, 2 a limited run, 3 cinemas,
    /// 4 digital, 5 physical, 6 television.
    pub const fn from_tmdb(code: i32) -> Option<Self> {
        match code {
            1 => Some(Self::Premiere),
            2 => Some(Self::Limited),
            3 => Some(Self::Theatrical),
            4 => Some(Self::Digital),
            5 => Some(Self::Physical),
            6 => Some(Self::Tv),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Premiere => "premiere",
            Self::Limited => "limited",
            Self::Theatrical => "theatrical",
            Self::Digital => "digital",
            Self::Physical => "physical",
            Self::Tv => "tv",
        }
    }

    /// Whether it puts the work in cinemas, as Radarr counts a release:
    /// a limited run is one, a festival's premiere is not.
    pub const fn in_cinemas(self) -> bool {
        matches!(self, Self::Limited | Self::Theatrical)
    }

    /// Whose rating stands for a country's when it lists several: the one
    /// it was given for cinemas, then one given without a release — a
    /// series', Radarr's — then a premiere's, then the rest.
    const fn rank(kind: Option<Self>) -> u8 {
        match kind {
            Some(Self::Theatrical) => 0,
            Some(Self::Limited) => 1,
            None => 2,
            Some(Self::Premiere) => 3,
            Some(Self::Digital) => 4,
            Some(Self::Physical) => 5,
            Some(Self::Tv) => 6,
        }
    }
}

impl FromStr for ReleaseKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "premiere" => Ok(Self::Premiere),
            "limited" => Ok(Self::Limited),
            "theatrical" => Ok(Self::Theatrical),
            "digital" => Ok(Self::Digital),
            "physical" => Ok(Self::Physical),
            "tv" => Ok(Self::Tv),
            other => anyhow::bail!("unknown release type: {other}"),
        }
    }
}

/// What a work was given in one country: a day it came out there and how,
/// the rating it carries there, or both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CountryRelease {
    /// ISO 3166-1 alpha-2, upper case: `FR`, `US`.
    pub country: String,
    /// How it came out there; none for a rating given without a release —
    /// a series' ratings, and Radarr's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_type: Option<ReleaseKind>,
    /// The day, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// The rating there: `TP`, `PG-13`, `12A`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certification: Option<String>,
    /// What TMDB notes beside it: a festival, a service, a re-release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The source it came from: `tmdb`, `tvdb`, `radarr`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
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
    /// The poster a person chose to lead the season with, by id among its
    /// own images: a lock (`season:N/primaryPoster`), as the work's is.
    #[serde(default, skip_serializing_if = "PrimaryImages::is_empty")]
    pub primary_images: PrimaryImages,
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

/// Another work, as a provider files it beside this one.
///
/// AniList's relations for the moment: what an entry is the sequel, prequel,
/// side story or spin-off of, with the entry named, dated and pictured as
/// AniList has it, so it can be shown whether or not the catalogue holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    pub id: String,
    /// How the other work stands to this one, as AniList names it: `SEQUEL`,
    /// `PREQUEL`, `PARENT`, `SIDE_STORY`, `SPIN_OFF`, `ALTERNATIVE`, `SUMMARY`,
    /// `COMPILATION`, `CONTAINS`, `SOURCE`, `ADAPTATION`, `CHARACTER` or
    /// `OTHER`.
    pub relation_type: String,
    /// Where the other work is filed — `anilist` — and its id there.
    pub source: String,
    pub external_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mal_id: Option<i64>,
    pub title: String,
    /// `anime` or `manga`.
    pub medium: String,
    /// AniList's format: `TV`, `MOVIE`, `OVA`, `ONA`, `SPECIAL`, `MANGA`, …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Whether the other work is for adults: as AniList flags it, or as the
    /// catalogue holds it. A reader kept from adult works is not shown it.
    pub is_adult: bool,
    /// The work here that the other one is, when the catalogue holds it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    pub sort_order: i32,
}

/// One of the other orders a series' episodes come in — `dvd`, `absolute`,
/// `alternate`, `regional` — as TheTVDB keeps it: the work's episodes, by
/// their TVDB id, in seasons and numbers of that order's own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeOrder {
    pub kind: String,
    pub episodes: Vec<PlacedEpisode>,
}

/// Where one episode stands in an order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlacedEpisode {
    pub tvdb_id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub absolute_number: Option<i32>,
}

/// An earlier work named beside one, as a line on a chart says it: its title,
/// and the work here that it is when the catalogue holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Kin {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
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
    /// The line under the title in this language, where the source gives
    /// one: TMDB does, in each of its translations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
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

    /// One image a kind: the chosen, else one added by hand, else the first.
    #[test]
    fn a_client_is_given_one_image_a_kind() {
        let image = |id: &str, kind: CoverType, manual: bool| Image {
            id: id.into(),
            season_number: None,
            cover_type: kind,
            url: format!("https://x/{id}.jpg"),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: manual,
        };
        let images = [
            image("p1", CoverType::Poster, false),
            image("p2", CoverType::Poster, true),
            image("p3", CoverType::Poster, false),
            image("f1", CoverType::Fanart, false),
            image("f2", CoverType::Fanart, false),
            image("b1", CoverType::Banner, false),
        ];
        let ids = |chosen: &PrimaryImages| -> Vec<String> {
            lead_images(&images, chosen)
                .into_iter()
                .map(|i| i.id.clone())
                .collect()
        };
        assert_eq!(
            ids(&PrimaryImages::default()),
            ["p2", "f1", "b1"],
            "by hand, then first"
        );
        let chosen = PrimaryImages {
            poster: Some("p3".into()),
            fanart: Some("f2".into()),
        };
        assert_eq!(ids(&chosen), ["p3", "f2", "b1"]);
        let stale = PrimaryImages {
            poster: Some("nowhere".into()),
            fanart: None,
        };
        assert_eq!(
            ids(&stale),
            ["p2", "f1", "b1"],
            "a choice that names nothing is no choice"
        );
    }

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

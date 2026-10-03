//! TMDB API response shapes.
//!
//! Only the fields this server maps are declared. Everything is `Option` or
//! `#[serde(default)]`: TMDB omits fields freely depending on the title, and a
//! missing one must not fail the whole fetch.

// These mirror TMDB's documents. Some fields are declared but not yet mapped;
// they are kept because the struct doubles as documentation of what upstream
// returns, and because adding one back later is a one-line change.
#![allow(dead_code)]

use serde::Deserialize;

// ─── search & find ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
// `#[serde(default)]` on a generic field makes the derive infer `T: Default`,
// which none of the response types satisfy. Only `Deserialize` is actually needed.
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct SearchResponse<T> {
    #[serde(default)]
    pub results: Vec<T>,
    #[serde(default)]
    pub total_results: i64,
    #[serde(default)]
    pub total_pages: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FindResponse {
    #[serde(default)]
    pub tv_results: Vec<TvSummary>,
    #[serde(default)]
    pub movie_results: Vec<MovieSummary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TvSummary {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub first_air_date: Option<String>,
    #[serde(default)]
    pub origin_country: Vec<String>,
    pub original_language: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    /// TVDB has no adult catalogue, so these titles never carry a TVDB id.
    pub adult: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MovieSummary {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub release_date: Option<String>,
    pub original_language: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub adult: Option<bool>,
}

// ─── series ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Tv {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub homepage: Option<String>,
    pub first_air_date: Option<String>,
    pub last_air_date: Option<String>,
    #[serde(default)]
    pub origin_country: Vec<String>,
    pub original_language: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub episode_run_time: Vec<i32>,
    #[serde(default)]
    pub networks: Vec<Named>,
    #[serde(default)]
    pub production_companies: Vec<Named>,
    #[serde(default)]
    pub genres: Vec<Named>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    #[serde(default)]
    pub seasons: Vec<SeasonSummary>,
    pub adult: Option<bool>,

    // append_to_response
    pub external_ids: Option<ExternalIds>,
    pub credits: Option<Credits>,
    pub content_ratings: Option<Results<ContentRating>>,
    pub alternative_titles: Option<AltTitles>,
    pub keywords: Option<KeywordsTv>,
    pub videos: Option<Results<Video>>,
    pub images: Option<Images>,
    pub translations: Option<Translations>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonSummary {
    pub id: Option<i64>,
    pub season_number: i32,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub poster_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Season {
    pub season_number: i32,
    #[serde(default)]
    pub episodes: Vec<Episode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Episode {
    pub id: Option<i64>,
    pub season_number: i32,
    pub episode_number: i32,
    /// As TMDB sends it, stand-in included: [`Episode::title`] is the name.
    pub name: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub runtime: Option<i32>,
    pub still_path: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    /// `finale`, `mid_season`, … on recent API versions.
    pub episode_type: Option<String>,
}

impl Episode {
    /// The episode's name, or none where TMDB only stands one in.
    ///
    /// TMDB leaves no episode unnamed: one it has no name for in the language
    /// asked is called by its number in that language, `Episode 3`, `Épisode 3`,
    /// `Folge 3`, `第3話`. Taken for a title, that is what Sonarr names files
    /// after where Skyhook sends `TBA`, and Sonarr's check that an episode is
    /// named before it is imported waits only on `TBA` or on nothing. Without
    /// it, another provider's title fills the gap, or the episode goes out as
    /// `TBA`.
    pub fn title(&self) -> Option<String> {
        self.name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .filter(|name| !is_placeholder(name, self.episode_number))
            .map(String::from)
    }
}

/// TMDB's stand-ins for an episode's name, `{n}` where its number goes.
///
/// What TMDB answered in each of its 144 primary translations for episodes of
/// three unrelated series that nobody has named (asked on 2026-10-03). A
/// language with no form of its own gets the English one.
const PLACEHOLDERS: &[&str] = &[
    "Episode {n}",      // English, and every language without its own
    "Épisode {n}",      // French
    "Folge {n}",        // German
    "Episodio {n}",     // Spanish, Italian, Galician
    "Episódio {n}",     // Portuguese
    "Episodi {n}",      // Catalan
    "Episodul {n}",     // Romanian
    "Aflevering {n}",   // Dutch
    "Afsnit {n}",       // Danish
    "Avsnitt {n}",      // Swedish
    "Jakso {n}",        // Finnish
    "Odcinek {n}",      // Polish
    "Epizoda {n}",      // Croatian
    "Epizóda {n}",      // Slovak
    "Epizodas {n}",     // Lithuanian
    "Epizodo {n}",      // Esperanto
    "Epızod {n}",       // Kazakh
    "Xalqada {n}",      // Somali
    "{n}. epizoda",     // Czech
    "{n}. epizód",      // Hungarian
    "{n}. sērija",      // Latvian
    "{n}. Bölüm",       // Turkish
    "{n}. Atala",       // Basque
    "Επεισόδιο {n}",    // Greek
    "Эпизод {n}",       // Russian
    "Епизод {n}",       // Bulgarian
    "Епизода {n}",      // Serbian
    "Серія {n}",        // Ukrainian
    "פרק {n}",          // Hebrew
    "الحلقة {n}",       // Arabic
    "\u{202b}قسمت {n}", // Persian, behind a right-to-left embedding mark
    "에피소드 {n}",     // Korean
    "第{n}話",          // Japanese
    "第 {n} 集",        // Chinese
];

/// Whether `name` is TMDB's stand-in for episode `number`.
///
/// Only with the episode's own number, as TMDB numbers it: that is the one
/// TMDB puts in its stand-in, never an absolute count. A name with another
/// number was given by somebody: Doraemon's 1082nd is `Episode 925` in
/// English, and `Épisode 1082` in French.
fn is_placeholder(name: &str, number: i32) -> bool {
    let name = name.trim();
    let number = number.to_string();

    PLACEHOLDERS.iter().any(|form| {
        form.split_once("{n}").is_some_and(|(before, after)| {
            name.strip_prefix(before)
                .and_then(|rest| rest.strip_suffix(after))
                == Some(number.as_str())
        })
    })
}

// ─── movie ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Movie {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub homepage: Option<String>,
    pub release_date: Option<String>,
    pub original_language: Option<String>,
    pub status: Option<String>,
    pub runtime: Option<i32>,
    pub imdb_id: Option<String>,
    #[serde(default)]
    pub genres: Vec<Named>,
    #[serde(default)]
    pub production_companies: Vec<Named>,
    #[serde(default)]
    pub production_countries: Vec<Country>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub belongs_to_collection: Option<CollectionRef>,
    pub adult: Option<bool>,

    // append_to_response
    pub external_ids: Option<ExternalIds>,
    pub credits: Option<Credits>,
    pub release_dates: Option<Results<ReleaseDates>>,
    pub alternative_titles: Option<MovieAltTitles>,
    pub keywords: Option<KeywordsMovie>,
    pub videos: Option<Results<Video>>,
    pub images: Option<Images>,
    pub translations: Option<Translations>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CollectionRef {
    pub id: i64,
    pub name: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Collection {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    #[serde(default)]
    pub parts: Vec<MovieSummary>,
}

/// One country's certification block from `/movie/{id}/release_dates`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseDates {
    pub iso_3166_1: String,
    #[serde(default)]
    pub release_dates: Vec<ReleaseDate>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseDate {
    pub certification: Option<String>,
    /// TMDB's type codes: 1 premiere, 2 limited, 3 theatrical, 4 digital, 5 physical, 6 TV.
    #[serde(rename = "type")]
    pub release_type: Option<i32>,
    pub release_date: Option<String>,
    pub iso_639_1: Option<String>,
}

// ─── shared ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Country {
    #[serde(default)]
    pub iso_3166_1: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct Results<T> {
    #[serde(default)]
    pub results: Vec<T>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExternalIds {
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub tvrage_id: Option<i64>,
    pub wikidata_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Credits {
    #[serde(default)]
    pub cast: Vec<CastMember>,
    #[serde(default)]
    pub crew: Vec<CrewMember>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CastMember {
    pub id: Option<i64>,
    /// Identifies the role, not the person.
    pub credit_id: Option<String>,
    #[serde(default)]
    pub name: String,
    pub character: Option<String>,
    pub profile_path: Option<String>,
    pub order: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CrewMember {
    pub id: Option<i64>,
    pub credit_id: Option<String>,
    #[serde(default)]
    pub name: String,
    pub job: Option<String>,
    pub department: Option<String>,
    pub profile_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContentRating {
    pub iso_3166_1: String,
    #[serde(default)]
    pub rating: String,
}

/// `/tv/{id}/alternative_titles` nests under `results`.
#[derive(Debug, Clone, Deserialize)]
pub struct AltTitles {
    #[serde(default)]
    pub results: Vec<AltTitle>,
}

/// `/movie/{id}/alternative_titles` nests under `titles` instead.
#[derive(Debug, Clone, Deserialize)]
pub struct MovieAltTitles {
    #[serde(default)]
    pub titles: Vec<AltTitle>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AltTitle {
    #[serde(default)]
    pub title: String,
    pub iso_3166_1: Option<String>,
    #[serde(rename = "type")]
    pub title_type: Option<String>,
}

/// Keywords are `results` for TV and `keywords` for movies.
#[derive(Debug, Clone, Deserialize)]
pub struct KeywordsTv {
    #[serde(default)]
    pub results: Vec<Named>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeywordsMovie {
    #[serde(default)]
    pub keywords: Vec<Named>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Video {
    pub key: Option<String>,
    pub site: Option<String>,
    #[serde(rename = "type")]
    pub video_type: Option<String>,
    pub official: Option<bool>,
    pub size: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Images {
    #[serde(default)]
    pub posters: Vec<ImageRef>,
    #[serde(default)]
    pub backdrops: Vec<ImageRef>,
    #[serde(default)]
    pub logos: Vec<ImageRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageRef {
    pub file_path: String,
    pub iso_639_1: Option<String>,
    pub vote_average: Option<f64>,
    pub width: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Translations {
    #[serde(default)]
    pub translations: Vec<Translation>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Translation {
    pub iso_639_1: Option<String>,
    /// The region the translation is for: `FR` and `CA` are both French.
    #[serde(default)]
    pub iso_3166_1: Option<String>,
    pub data: Option<TranslationData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TranslationData {
    pub title: Option<String>,
    pub name: Option<String>,
    pub overview: Option<String>,
}

/// `/movie/changes` and `/tv/changes`.
#[derive(Debug, Clone, Deserialize)]
pub struct ChangesResponse {
    #[serde(default)]
    pub results: Vec<ChangedId>,
    #[serde(default)]
    pub total_pages: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangedId {
    pub id: i64,
    pub adult: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(number: i32, name: Option<&str>) -> Episode {
        Episode {
            id: None,
            season_number: 2,
            episode_number: number,
            name: name.map(String::from),
            overview: None,
            air_date: None,
            runtime: None,
            still_path: None,
            vote_average: None,
            vote_count: None,
            episode_type: None,
        }
    }

    #[test]
    fn tmdbs_stand_in_is_no_name_in_any_of_its_languages() {
        // What TMDB answers, in some of its languages, for the third episode
        // of Reincarnated as a Sword's second season, which nobody has named
        // yet. A server set to French sent Sonarr "Épisode 3" for its title,
        // where Skyhook sends TBA.
        for name in [
            "Episode 3",
            "Épisode 3",
            "Folge 3",
            "Episodio 3",
            "Episódio 3",
            "Aflevering 3",
            "Odcinek 3",
            "3. epizoda",
            "第3話",
            "第 3 集",
            "\u{202b}قسمت 3",
        ] {
            assert_eq!(episode(3, Some(name)).title(), None, "{name:?}");
        }
        assert_eq!(episode(3, Some("  Épisode 3 ")).title(), None);
    }

    #[test]
    fn a_stand_in_names_the_episode_by_its_own_number() {
        // TMDB puts the episode's own number in it. Another number is a name
        // somebody gave it: Doraemon's 1082nd is "Episode 925" in English.
        assert_eq!(
            episode(1082, Some("Episode 925")).title().as_deref(),
            Some("Episode 925")
        );
        assert_eq!(episode(1082, Some("Épisode 1082")).title(), None);
        assert_eq!(
            episode(3, Some("Épisode 30")).title().as_deref(),
            Some("Épisode 30")
        );
        assert_eq!(
            episode(30, Some("Épisode 3")).title().as_deref(),
            Some("Épisode 3")
        );
    }

    #[test]
    fn a_real_name_is_kept_as_it_came() {
        assert_eq!(
            episode(1, Some("The Floating Island")).title().as_deref(),
            Some("The Floating Island")
        );
        // Like a stand-in, but somebody's title all the same.
        for name in [
            "Episode of Rain",
            "Episode",
            "Épisode 3 : le retour",
            "Episode III",
        ] {
            assert_eq!(episode(3, Some(name)).title().as_deref(), Some(name));
        }
        // Spelled otherwise than TMDB spells its own.
        assert_eq!(
            episode(3, Some("episode 3")).title().as_deref(),
            Some("episode 3")
        );
    }

    #[test]
    fn a_blank_name_is_none() {
        assert_eq!(episode(1, None).title(), None);
        assert_eq!(episode(1, Some("")).title(), None);
        assert_eq!(episode(1, Some("   ")).title(), None);
    }
}

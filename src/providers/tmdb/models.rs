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
    #[serde(default)]
    pub name: String,
    pub character: Option<String>,
    pub profile_path: Option<String>,
    pub order: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CrewMember {
    pub id: Option<i64>,
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

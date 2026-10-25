//! Radarr's metadata format.
//!
//! Field names follow `NzbDrone.Core/MetadataSource/SkyHook/Resource/*.cs` in
//! the Radarr source. Radarr's own `MovieResource` is what `api.radarr.video`
//! returns, so matching it exactly is what makes the substitution invisible.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Ceilings on what one upstream answer may turn into; see the same pair in
/// [`crate::wire::sonarr`]. `api.radarr.video` is an operator-configurable URL
/// like every other, and every element here becomes a database row.
const MAX_IMAGES: usize = 200;
const MAX_TITLES: usize = 500;

use super::string_or_null;
use crate::domain::{CountryRelease, Credit, CreditType, Image, MediaItem, Rating};

/// A list that the upstream sometimes writes as `null`.
///
/// api.radarr.video answers `"Parts": null` for a collection it has not
/// expanded, and `#[serde(default)]` does not cover that: it fills in a missing
/// field, not a present one holding null.
fn list_or_null<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

/// ISO 639-2 for "undetermined": the original language of a film whose
/// language is not known.
///
/// Radarr lowercases `OriginalLanguage` without a null check
/// (`SkyHookProxy.MapMovie`), so leaving it out fails the film's refresh, and
/// a search whose results include the film. It finds no language for this
/// code and takes English, as it does for any code it does not know.
const UNDETERMINED: &str = "und";

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct MovieResource {
    pub tmdb_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    pub title_slug: String,
    /// Deprecated upstream but still read by older Radarr versions.
    #[serde(default, deserialize_with = "list_or_null")]
    pub ratings: Vec<RatingItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_ratings: Option<RatingResource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    #[serde(default, deserialize_with = "list_or_null")]
    pub images: Vec<ImageResource>,
    #[serde(default, deserialize_with = "list_or_null")]
    pub genres: Vec<String>,
    #[serde(default, deserialize_with = "list_or_null")]
    pub keywords: Vec<String>,
    pub year: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub premier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_cinema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digital_release: Option<String>,
    #[serde(default, deserialize_with = "list_or_null")]
    pub alternative_titles: Vec<AlternativeTitleResource>,
    #[serde(default, deserialize_with = "list_or_null")]
    pub translations: Vec<TranslationResource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<Credits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub youtube_trailer_id: Option<String>,
    #[serde(default, deserialize_with = "list_or_null")]
    pub certifications: Vec<CertificationResource>,
    /// Always written, but read as optional: the real api.radarr.video sends
    /// `"Status": null` for films it has no status for, and a client that
    /// tolerates that from the upstream must tolerate it from us.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<CollectionResource>,
    /// Always written; [`UNDETERMINED`] when the language is not known.
    #[serde(default, deserialize_with = "string_or_null")]
    pub original_language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, deserialize_with = "list_or_null")]
    pub recommendations: Vec<RecommendationResource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub popularity: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct RatingResource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb: Option<RatingItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb: Option<RatingItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metacritic: Option<RatingItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotten_tomatoes: Option<RatingItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trakt: Option<RatingItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct RatingItem {
    pub count: i64,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// `User`, as api.radarr.video writes it. Radarr parses this into an enum
    /// and throws on a null, so it is always written and never optional.
    #[serde(rename = "Type", default = "user_rating")]
    pub rating_type: String,
}

fn user_rating() -> String {
    "User".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct ImageResource {
    pub cover_type: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct AlternativeTitleResource {
    pub title: String,
    /// Where the title came from, in Radarr's spelling. Another enum it parses,
    /// so it carries a value even when the provider did not name one.
    #[serde(rename = "Type", default = "tmdb_source")]
    pub title_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

fn tmdb_source() -> String {
    "Tmdb".to_string()
}

/// A title IMDb gives a film in one of the regions a caller is set to, as
/// `title.akas` lists it: what Radarr is given beside TMDB's alternative
/// titles — see [`crate::service::imdb::regional_titles`].
#[derive(Clone, Debug, PartialEq)]
pub struct RegionalTitle {
    pub title: String,
    /// IMDb's code for its language, where it gives one: most often none.
    pub language: Option<String>,
}

/// How many of IMDb's titles a film is sent at most, whatever the regions.
pub const MAX_REGIONAL: usize = 50;

/// Add IMDb's titles of a film to its alternative titles: those it is not
/// sent under already — as its title, its original title, a translation's
/// title or an alternative title — compared without regard to case,
/// spacing or punctuation.
///
/// Radarr reads every title it is sent as one more name a release of the
/// film may go by. It writes each down as TMDB's whatever `Type` says
/// (`SkyHookProxy.MapAlternativeTitle`), and `Tmdb` is all the real service
/// sends; it reads no language for one, which is sent only where IMDb names
/// one as two letters, as the real service writes them.
pub fn add_regional_titles(resource: &mut MovieResource, titles: &[RegionalTitle]) {
    let mut known: std::collections::HashSet<String> = std::iter::once(resource.title.as_str())
        .chain(resource.original_title.as_deref())
        .chain(resource.alternative_titles.iter().map(|t| t.title.as_str()))
        .chain(resource.translations.iter().map(|t| t.title.as_str()))
        .map(plain)
        .collect();

    let mut added = 0;
    for regional in titles {
        if added == MAX_REGIONAL {
            break;
        }
        let title = regional.title.trim();
        let key = plain(title);
        if key.is_empty() || !known.insert(key) {
            continue;
        }
        resource.alternative_titles.push(AlternativeTitleResource {
            title: title.to_string(),
            title_type: tmdb_source(),
            language: regional
                .language
                .as_deref()
                .filter(|language| {
                    language.len() == 2 && language.bytes().all(|b| b.is_ascii_alphabetic())
                })
                .map(str::to_ascii_lowercase),
        });
        added += 1;
    }
}

/// A title as it is compared with the others: its letters and digits, in
/// lower case.
fn plain(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct TranslationResource {
    /// Always written; see [`translated_title`].
    #[serde(default, deserialize_with = "string_or_null")]
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub language: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct CertificationResource {
    pub country: String,
    pub certification: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct Credits {
    #[serde(default, deserialize_with = "list_or_null")]
    pub cast: Vec<CastResource>,
    #[serde(default, deserialize_with = "list_or_null")]
    pub crew: Vec<CrewResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct CastResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    /// Radarr stores this with a NOT NULL constraint.
    pub credit_id: String,
    #[serde(default, deserialize_with = "list_or_null")]
    pub images: Vec<ImageResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct CrewResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    /// Radarr stores this with a NOT NULL constraint.
    pub credit_id: String,
    #[serde(default, deserialize_with = "list_or_null")]
    pub images: Vec<ImageResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct CollectionResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub tmdb_id: i64,
    #[serde(default, deserialize_with = "list_or_null")]
    pub images: Vec<ImageResource>,
    /// A movie may belong to a collection, and a collection holds movies. That
    /// loop makes schema collection recurse forever unless it is cut here.
    #[schema(no_recursion)]
    #[serde(default, deserialize_with = "list_or_null")]
    pub parts: Vec<MovieResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct RecommendationResource {
    pub tmdb_id: i64,
    pub title: String,
}

// ─── canonical → wire ────────────────────────────────────────────────────────

pub fn from_item(item: &MediaItem) -> MovieResource {
    // Radarr dereferences `resource.Ratings.FirstOrDefault()` without a null
    // check when `movieRatings.tmdb` is absent (SkyHookProxy.cs:285), so an
    // unrated title takes down the whole search response rather than just
    // itself. Always emit a TMDB entry here — zeroed when there are no votes.
    //
    // The canonical model still records no rating for an unrated title; this is
    // a concession to one client's parser, made where that client is served.
    let tmdb_rating = item.rating("tmdb").map(rating_item).unwrap_or(RatingItem {
        count: 0,
        value: 0.0,
        origin: Some("tmdb".to_string()),
        rating_type: user_rating(),
    });
    let imdb_rating = item.rating("imdb").map(rating_item);

    MovieResource {
        tmdb_id: item.external_ids.tmdb.unwrap_or(0),
        imdb_id: item.external_ids.imdb.clone(),
        overview: item.overview.clone(),
        title: item.title.clone(),
        original_title: item.original_title.clone(),
        title_slug: item.slug.clone(),
        // Radarr reads `ratings` on older versions and `movieRatings` on newer
        // ones; emitting both keeps either working.
        ratings: vec![tmdb_rating.clone()],
        movie_ratings: Some(RatingResource {
            tmdb: Some(tmdb_rating),
            imdb: imdb_rating,
            metacritic: item.rating("metacritic").map(rating_item),
            rotten_tomatoes: item.rating("rottenTomatoes").map(rating_item),
            trakt: item.rating("trakt").map(rating_item),
        }),
        runtime: item.runtime,
        // One a kind: Radarr writes every image of a kind to one file.
        images: crate::domain::lead_images(
            item.images.iter().filter(|i| i.season_number.is_none()),
            &item.primary_images,
        )
        .into_iter()
        .map(image_resource)
        .collect(),
        genres: item.genres.clone(),
        keywords: item.keywords.clone(),
        year: item.year.unwrap_or(0),
        premier: item.in_cinemas.clone(),
        in_cinema: item.in_cinemas.clone(),
        physical_release: item.physical_release.clone(),
        digital_release: item.digital_release.clone(),
        alternative_titles: item
            .alternative_titles
            .iter()
            .map(|t| AlternativeTitleResource {
                title: t.title.clone(),
                title_type: t.title_type.clone().unwrap_or_else(tmdb_source),
                language: t.language.clone(),
            })
            .collect(),
        translations: item
            .translations
            .iter()
            .map(|t| TranslationResource {
                title: translated_title(t, item),
                overview: t.overview.clone(),
                language: t.language.clone(),
            })
            .collect(),
        credits: Some(credits(&item.credits)),
        studio: item.studio.clone(),
        youtube_trailer_id: item.trailer_youtube_id.clone(),
        // Every country's, as api.radarr.video sends them: Radarr takes the
        // one of the country it is set to (`CertificationCountry`, ISO 3166-1
        // alpha-2 upper case), so it picks its own. The production country
        // is not a certification country, and must not stand in for one.
        certifications: item
            .certifications()
            .into_iter()
            .map(|(country, certification)| CertificationResource {
                country,
                certification,
            })
            .collect(),
        status: Some(radarr_status(item.status.as_deref())),
        collection: None,
        original_language: non_empty(item.original_language.as_deref())
            .unwrap_or_else(|| UNDETERMINED.to_string()),
        homepage: item.homepage.clone(),
        recommendations: Vec::new(),
        popularity: item.popularity,
    }
}

/// How many films one is sent recommended beside it, at most: a page of
/// TMDB's. api.radarr.video sends ten.
pub const MAX_RECOMMENDED: usize = 20;

/// What a film is sent with beside its own record, which holds neither: the
/// collection it is part of — Radarr adds the collection, and offers its
/// other films, from this — and the films recommended with it, which feed
/// Radarr's Discover page. Read from what the providers answered; see
/// [`crate::service::movie::beside`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Beside {
    pub collection: Option<NamedCollection>,
    /// By TMDB id and title, the most recommended first.
    pub recommended: Vec<(i64, String)>,
}

/// A collection as far as it is known: its TMDB id and its name, and its
/// pictures' addresses where TMDB gave them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedCollection {
    pub tmdb_id: i64,
    pub name: String,
    pub poster: Option<String>,
    pub fanart: Option<String>,
}

/// [`from_item`], with what goes beside the film.
///
/// The collection only when the film is filed under it, and only with a name:
/// Radarr makes its own record of a collection from the first film that names
/// it, title and all. The recommendations as api.radarr.video sends them — by
/// TMDB id and title, of which Radarr keeps the id — once each, never the
/// film itself, and [`MAX_RECOMMENDED`] at most.
pub fn from_item_beside(item: &MediaItem, beside: &Beside) -> MovieResource {
    let mut resource = from_item(item);

    resource.collection = beside
        .collection
        .as_ref()
        .filter(|c| item.collection_tmdb_id == Some(c.tmdb_id))
        .and_then(|c| {
            Some(CollectionResource {
                name: non_empty(Some(&c.name))?,
                overview: None,
                tmdb_id: c.tmdb_id,
                images: collection_images(c.poster.as_deref(), c.fanart.as_deref()),
                parts: Vec::new(),
            })
        });

    let mut seen = std::collections::HashSet::new();
    resource.recommendations = beside
        .recommended
        .iter()
        .filter(|(tmdb_id, _)| {
            *tmdb_id > 0 && Some(*tmdb_id) != item.external_ids.tmdb && seen.insert(*tmdb_id)
        })
        .take(MAX_RECOMMENDED)
        .map(|(tmdb_id, title)| RecommendationResource {
            tmdb_id: *tmdb_id,
            title: title.trim().to_string(),
        })
        .collect();

    resource
}

/// A collection's poster and background, as Radarr reads a collection's
/// pictures, where they are known.
pub fn collection_images(poster: Option<&str>, fanart: Option<&str>) -> Vec<ImageResource> {
    [
        (crate::domain::CoverType::Poster, poster),
        (crate::domain::CoverType::Fanart, fanart),
    ]
    .into_iter()
    .filter_map(|(cover_type, url)| {
        Some(ImageResource {
            cover_type: cover_type.as_str().to_string(),
            url: non_empty(url)?,
        })
    })
    .collect()
}

/// The title a translation is sent with: its own, or else the one the film goes
/// by in that language, which is its original title.
///
/// Radarr cleans every translated title and, when it looks for releases of the
/// film, calls `Replace` on each of them without a null check
/// (`ParsingService.TryGetMovieBySearchCriteria`). One translation sent without
/// a title fails every interactive search of the film, and the release list
/// with it. api.radarr.video never sends one: where TMDB has a synopsis in a
/// language but no title, it sends the original title, and so does this.
fn translated_title(translation: &crate::domain::Translation, item: &MediaItem) -> String {
    [translation.title.as_deref(), item.original_title.as_deref()]
        .into_iter()
        .flatten()
        .find(|title| !title.trim().is_empty())
        .unwrap_or(item.title.as_str())
        .to_string()
}

/// Radarr writes every credit into a table where `CreditTmdbId` is NOT NULL, so
/// one credit missing that identifier aborts the whole movie refresh inside
/// Radarr's database. Credits we cannot identify are dropped instead: losing one
/// name is better than losing the movie.
fn credits(all: &[Credit]) -> Credits {
    Credits {
        cast: all
            .iter()
            .filter(|c| c.credit_type == CreditType::Actor)
            .filter_map(|c| {
                Some(CastResource {
                    name: c.person_name.clone(),
                    character: c.character_name.clone(),
                    order: Some(c.sort_order),
                    tmdb_id: c.tmdb_person_id,
                    credit_id: c.credit_tmdb_id.clone()?,
                    images: person_image(c.image.as_deref()),
                })
            })
            .collect(),
        crew: all
            .iter()
            .filter(|c| c.credit_type != CreditType::Actor)
            .filter_map(|c| {
                Some(CrewResource {
                    name: c.person_name.clone(),
                    // The job as TMDB names it, `Screenplay`, where it was
                    // kept; else the kind of credit.
                    job: c
                        .character_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|job| !job.is_empty())
                        .map(String::from)
                        .or_else(|| Some(capitalize(c.credit_type.as_str()))),
                    department: Some(department_for(c.credit_type)),
                    order: Some(c.sort_order),
                    tmdb_id: c.tmdb_person_id,
                    credit_id: c.credit_tmdb_id.clone()?,
                    images: person_image(c.image.as_deref()),
                })
            })
            .collect(),
    }
}

fn person_image(url: Option<&str>) -> Vec<ImageResource> {
    url.map(|u| {
        vec![ImageResource {
            cover_type: "headshot".to_string(),
            url: u.to_string(),
        }]
    })
    .unwrap_or_default()
}

fn department_for(credit_type: CreditType) -> String {
    match credit_type {
        CreditType::Director => "Directing",
        CreditType::Producer => "Production",
        CreditType::Writer => "Writing",
        _ => "Acting",
    }
    .to_string()
}

fn image_resource(image: &Image) -> ImageResource {
    ImageResource {
        cover_type: image.cover_type.as_str().to_string(),
        url: image.url.clone(),
    }
}

fn rating_item(rating: &Rating) -> RatingItem {
    RatingItem {
        count: rating.votes.unwrap_or(0),
        value: rating.value.unwrap_or(0.0),
        origin: Some(rating.source.clone()),
        rating_type: rating.rating_type.clone().unwrap_or_else(user_rating),
    }
}

/// Radarr's `MovieStatusType`: `tba`, `announced`, `inCinemas`, `released`, `deleted`.
fn radarr_status(status: Option<&str>) -> String {
    match status.unwrap_or("tba").to_ascii_lowercase().as_str() {
        "released" => "released",
        "incinemas" => "inCinemas",
        "announced" => "announced",
        "deleted" => "deleted",
        _ => "tba",
    }
    .to_string()
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ─── wire → canonical ────────────────────────────────────────────────────────

/// Absorb a response from the real `api.radarr.video` into the canonical model.
///
/// Radarr's own metadata service is a well-curated view of TMDB with extra
/// ratings and certifications attached, so it is worth taking as a second
/// opinion rather than only as a fallback.
pub fn to_item(resource: &MovieResource) -> MediaItem {
    use crate::{
        db::{new_id, now},
        domain::{AlternativeTitle, CoverType, ExternalIds, MediaKind, Translation},
    };

    let mut item = MediaItem::empty(MediaKind::Movie);

    item.title = resource.title.clone();
    item.original_title = non_empty(resource.original_title.as_deref());
    item.overview = non_empty(resource.overview.as_deref());
    item.homepage = non_empty(resource.homepage.as_deref());
    // The placeholder this server writes for a language it does not know is not
    // a language to absorb back.
    item.original_language =
        non_empty(Some(&resource.original_language)).filter(|l| l != UNDETERMINED);
    item.runtime = resource.runtime.filter(|r| *r > 0);
    item.year = (resource.year > 0).then_some(resource.year);
    item.studio = non_empty(resource.studio.as_deref());
    item.trailer_youtube_id = non_empty(resource.youtube_trailer_id.as_deref());
    item.popularity = resource.popularity;
    item.genres = resource.genres.clone();
    item.keywords = resource.keywords.clone();
    item.status = resource.status.as_deref().map(str::to_ascii_lowercase);
    item.in_cinemas =
        non_empty(resource.in_cinema.as_deref()).or_else(|| non_empty(resource.premier.as_deref()));
    item.physical_release = non_empty(resource.physical_release.as_deref());
    item.digital_release = non_empty(resource.digital_release.as_deref());
    item.slug = if resource.title_slug.trim().is_empty() {
        crate::domain::make_slug(&resource.title, item.year)
    } else {
        resource.title_slug.clone()
    };

    item.external_ids = ExternalIds {
        tmdb: (resource.tmdb_id > 0).then_some(resource.tmdb_id),
        imdb: resource
            .imdb_id
            .as_deref()
            .and_then(crate::domain::ids::normalize_imdb_id),
        ..Default::default()
    };

    // Radarr's certifications carry the country the rating applies to, which is
    // the part TMDB's own block makes us dig for. Every country's is kept; the
    // record's own is the American one, as TMDB's is, and else the first —
    // api.radarr.video lists them in the order of their codes, so the first
    // was Argentina's.
    item.releases = certifications(&resource.certifications);
    if let Some(cert) = item
        .releases
        .iter()
        .find(|r| r.country == "US")
        .or_else(|| item.releases.first())
    {
        item.content_rating.clone_from(&cert.certification);
        item.content_rating_country = Some(cert.country.clone());
    }

    item.ratings = collect_ratings(resource);

    item.images = resource
        .images
        .iter()
        .take(MAX_IMAGES)
        .enumerate()
        .map(|(i, image)| Image {
            id: new_id(),
            season_number: None,
            cover_type: image.cover_type.parse().unwrap_or(CoverType::Unknown),
            url: image.url.clone(),
            language: None,
            sort_order: i as i32,
            source: Some(crate::providers::names::RADARR.to_string()),
            is_manual: false,
        })
        .collect();

    item.alternative_titles = resource
        .alternative_titles
        .iter()
        .take(MAX_TITLES)
        .filter(|t| !t.title.trim().is_empty())
        .map(|t| AlternativeTitle {
            id: new_id(),
            title: t.title.clone(),
            title_type: Some(t.title_type.clone()),
            language: t.language.clone(),
            is_manual: false,
        })
        .collect();

    item.translations = resource
        .translations
        .iter()
        .take(MAX_TITLES)
        .filter(|t| !t.title.trim().is_empty() || t.overview.is_some())
        .map(|t| Translation {
            language: t.language.clone(),
            title: non_empty(Some(&t.title)),
            overview: non_empty(t.overview.as_deref()),
            tagline: None,
            is_manual: false,
        })
        .collect();

    if let Some(credits) = &resource.credits {
        item.credits = to_credits(credits);
    }

    item.updated_at = now();
    item
}

/// api.radarr.video's certifications, a country each: one with no country
/// code, or none Radarr could match, is left out.
fn certifications(all: &[CertificationResource]) -> Vec<CountryRelease> {
    let mut out: Vec<CountryRelease> = Vec::new();
    for cert in all.iter().take(MAX_TITLES) {
        let country = cert.country.trim().to_ascii_uppercase();
        let Some(certification) = non_empty(Some(&cert.certification)) else {
            continue;
        };
        if country.len() != 2
            || !country.bytes().all(|b| b.is_ascii_uppercase())
            || out.iter().any(|r| r.country == country)
        {
            continue;
        }
        out.push(CountryRelease {
            country,
            release_type: None,
            date: None,
            certification: Some(certification),
            note: None,
            source: Some(crate::providers::names::RADARR.to_string()),
        });
    }
    out
}

fn collect_ratings(resource: &MovieResource) -> Vec<Rating> {
    let Some(ratings) = &resource.movie_ratings else {
        return Vec::new();
    };

    [
        ("tmdb", &ratings.tmdb),
        ("imdb", &ratings.imdb),
        ("metacritic", &ratings.metacritic),
        ("rottenTomatoes", &ratings.rotten_tomatoes),
        ("trakt", &ratings.trakt),
    ]
    .into_iter()
    .filter_map(|(source, item)| {
        let item = item.as_ref()?;
        // A zeroed entry is the placeholder this server itself emits for an
        // unrated title; absorbing it back would invent a rating of 0/10.
        (item.count > 0).then(|| Rating {
            source: source.to_string(),
            value: Some(item.value),
            votes: Some(item.count),
            rating_type: Some(item.rating_type.clone()),
        })
    })
    .collect()
}

fn to_credits(credits: &Credits) -> Vec<Credit> {
    use crate::db::new_id;

    let cast = credits.cast.iter().enumerate().map(|(i, member)| Credit {
        id: new_id(),
        credit_type: CreditType::Actor,
        person_name: member.name.clone(),
        character_name: member.character.clone(),
        image: member.images.first().map(|i| i.url.clone()),
        tmdb_person_id: member.tmdb_id,
        credit_tmdb_id: Some(member.credit_id.clone()).filter(|c| !c.is_empty()),
        sort_order: member.order.unwrap_or(i as i32),
        is_manual: false,
    });

    // The crew the catalogue keeps — directors, writers, producers — with
    // the job Radarr names: anyone else was filed as cast, the composer and
    // the editor among the actors.
    let crew = credits.crew.iter().enumerate().filter_map(|(i, member)| {
        Some(Credit {
            id: new_id(),
            credit_type: crew_kind(member)?,
            person_name: member.name.clone(),
            character_name: non_empty(member.job.as_deref()),
            image: member.images.first().map(|i| i.url.clone()),
            tmdb_person_id: member.tmdb_id,
            credit_tmdb_id: Some(member.credit_id.clone()).filter(|c| !c.is_empty()),
            sort_order: member.order.unwrap_or(i as i32),
            is_manual: false,
        })
    });

    cast.chain(crew).collect()
}

/// What one of Radarr's crew is here, by the job and the department it
/// gives: a director, a writer, a producer, or nobody the catalogue keeps.
fn crew_kind(member: &CrewResource) -> Option<CreditType> {
    let job = member.job.as_deref().map(str::trim).unwrap_or_default();
    let department = member.department.as_deref().map(str::trim);
    if job.eq_ignore_ascii_case("director") {
        Some(CreditType::Director)
    } else if department == Some("Writing") || job.eq_ignore_ascii_case("writer") {
        Some(CreditType::Writer)
    } else if job.eq_ignore_ascii_case("producer") {
        Some(CreditType::Producer)
    } else {
        None
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CoverType, ExternalIds, MediaKind};

    fn item() -> MediaItem {
        let mut it = MediaItem::empty(MediaKind::Movie);
        it.title = "Arrival".into();
        it.slug = "arrival-2016".into();
        it.year = Some(2016);
        it.runtime = Some(116);
        it.status = Some("released".into());
        it.in_cinemas = Some("2016-11-11".into());
        it.digital_release = Some("2017-01-31".into());
        it.content_rating = Some("PG-13".into());
        it.original_country = Some("usa".into());
        it.content_rating_country = Some("US".into());
        it.external_ids = ExternalIds {
            tmdb: Some(329865),
            imdb: Some("tt2543164".into()),
            ..Default::default()
        };
        it.ratings = vec![Rating {
            source: "tmdb".into(),
            value: Some(7.6),
            votes: Some(18_000),
            rating_type: Some("user".into()),
        }];
        it.credits = vec![
            Credit {
                id: "1".into(),
                credit_type: CreditType::Actor,
                person_name: "Amy Adams".into(),
                character_name: Some("Louise Banks".into()),
                image: Some("https://example.invalid/a.jpg".into()),
                tmdb_person_id: Some(9273),
                credit_tmdb_id: Some("52fe4726c3a36847f812048b".into()),
                sort_order: 0,
                is_manual: false,
            },
            Credit {
                id: "2".into(),
                credit_type: CreditType::Director,
                person_name: "Denis Villeneuve".into(),
                character_name: None,
                image: None,
                tmdb_person_id: Some(137427),
                credit_tmdb_id: Some("5751b41cc3a3685ba7002c1f".into()),
                sort_order: 0,
                is_manual: false,
            },
        ];
        it.images = vec![Image {
            id: "i".into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: "https://example.invalid/p.jpg".into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: false,
        }];
        it
    }

    #[test]
    fn statuses_use_radarrs_names() {
        assert_eq!(radarr_status(Some("released")), "released");
        assert_eq!(radarr_status(Some("inCinemas")), "inCinemas");
        assert_eq!(radarr_status(Some("announced")), "announced");
        assert_eq!(radarr_status(None), "tba");
        assert_eq!(radarr_status(Some("nonsense")), "tba");
    }

    #[test]
    fn both_rating_shapes_are_emitted() {
        let resource = from_item(&item());

        // Old Radarr reads `ratings`, new Radarr reads `movieRatings`.
        assert_eq!(resource.ratings.len(), 1);
        assert_eq!(resource.ratings[0].value, 7.6);
        assert_eq!(resource.movie_ratings.unwrap().tmdb.unwrap().count, 18_000);
    }

    #[test]
    fn a_credit_with_no_identifier_is_dropped_rather_than_sent() {
        // Radarr's Credits.CreditTmdbId is NOT NULL: sending one without it
        // fails the whole movie refresh, not just that credit.
        let mut it = item();
        it.credits[0].credit_tmdb_id = None;

        let credits = from_item(&it).credits.unwrap();
        assert_eq!(credits.cast.len(), 0);
        assert_eq!(credits.crew.len(), 1, "the crew credit still has its id");
    }

    #[test]
    fn credit_identifiers_are_emitted() {
        let credits = from_item(&item()).credits.unwrap();
        assert_eq!(credits.cast[0].credit_id, "52fe4726c3a36847f812048b");
        assert_eq!(credits.crew[0].credit_id, "5751b41cc3a3685ba7002c1f");
    }

    #[test]
    fn cast_and_crew_are_separated() {
        let credits = from_item(&item()).credits.unwrap();

        assert_eq!(credits.cast.len(), 1);
        assert_eq!(credits.cast[0].name, "Amy Adams");
        assert_eq!(credits.cast[0].images.len(), 1);

        assert_eq!(credits.crew.len(), 1);
        assert_eq!(credits.crew[0].job.as_deref(), Some("Director"));
        assert_eq!(credits.crew[0].department.as_deref(), Some("Directing"));
    }

    #[test]
    fn premier_and_in_cinema_carry_the_same_date() {
        // Radarr reads whichever of the two its version knows about.
        let resource = from_item(&item());
        assert_eq!(resource.premier.as_deref(), Some("2016-11-11"));
        assert_eq!(resource.in_cinema.as_deref(), Some("2016-11-11"));
    }

    #[test]
    fn a_content_rating_becomes_a_certification_with_an_alpha2_country() {
        // Radarr matches on `country == "US"`; the alpha-3 production country
        // would never match and the certification would silently vanish.
        let resource = from_item(&item());
        assert_eq!(resource.certifications.len(), 1);
        assert_eq!(resource.certifications[0].certification, "PG-13");
        assert_eq!(resource.certifications[0].country, "US");
    }

    #[test]
    fn a_rating_with_no_known_country_is_not_guessed_at() {
        let mut it = item();
        it.content_rating_country = None;
        assert!(from_item(&it).certifications.is_empty());
    }

    #[test]
    fn an_unrated_movie_still_carries_a_zeroed_tmdb_rating() {
        // Radarr dereferences this without a null check; leaving it out fails
        // the entire search response, not just this one title.
        let mut it = item();
        it.ratings.clear();

        let resource = from_item(&it);

        assert_eq!(resource.ratings.len(), 1);
        assert_eq!(resource.ratings[0].count, 0);

        let tmdb = resource.movie_ratings.unwrap().tmdb.unwrap();
        assert_eq!(tmdb.count, 0);
        assert_eq!(tmdb.value, 0.0);
        // Radarr parses this into an enum; a null would throw.
        assert_eq!(tmdb.rating_type, "User");
    }

    #[test]
    fn a_radarr_response_round_trips_through_the_canonical_model() {
        let original = from_item(&item());
        let canonical = to_item(&original);

        assert_eq!(canonical.title, "Arrival");
        assert_eq!(canonical.year, Some(2016));
        assert_eq!(canonical.runtime, Some(116));
        assert_eq!(canonical.status.as_deref(), Some("released"));
        assert_eq!(canonical.external_ids.tmdb, Some(329865));
        assert_eq!(canonical.external_ids.imdb.as_deref(), Some("tt2543164"));
        assert_eq!(canonical.content_rating.as_deref(), Some("PG-13"));
        assert_eq!(canonical.content_rating_country.as_deref(), Some("US"));
        assert_eq!(canonical.in_cinemas.as_deref(), Some("2016-11-11"));
        assert_eq!(canonical.credits.len(), 2);
        assert_eq!(canonical.images.len(), 1);
    }

    #[test]
    fn the_zeroed_rating_placeholder_is_not_absorbed_back() {
        // `from_item` emits a zeroed TMDB entry for an unrated title so Radarr
        // does not crash on it. Reading that back as a real rating of 0/10 would
        // be worse than having none.
        let mut it = item();
        it.ratings.clear();

        let canonical = to_item(&from_item(&it));
        assert!(canonical.ratings.is_empty());
    }

    #[test]
    fn a_real_rating_does_survive_the_round_trip() {
        let canonical = to_item(&from_item(&item()));

        assert_eq!(canonical.ratings.len(), 1);
        assert_eq!(canonical.ratings[0].source, "tmdb");
        assert_eq!(canonical.ratings[0].votes, Some(18_000));
    }

    #[test]
    fn the_upstreams_nulls_are_read_as_empty_lists() {
        // api.radarr.video answers exactly this for The Matrix: a status it has
        // no value for, and a collection whose lists are null rather than [].
        let body = serde_json::json!({
            "TmdbId": 603,
            "Title": "The Matrix",
            "TitleSlug": "603",
            "Year": 1999,
            "Status": null,
            "Collection": {
                "TmdbId": 2344,
                "Name": "The Matrix Collection",
                "Images": null,
                "Overview": null,
                "Translations": null,
                "Parts": null
            }
        });

        let movie: MovieResource = serde_json::from_value(body).expect("reads");

        assert_eq!(movie.title, "The Matrix");
        assert_eq!(movie.status, None);

        let collection = movie.collection.expect("collection");
        assert!(collection.parts.is_empty());
        assert!(collection.images.is_empty());
    }

    #[test]
    fn a_rating_without_a_type_is_read_as_a_user_rating() {
        // The field is an enum on Radarr's side; it must never reach it empty.
        let item: RatingItem =
            serde_json::from_value(serde_json::json!({ "Count": 1, "Value": 8.0 })).unwrap();

        assert_eq!(item.rating_type, "User");
    }

    #[test]
    fn the_wire_form_serialises_to_radarrs_field_names() {
        // api.radarr.video answers in PascalCase, and this server stands in for
        // it: the same names have to come back, or reading its replies and
        // writing our own would need two different shapes of this type.
        let json = serde_json::to_value(from_item(&item())).unwrap();

        assert_eq!(json["TmdbId"], 329865);
        assert_eq!(json["TitleSlug"], "arrival-2016");
        assert_eq!(json["InCinema"], "2016-11-11");
        assert_eq!(json["DigitalRelease"], "2017-01-31");
        assert_eq!(json["YoutubeTrailerId"], serde_json::Value::Null);
    }

    fn translation(language: &str, title: Option<&str>) -> crate::domain::Translation {
        crate::domain::Translation {
            language: language.into(),
            title: title.map(String::from),
            overview: Some("A linguist is recruited when twelve ships land.".into()),
            tagline: None,
            is_manual: false,
        }
    }

    #[test]
    fn every_translation_is_sent_with_a_title() {
        // Radarr calls `Replace` on every translated title when it looks for
        // releases. One sent without a title failed every interactive search
        // of the film: "Object reference not set to an instance of an object"
        // in `TryGetMovieBySearchCriteria`, then in the release list itself.
        let mut it = item();
        // Served in French: the title is the French one, the original is not.
        it.title = "Premier Contact".into();
        it.original_title = Some("Arrival".into());
        it.translations = vec![
            translation("fra", Some("Premier Contact")),
            translation("dan", None),
            translation("nld", Some("  ")),
        ];

        let resource = from_item(&it);
        let titles: Vec<&str> = resource
            .translations
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        // What api.radarr.video sends where TMDB has no title in a language.
        assert_eq!(titles, ["Premier Contact", "Arrival", "Arrival"]);

        let json = serde_json::to_value(&resource).unwrap();
        let sent = json["Translations"].as_array().unwrap();
        assert!(sent.iter().all(|t| t["Title"].is_string()), "{sent:?}");

        // A work with no original title falls back on the one it is served under.
        it.original_title = None;
        assert_eq!(from_item(&it).translations[1].title, "Premier Contact");
    }

    #[test]
    fn a_film_of_unknown_language_is_sent_one_radarr_can_read() {
        // Radarr lowercases this without a null check. Leaving it out failed the
        // film's refresh, and every search whose results included it.
        let mut it = item();
        it.original_language = None;

        let resource = from_item(&it);
        assert_eq!(resource.original_language, "und");
        let json = serde_json::to_value(&resource).unwrap();
        assert_eq!(json["OriginalLanguage"], "und");

        // The placeholder is not read back as a language.
        assert_eq!(to_item(&resource).original_language, None);

        it.original_language = Some("eng".into());
        assert_eq!(from_item(&it).original_language, "eng");
    }

    #[test]
    fn an_upstream_translation_without_a_title_keeps_its_synopsis() {
        let body = serde_json::json!({
            "TmdbId": 603,
            "Title": "The Matrix",
            "TitleSlug": "603",
            "Year": 1999,
            "OriginalLanguage": null,
            "Translations": [
                { "Title": null, "Overview": "Forestil dig…", "Language": "da-DK" },
                { "Overview": "Tenk deg…", "Language": "no-NO" },
                { "Title": "Matrix", "Language": "de-DE" }
            ]
        });

        let movie: MovieResource = serde_json::from_value(body).expect("reads");
        let canonical = to_item(&movie);

        assert_eq!(canonical.original_language, None);
        let read: Vec<(Option<&str>, bool)> = canonical
            .translations
            .iter()
            .map(|t| (t.title.as_deref(), t.overview.is_some()))
            .collect();
        assert_eq!(read, [(None, true), (None, true), (Some("Matrix"), false)]);
    }

    /// What goes beside The Matrix, as api.radarr.video has it.
    fn beside_the_matrix() -> Beside {
        Beside {
            collection: Some(NamedCollection {
                tmdb_id: 2344,
                name: "The Matrix Collection".into(),
                poster: Some(
                    "https://image.tmdb.org/t/p/original/bV9qTVHTVf0gkW0j7p7M0ILD4pG.jpg".into(),
                ),
                fanart: None,
            }),
            recommended: vec![
                (99861, "Avengers: Age of Ultron".into()),
                (87101, "Terminator Genisys".into()),
            ],
        }
    }

    fn the_matrix() -> MediaItem {
        let mut it = item();
        it.title = "The Matrix".into();
        it.external_ids.tmdb = Some(603);
        it.collection_tmdb_id = Some(2344);
        it
    }

    #[test]
    fn a_film_is_sent_its_collection_and_recommendations_as_the_upstream_writes_them() {
        // Radarr adds a film's collection, and offers the rest of it, from
        // `Collection`; its Discover page is fed from `Recommendations`. Both
        // went out empty, so Radarr's collections stayed empty.
        let json =
            serde_json::to_value(from_item_beside(&the_matrix(), &beside_the_matrix())).unwrap();

        assert_eq!(json["Collection"]["TmdbId"], 2344);
        assert_eq!(json["Collection"]["Name"], "The Matrix Collection");
        assert_eq!(json["Collection"]["Images"][0]["CoverType"], "poster");
        assert_eq!(
            json["Recommendations"],
            serde_json::json!([
                { "TmdbId": 99861, "Title": "Avengers: Age of Ultron" },
                { "TmdbId": 87101, "Title": "Terminator Genisys" },
            ])
        );

        // What is read back is what api.radarr.video would have been read for.
        let read: MovieResource = serde_json::from_value(json).unwrap();
        assert_eq!(read.collection.map(|c| c.tmdb_id), Some(2344));
        assert_eq!(read.recommendations.len(), 2);
    }

    #[test]
    fn a_collection_goes_only_with_its_name_and_only_to_its_own_films() {
        // Radarr makes its record of the collection from the first film that
        // names it: one without a name is not sent at all.
        let mut nameless = beside_the_matrix();
        nameless.collection.as_mut().unwrap().name = "  ".into();
        assert!(
            from_item_beside(&the_matrix(), &nameless)
                .collection
                .is_none()
        );

        // What is known of 2344 is not the collection of a film filed under
        // another one, or under none.
        let mut elsewhere = the_matrix();
        elsewhere.collection_tmdb_id = Some(10);
        assert!(
            from_item_beside(&elsewhere, &beside_the_matrix())
                .collection
                .is_none()
        );
        elsewhere.collection_tmdb_id = None;
        let json =
            serde_json::to_value(from_item_beside(&elsewhere, &beside_the_matrix())).unwrap();
        assert!(
            json.get("Collection")
                .is_none_or(serde_json::Value::is_null)
        );
        // The recommendations go all the same.
        assert_eq!(json["Recommendations"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn recommendations_go_once_each_without_the_film_itself_and_twenty_at_most() {
        let recommended: Vec<(i64, String)> = [
            (603, "The Matrix".to_string()),
            (1, "Film 1".to_string()),
            (1, "Film 1, again".to_string()),
            (0, "No id".to_string()),
        ]
        .into_iter()
        .chain((2..=30).map(|n| (n, format!("Film {n}"))))
        .collect();

        let sent = from_item_beside(
            &the_matrix(),
            &Beside {
                collection: None,
                recommended,
            },
        )
        .recommendations;

        let ids: Vec<i64> = sent.iter().map(|r| r.tmdb_id).collect();
        assert_eq!(ids, (1..=20).collect::<Vec<i64>>());
    }

    #[test]
    fn a_search_result_goes_without_either() {
        // Search results are shallow: nothing beside them is looked up.
        let resource = from_item(&the_matrix());
        assert!(resource.collection.is_none());
        assert!(resource.recommendations.is_empty());
    }

    #[test]
    fn the_upstreams_recommendations_are_read() {
        // api.radarr.video's Interstellar: no collection, and its
        // recommendations by id and title.
        let body = serde_json::json!({
            "TmdbId": 157336,
            "Title": "Interstellar",
            "TitleSlug": "157336",
            "Year": 2014,
            "Collection": null,
            "Recommendations": [
                { "TmdbId": 687163, "Title": "Project Hail Mary" },
                { "TmdbId": 299534, "Title": "Avengers: Endgame" }
            ]
        });

        let movie: MovieResource = serde_json::from_value(body).expect("reads");

        assert!(movie.collection.is_none());
        let read: Vec<(i64, &str)> = movie
            .recommendations
            .iter()
            .map(|r| (r.tmdb_id, r.title.as_str()))
            .collect();
        assert_eq!(
            read,
            [(687163, "Project Hail Mary"), (299534, "Avengers: Endgame")]
        );
    }

    #[test]
    fn every_certification_of_the_upstream_is_kept_and_the_american_is_the_record_s() {
        // api.radarr.video's The Matrix, in part: its countries in the order
        // of their codes, one written with a space after it.
        let body = serde_json::json!({
            "TmdbId": 603,
            "Title": "The Matrix",
            "TitleSlug": "603",
            "Year": 1999,
            "Certifications": [
                { "Country": "AR", "Certification": "13" },
                { "Country": "FR", "Certification": "TP" },
                { "Country": "GR", "Certification": "K-8 " },
                { "Country": "US", "Certification": "R" }
            ]
        });
        let movie: MovieResource = serde_json::from_value(body).expect("reads");
        let item = to_item(&movie);

        // Argentina's came first, and was the record's.
        assert_eq!(item.content_rating.as_deref(), Some("R"));
        assert_eq!(item.content_rating_country.as_deref(), Some("US"));
        assert_eq!(item.releases.len(), 4);
        assert_eq!(item.certification_in("GR"), Some("K-8"));

        // Sent on as they came: Radarr takes its own country's.
        let sent: Vec<(String, String)> = from_item(&item)
            .certifications
            .into_iter()
            .map(|c| (c.country, c.certification))
            .collect();
        let expected: Vec<(String, String)> =
            [("AR", "13"), ("FR", "TP"), ("GR", "K-8"), ("US", "R")]
                .into_iter()
                .map(|(c, r)| (c.to_string(), r.to_string()))
                .collect();
        assert_eq!(sent, expected);
    }

    #[test]
    fn a_rating_locked_for_a_country_is_the_one_sent_for_it() {
        let mut it = item();
        it.releases = vec![CountryRelease {
            country: "US".into(),
            release_type: None,
            date: None,
            certification: Some("R".into()),
            note: None,
            source: Some("tmdb".into()),
        }];
        // The record says PG-13 for the United States: somebody locked it.
        let sent = from_item(&it).certifications;
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].certification, "PG-13");
    }

    #[test]
    fn only_the_crew_the_catalogue_keeps_is_taken_from_the_upstream() {
        let body = serde_json::json!({
            "TmdbId": 329865, "Title": "Arrival", "TitleSlug": "329865", "Year": 2016,
            "Credits": { "Cast": [], "Crew": [
                { "Name": "Denis Villeneuve", "Job": "Director", "Department": "Directing", "CreditId": "a" },
                { "Name": "Eric Heisserer", "Job": "Screenplay", "Department": "Writing", "CreditId": "b" },
                { "Name": "Shawn Levy", "Job": "Producer", "Department": "Production", "CreditId": "c" },
                { "Name": "Jóhann Jóhannsson", "Job": "Original Music Composer", "Department": "Sound", "CreditId": "d" }
            ] }
        });
        let movie: MovieResource = serde_json::from_value(body).expect("reads");
        let kept: Vec<(CreditType, Option<String>)> = to_item(&movie)
            .credits
            .into_iter()
            .map(|c| (c.credit_type, c.character_name))
            .collect();
        assert_eq!(
            kept,
            [
                (CreditType::Director, Some("Director".to_string())),
                (CreditType::Writer, Some("Screenplay".to_string())),
                (CreditType::Producer, Some("Producer".to_string())),
            ],
            "the composer is not filed as cast"
        );
    }

    #[test]
    fn a_collections_pictures_are_those_known() {
        let images = collection_images(Some("https://p.jpg"), Some(" "));
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].cover_type, "poster");
        assert!(collection_images(None, None).is_empty());
    }
}

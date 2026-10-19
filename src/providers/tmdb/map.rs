//! TMDB → canonical model.
//!
//! This grew out of an earlier Skyhook stand-in's mapper, retargeted: it now
//! produces a canonical [`MediaItem`] rather than a Skyhook response, and it
//! handles movies as well as series. Rendering to Sonarr's or Radarr's wire
//! format happens later, in `api::compat`, from the canonical form.

use crate::{
    db::{new_id, now},
    domain::{
        AlternativeTitle, CountryRelease, CoverType, Credit, CreditType, Episode, EpisodeOrder,
        ExternalIds, Image, MediaItem, MediaKind, OrderKind, OrderRecord, PlacedEpisode, Rating,
        RatingValue, ReleaseKind, Season, Translation, make_slug,
    },
    providers::{
        lang::{iso_639_1_to_3, iso_3166_2_to_3},
        tmdb::{image_url, models},
    },
};

/// Cast members kept per work when nothing says otherwise: where the
/// `tmdb.castLimit` setting starts. Sonarr shows a handful; the full list for
/// a long-running series runs to hundreds of rows with no consumer.
pub const DEFAULT_CAST: usize = 25;

/// The most `tmdb.castLimit` keeps: every one is a row, and a portrait to fetch.
pub const MAX_CAST: usize = 200;

/// Writers, and producers, kept of each: a film's crew names a dozen of
/// either, and a page shows a few.
const MAX_CREW: usize = 6;

/// Region preferred when a provider offers per-country data.
const PREFERRED_REGION: &str = "US";

// ─── series ──────────────────────────────────────────────────────────────────

/// A series, keeping `cast` of its actors.
pub fn tv_to_item(tv: &models::Tv, seasons: &[models::Season], cast: usize) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Series);

    item.title = tv.name.clone();
    item.is_adult = tv.adult.unwrap_or(false);
    item.original_title = non_empty(tv.original_name.as_deref());
    item.overview = non_empty(tv.overview.as_deref());
    item.tagline = non_empty(tv.tagline.as_deref());
    item.homepage = non_empty(tv.homepage.as_deref());
    item.status =
        Some(series_status(tv.status.as_deref(), tv.first_air_date.as_deref()).to_string());
    item.original_language = tv.original_language.as_deref().map(iso_639_1_to_3);
    item.original_country = tv.origin_country.first().map(|c| iso_3166_2_to_3(c));
    item.first_aired = non_empty(tv.first_air_date.as_deref());
    item.last_aired = non_empty(tv.last_air_date.as_deref());
    item.year = year_of(tv.first_air_date.as_deref());
    item.runtime = tv
        .episode_run_time
        .first()
        .copied()
        .or_else(|| typical_runtime(seasons));
    item.network = tv.networks.first().map(|n| n.name.clone());
    item.studio = tv
        .production_companies
        .first()
        .map(|c| c.name.clone())
        .or_else(|| item.network.clone());
    item.production_companies = names_once(tv.production_companies.iter().map(|c| c.name.as_str()));
    // Where it was made, or else where it comes from: TMDB leaves the first
    // empty for many a series it knows the second of.
    item.production_countries = country_codes(
        tv.production_countries
            .iter()
            .map(|c| c.iso_3166_1.as_str()),
    );
    if item.production_countries.is_empty() {
        item.production_countries = country_codes(tv.origin_country.iter().map(String::as_str));
    }
    let (rating, rating_country) = content_rating(tv.content_ratings.as_ref());
    item.content_rating = rating;
    item.content_rating_country = rating_country;
    item.releases = tv
        .content_ratings
        .as_ref()
        .map(|r| ratings_by_country(&r.results))
        .unwrap_or_default();
    item.popularity = tv.popularity;
    item.genres = tv.genres.iter().map(|g| g.name.clone()).collect();
    item.keywords = tv
        .keywords
        .as_ref()
        .map(|k| k.results.iter().map(|n| n.name.clone()).collect())
        .unwrap_or_default();
    item.trailer_youtube_id = tv.videos.as_ref().and_then(|v| youtube_trailer(&v.results));
    item.slug = make_slug(&item.title, item.year);

    item.external_ids = ExternalIds {
        tmdb: Some(tv.id),
        tvdb: tv.external_ids.as_ref().and_then(|e| e.tvdb_id),
        imdb: tv
            .external_ids
            .as_ref()
            .and_then(|e| e.imdb_id.as_deref())
            .and_then(crate::domain::ids::normalize_imdb_id),
        tvrage: tv.external_ids.as_ref().and_then(|e| e.tvrage_id),
        wikidata: wikidata(tv.external_ids.as_ref()),
        ..Default::default()
    };

    item.ratings = vote_rating(tv.vote_average, tv.vote_count);
    item.images = artwork(
        tv.poster_path.as_deref(),
        tv.backdrop_path.as_deref(),
        tv.images.as_ref(),
    );
    item.credits = credits(tv.credits.as_ref(), &tv.created_by, cast);
    item.alternative_titles = tv
        .alternative_titles
        .as_ref()
        .map(|a| alt_titles(&a.results))
        .unwrap_or_default();

    item.seasons = tv
        .seasons
        .iter()
        .map(|s| Season {
            id: new_id(),
            season_number: s.season_number,
            title: non_empty(s.name.as_deref()),
            overview: non_empty(s.overview.as_deref()),
            air_date: non_empty(s.air_date.as_deref()),
            tmdb_id: s.id,
            tvdb_id: None,
            is_manual: false,
            images: s
                .poster_path
                .as_deref()
                .map(|p| {
                    vec![image(
                        CoverType::Poster,
                        &image_url(p),
                        Some(s.season_number),
                        0,
                    )]
                })
                .unwrap_or_default(),
            primary_images: Default::default(),
        })
        .collect();

    item.translations = tv
        .translations
        .as_ref()
        .map(|t| translations(&t.translations))
        .unwrap_or_default();

    item.episodes = episodes(seasons);
    item.updated_at = now();

    item
}

/// Map a search hit: enough to display and to resolve, without a second call.
pub fn tv_summary_to_item(summary: &models::TvSummary) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Series);

    item.title = summary.name.clone();
    item.is_adult = summary.adult.unwrap_or(false);
    item.original_title = non_empty(summary.original_name.as_deref());
    item.overview = non_empty(summary.overview.as_deref());
    item.first_aired = non_empty(summary.first_air_date.as_deref());
    item.year = year_of(summary.first_air_date.as_deref());
    item.original_language = summary.original_language.as_deref().map(iso_639_1_to_3);
    item.original_country = summary.origin_country.first().map(|c| iso_3166_2_to_3(c));
    item.popularity = summary.popularity;
    // A search hit carries no status field; "continuing" is what Sonarr assumes
    // for anything it has not fetched in full.
    item.status = Some("continuing".to_string());
    item.slug = make_slug(&item.title, item.year);
    item.external_ids = ExternalIds {
        tmdb: Some(summary.id),
        ..Default::default()
    };
    item.ratings = vote_rating(summary.vote_average, summary.vote_count);
    item.images = artwork(
        summary.poster_path.as_deref(),
        summary.backdrop_path.as_deref(),
        None,
    );

    item
}

fn episodes(seasons: &[models::Season]) -> Vec<Episode> {
    let mut out = Vec::new();

    for season in seasons {
        for ep in &season.episodes {
            let air_date = non_empty(ep.air_date.as_deref());
            // TMDB gives a date with no time, so it says nothing about the
            // moment an episode aired — and says so, rather than inventing
            // midnight UTC. An invented time used to win the merge over the
            // real one Skyhook and TVmaze know, and Sonarr, which reads this
            // as a timestamp, saw every episode of a US evening show as aired
            // a day before it was. The midnight fallback happens on the way
            // out, only when no provider knew better: `wire::sonarr`.
            let air_date_utc = None;

            out.push(Episode {
                id: new_id(),
                season_number: ep.season_number,
                episode_number: ep.episode_number,
                absolute_episode_number: None,
                aired_after_season_number: None,
                aired_before_season_number: None,
                aired_before_episode_number: None,
                // Not TMDB's "Épisode 3" for one it has no name for: that
                // would be the title Sonarr names files after.
                title: ep.title().unwrap_or_default(),
                overview: non_empty(ep.overview.as_deref()),
                air_date,
                air_date_utc,
                runtime: ep.runtime,
                finale_type: finale_type(ep.episode_type.as_deref()),
                image: ep.still_path.as_deref().map(image_url),
                tvdb_id: None,
                tmdb_id: ep.id,
                rating: ep.vote_count.filter(|&c| c > 0).map(|votes| RatingValue {
                    value: ep.vote_average.unwrap_or(0.0),
                    votes,
                }),
                is_manual: false,
            });
        }
    }

    out
}

// ─── episode groups ──────────────────────────────────────────────────────────

/// The episode groups TMDB lists with a series, as orders still to be read:
/// each under `tmdb-` and its id, of the kind its type says, named as it is
/// on TMDB, with the count of episodes it says it holds. A group of a type
/// this server does not know is left out. `None` when the answer lists no
/// groups at all — one asked before they were — which is not the same as a
/// series with none.
pub fn episode_groups(raw: &serde_json::Value) -> Option<Vec<EpisodeOrder>> {
    let listed = raw.get("episode_groups")?;
    let groups: models::EpisodeGroups = serde_json::from_value(listed.clone()).ok()?;
    let at = now();
    Some(
        groups
            .results
            .into_iter()
            .filter_map(|group| {
                let kind = OrderKind::of_tmdb(group.group_type?)?;
                let id = group.id.trim().to_ascii_lowercase();
                let key = format!("{}{id}", crate::domain::orders::TMDB_GROUP);
                crate::domain::orders::is_key(&key).then(|| EpisodeOrder {
                    key,
                    kind,
                    source: crate::providers::names::TMDB.to_string(),
                    source_ref: Some(group.id.clone()),
                    label: non_empty(Some(&group.name))
                        .unwrap_or_else(|| format!("TMDB group {}", group.id)),
                    is_default: false,
                    is_frozen: false,
                    announced: group.episode_count,
                    updated_at: at.clone(),
                    episodes: Vec::new(),
                })
            })
            .collect(),
    )
}

/// An episode group's places: each of its groups a season, numbered by the
/// group's `order`, and each episode numbered in it from one by its own —
/// TMDB's record of it kept with it, and its TMDB id, which names the
/// episode the work holds where it holds one. A group of the absolute type
/// numbers its episodes straight through besides, the specials aside.
pub fn episode_group_places(group: &models::EpisodeGroup) -> Vec<PlacedEpisode> {
    let absolute = group.group_type == Some(2);
    let mut seasons: Vec<&models::GroupSeason> = group.groups.iter().collect();
    seasons.sort_by_key(|season| season.order.unwrap_or(i32::MAX));

    let mut places = Vec::new();
    let mut running = 0;
    for season in seasons {
        let Some(number) = season.order else { continue };
        let mut episodes: Vec<&models::GroupEpisode> = season.episodes.iter().collect();
        episodes.sort_by_key(|episode| episode.order.unwrap_or(i32::MAX));
        for (at, entry) in episodes.into_iter().enumerate() {
            let ep = &entry.episode;
            let place_number = entry
                .order
                .unwrap_or_else(|| i32::try_from(at).unwrap_or(i32::MAX))
                .saturating_add(1);
            if absolute && number > 0 {
                running += 1;
            }
            places.push(PlacedEpisode {
                id: new_id(),
                season_number: number,
                episode_number: place_number,
                absolute_number: (absolute && number > 0).then_some(running),
                episode_id: None,
                tvdb_id: None,
                tmdb_id: ep.id,
                record: Some(OrderRecord {
                    title: ep.title().unwrap_or_default(),
                    overview: non_empty(ep.overview.as_deref()),
                    aired: non_empty(ep.air_date.as_deref()),
                    runtime: ep.runtime.filter(|r| *r > 0),
                    image: ep.still_path.as_deref().map(image_url),
                    finale_type: finale_type(ep.episode_type.as_deref()),
                }),
            });
        }
    }
    places
}

// ─── movie ───────────────────────────────────────────────────────────────────

/// A film, keeping `cast` of its actors.
pub fn movie_to_item(movie: &models::Movie, cast: usize) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Movie);

    item.title = movie.title.clone();
    item.is_adult = movie.adult.unwrap_or(false);
    item.original_title = non_empty(movie.original_title.as_deref());
    item.overview = non_empty(movie.overview.as_deref());
    item.tagline = non_empty(movie.tagline.as_deref());
    item.homepage = non_empty(movie.homepage.as_deref());
    item.original_language = movie.original_language.as_deref().map(iso_639_1_to_3);
    item.original_country = movie
        .production_countries
        .first()
        .map(|c| iso_3166_2_to_3(&c.iso_3166_1));
    item.runtime = movie.runtime;
    item.year = year_of(movie.release_date.as_deref());
    item.popularity = movie.popularity;
    item.studio = movie.production_companies.first().map(|c| c.name.clone());
    item.production_companies =
        names_once(movie.production_companies.iter().map(|c| c.name.as_str()));
    item.production_countries = country_codes(
        movie
            .production_countries
            .iter()
            .map(|c| c.iso_3166_1.as_str()),
    );
    item.genres = movie.genres.iter().map(|g| g.name.clone()).collect();
    item.keywords = movie
        .keywords
        .as_ref()
        .map(|k| k.keywords.iter().map(|n| n.name.clone()).collect())
        .unwrap_or_default();
    item.trailer_youtube_id = movie
        .videos
        .as_ref()
        .and_then(|v| youtube_trailer(&v.results));
    item.collection_tmdb_id = movie.belongs_to_collection.as_ref().map(|c| c.id);
    item.slug = make_slug(&item.title, item.year);

    let releases = release_dates(movie);
    let in_cinemas = releases
        .in_cinemas
        .clone()
        .or_else(|| non_empty(movie.release_date.as_deref()));

    // Status depends on the whole release picture, so derive it before the
    // individual dates are moved onto the item.
    item.status =
        Some(movie_status(movie.status.as_deref(), in_cinemas.as_deref(), &releases).to_string());
    item.in_cinemas = in_cinemas;
    item.physical_release = releases.physical;
    item.digital_release = releases.digital;
    item.content_rating = releases.certification;
    item.content_rating_country = releases.certification_country;
    item.releases = movie
        .release_dates
        .as_ref()
        .map(|all| releases_by_country(&all.results))
        .unwrap_or_default();

    item.external_ids = ExternalIds {
        tmdb: Some(movie.id),
        imdb: movie
            .imdb_id
            .as_deref()
            .or_else(|| {
                movie
                    .external_ids
                    .as_ref()
                    .and_then(|e| e.imdb_id.as_deref())
            })
            .and_then(crate::domain::ids::normalize_imdb_id),
        tvdb: movie.external_ids.as_ref().and_then(|e| e.tvdb_id),
        wikidata: wikidata(movie.external_ids.as_ref()),
        ..Default::default()
    };

    item.ratings = vote_rating(movie.vote_average, movie.vote_count);
    item.images = artwork(
        movie.poster_path.as_deref(),
        movie.backdrop_path.as_deref(),
        movie.images.as_ref(),
    );
    item.credits = credits(movie.credits.as_ref(), &[], cast);
    item.alternative_titles = movie
        .alternative_titles
        .as_ref()
        .map(|a| alt_titles(&a.titles))
        .unwrap_or_default();
    item.translations = movie
        .translations
        .as_ref()
        .map(|t| translations(&t.translations))
        .unwrap_or_default();

    item.updated_at = now();
    item
}

pub fn movie_summary_to_item(summary: &models::MovieSummary) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Movie);

    item.title = summary.title.clone();
    item.is_adult = summary.adult.unwrap_or(false);
    item.original_title = non_empty(summary.original_title.as_deref());
    item.overview = non_empty(summary.overview.as_deref());
    item.in_cinemas = non_empty(summary.release_date.as_deref());
    item.year = year_of(summary.release_date.as_deref());
    item.original_language = summary.original_language.as_deref().map(iso_639_1_to_3);
    item.popularity = summary.popularity;
    item.status = Some(
        match item.in_cinemas.as_deref() {
            Some(date) if is_past(date) => "released",
            Some(_) => "announced",
            None => "tba",
        }
        .to_string(),
    );
    item.slug = make_slug(&item.title, item.year);
    item.external_ids = ExternalIds {
        tmdb: Some(summary.id),
        ..Default::default()
    };
    item.ratings = vote_rating(summary.vote_average, summary.vote_count);
    item.images = artwork(
        summary.poster_path.as_deref(),
        summary.backdrop_path.as_deref(),
        None,
    );

    item
}

#[derive(Default)]
struct Releases {
    in_cinemas: Option<String>,
    physical: Option<String>,
    digital: Option<String>,
    certification: Option<String>,
    /// ISO 3166-1 alpha-2, uppercase.
    certification_country: Option<String>,
}

/// Split TMDB's per-country release list into the dates Radarr tracks.
///
/// TMDB's type codes: 1 premiere, 2 limited theatrical, 3 theatrical,
/// 4 digital, 5 physical, 6 TV.
fn release_dates(movie: &models::Movie) -> Releases {
    let Some(all) = movie.release_dates.as_ref() else {
        return Releases::default();
    };

    // Prefer the US block, which is the most consistently populated, but accept
    // any country rather than returning nothing for a non-US release.
    let block = all
        .results
        .iter()
        .find(|r| r.iso_3166_1 == PREFERRED_REGION)
        .or_else(|| all.results.first());

    let Some(block) = block else {
        return Releases::default();
    };

    let mut out = Releases::default();

    for entry in &block.release_dates {
        let date = entry.release_date.as_deref().map(trim_to_date);

        // The earliest of each kind. TMDB lists a country's dates in no order
        // it promises, and files a re-release under the same type as the
        // original: taken as they came, the fiftieth-anniversary run made The
        // Godfather a 2022 film and a 2026 one had Fight Club not yet reach
        // cinemas — which Radarr reads as not yet available.
        let slot = match entry.release_type {
            Some(2 | 3) => Some(&mut out.in_cinemas),
            Some(4) => Some(&mut out.digital),
            Some(5) => Some(&mut out.physical),
            _ => None,
        };
        if let (Some(slot), Some(date)) = (slot, date)
            && slot
                .as_deref()
                .is_none_or(|current| date.as_str() < current)
        {
            *slot = Some(date);
        }

        if out.certification.is_none() {
            out.certification = non_empty(entry.certification.as_deref());
            if out.certification.is_some() {
                out.certification_country = Some(block.iso_3166_1.to_ascii_uppercase());
            }
        }
    }

    out
}

/// Every release TMDB lists, country by country, as it lists them: a
/// country's premieres, runs and copies, each with its day, its rating and
/// its note. A row with neither a day nor a rating says nothing, and one
/// given twice is kept once.
fn releases_by_country(blocks: &[models::ReleaseDates]) -> Vec<CountryRelease> {
    let mut out: Vec<CountryRelease> = Vec::new();

    for block in blocks {
        let Some(country) = country_code(&block.iso_3166_1) else {
            continue;
        };
        for entry in &block.release_dates {
            let release = CountryRelease {
                country: country.clone(),
                release_type: entry.release_type.and_then(ReleaseKind::from_tmdb),
                date: non_empty(entry.release_date.as_deref()).map(|d| trim_to_date(&d)),
                certification: non_empty(entry.certification.as_deref()),
                note: non_empty(entry.note.as_deref()),
                source: Some(crate::providers::names::TMDB.to_string()),
            };
            if (release.date.is_some() || release.certification.is_some())
                && !out.contains(&release)
            {
                out.push(release);
            }
        }
    }

    out
}

/// A series' ratings, one a country, as TMDB lists them.
fn ratings_by_country(ratings: &[models::ContentRating]) -> Vec<CountryRelease> {
    let mut out: Vec<CountryRelease> = Vec::new();

    for rating in ratings {
        let (Some(country), Some(certification)) = (
            country_code(&rating.iso_3166_1),
            non_empty(Some(&rating.rating)),
        ) else {
            continue;
        };
        if out.iter().any(|r| r.country == country) {
            continue;
        }
        out.push(CountryRelease {
            country,
            release_type: None,
            date: None,
            certification: Some(certification),
            note: None,
            source: Some(crate::providers::names::TMDB.to_string()),
        });
    }

    out
}

// ─── shared helpers ──────────────────────────────────────────────────────────

/// A country as TMDB codes it, ISO 3166-1 alpha-2, in upper case; nothing
/// for what is not one.
fn country_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    (code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase())).then_some(code)
}

/// Countries, each once, in the order given.
fn country_codes<'a>(codes: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for code in codes.into_iter().filter_map(country_code) {
        if !out.contains(&code) {
            out.push(code);
        }
    }
    out
}

/// Names, each once whatever its case, the blank ones left out.
fn names_once<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names.into_iter().map(str::trim).filter(|n| !n.is_empty()) {
        if !out.iter().any(|kept| kept.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
        }
    }
    out
}

/// Wikidata's item for the work, as TMDB files it beside its own ids.
fn wikidata(ids: Option<&models::ExternalIds>) -> Option<String> {
    ids.and_then(|e| e.wikidata_id.as_deref())
        .and_then(crate::domain::ids::wikidata_item)
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// The runtime to report for a series whose `episode_run_time` is empty.
///
/// TMDB has stopped populating that field for many shows — Breaking Bad returns
/// `[]` today — and Sonarr uses runtime when matching releases, so leaving it
/// unset degrades matching. The most common episode length is a better answer
/// than the mean: one feature-length finale should not drag the figure up.
///
/// Specials are excluded. They run to whatever length they like (a three-minute
/// webisode is still season 0) and would skew the count.
fn typical_runtime(seasons: &[models::Season]) -> Option<i32> {
    let mut counts: Vec<(i32, usize)> = Vec::new();

    let runtimes = seasons
        .iter()
        .flat_map(|s| s.episodes.iter())
        .filter(|e| e.season_number > 0)
        .filter_map(|e| e.runtime)
        .filter(|&r| r > 0);

    for runtime in runtimes {
        match counts.iter_mut().find(|(value, _)| *value == runtime) {
            Some((_, count)) => *count += 1,
            None => counts.push((runtime, 1)),
        }
    }

    // Ties go to the shorter runtime, which is the safer guess for a series that
    // mixes standard episodes with longer finales.
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(runtime, _)| runtime)
}

/// Leading `YYYY` of a date string.
fn year_of(date: Option<&str>) -> Option<i32> {
    date?.get(0..4)?.parse().ok()
}

/// TMDB dates in `release_dates` carry a time; Radarr wants the date only.
fn trim_to_date(s: &str) -> String {
    s.split('T').next().unwrap_or(s).to_string()
}

fn is_past(date: &str) -> bool {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    trim_to_date(date) <= today
}

/// TMDB's series statuses mapped to Sonarr's vocabulary.
///
/// A series whose first episode is still to come is upcoming, whatever else
/// TMDB says of it: it calls one being shot "In Production", where TheTVDB —
/// and so Skyhook, and Sonarr — call it upcoming.
fn series_status(status: Option<&str>, first_aired: Option<&str>) -> &'static str {
    let first_aired = first_aired.map(str::trim).filter(|d| !d.is_empty());
    let yet_to_air = first_aired.is_some_and(|d| !is_past(d));
    match status {
        Some("Ended") | Some("Canceled") | Some("Cancelled") => "ended",
        Some("Planned") | Some("Pilot") => "upcoming",
        Some("In Production") if first_aired.is_none() || yet_to_air => "upcoming",
        _ if yet_to_air => "upcoming",
        // "Returning Series", "In Production" and anything unrecognised. Sonarr
        // treats an unknown status as still airing, which is the safe default:
        // it keeps searching for new episodes.
        _ => "continuing",
    }
}

/// Radarr's vocabulary: `tba`, `announced`, `inCinemas`, `released`.
fn movie_status(
    status: Option<&str>,
    in_cinemas: Option<&str>,
    releases: &Releases,
) -> &'static str {
    let home_release = releases.physical.is_some() || releases.digital.is_some();

    match status {
        Some("Released") if home_release => "released",
        Some("Released") => match in_cinemas {
            Some(date) if is_past(date) => "inCinemas",
            _ => "released",
        },
        Some("Post Production") | Some("In Production") | Some("Planned") => "announced",
        Some("Canceled") | Some("Cancelled") => "tba",
        _ => match in_cinemas {
            Some(date) if is_past(date) => "released",
            Some(_) => "announced",
            None => "tba",
        },
    }
}

fn finale_type(episode_type: Option<&str>) -> Option<String> {
    match episode_type {
        Some("finale") => Some("series".to_string()),
        Some("mid_season") => Some("midseason".to_string()),
        _ => None,
    }
}

/// The content rating, and the country it applies to.
///
/// The country matters: Radarr matches certifications against its configured
/// `CertificationCountry`, an ISO 3166-1 alpha-2 uppercase code. Labelling a
/// French rating as American would be worse than reporting none.
fn content_rating(
    ratings: Option<&models::Results<models::ContentRating>>,
) -> (Option<String>, Option<String>) {
    let Some(ratings) = ratings else {
        return (None, None);
    };

    ratings
        .results
        .iter()
        .find(|r| r.iso_3166_1 == PREFERRED_REGION && !r.rating.is_empty())
        .or_else(|| ratings.results.iter().find(|r| !r.rating.is_empty()))
        .map(|r| {
            (
                Some(r.rating.clone()),
                Some(r.iso_3166_1.to_ascii_uppercase()),
            )
        })
        .unwrap_or((None, None))
}

fn vote_rating(average: Option<f64>, votes: Option<i64>) -> Vec<Rating> {
    let votes = votes.unwrap_or(0);

    // A zero-vote average is 0.0, which would read as "rated 0/10" downstream.
    if votes == 0 {
        return Vec::new();
    }

    vec![Rating {
        source: "tmdb".to_string(),
        value: average,
        votes: Some(votes),
        rating_type: Some("user".to_string()),
    }]
}

fn image(cover_type: CoverType, url: &str, season_number: Option<i32>, sort_order: i32) -> Image {
    Image {
        id: new_id(),
        season_number,
        cover_type,
        url: url.to_string(),
        language: None,
        sort_order,
        source: Some(crate::providers::names::TMDB.to_string()),
        is_manual: false,
    }
}

/// Primary poster and backdrop, then any extra artwork from `images`.
///
/// The primary paths come first so `MediaItem::image` picks TMDB's own choice
/// rather than whichever alternate happens to sort first.
fn artwork(
    poster: Option<&str>,
    backdrop: Option<&str>,
    extra: Option<&models::Images>,
) -> Vec<Image> {
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    let push =
        |out: &mut Vec<Image>, seen: &mut Vec<String>, kind: CoverType, path: &str, order: i32| {
            let url = image_url(path);
            if seen.contains(&url) {
                return;
            }
            seen.push(url.clone());
            out.push(image(kind, &url, None, order));
        };

    if let Some(path) = poster {
        push(&mut out, &mut seen, CoverType::Poster, path, 0);
    }
    if let Some(path) = backdrop {
        push(&mut out, &mut seen, CoverType::Fanart, path, 0);
    }

    if let Some(images) = extra {
        for (order, p) in images.posters.iter().take(5).enumerate() {
            push(
                &mut out,
                &mut seen,
                CoverType::Poster,
                &p.file_path,
                order as i32 + 1,
            );
        }
        for (order, b) in images.backdrops.iter().take(5).enumerate() {
            push(
                &mut out,
                &mut seen,
                CoverType::Fanart,
                &b.file_path,
                order as i32 + 1,
            );
        }
        for (order, l) in images.logos.iter().take(3).enumerate() {
            push(
                &mut out,
                &mut seen,
                CoverType::Clearlogo,
                &l.file_path,
                order as i32,
            );
        }
    }

    out
}

/// The cast, the first `limit` of it, then the crew.
fn credits(
    credits: Option<&models::Credits>,
    creators: &[models::Creator],
    limit: usize,
) -> Vec<Credit> {
    let mut out: Vec<Credit> = credits
        .map(|credits| {
            credits
                .cast
                .iter()
                .take(limit.clamp(1, MAX_CAST))
                .enumerate()
                .map(|(i, member)| Credit {
                    id: new_id(),
                    credit_type: CreditType::Actor,
                    person_name: member.name.clone(),
                    character_name: non_empty(member.character.as_deref()),
                    image: member.profile_path.as_deref().map(image_url),
                    tmdb_person_id: member.id,
                    credit_tmdb_id: member.credit_id.clone(),
                    sort_order: member.order.unwrap_or(i as i32),
                    is_manual: false,
                })
                .collect()
        })
        .unwrap_or_default();

    out.extend(crew(credits.map_or(&[][..], |c| &c.crew), creators));
    out
}

/// Who made the work behind the camera: its directors, whoever created a
/// series, its writers, then its producers — numbered on from one another,
/// so a page that lists the crew in its order lists them so.
///
/// Each person once a kind, with the job TMDB gives them first, `Screenplay`
/// or `Novel`, where Radarr and the NFO read the kind alone. Radarr shows a
/// director and a writer; Sonarr ignores the crew entirely.
fn crew(members: &[models::CrewMember], creators: &[models::Creator]) -> Vec<Credit> {
    struct Member<'a> {
        id: Option<i64>,
        credit_id: Option<&'a String>,
        name: &'a str,
        profile: Option<&'a str>,
    }

    let mut out: Vec<Credit> = Vec::new();
    let mut add = |kind: CreditType, job: &str, member: Member<'_>| -> bool {
        let name = member.name.trim();
        let same = |c: &Credit| {
            c.credit_type == kind
                && match (c.tmdb_person_id, member.id) {
                    (Some(a), Some(b)) => a == b,
                    _ => c.person_name == name,
                }
        };
        if name.is_empty() || out.iter().any(same) {
            return false;
        }
        out.push(Credit {
            id: new_id(),
            credit_type: kind,
            person_name: name.to_string(),
            character_name: Some(job.to_string()),
            image: member.profile.map(image_url),
            tmdb_person_id: member.id,
            credit_tmdb_id: member.credit_id.cloned(),
            sort_order: i32::try_from(out.len()).unwrap_or(i32::MAX),
            is_manual: false,
        });
        true
    };
    fn of(m: &models::CrewMember) -> Member<'_> {
        Member {
            id: m.id,
            credit_id: m.credit_id.as_ref(),
            name: &m.name,
            profile: m.profile_path.as_deref(),
        }
    }

    for member in members
        .iter()
        .filter(|c| c.job.as_deref() == Some("Director"))
        .take(3)
    {
        add(CreditType::Director, "Director", of(member));
    }
    for creator in creators.iter().take(MAX_CREW) {
        add(
            CreditType::Writer,
            "Creator",
            Member {
                id: creator.id,
                credit_id: creator.credit_id.as_ref(),
                name: &creator.name,
                profile: creator.profile_path.as_deref(),
            },
        );
    }
    // TMDB's writing department — a screenplay, a story, the novel — and
    // its producers proper, not the executive ones a series lists by the
    // dozen.
    let mut writers = 0;
    for member in members
        .iter()
        .filter(|c| c.department.as_deref() == Some("Writing"))
    {
        if writers == MAX_CREW {
            break;
        }
        let job = member
            .job
            .as_deref()
            .map(str::trim)
            .filter(|j| !j.is_empty())
            .unwrap_or("Writer");
        if add(CreditType::Writer, job, of(member)) {
            writers += 1;
        }
    }
    let mut producers = 0;
    for member in members
        .iter()
        .filter(|c| c.job.as_deref() == Some("Producer"))
    {
        if producers == MAX_CREW {
            break;
        }
        if add(CreditType::Producer, "Producer", of(member)) {
            producers += 1;
        }
    }

    out
}

fn alt_titles(titles: &[models::AltTitle]) -> Vec<AlternativeTitle> {
    let mut seen: Vec<String> = Vec::new();

    titles
        .iter()
        .filter_map(|t| {
            let title = non_empty(Some(&t.title))?;
            if seen.contains(&title) {
                return None;
            }
            seen.push(title.clone());

            Some(AlternativeTitle {
                id: new_id(),
                title,
                title_type: non_empty(t.title_type.as_deref()),
                language: t.iso_3166_1.as_deref().map(iso_3166_2_to_3),
                is_manual: false,
            })
        })
        .collect()
}

/// One translation per language, from the region that language is at home in.
///
/// TMDB publishes a translation per language *and region* — French for France
/// and French for Québec are two entries — while a translation is stored, and
/// asked for, by language alone. Every variant used to be kept, so which one a
/// French reader saw depended on the order TMDB listed them in, and Québec
/// sorts first: *Pulp Fiction* became *Fiction pulpeuse*, *Inception* became
/// *Origine*, for readers in France. The home region wins now; a language with
/// no entry from its home region keeps the first it has.
fn translations(entries: &[models::Translation]) -> Vec<Translation> {
    let mut chosen: Vec<(Translation, bool)> = Vec::new();

    for t in entries {
        let Some(code) = t.iso_639_1.as_deref() else {
            continue;
        };
        let Some(data) = t.data.as_ref() else {
            continue;
        };

        let title = non_empty(data.title.as_deref().or(data.name.as_deref()));
        let overview = non_empty(data.overview.as_deref());
        let tagline = non_empty(data.tagline.as_deref());

        // A translation with none of the three is noise.
        if title.is_none() && overview.is_none() && tagline.is_none() {
            continue;
        }

        let language = iso_639_1_to_3(code);
        let at_home = t
            .iso_3166_1
            .as_deref()
            .is_some_and(|region| region.eq_ignore_ascii_case(home_region(code)));
        let candidate = Translation {
            language,
            title,
            overview,
            tagline,
            is_manual: false,
        };

        match chosen
            .iter_mut()
            .find(|(kept, _)| kept.language == candidate.language)
        {
            Some(slot) if at_home && !slot.1 => *slot = (candidate, true),
            Some(_) => {}
            None => chosen.push((candidate, at_home)),
        }
    }

    chosen
        .into_iter()
        .map(|(translation, _)| translation)
        .collect()
}

/// Where a language is spoken by default, for choosing among TMDB's regional
/// variants of it. Mostly the language's own code in capitals; the table holds
/// the ones where that is wrong or ambiguous.
fn home_region(language: &str) -> &'static str {
    match language.to_ascii_lowercase().as_str() {
        "en" => "US",
        "pt" => "BR",
        "ja" => "JP",
        "ko" => "KR",
        "zh" => "CN",
        "sv" => "SE",
        "da" => "DK",
        "cs" => "CZ",
        "el" => "GR",
        "he" => "IL",
        "uk" => "UA",
        "ar" => "SA",
        "hi" => "IN",
        "vi" => "VN",
        "sr" => "RS",
        "et" => "EE",
        "sl" => "SI",
        "nb" | "no" => "NO",
        "fa" => "IR",
        "ms" => "MY",
        "ca" => "ES",
        "fr" => "FR",
        "de" => "DE",
        "es" => "ES",
        "it" => "IT",
        "nl" => "NL",
        "pl" => "PL",
        "ru" => "RU",
        "tr" => "TR",
        "fi" => "FI",
        "hu" => "HU",
        "ro" => "RO",
        "bg" => "BG",
        _ => "",
    }
}

/// The best YouTube trailer key, preferring an official one.
fn youtube_trailer(videos: &[models::Video]) -> Option<String> {
    let is_youtube_trailer = |v: &&models::Video| {
        v.site.as_deref() == Some("YouTube")
            && v.video_type.as_deref() == Some("Trailer")
            && v.key.is_some()
    };

    videos
        .iter()
        .filter(is_youtube_trailer)
        .find(|v| v.official == Some(true))
        .or_else(|| videos.iter().find(is_youtube_trailer))
        .and_then(|v| v.key.clone())
}

#[cfg(test)]
mod tests {
    fn translation(language: &str, region: &str, title: &str) -> models::Translation {
        serde_json::from_value(serde_json::json!({
            "iso_639_1": language,
            "iso_3166_1": region,
            "data": { "title": title }
        }))
        .expect("a translation")
    }

    #[test]
    fn a_language_is_read_in_its_home_region() {
        // TMDB lists Québec before France. Stored by language alone, whichever
        // came first won — and a reader in France got "Fiction pulpeuse".
        let kept = translations(&[
            translation("fr", "CA", "Fiction pulpeuse"),
            translation("fr", "FR", "Pulp Fiction"),
            translation("pt", "PT", "Pulp Fiction: Tempo de Violência"),
            translation("pt", "BR", "Pulp Fiction: Tempo de Violência (BR)"),
        ]);

        let title = |language: &str| {
            kept.iter()
                .find(|t| t.language == language)
                .and_then(|t| t.title.as_deref())
                .map(str::to_string)
        };

        assert_eq!(kept.len(), 2, "one per language");
        assert_eq!(title("fra").as_deref(), Some("Pulp Fiction"));
        assert_eq!(
            title("por").as_deref(),
            Some("Pulp Fiction: Tempo de Violência (BR)")
        );
    }

    #[test]
    fn a_language_with_no_home_entry_keeps_the_first_it_has() {
        let kept = translations(&[
            translation("fr", "CA", "Fiction pulpeuse"),
            translation("fr", "BE", "Pulp Fiction (BE)"),
        ]);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title.as_deref(), Some("Fiction pulpeuse"));
    }

    #[test]
    fn a_translations_tagline_is_kept_beside_its_title() {
        let with_tagline = |language: &str, title: Option<&str>, tagline: &str| {
            serde_json::from_value::<models::Translation>(serde_json::json!({
                "iso_639_1": language,
                "iso_3166_1": language.to_uppercase(),
                "data": { "title": title, "tagline": tagline }
            }))
            .expect("a translation")
        };
        let kept = translations(&[
            with_tagline("fr", Some("Premier Contact"), "Pourquoi sont-ils ici ?"),
            // A tagline alone is still a translation; nothing at all is not.
            with_tagline("de", None, "Warum sind sie hier?"),
            with_tagline("it", None, ""),
        ]);

        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].language, "fra");
        assert_eq!(kept[0].tagline.as_deref(), Some("Pourquoi sont-ils ici ?"));
        assert_eq!(kept[1].language, "deu");
        assert_eq!(kept[1].title, None);
        assert_eq!(kept[1].tagline.as_deref(), Some("Warum sind sie hier?"));
    }

    use super::*;

    #[test]
    fn series_statuses_map_to_sonarrs_vocabulary() {
        let aired = Some("2008-01-20");
        assert_eq!(series_status(Some("Returning Series"), aired), "continuing");
        assert_eq!(series_status(Some("Ended"), aired), "ended");
        assert_eq!(series_status(Some("Canceled"), aired), "ended");
        assert_eq!(series_status(Some("Planned"), None), "upcoming");
        // Unknown means "keep looking for episodes", which is the safe default.
        assert_eq!(series_status(Some("Something New"), aired), "continuing");
        assert_eq!(series_status(None, None), "continuing");
        assert_eq!(series_status(Some("Returning Series"), None), "continuing");
    }

    #[test]
    fn a_series_yet_to_air_is_upcoming_as_on_skyhook() {
        // HBO's Harry Potter, a year before its first episode: TMDB says
        // "In Production", TheTVDB and Skyhook say upcoming.
        let later = Some("2999-12-25");
        assert_eq!(series_status(Some("In Production"), later), "upcoming");
        assert_eq!(series_status(Some("In Production"), None), "upcoming");
        assert_eq!(series_status(Some("Returning Series"), later), "upcoming");
        // Shooting a new season of one that has aired: still on.
        assert_eq!(
            series_status(Some("In Production"), Some("2019-06-16")),
            "continuing"
        );
        assert_eq!(series_status(Some("Ended"), later), "ended");
    }

    #[test]
    fn a_movie_with_a_home_release_is_released() {
        let releases = Releases {
            digital: Some("2020-01-01".into()),
            ..Default::default()
        };
        assert_eq!(
            movie_status(Some("Released"), Some("2019-06-01"), &releases),
            "released"
        );
    }

    #[test]
    fn a_movie_only_in_theatres_says_so() {
        let releases = Releases::default();
        assert_eq!(
            movie_status(Some("Released"), Some("2019-06-01"), &releases),
            "inCinemas"
        );
    }

    #[test]
    fn an_unreleased_movie_is_announced_or_tba() {
        let releases = Releases::default();
        assert_eq!(
            movie_status(Some("Post Production"), None, &releases),
            "announced"
        );
        assert_eq!(
            movie_status(None, Some("2999-01-01"), &releases),
            "announced"
        );
        assert_eq!(movie_status(None, None, &releases), "tba");
    }

    #[test]
    fn years_come_from_the_date_prefix() {
        assert_eq!(year_of(Some("2008-01-20")), Some(2008));
        assert_eq!(year_of(Some("")), None);
        assert_eq!(year_of(Some("not-a-date")), None);
        assert_eq!(year_of(None), None);
    }

    #[test]
    fn an_unvoted_title_gets_no_rating_rather_than_a_zero() {
        assert!(vote_rating(Some(0.0), Some(0)).is_empty());
        assert!(vote_rating(None, None).is_empty());

        let rated = vote_rating(Some(8.3), Some(120));
        assert_eq!(rated.len(), 1);
        assert_eq!(rated[0].value, Some(8.3));
        assert_eq!(rated[0].votes, Some(120));
    }

    #[test]
    fn artwork_keeps_the_primary_images_first_and_deduplicates() {
        let extra = models::Images {
            posters: vec![models::ImageRef {
                // Same path as the primary poster: must not appear twice.
                file_path: "/p.jpg".into(),
                iso_639_1: None,
                vote_average: None,
                width: None,
            }],
            backdrops: vec![],
            logos: vec![],
        };

        let images = artwork(Some("/p.jpg"), Some("/b.jpg"), Some(&extra));

        assert_eq!(images.len(), 2);
        assert_eq!(images[0].cover_type, CoverType::Poster);
        assert_eq!(images[0].sort_order, 0);
        assert_eq!(images[1].cover_type, CoverType::Fanart);
    }

    #[test]
    fn an_official_trailer_wins_over_a_fan_upload() {
        let videos = vec![
            models::Video {
                key: Some("fan".into()),
                site: Some("YouTube".into()),
                video_type: Some("Trailer".into()),
                official: Some(false),
                size: None,
            },
            models::Video {
                key: Some("official".into()),
                site: Some("YouTube".into()),
                video_type: Some("Trailer".into()),
                official: Some(true),
                size: None,
            },
        ];

        assert_eq!(youtube_trailer(&videos).as_deref(), Some("official"));
    }

    #[test]
    fn non_youtube_videos_are_ignored() {
        let videos = vec![models::Video {
            key: Some("x".into()),
            site: Some("Vimeo".into()),
            video_type: Some("Trailer".into()),
            official: Some(true),
            size: None,
        }];

        assert_eq!(youtube_trailer(&videos), None);
    }

    fn ep(season: i32, number: i32, runtime: Option<i32>) -> models::Episode {
        models::Episode {
            id: None,
            season_number: season,
            episode_number: number,
            name: None,
            overview: None,
            air_date: None,
            runtime,
            still_path: None,
            vote_average: None,
            vote_count: None,
            episode_type: None,
        }
    }

    fn season_of(episodes: Vec<models::Episode>) -> models::Season {
        models::Season {
            season_number: 1,
            episodes,
        }
    }

    #[test]
    fn runtime_falls_back_to_the_most_common_episode_length() {
        // TMDB returns an empty episode_run_time for many shows now, so this is
        // the only source of a runtime for them.
        let seasons = vec![season_of(vec![
            ep(1, 1, Some(47)),
            ep(1, 2, Some(47)),
            ep(1, 3, Some(45)),
            ep(1, 4, Some(75)), // a long finale must not win
        ])];

        assert_eq!(typical_runtime(&seasons), Some(47));
    }

    #[test]
    fn specials_do_not_skew_the_runtime() {
        // A three-minute webisode is still season 0.
        let seasons = vec![
            models::Season {
                season_number: 0,
                episodes: vec![ep(0, 1, Some(3)), ep(0, 2, Some(3)), ep(0, 3, Some(3))],
            },
            season_of(vec![ep(1, 1, Some(52)), ep(1, 2, Some(52))]),
        ];

        assert_eq!(typical_runtime(&seasons), Some(52));
    }

    #[test]
    fn a_tie_picks_the_shorter_runtime() {
        let seasons = vec![season_of(vec![ep(1, 1, Some(60)), ep(1, 2, Some(30))])];
        assert_eq!(typical_runtime(&seasons), Some(30));
    }

    #[test]
    fn no_usable_episode_runtime_yields_nothing() {
        assert_eq!(typical_runtime(&[]), None);
        assert_eq!(
            typical_runtime(&[season_of(vec![ep(1, 1, None), ep(1, 2, Some(0))])]),
            None
        );
    }

    #[test]
    fn an_explicit_episode_run_time_still_wins() {
        // The fixture declares [45, 47]; the episodes say 58 and 48.
        let item = super::tv_to_item(
            &serde_json::from_str(include_str!("fixtures/tv.json")).unwrap(),
            &[serde_json::from_str(include_str!("fixtures/season.json")).unwrap()],
            DEFAULT_CAST,
        );
        assert_eq!(item.runtime, Some(45));
    }

    #[test]
    fn the_groups_tmdb_lists_are_orders_still_to_be_read() {
        let raw = serde_json::json!({ "episode_groups": { "results": [
            { "id": "66bd170e3990ba12fee9990d", "name": "Superfan Order", "type": 4, "episode_count": 186, "group_count": 9 },
            { "id": "6054b076fab3fa003d4f10d7", "name": "Blu-ray Order", "type": 3, "episode_count": 281 },
            // A type this server does not know is left out.
            { "id": "abc", "name": "Mystery", "type": 9, "episode_count": 3 }
        ] } });
        let groups = episode_groups(&raw).expect("listed");
        let read: Vec<(&str, OrderKind, &str, Option<i32>)> = groups
            .iter()
            .map(|g| (g.key.as_str(), g.kind, g.label.as_str(), g.announced))
            .collect();
        assert_eq!(
            read,
            [
                (
                    "tmdb-66bd170e3990ba12fee9990d",
                    OrderKind::Digital,
                    "Superfan Order",
                    Some(186)
                ),
                (
                    "tmdb-6054b076fab3fa003d4f10d7",
                    OrderKind::Dvd,
                    "Blu-ray Order",
                    Some(281)
                ),
            ]
        );
        assert!(
            groups
                .iter()
                .all(|g| g.source == "tmdb" && g.episodes.is_empty())
        );
        assert_eq!(
            groups[0].source_ref.as_deref(),
            Some("66bd170e3990ba12fee9990d")
        );
        // A series asked for before groups were: nothing said, not none.
        assert!(episode_groups(&serde_json::json!({ "id": 2316 })).is_none());
        assert_eq!(
            episode_groups(&serde_json::json!({ "episode_groups": { "results": [] } })),
            Some(Vec::new())
        );
    }

    #[test]
    fn a_groups_seasons_are_its_groups_and_its_episodes_numbered_from_one() {
        let group: models::EpisodeGroup = serde_json::from_value(serde_json::json!({
            "id": "g", "type": 2,
            "groups": [
                { "id": "s2", "name": "Season 2", "order": 2, "locked": false, "episodes": [
                    { "id": 21, "season_number": 2, "episode_number": 1, "order": 0, "name": "Two", "air_date": "2006-09-21", "runtime": 22 }
                ] },
                { "id": "s1", "name": "Season 1", "order": 1, "locked": false, "episodes": [
                    { "id": 12, "season_number": 1, "episode_number": 2, "order": 1, "name": "Diversity Day", "still_path": "/d.jpg", "episode_type": "finale" },
                    { "id": 11, "season_number": 1, "episode_number": 1, "order": 0, "name": "Pilot", "overview": "It begins." }
                ] },
                { "id": "s0", "name": "Specials", "order": 0, "episodes": [
                    { "id": 1, "season_number": 0, "episode_number": 1, "order": 0, "name": "Episode 1" }
                ] }
            ]
        }))
        .expect("a group");
        let places = episode_group_places(&group);
        /// A place: its season, number and absolute number, its episode's
        /// TMDB id and its title.
        type Read<'a> = (i32, i32, Option<i32>, Option<i64>, &'a str);
        let read: Vec<Read<'_>> = places
            .iter()
            .map(|p| {
                (
                    p.season_number,
                    p.episode_number,
                    p.absolute_number,
                    p.tmdb_id,
                    p.record.as_ref().map_or("", |r| r.title.as_str()),
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                // TMDB's stand-in for a name it has none for is no title.
                (0, 1, None, Some(1), ""),
                (1, 1, Some(1), Some(11), "Pilot"),
                (1, 2, Some(2), Some(12), "Diversity Day"),
                (2, 1, Some(3), Some(21), "Two"),
            ]
        );
        let diversity = places[2].record.as_ref().unwrap();
        assert_eq!(
            diversity.image.as_deref(),
            Some("https://image.tmdb.org/t/p/original/d.jpg")
        );
        assert_eq!(diversity.finale_type.as_deref(), Some("series"));
        assert_eq!(
            places[3].record.as_ref().unwrap().aired.as_deref(),
            Some("2006-09-21")
        );
    }

    #[test]
    fn dates_are_trimmed_to_their_day() {
        assert_eq!(trim_to_date("2020-05-01T00:00:00.000Z"), "2020-05-01");
        assert_eq!(trim_to_date("2020-05-01"), "2020-05-01");
    }
}

// ─── against real-shaped TMDB documents ──────────────────────────────────────
//
// These fixtures are trimmed copies of what TMDB actually returns, kept whole
// enough to exercise the awkward parts: an empty episode, a non-US release
// block, a translation with nothing in it. They are what stands in for calling
// the live API in CI.

#[cfg(test)]
mod fixtures {
    use super::*;
    use crate::{
        domain::{CoverType, CreditType, MediaKind},
        wire::{radarr, sonarr},
    };

    fn tv() -> models::Tv {
        serde_json::from_str(include_str!("fixtures/tv.json")).expect("tv fixture")
    }

    fn season() -> models::Season {
        serde_json::from_str(include_str!("fixtures/season.json")).expect("season fixture")
    }

    fn movie() -> models::Movie {
        serde_json::from_str(include_str!("fixtures/movie.json")).expect("movie fixture")
    }

    #[test]
    fn a_series_document_maps_to_the_canonical_model() {
        let item = tv_to_item(&tv(), &[season()], DEFAULT_CAST);

        assert_eq!(item.kind, MediaKind::Series);
        assert_eq!(item.title, "Breaking Bad");
        assert_eq!(item.slug, "breaking-bad-2008");
        assert_eq!(item.year, Some(2008));
        assert_eq!(item.status.as_deref(), Some("ended"));
        // The first entry of episode_run_time, not the last.
        assert_eq!(item.runtime, Some(45));
        assert_eq!(item.network.as_deref(), Some("AMC"));
        assert_eq!(item.original_language.as_deref(), Some("eng"));
        assert_eq!(item.original_country.as_deref(), Some("usa"));
        assert_eq!(item.genres, vec!["Drama", "Crime"]);
        assert_eq!(item.keywords, vec!["new mexico", "drug dealer"]);
        assert_eq!(item.trailer_youtube_id.as_deref(), Some("HhesaQXLuRY"));
    }

    #[test]
    fn external_ids_are_lifted_out_of_the_appended_block() {
        let item = tv_to_item(&tv(), &[], DEFAULT_CAST);

        assert_eq!(item.external_ids.tmdb, Some(1396));
        assert_eq!(item.external_ids.tvdb, Some(81189));
        assert_eq!(item.external_ids.imdb.as_deref(), Some("tt0903747"));
        assert_eq!(item.external_ids.tvrage, Some(18164));
    }

    #[test]
    fn the_us_content_rating_is_preferred() {
        // The fixture lists GB first; US is what Sonarr expects to see.
        assert_eq!(
            tv_to_item(&tv(), &[], DEFAULT_CAST)
                .content_rating
                .as_deref(),
            Some("TV-MA")
        );
    }

    #[test]
    fn episodes_carry_dates_titles_and_ratings() {
        let item = tv_to_item(&tv(), &[season()], DEFAULT_CAST);
        assert_eq!(item.episodes.len(), 3);

        let first = &item.episodes[0];
        assert_eq!(first.title, "Pilot");
        assert_eq!(first.season_number, 1);
        assert_eq!(first.air_date.as_deref(), Some("2008-01-20"));
        // A date, and no pretence of knowing the time: see `episode` above.
        assert_eq!(first.air_date.as_deref(), Some("2008-01-20"));
        assert_eq!(first.air_date_utc, None);
        assert_eq!(first.runtime, Some(58));
        assert_eq!(first.rating.map(|r| r.votes), Some(260));
        assert!(
            first
                .image
                .as_deref()
                .unwrap()
                .starts_with("https://image.tmdb.org")
        );
    }

    #[test]
    fn an_unaired_episode_keeps_its_place_without_inventing_data() {
        // TMDB lists announced episodes with empty strings and zero votes.
        let item = tv_to_item(&tv(), &[season()], DEFAULT_CAST);
        let third = &item.episodes[2];

        assert_eq!(third.episode_number, 3);
        assert_eq!(third.title, "");
        assert_eq!(third.air_date, None);
        assert_eq!(third.overview, None);
        assert!(
            third.rating.is_none(),
            "zero votes must not become a rating of 0"
        );
    }

    #[test]
    fn seasons_carry_their_own_poster() {
        let item = tv_to_item(&tv(), &[], DEFAULT_CAST);
        assert_eq!(item.seasons.len(), 2);

        let specials = &item.seasons[0];
        assert_eq!(specials.season_number, 0);
        assert_eq!(specials.images.len(), 1);
        assert_eq!(specials.images[0].season_number, Some(0));
        assert_eq!(specials.images[0].cover_type, CoverType::Poster);
    }

    #[test]
    fn cast_and_directors_are_separated() {
        let item = tv_to_item(&tv(), &[], DEFAULT_CAST);

        let actors: Vec<_> = item
            .credits
            .iter()
            .filter(|c| c.credit_type == CreditType::Actor)
            .collect();
        assert_eq!(actors.len(), 2);
        assert_eq!(actors[0].person_name, "Bryan Cranston");
        assert_eq!(actors[0].character_name.as_deref(), Some("Walter White"));
        assert_eq!(
            actors[0].credit_tmdb_id.as_deref(),
            Some("52542282760ee313280017f9")
        );

        let directors: Vec<_> = item
            .credits
            .iter()
            .filter(|c| c.credit_type == CreditType::Director)
            .collect();
        assert_eq!(directors.len(), 1);
        assert_eq!(directors[0].person_name, "Vince Gilligan");
    }

    #[test]
    fn a_movie_document_maps_to_the_canonical_model() {
        let item = movie_to_item(&movie(), DEFAULT_CAST);

        assert_eq!(item.kind, MediaKind::Movie);
        assert_eq!(item.title, "Arrival");
        assert_eq!(item.slug, "arrival-2016");
        assert_eq!(item.year, Some(2016));
        assert_eq!(item.runtime, Some(116));
        assert_eq!(item.studio.as_deref(), Some("21 Laps Entertainment"));
        assert_eq!(item.external_ids.imdb.as_deref(), Some("tt2543164"));
        assert_eq!(item.trailer_youtube_id.as_deref(), Some("tFMo3UJ4B4g"));
    }

    #[test]
    fn release_dates_are_split_by_type_from_the_us_block() {
        let item = movie_to_item(&movie(), DEFAULT_CAST);

        // Type 3 is theatrical, 4 digital, 5 physical. Type 1 (premiere) is not
        // a release date and must not become inCinemas.
        assert_eq!(item.in_cinemas.as_deref(), Some("2016-11-11"));
        assert_eq!(item.digital_release.as_deref(), Some("2017-01-31"));
        assert_eq!(item.physical_release.as_deref(), Some("2017-02-14"));
        assert_eq!(item.content_rating.as_deref(), Some("PG-13"));
        assert_eq!(item.status.as_deref(), Some("released"));
    }

    #[test]
    fn a_re_release_does_not_move_a_film_forward() {
        // Fight Club, as TMDB lists its US dates: the 2026 re-release first.
        let mut m = movie();
        m.release_dates = serde_json::from_value(serde_json::json!({
            "results": [{
                "iso_3166_1": "US",
                "release_dates": [
                    { "type": 3, "release_date": "2026-04-22T00:00:00.000Z", "certification": "R" },
                    { "type": 4, "release_date": "2026-05-12T00:00:00.000Z", "certification": "R" },
                    { "type": 3, "release_date": "1999-10-15T00:00:00.000Z", "certification": "R" },
                    { "type": 5, "release_date": "2000-04-25T00:00:00.000Z", "certification": "R" },
                    { "type": 4, "release_date": "2009-11-17T00:00:00.000Z", "certification": "R" },
                    { "type": 2, "release_date": "1999-10-14T00:00:00.000Z", "certification": "R" }
                ]
            }]
        }))
        .unwrap();

        let item = movie_to_item(&m, DEFAULT_CAST);

        // A limited run before the wide one is still the first time in cinemas.
        assert_eq!(item.in_cinemas.as_deref(), Some("1999-10-14"));
        assert_eq!(item.digital_release.as_deref(), Some("2009-11-17"));
        assert_eq!(item.physical_release.as_deref(), Some("2000-04-25"));
    }

    #[test]
    fn empty_translations_are_dropped() {
        let item = movie_to_item(&movie(), DEFAULT_CAST);

        // The fixture has three: one full, one title-only, one empty.
        assert_eq!(item.translations.len(), 2);
        let french = item
            .translations
            .iter()
            .find(|t| t.language == "fra")
            .unwrap();
        assert_eq!(french.title.as_deref(), Some("Premier Contact"));
    }

    #[test]
    fn a_mapped_series_survives_the_trip_to_sonarrs_wire_format() {
        let item = tv_to_item(&tv(), &[season()], DEFAULT_CAST);
        let resource = sonarr::from_item(&item, 81189, "en");

        assert_eq!(resource.tvdb_id, 81189);
        assert_eq!(resource.tmdb_id, Some(1396));
        assert_eq!(resource.title, "Breaking Bad");
        assert_eq!(resource.status, "Ended");
        assert_eq!(resource.episodes.len(), 3);
        assert_eq!(resource.seasons.len(), 2);
        assert_eq!(resource.actors.len(), 2);
        assert_eq!(resource.rating.as_ref().unwrap().value, "8.9");
        // Every episode must carry the id the client asked for.
        assert!(resource.episodes.iter().all(|e| e.tvdb_show_id == 81189));
    }

    #[test]
    fn an_episode_tmdb_only_numbers_reaches_sonarr_as_tba() {
        // TMDB in French for Reincarnated as a Sword's second season: the
        // first episode is named, the others are called by their number, in
        // French. "Épisode 3" reached Sonarr as a title where Skyhook sends
        // TBA, and Sonarr named the file after it. Left empty, the title is
        // TheTVDB's if it has one, and TBA otherwise.
        let episode = |number: i32, name: &str| models::Episode {
            id: None,
            season_number: 2,
            episode_number: number,
            name: Some(name.into()),
            overview: None,
            air_date: None,
            runtime: None,
            still_path: None,
            vote_average: None,
            vote_count: None,
            episode_type: None,
        };
        let seasons = [models::Season {
            season_number: 2,
            episodes: vec![
                episode(1, "Cette île qui flotte dans le ciel"),
                episode(2, "Épisode 2"),
                episode(3, "Épisode 3"),
            ],
        }];
        let item = tv_to_item(&tv(), &seasons, DEFAULT_CAST);

        let stored: Vec<&str> = item.episodes.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(stored, ["Cette île qui flotte dans le ciel", "", ""]);

        let sent: Vec<String> = sonarr::from_item(&item, 410378, "en")
            .episodes
            .into_iter()
            .map(|e| e.title)
            .collect();
        assert_eq!(sent, ["Cette île qui flotte dans le ciel", "TBA", "TBA"]);
    }

    #[test]
    fn a_mapped_movie_survives_the_trip_to_radarrs_wire_format() {
        let item = movie_to_item(&movie(), DEFAULT_CAST);
        let resource = radarr::from_item(&item);

        assert_eq!(resource.tmdb_id, 329865);
        assert_eq!(resource.imdb_id.as_deref(), Some("tt2543164"));
        assert_eq!(resource.title_slug, "arrival-2016");
        assert_eq!(resource.year, 2016);
        assert_eq!(resource.status.as_deref(), Some("released"));
        assert_eq!(resource.in_cinema.as_deref(), Some("2016-11-11"));
        // Every country's rating, each its own, for Radarr to pick its own.
        let sent: Vec<(&str, &str)> = resource
            .certifications
            .iter()
            .map(|c| (c.country.as_str(), c.certification.as_str()))
            .collect();
        assert_eq!(
            sent,
            [("DE", "12"), ("FR", "TP"), ("GB", "12A"), ("US", "PG-13")]
        );

        let credits = resource.credits.unwrap();
        assert_eq!(credits.cast.len(), 2);
        // The director and the writer, with the jobs TMDB gives them.
        let crew: Vec<(Option<&str>, Option<&str>)> = credits
            .crew
            .iter()
            .map(|c| (c.job.as_deref(), c.department.as_deref()))
            .collect();
        assert_eq!(
            crew,
            [
                (Some("Director"), Some("Directing")),
                (Some("Screenplay"), Some("Writing"))
            ]
        );
        // Radarr rejects a credit without this.
        assert!(credits.cast.iter().all(|c| !c.credit_id.is_empty()));
        assert!(credits.crew.iter().all(|c| !c.credit_id.is_empty()));
    }

    #[test]
    fn the_wire_forms_serialise_without_losing_required_fields() {
        let series = serde_json::to_value(sonarr::from_item(
            &tv_to_item(&tv(), &[season()], DEFAULT_CAST),
            81189,
            "en",
        ))
        .unwrap();
        assert_eq!(series["tvdbId"], 81189);
        assert_eq!(series["title"], "Breaking Bad");
        assert!(series["episodes"].as_array().unwrap().len() == 3);

        // Radarr's own service answers in PascalCase; Sonarr's Skyhook does not.
        let film = serde_json::to_value(radarr::from_item(&movie_to_item(&movie(), DEFAULT_CAST)))
            .unwrap();
        assert_eq!(film["TmdbId"], 329865);
        assert_eq!(film["TitleSlug"], "arrival-2016");
        assert_eq!(film["PhysicalRelease"], "2017-02-14");
    }

    #[test]
    fn every_country_s_releases_are_kept_with_their_ratings_and_notes() {
        let item = movie_to_item(&movie(), DEFAULT_CAST);
        // The record's own dates and rating are still the American ones.
        assert_eq!(item.in_cinemas.as_deref(), Some("2016-11-11"));
        assert_eq!(item.content_rating.as_deref(), Some("PG-13"));
        assert!(
            item.releases
                .iter()
                .all(|r| r.source.as_deref() == Some("tmdb"))
        );

        let france: Vec<_> = item.releases.iter().filter(|r| r.country == "FR").collect();
        assert_eq!(france.len(), 3);
        assert_eq!(france[0].release_type, Some(ReleaseKind::Theatrical));
        assert_eq!(france[0].date.as_deref(), Some("2016-12-07"));
        assert_eq!(france[0].certification.as_deref(), Some("TP"));
        assert_eq!(france[1].release_type, Some(ReleaseKind::Digital));
        assert_eq!(france[1].note.as_deref(), Some("VOD"));

        // As a reader in each country has it.
        assert_eq!(item.certification_in("FR"), Some("TP"));
        assert_eq!(
            item.certification_in("GB"),
            Some("12A"),
            "the cinemas' rating, not the disc's"
        );
        assert_eq!(item.certification_in("US"), Some("PG-13"));
        assert_eq!(item.certification_in("JP"), None);
        let france = item.days_in("FR");
        assert_eq!(france.cinema.as_deref(), Some("2016-12-07"));
        assert_eq!(france.digital.as_deref(), Some("2017-04-07"));
        assert_eq!(france.physical.as_deref(), Some("2017-04-12"));
        // A festival's showing is not a release in cinemas.
        assert_eq!(item.days_in("GB").cinema.as_deref(), Some("2016-11-10"));
        assert_eq!(item.days_in("JP"), crate::domain::ReleaseDays::default());
    }

    #[test]
    fn what_tmdb_sends_beside_the_rest_is_kept() {
        let film = movie_to_item(&movie(), DEFAULT_CAST);
        assert_eq!(film.tagline.as_deref(), Some("Why are they here?"));
        assert_eq!(film.production_countries, ["US"]);
        assert_eq!(film.production_companies, ["21 Laps Entertainment"]);
        assert_eq!(film.external_ids.wikidata.as_deref(), Some("Q21062142"));

        let series = tv_to_item(&tv(), &[], DEFAULT_CAST);
        assert_eq!(series.external_ids.wikidata.as_deref(), Some("Q1079"));
        // No country of production listed: the one it comes from.
        assert_eq!(series.production_countries, ["US"]);
        let rated: Vec<(&str, Option<&str>)> = series
            .releases
            .iter()
            .map(|r| (r.country.as_str(), r.certification.as_deref()))
            .collect();
        assert_eq!(rated, [("GB", Some("15")), ("US", Some("TV-MA"))]);
        assert!(series.releases.iter().all(|r| r.release_type.is_none()));
    }

    /// TMDB's own example of `/tv/{id}` and `/tv/{id}/content_ratings`.
    #[test]
    fn a_series_keeps_its_creators_countries_companies_and_every_rating() {
        let tv: models::Tv = serde_json::from_value(serde_json::json!({
            "id": 1399,
            "name": "Game of Thrones",
            "first_air_date": "2011-04-17",
            "tagline": "Winter Is Coming",
            "origin_country": ["US"],
            "created_by": [
                { "id": 9813, "credit_id": "5256c8c219c2956ff604858a", "name": "David Benioff",
                  "gender": 2, "profile_path": "/xvNN5huL0X8yJ7h3IZfGG4O2zBD.jpg" },
                { "id": 228068, "credit_id": "552e611e9251413fea000901", "name": "D.B. Weiss",
                  "gender": 2, "profile_path": "/2RMejaT793U9KRk2IEbFfteQntE.jpg" }
            ],
            "networks": [{ "id": 49, "name": "HBO" }],
            "production_companies": [
                { "id": 76043, "logo_path": "/9RO2vbQ67otPrBLXCaC8UMp3Qat.png", "name": "Revolution Sun Studios", "origin_country": "US" },
                { "id": 12525, "logo_path": null, "name": "Television 360", "origin_country": "" },
                { "id": 5820, "logo_path": null, "name": "Generator Entertainment", "origin_country": "GB" },
                { "id": 12526, "logo_path": null, "name": "Bighead Littlehead", "origin_country": "" }
            ],
            "production_countries": [
                { "iso_3166_1": "GB", "name": "United Kingdom" },
                { "iso_3166_1": "US", "name": "United States of America" }
            ],
            "content_ratings": { "id": 1399, "results": [
                { "descriptors": [], "iso_3166_1": "DE", "rating": "16" },
                { "descriptors": [], "iso_3166_1": "AU", "rating": "R18+" },
                { "descriptors": [], "iso_3166_1": "FR", "rating": "16" },
                { "descriptors": [], "iso_3166_1": "US", "rating": "TV-MA" },
                { "descriptors": [], "iso_3166_1": "CA", "rating": "18+" },
                { "descriptors": [], "iso_3166_1": "RU", "rating": "18+" },
                { "descriptors": [], "iso_3166_1": "KR", "rating": "19" },
                { "descriptors": [], "iso_3166_1": "GB", "rating": "18" },
                { "descriptors": [], "iso_3166_1": "BR", "rating": "16" },
                { "descriptors": [], "iso_3166_1": "NL", "rating": "16" },
                { "descriptors": [], "iso_3166_1": "PT", "rating": "18" },
                { "descriptors": [], "iso_3166_1": "HU", "rating": "18" },
                { "descriptors": [], "iso_3166_1": "ES", "rating": "18" },
                { "descriptors": [], "iso_3166_1": "SG", "rating": "R21" },
                { "descriptors": [], "iso_3166_1": "IN", "rating": "A" },
                { "descriptors": [], "iso_3166_1": "MX", "rating": "C" }
            ] }
        }))
        .expect("TMDB's example");

        let item = tv_to_item(&tv, &[], DEFAULT_CAST);
        assert_eq!(item.tagline.as_deref(), Some("Winter Is Coming"));
        assert_eq!(item.production_countries, ["GB", "US"]);
        assert_eq!(
            item.production_companies,
            [
                "Revolution Sun Studios",
                "Television 360",
                "Generator Entertainment",
                "Bighead Littlehead"
            ]
        );
        assert_eq!(item.studio.as_deref(), Some("Revolution Sun Studios"));

        let creators: Vec<(&str, Option<&str>, Option<&str>)> = item
            .credits
            .iter()
            .filter(|c| c.credit_type == CreditType::Writer)
            .map(|c| {
                (
                    c.person_name.as_str(),
                    c.character_name.as_deref(),
                    c.credit_tmdb_id.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            creators,
            [
                (
                    "David Benioff",
                    Some("Creator"),
                    Some("5256c8c219c2956ff604858a")
                ),
                (
                    "D.B. Weiss",
                    Some("Creator"),
                    Some("552e611e9251413fea000901")
                )
            ]
        );

        // Sixteen countries, each its own; the record's is the American one.
        assert_eq!(item.releases.len(), 16);
        assert_eq!(item.content_rating.as_deref(), Some("TV-MA"));
        assert_eq!(item.certification_in("FR"), Some("16"));
        assert_eq!(item.certification_in("KR"), Some("19"));
        let certifications = item.certifications();
        assert_eq!(certifications.len(), 16);
        assert_eq!(certifications[0], ("AU".to_string(), "R18+".to_string()));
    }

    /// Shaped as TMDB answers `credits`: the director, the writing
    /// department, a producer — and the rest of the crew, which is not kept.
    #[test]
    fn the_crew_is_directors_creators_writers_then_producers() {
        let document: models::Credits = serde_json::from_value(serde_json::json!({
            "cast": [],
            "crew": [
                { "id": 4, "credit_id": "e", "name": "Shawn Levy", "job": "Producer", "department": "Production" },
                { "id": 1, "credit_id": "a", "name": "Denis Villeneuve", "job": "Director", "department": "Directing" },
                { "id": 2, "credit_id": "b", "name": "Eric Heisserer", "job": "Screenplay", "department": "Writing" },
                { "id": 3, "credit_id": "c", "name": "Ted Chiang", "job": "Novel", "department": "Writing" },
                { "id": 2, "credit_id": "d", "name": "Eric Heisserer", "job": "Executive Producer", "department": "Production" },
                { "id": 5, "credit_id": "f", "name": "Jóhann Jóhannsson", "job": "Original Music Composer", "department": "Sound" },
                { "id": 2, "credit_id": "g", "name": "Eric Heisserer", "job": "Story", "department": "Writing" }
            ]
        }))
        .unwrap();

        let kept: Vec<(CreditType, String, Option<String>, i32)> =
            credits(Some(&document), &[], DEFAULT_CAST)
                .into_iter()
                .map(|c| (c.credit_type, c.person_name, c.character_name, c.sort_order))
                .collect();
        let crew = |kind, name: &str, job: &str, order| {
            (kind, name.to_string(), Some(job.to_string()), order)
        };
        assert_eq!(
            kept,
            [
                crew(CreditType::Director, "Denis Villeneuve", "Director", 0),
                crew(CreditType::Writer, "Eric Heisserer", "Screenplay", 1),
                crew(CreditType::Writer, "Ted Chiang", "Novel", 2),
                crew(CreditType::Producer, "Shawn Levy", "Producer", 3),
            ]
        );
    }

    #[test]
    fn the_cast_is_cut_where_the_setting_says() {
        let mut film = movie();
        let cast: Vec<serde_json::Value> = (0..40)
            .map(|n| {
                serde_json::json!({
                    "id": n,
                    "credit_id": format!("c{n}"),
                    "name": format!("Actor {n}"),
                    "order": n
                })
            })
            .collect();
        film.credits =
            serde_json::from_value(serde_json::json!({ "cast": cast, "crew": [] })).unwrap();
        let actors = |limit: usize| {
            movie_to_item(&film, limit)
                .credits
                .iter()
                .filter(|c| c.credit_type == CreditType::Actor)
                .count()
        };

        assert_eq!(actors(DEFAULT_CAST), 25);
        assert_eq!(actors(3), 3);
        assert_eq!(actors(1_000), 40, "no more than there are");
        assert_eq!(actors(0), 1, "one at the least");
    }

    #[test]
    fn a_mapped_film_s_document_names_its_tagline_countries_and_companies() {
        let mut film = movie_to_item(&movie(), DEFAULT_CAST);
        film.production_companies
            .push("FilmNation Entertainment".into());
        film.production_countries.push("CA".into());

        let nfo = crate::export::nfo::for_item(&film);
        assert!(
            nfo.contains("<tagline>Why are they here?</tagline>"),
            "{nfo}"
        );
        assert!(nfo.contains("<country>United States of America</country>"));
        assert!(nfo.contains("<country>Canada</country>"));
        assert_eq!(
            nfo.matches("<studio>21 Laps Entertainment</studio>")
                .count(),
            1,
            "the studio once"
        );
        assert!(nfo.contains("<studio>FilmNation Entertainment</studio>"));
    }

    #[test]
    fn a_country_tmdb_dates_without_a_rating_takes_radarr_s() {
        use crate::merge::{Contribution, combine};

        let mut tmdb = movie_to_item(&movie(), DEFAULT_CAST);
        // TMDB lists the French release without its rating.
        for release in tmdb.releases.iter_mut().filter(|r| r.country == "FR") {
            release.certification = None;
        }
        let rated = |country: &str, rating: &str| CountryRelease {
            country: country.into(),
            release_type: None,
            date: None,
            certification: Some(rating.into()),
            note: None,
            source: Some("radarr".into()),
        };
        let mut radarr = MediaItem::empty(MediaKind::Movie);
        radarr.releases = vec![rated("FR", "TP"), rated("JP", "G"), rated("US", "R")];

        let merged = combine(
            vec![
                Contribution {
                    provider: "tmdb".into(),
                    item: tmdb,
                },
                Contribution {
                    provider: "radarr".into(),
                    item: radarr,
                },
            ],
            &["tmdb".to_string(), "radarr".to_string()],
        )
        .expect("merged");

        assert_eq!(merged.certification_in("FR"), Some("TP"));
        assert_eq!(
            merged.days_in("FR").cinema.as_deref(),
            Some("2016-12-07"),
            "the days stay TMDB's"
        );
        assert_eq!(
            merged.certification_in("JP"),
            Some("G"),
            "a country TMDB has nothing of"
        );
        assert_eq!(
            merged.certification_in("US"),
            Some("PG-13"),
            "a country TMDB rated keeps its rating"
        );
    }
}

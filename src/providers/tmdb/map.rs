//! TMDB → canonical model.
//!
//! This grew out of an earlier Skyhook stand-in's mapper, retargeted: it now
//! produces a canonical [`MediaItem`] rather than a Skyhook response, and it
//! handles movies as well as series. Rendering to Sonarr's or Radarr's wire
//! format happens later, in `api::compat`, from the canonical form.

use crate::{
    db::{new_id, now},
    domain::{
        AlternativeTitle, CoverType, Credit, CreditType, Episode, ExternalIds, Image, MediaItem,
        MediaKind, Rating, RatingValue, Season, Translation, make_slug,
    },
    providers::{
        lang::{iso_639_1_to_3, iso_3166_2_to_3},
        tmdb::{image_url, models},
    },
};

/// Cast members kept per work. Sonarr shows a handful; the full list for a long
/// running series runs to hundreds of rows with no consumer.
const MAX_CAST: usize = 25;

/// Region preferred when a provider offers per-country data.
const PREFERRED_REGION: &str = "US";

// ─── series ──────────────────────────────────────────────────────────────────

pub fn tv_to_item(tv: &models::Tv, seasons: &[models::Season]) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Series);

    item.title = tv.name.clone();
    item.is_adult = tv.adult.unwrap_or(false);
    item.original_title = non_empty(tv.original_name.as_deref());
    item.overview = non_empty(tv.overview.as_deref());
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
    let (rating, rating_country) = content_rating(tv.content_ratings.as_ref());
    item.content_rating = rating;
    item.content_rating_country = rating_country;
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
        ..Default::default()
    };

    item.ratings = vote_rating(tv.vote_average, tv.vote_count);
    item.images = artwork(
        tv.poster_path.as_deref(),
        tv.backdrop_path.as_deref(),
        tv.images.as_ref(),
    );
    item.credits = cast(tv.credits.as_ref());
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

// ─── movie ───────────────────────────────────────────────────────────────────

pub fn movie_to_item(movie: &models::Movie) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Movie);

    item.title = movie.title.clone();
    item.is_adult = movie.adult.unwrap_or(false);
    item.original_title = non_empty(movie.original_title.as_deref());
    item.overview = non_empty(movie.overview.as_deref());
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
        ..Default::default()
    };

    item.ratings = vote_rating(movie.vote_average, movie.vote_count);
    item.images = artwork(
        movie.poster_path.as_deref(),
        movie.backdrop_path.as_deref(),
        movie.images.as_ref(),
    );
    item.credits = cast(movie.credits.as_ref());
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

// ─── shared helpers ──────────────────────────────────────────────────────────

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

fn cast(credits: Option<&models::Credits>) -> Vec<Credit> {
    let Some(credits) = credits else {
        return Vec::new();
    };

    let mut out: Vec<Credit> = credits
        .cast
        .iter()
        .take(MAX_CAST)
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
        .collect();

    // Radarr shows a director; Sonarr ignores crew entirely.
    out.extend(
        credits
            .crew
            .iter()
            .filter(|c| c.job.as_deref() == Some("Director"))
            .take(3)
            .enumerate()
            .map(|(i, member)| Credit {
                id: new_id(),
                credit_type: CreditType::Director,
                person_name: member.name.clone(),
                character_name: None,
                image: member.profile_path.as_deref().map(image_url),
                tmdb_person_id: member.id,
                credit_tmdb_id: member.credit_id.clone(),
                sort_order: i as i32,
                is_manual: false,
            }),
    );

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

        // A translation with neither field is noise.
        if title.is_none() && overview.is_none() {
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
        );
        assert_eq!(item.runtime, Some(45));
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
        let item = tv_to_item(&tv(), &[season()]);

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
        let item = tv_to_item(&tv(), &[]);

        assert_eq!(item.external_ids.tmdb, Some(1396));
        assert_eq!(item.external_ids.tvdb, Some(81189));
        assert_eq!(item.external_ids.imdb.as_deref(), Some("tt0903747"));
        assert_eq!(item.external_ids.tvrage, Some(18164));
    }

    #[test]
    fn the_us_content_rating_is_preferred() {
        // The fixture lists GB first; US is what Sonarr expects to see.
        assert_eq!(
            tv_to_item(&tv(), &[]).content_rating.as_deref(),
            Some("TV-MA")
        );
    }

    #[test]
    fn episodes_carry_dates_titles_and_ratings() {
        let item = tv_to_item(&tv(), &[season()]);
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
        let item = tv_to_item(&tv(), &[season()]);
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
        let item = tv_to_item(&tv(), &[]);
        assert_eq!(item.seasons.len(), 2);

        let specials = &item.seasons[0];
        assert_eq!(specials.season_number, 0);
        assert_eq!(specials.images.len(), 1);
        assert_eq!(specials.images[0].season_number, Some(0));
        assert_eq!(specials.images[0].cover_type, CoverType::Poster);
    }

    #[test]
    fn cast_and_directors_are_separated() {
        let item = tv_to_item(&tv(), &[]);

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
        let item = movie_to_item(&movie());

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
        let item = movie_to_item(&movie());

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

        let item = movie_to_item(&m);

        // A limited run before the wide one is still the first time in cinemas.
        assert_eq!(item.in_cinemas.as_deref(), Some("1999-10-14"));
        assert_eq!(item.digital_release.as_deref(), Some("2009-11-17"));
        assert_eq!(item.physical_release.as_deref(), Some("2000-04-25"));
    }

    #[test]
    fn empty_translations_are_dropped() {
        let item = movie_to_item(&movie());

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
        let item = tv_to_item(&tv(), &[season()]);
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
        let item = tv_to_item(&tv(), &seasons);

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
        let item = movie_to_item(&movie());
        let resource = radarr::from_item(&item);

        assert_eq!(resource.tmdb_id, 329865);
        assert_eq!(resource.imdb_id.as_deref(), Some("tt2543164"));
        assert_eq!(resource.title_slug, "arrival-2016");
        assert_eq!(resource.year, 2016);
        assert_eq!(resource.status.as_deref(), Some("released"));
        assert_eq!(resource.in_cinema.as_deref(), Some("2016-11-11"));
        assert_eq!(resource.certifications.len(), 1);

        let credits = resource.credits.unwrap();
        assert_eq!(credits.cast.len(), 2);
        assert_eq!(credits.crew.len(), 1);
        // Radarr rejects a credit without this.
        assert!(credits.cast.iter().all(|c| !c.credit_id.is_empty()));
        assert!(credits.crew.iter().all(|c| !c.credit_id.is_empty()));
    }

    #[test]
    fn the_wire_forms_serialise_without_losing_required_fields() {
        let series = serde_json::to_value(sonarr::from_item(
            &tv_to_item(&tv(), &[season()]),
            81189,
            "en",
        ))
        .unwrap();
        assert_eq!(series["tvdbId"], 81189);
        assert_eq!(series["title"], "Breaking Bad");
        assert!(series["episodes"].as_array().unwrap().len() == 3);

        // Radarr's own service answers in PascalCase; Sonarr's Skyhook does not.
        let film = serde_json::to_value(radarr::from_item(&movie_to_item(&movie()))).unwrap();
        assert_eq!(film["TmdbId"], 329865);
        assert_eq!(film["TitleSlug"], "arrival-2016");
        assert_eq!(film["PhysicalRelease"], "2017-02-14");
    }
}

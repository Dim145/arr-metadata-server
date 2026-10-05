//! Fankai: the Fan-Kai productions.
//!
//! A Fan-Kai is an anime recut into films — the filler gone, each arc kept
//! whole — by the Fankai team, who publish metadata for their own work at
//! metadata.fankai.fr: each production as a series, its sagas as seasons, its
//! films as episodes, with artwork and synopses. It is what their Jellyfin and
//! Kodi plugins read. TheTVDB and TMDB list none of it, a recut being outside
//! what either takes, so a production has no id there and cannot be found by
//! one: this source names it itself, and the client id Sonarr is handed is
//! made from Fankai's — see `service::ids`.
//!
//! The numbering follows the files as Fankai names them: `Title.S02E08` is the
//! eighth film overall, in the second saga. A season's numbers carry on from
//! the last, which is what `episode_number` holds and what `formatted_name`
//! repeats; the file name is read first, the fields second. The specials —
//! season 0, the anime's own films — count on their own from one, and so carry
//! no absolute number, as TheTVDB's convention has it and Sonarr expects.
//!
//! No key. One call a second, and the listing is kept and revalidated by its
//! ETag rather than fetched again for every search.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    config,
    db::{new_id, now},
    domain::{
        CoverType, Credit, CreditType, Episode, ExternalIds, Image, MediaItem, MediaKind, Rating,
        Relation, Season,
    },
    providers::{PATIENCE, Pacer, alternative_title, names},
};

/// How long the listing is trusted before it is revalidated.
const CATALOGUE_TTL: Duration = Duration::from_secs(10 * 60);

/// How long a listing that could not be fetched is not asked for again. Every
/// search would otherwise wait on its own attempt, one after the other.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// The most seasons one production fetch will ask about. Each is a call.
const MAX_SEASONS: usize = 50;

/// The kinds of recut Fankai makes, which its listing files as the first
/// genre. Kept as a keyword instead: a kind is not a genre, and a filter on
/// Action should not offer Yabai beside it.
const KINDS: &[&str] = &["kai", "henshu", "yabai", "fan-cut", "kyodai"];

pub struct FankaiClient {
    http: reqwest::Client,
    base: String,
    /// Whether the catalogue speaks French, which Fankai's genres do. Any
    /// other language gets them as TMDB names them in English, so a filter
    /// on Drama does not stand beside one on Drame.
    french: bool,
    pacer: Pacer,
    gate: crate::providers::Gate,
    catalogue: tokio::sync::Mutex<Option<Catalogue>>,
}

/// The listing, as last fetched — or as last kept, when the fetch failed.
struct Catalogue {
    etag: Option<String>,
    /// Until when it is served as it is, without asking the service.
    until: tokio::time::Instant,
    productions: Arc<Vec<Production>>,
    /// Whether the last attempt failed and this is what was held before it —
    /// or nothing, when there was nothing.
    stale: bool,
}

enum Fetched {
    Body(Value, Option<String>),
    NotModified,
    NotFound,
}

impl FankaiClient {
    pub fn new(http: reqwest::Client, cfg: &config::Fankai, language: &str) -> Self {
        Self {
            http,
            base: cfg.upstream.clone(),
            french: language.trim().to_ascii_lowercase().starts_with("fr"),
            // A small service run by volunteers: one call a second.
            pacer: Pacer::new(Duration::from_secs(1)),
            // Spaced already; this is for its `Retry-After`.
            gate: crate::providers::Gate::new(names::FANKAI, "Fankai", 2),
            catalogue: tokio::sync::Mutex::new(None),
        }
    }

    /// The productions whose name contains `term`, accents and case aside, as
    /// shallow works: enough to recognise one in a list, not a fetch.
    pub async fn search(&self, term: &str, limit: usize) -> Result<Vec<MediaItem>> {
        let needle = fold(term);
        if needle.is_empty() {
            return Ok(Vec::new());
        }

        let productions = self.catalogue().await?;

        Ok(matching(&productions, &needle)
            .into_iter()
            .take(limit)
            .map(|production| to_item(production, &[], &[], self.french))
            .collect())
    }

    /// A production by its name, as another source writes it — the wiki names
    /// the Fan-Kai that follows one by its page — as a relation of
    /// `relation_type`. Only a name Fankai gives it, however it is spelt.
    pub async fn relation(&self, name: &str, relation_type: &str) -> Result<Option<Relation>> {
        let wanted = fold(name);
        if wanted.is_empty() {
            return Ok(None);
        }

        let productions = self.catalogue().await?;
        let Some(production) = productions.iter().find(|p| {
            [
                Some(p.title.as_str()),
                p.show_title.as_deref(),
                p.title_for_plex.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|n| fold(n) == wanted)
        }) else {
            return Ok(None);
        };

        Ok(Some(Relation {
            id: String::new(),
            relation_type: relation_type.to_string(),
            source: names::FANKAI.to_string(),
            external_id: production.id,
            mal_id: None,
            title: production.title.trim().to_string(),
            medium: "anime".to_string(),
            format: None,
            year: production
                .year
                .or_else(|| production.premiered.as_deref()?.get(..4)?.parse().ok()),
            image: production.images.poster.clone().filter(|u| !u.is_empty()),
            is_adult: false,
            work_id: None,
            sort_order: 0,
        }))
    }

    /// A production with its sagas, films and people, by Fankai's id. Returns
    /// the raw answers alongside the parsed work.
    ///
    /// One call for the production, one for its seasons, one per season for
    /// the films, one for the people: a season the films could not be fetched
    /// for fails the whole fetch, because a series stored short of a season
    /// is wrong in a way the next refresh would not notice. The people are the
    /// one part it goes without.
    pub async fn series(&self, id: i64) -> Result<Option<(Value, MediaItem)>> {
        let production_raw = match self.fetch(&format!("/series/{id}"), None).await? {
            Fetched::Body(value, _) => value,
            Fetched::NotFound => return Ok(None),
            Fetched::NotModified => anyhow::bail!("Fankai answered 304 to a plain request"),
        };
        let production = Production::deserialize(&production_raw)
            .context("Fankai returned a production this server could not read")?;

        let seasons_raw = match self.fetch(&format!("/series/{id}/seasons"), None).await? {
            Fetched::Body(value, _) => value,
            _ => anyhow::bail!("Fankai has no seasons for production {id}"),
        };
        let listed = SeasonList::deserialize(&seasons_raw)
            .context("Fankai returned seasons this server could not read")?;

        if listed.seasons.len() > MAX_SEASONS {
            tracing::warn!(
                fankai_id = id,
                seasons = listed.seasons.len(),
                "more seasons than this server will fetch; taking the first {MAX_SEASONS}"
            );
        }

        let mut seasons = Vec::with_capacity(listed.seasons.len());
        let mut episodes_raw = Vec::with_capacity(listed.seasons.len());

        for season in listed.seasons.into_iter().take(MAX_SEASONS) {
            let films = match self
                .fetch(&format!("/seasons/{}/episodes", season.id), None)
                .await?
            {
                Fetched::Body(value, _) => {
                    let films = EpisodeList::deserialize(&value)
                        .context("Fankai returned films this server could not read")?
                        .episodes;
                    episodes_raw.push(json!({ "season_id": season.id, "episodes": value }));
                    films
                }
                _ => anyhow::bail!(
                    "Fankai has no films for season {} of production {id}",
                    season.id
                ),
            };
            seasons.push((season, films));
        }

        let actors_raw = match self.fetch(&format!("/series/{id}/actors"), None).await {
            Ok(Fetched::Body(value, _)) => value,
            Ok(_) => Value::Null,
            Err(e) => {
                tracing::warn!(
                    fankai_id = id,
                    error = format_args!("{e:#}"),
                    "Fankai's people could not be fetched; going without"
                );
                Value::Null
            }
        };
        let actors = ActorList::deserialize(&actors_raw)
            .map(|list| list.actors)
            .unwrap_or_default();

        let item = to_item(&production, &seasons, &actors, self.french);

        Ok(Some((
            json!({
                "series": production_raw,
                "seasons": seasons_raw,
                "episodes": episodes_raw,
                "actors": actors_raw,
            }),
            item,
        )))
    }

    /// The listing, fetched once and then revalidated: the service answers a
    /// request carrying the ETag it gave with 304 while nothing has changed.
    ///
    /// One caller fetches while the rest wait on the lock and then read what
    /// it stored. A fetch that fails leaves the listing held before it, served
    /// stale for a while rather than attempted again by every search; with
    /// nothing held, the searches in that while fail at once instead of each
    /// waiting on the service.
    async fn catalogue(&self) -> Result<Arc<Vec<Production>>> {
        let mut held = self.catalogue.lock().await;
        let now = tokio::time::Instant::now();

        if let Some(catalogue) = held.as_ref()
            && now < catalogue.until
        {
            if catalogue.stale && catalogue.productions.is_empty() {
                anyhow::bail!(
                    "Fankai's listing could not be fetched a moment ago; not asking again yet"
                );
            }
            return Ok(catalogue.productions.clone());
        }

        let etag = held
            .as_ref()
            .filter(|c| !c.productions.is_empty())
            .and_then(|c| c.etag.clone());

        let failure = match self.fetch("/series", etag.as_deref()).await {
            Ok(Fetched::Body(value, etag)) => {
                let productions = Arc::new(listing(&value)?);
                *held = Some(Catalogue {
                    etag,
                    until: now + CATALOGUE_TTL,
                    productions: productions.clone(),
                    stale: false,
                });
                return Ok(productions);
            }
            Ok(Fetched::NotModified) => match held.as_mut() {
                Some(catalogue) if !catalogue.productions.is_empty() => {
                    catalogue.until = now + CATALOGUE_TTL;
                    catalogue.stale = false;
                    return Ok(catalogue.productions.clone());
                }
                _ => anyhow::anyhow!("Fankai answered 304 to a request without an ETag"),
            },
            Ok(Fetched::NotFound) => anyhow::anyhow!("Fankai's listing was not found"),
            Err(e) => e,
        };

        // Stale beats empty: a listing that was right ten minutes ago still
        // finds a title. Either way, nobody asks again for a while.
        let kept = held
            .as_ref()
            .map(|c| c.productions.clone())
            .unwrap_or_default();
        *held = Some(Catalogue {
            etag: held.as_ref().and_then(|c| c.etag.clone()),
            until: now + RETRY_AFTER,
            productions: kept.clone(),
            stale: true,
        });

        if kept.is_empty() {
            return Err(failure);
        }
        tracing::warn!(
            error = format_args!("{failure:#}"),
            "Fankai's listing could not be revalidated; using the one held"
        );
        Ok(kept)
    }

    async fn fetch(&self, path: &str, etag: Option<&str>) -> Result<Fetched> {
        if !self.pacer.turn(1, PATIENCE).await {
            anyhow::bail!("Fankai's queue is full; this fetch goes without it");
        }

        let url = format!("{}{path}", self.base);
        let request = || {
            let request = self.http.get(&url).timeout(Duration::from_secs(20));
            match etag {
                Some(etag) => request.header(reqwest::header::IF_NONE_MATCH, etag),
                None => request,
            }
        };

        let (response, _permit) = self
            .gate
            .send(request)
            .await
            .map_err(|e| anyhow::anyhow!("Fankai request failed: {url}: {e}"))?;

        match response.status() {
            reqwest::StatusCode::NOT_MODIFIED => return Ok(Fetched::NotModified),
            reqwest::StatusCode::NOT_FOUND => return Ok(Fetched::NotFound),
            reqwest::StatusCode::TOO_MANY_REQUESTS => {
                anyhow::bail!("Fankai's rate limit was reached")
            }
            status if !status.is_success() => {
                let reason = crate::providers::error_text(response).await;
                anyhow::bail!("Fankai returned {status} for {url}: {reason}")
            }
            _ => {}
        }

        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let body = crate::providers::read_json(response)
            .await
            .with_context(|| format!("Fankai returned a body this server could not read: {url}"))?;

        Ok(Fetched::Body(body, etag))
    }
}

/// The productions in a listing, minus any row this server cannot read: one
/// newly added production with a field of the wrong shape should not take the
/// other eighty-five out of the search.
fn listing(value: &Value) -> Result<Vec<Production>> {
    let rows = Vec::<Value>::deserialize(value)
        .context("Fankai returned a listing this server could not read")?;

    Ok(rows
        .iter()
        .filter_map(|row| match Production::deserialize(row) {
            Ok(production) => Some(production),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "a production in Fankai's listing could not be read; skipping it"
                );
                None
            }
        })
        .collect())
}

// ─── the wire ────────────────────────────────────────────────────────────────

/// A production, as the listing and the detail both describe it. The detail
/// adds the statistics.
#[derive(Debug, Deserialize)]
struct Production {
    id: i64,
    #[serde(default)]
    title: String,
    show_title: Option<String>,
    original_title: Option<String>,
    title_for_plex: Option<String>,
    plot: Option<String>,
    /// Comma-separated; the first is the kind of recut.
    #[serde(default)]
    genres: String,
    status: Option<String>,
    /// In French: `Japon`.
    country: Option<String>,
    studio: Option<String>,
    year: Option<i32>,
    /// The anime's own premiere, not the first film's release: what Fankai's
    /// plugins file the production under, and so what its folders are named.
    premiered: Option<String>,
    rating: Option<RatingRecord>,
    #[serde(default)]
    images: Images,
    statistics: Option<Statistics>,
    /// The production's theme, an MP3 of a couple of minutes.
    theme_music: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Images {
    poster: Option<String>,
    banner: Option<String>,
    fanart: Option<String>,
    logo: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RatingRecord {
    /// `themoviedb`: the original anime's rating there, relayed.
    name: Option<String>,
    value: Option<f64>,
    votes: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Statistics {
    last_aired: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SeasonList {
    #[serde(default)]
    seasons: Vec<SeasonRecord>,
}

#[derive(Debug, Deserialize)]
struct SeasonRecord {
    id: i64,
    season_number: i32,
    title: Option<String>,
    plot: Option<String>,
    premiered: Option<String>,
    #[serde(default)]
    images: Images,
}

#[derive(Debug, Deserialize)]
struct EpisodeList {
    #[serde(default)]
    episodes: Vec<EpisodeRecord>,
}

#[derive(Debug, Deserialize)]
struct EpisodeRecord {
    /// Counted across the whole production, not from the start of the season.
    episode_number: Option<i32>,
    /// Strings on the wire — `"2"` — hence not typed.
    display_season: Option<Value>,
    display_episode: Option<Value>,
    /// `Title.S02E08.MULTI.1080p.x264-FANKAI`: the name of the file.
    formatted_name: Option<String>,
    title: Option<String>,
    plot: Option<String>,
    aired: Option<String>,
    /// Seconds.
    duration: Option<i64>,
    thumb_image: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ActorList {
    #[serde(default)]
    actors: Vec<ActorRecord>,
}

#[derive(Debug, Deserialize)]
struct ActorRecord {
    name: Option<String>,
    /// `Kaïeur` for whoever made the cut; a character for a voice actor.
    role: Option<String>,
    thumb_url: Option<String>,
    /// A TMDB person page, for the voice actors Fankai copies from there.
    profile_url: Option<String>,
}

// ─── mapping ─────────────────────────────────────────────────────────────────

fn to_item(
    production: &Production,
    seasons: &[(SeasonRecord, Vec<EpisodeRecord>)],
    actors: &[ActorRecord],
    french: bool,
) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Series);

    item.title = production.title.trim().to_string();
    item.original_title = production
        .original_title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case(&item.title))
        .map(String::from);
    item.overview = production.plot.as_deref().and_then(clean);
    item.status = Some(status(production.status.as_deref()).to_string());
    item.year = production
        .year
        .or_else(|| production.premiered.as_deref()?.get(..4)?.parse().ok());
    item.slug = crate::domain::make_slug(&item.title, item.year);
    item.first_aired = production.premiered.clone().filter(|d| !d.is_empty());
    item.last_aired = production
        .statistics
        .as_ref()
        .and_then(|s| s.last_aired.clone())
        .filter(|d| !d.is_empty());
    // Who put it out. Fankai names itself as the studio too, but the studio of
    // an anime is whoever animated it, and that is not known here.
    item.network = Some(
        production
            .studio
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Fan-Kai".to_string()),
    );
    // No homepage: the website numbers its productions its own way, behind a
    // sign-in, and nothing here says which of its pages this one is. The
    // wiki's page for it is the one given, by that source.
    item.theme_music = production
        .theme_music
        .clone()
        .filter(|u| u.starts_with("https://"));

    if let Some((country, language)) = production.country.as_deref().and_then(country) {
        item.original_country = Some(country.to_string());
        item.original_language = Some(language.to_string());
    }

    let (kinds, genres) = split_genres(&production.genres, french);
    item.genres = genres;
    item.keywords = std::iter::once("Fan-Kai".to_string())
        .chain(kinds)
        .collect();

    item.external_ids = ExternalIds {
        fankai: Some(production.id),
        ..Default::default()
    };

    if let Some(rating) = &production.rating
        && let Some(value) = rating.value.filter(|v| *v > 0.0)
    {
        item.ratings.push(Rating {
            source: rating_source(rating.name.as_deref()),
            value: Some(value),
            votes: rating.votes,
            rating_type: Some("user".to_string()),
        });
    }

    let artwork = [
        (CoverType::Poster, &production.images.poster),
        (CoverType::Banner, &production.images.banner),
        (CoverType::Fanart, &production.images.fanart),
        (CoverType::Clearlogo, &production.images.logo),
    ];
    for (slot, (cover, url)) in artwork.into_iter().enumerate() {
        if let Some(url) = url.as_deref().filter(|u| !u.is_empty()) {
            item.images.push(image(cover, url, None, slot as i32));
        }
    }

    for other in [
        production.show_title.as_deref(),
        production.title_for_plex.as_deref(),
    ] {
        if let Some(title) = other
            && !title.trim().eq_ignore_ascii_case(&item.title)
            && let Some(alternative) = alternative_title(title, names::FANKAI, Some("fr"))
            && !item
                .alternative_titles
                .iter()
                .any(|t| t.title == alternative.title)
        {
            item.alternative_titles.push(alternative);
        }
    }

    let mut placed = std::collections::HashSet::new();

    for (season, films) in seasons {
        // The store keeps one row per season number and one per placing, and
        // would quietly keep the last of two; the first is kept here, and said.
        if !item
            .seasons
            .iter()
            .all(|s| s.season_number != season.season_number)
        {
            tracing::warn!(
                fankai_id = production.id,
                season = season.season_number,
                "Fankai lists two seasons with one number; keeping the first"
            );
            continue;
        }

        let artwork = [
            (CoverType::Poster, &season.images.poster),
            (CoverType::Fanart, &season.images.fanart),
        ];
        item.seasons.push(Season {
            id: new_id(),
            season_number: season.season_number,
            title: season
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from),
            overview: season.plot.as_deref().and_then(clean),
            air_date: season.premiered.clone().filter(|d| !d.is_empty()),
            tmdb_id: None,
            tvdb_id: None,
            is_manual: false,
            images: artwork
                .into_iter()
                .enumerate()
                .filter_map(|(slot, (cover, url))| {
                    url.as_deref()
                        .filter(|u| !u.is_empty())
                        .map(|u| image(cover, u, Some(season.season_number), slot as i32))
                })
                .collect(),
            primary_images: Default::default(),
        });

        for film in films {
            let Some((season_number, number)) = placing(film, season.season_number) else {
                continue;
            };
            if !placed.insert((season_number, number)) {
                tracing::warn!(
                    fankai_id = production.id,
                    season = season_number,
                    number,
                    "Fankai lists two films at one place; keeping the first"
                );
                continue;
            }

            item.episodes.push(Episode {
                id: new_id(),
                season_number,
                episode_number: number,
                // The count runs across the sagas, and the specials count on
                // their own: TheTVDB's convention, which Sonarr matches files by.
                absolute_episode_number: (season_number > 0).then_some(number),
                aired_after_season_number: None,
                aired_before_season_number: None,
                aired_before_episode_number: None,
                title: film
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map_or_else(|| format!("Film {number}"), String::from),
                overview: film.plot.as_deref().and_then(clean),
                air_date: film.aired.clone().filter(|d| !d.is_empty()),
                air_date_utc: None,
                runtime: film
                    .duration
                    .map(|d| ((d as f64) / 60.0).round() as i32)
                    .filter(|m| *m > 0),
                finale_type: None,
                image: film.thumb_image.clone().filter(|u| !u.is_empty()),
                tvdb_id: None,
                tmdb_id: None,
                rating: None,
                is_manual: false,
            });
        }
    }

    item.seasons.sort_by_key(|s| s.season_number);
    item.episodes
        .sort_by_key(|e| (e.season_number, e.episode_number));

    // A film is an hour or so; the mean is what a client expects of "runtime".
    // Summed as floats: the lengths are the service's numbers, and a sum of
    // them past `i32` panics in a debug build and wraps in a release one.
    let lengths: Vec<i32> = item.episodes.iter().filter_map(|e| e.runtime).collect();
    if !lengths.is_empty() {
        let mean = lengths.iter().map(|n| f64::from(*n)).sum::<f64>() / lengths.len() as f64;
        item.runtime = Some(mean.round() as i32);
    }

    for (slot, actor) in actors.iter().enumerate() {
        let Some(name) = actor
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        else {
            continue;
        };

        item.credits.push(Credit {
            id: new_id(),
            credit_type: CreditType::Actor,
            person_name: name.to_string(),
            character_name: actor
                .role
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(String::from),
            // The kaïeurs' picture is the wiki's own logo, on Fandom's
            // servers, which refuse to be shown on any other site: a broken
            // image where the initials would do.
            image: actor
                .thumb_url
                .clone()
                .filter(|u| !u.is_empty() && !u.contains("static.wikia.nocookie.net")),
            tmdb_person_id: actor.profile_url.as_deref().and_then(tmdb_person),
            credit_tmdb_id: None,
            sort_order: slot as i32,
            is_manual: false,
        });
    }

    item.updated_at = now();
    item
}

/// Where a film goes: its season and its number, read off the file name first
/// and off the fields when the name does not carry them.
fn placing(film: &EpisodeRecord, season_number: i32) -> Option<(i32, i32)> {
    if let Some(found) = film.formatted_name.as_deref().and_then(numbers_in) {
        return Some(found);
    }

    let placed = film
        .episode_number
        .or_else(|| film.display_episode.as_ref().and_then(number))?;
    let season = film
        .display_season
        .as_ref()
        .and_then(number)
        .unwrap_or(season_number);

    (placed > 0).then_some((season, placed))
}

/// `S02E08` out of `Title.S02E08.MULTI.1080p.x264-FANKAI`.
fn numbers_in(name: &str) -> Option<(i32, i32)> {
    name.split('.').find_map(|part| {
        let (season, episode) = part.strip_prefix('S')?.split_once('E')?;
        let season = season.parse::<i32>().ok()?;
        let episode = episode.parse::<i32>().ok()?;
        (season >= 0 && episode > 0).then_some((season, episode))
    })
}

/// A number the wire may carry as a string.
fn number(value: &Value) -> Option<i32> {
    value
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .or_else(|| value.as_str()?.trim().parse().ok())
}

/// Fankai's statuses in the catalogue's vocabulary. A production on hold —
/// `En suspens` — is still one more films may come to, so it stays
/// `continuing`, which is what keeps a client watching for them.
fn status(raw: Option<&str>) -> &'static str {
    match fold(raw.unwrap_or_default()).as_str() {
        "ended" | "canceled" | "cancelled" | "termine" => "ended",
        _ => "continuing",
    }
}

/// The kinds of recut, apart from the genres proper — the genres in French
/// as Fankai gives them, or in English for a catalogue that speaks it.
fn split_genres(raw: &str, french: bool) -> (Vec<String>, Vec<String>) {
    let mut kinds = Vec::new();
    let mut genres: Vec<String> = Vec::new();

    for token in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if KINDS.contains(&fold(token).as_str()) {
            kinds.push(token.to_string());
            continue;
        }

        let genre = if french { token } else { in_english(token) };
        if !genres.iter().any(|g| g.eq_ignore_ascii_case(genre)) {
            genres.push(genre.to_string());
        }
    }

    (kinds, genres)
}

/// A genre as TMDB names it in English, for the ones Fankai uses. One it
/// does not know stays as it came.
fn in_english(genre: &str) -> &str {
    match fold(genre).as_str() {
        "aventure" => "Adventure",
        "comedie" => "Comedy",
        "drame" => "Drama",
        "fantastique" => "Fantasy",
        "science-fiction" => "Science Fiction",
        "mystere" => "Mystery",
        "horreur" => "Horror",
        "histoire" => "History",
        "arts martiaux" => "Martial Arts",
        "suspens" | "suspense" => "Suspense",
        "guerre" => "War",
        "familial" => "Family",
        "nourriture" => "Food",
        "sport" => "Sport",
        "policier" => "Crime",
        "documentaire" => "Documentary",
        "musique" => "Music",
        "surnaturel" => "Supernatural",
        "psychologique" => "Psychological",
        "enfants" => "Children",
        _ => genre,
    }
}

/// The catalogue's name for where a relayed rating comes from.
fn rating_source(name: Option<&str>) -> String {
    match name.map(str::trim) {
        Some("themoviedb" | "tmdb") => "tmdb".to_string(),
        Some("imdb") => "imdb".to_string(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => names::FANKAI.to_string(),
    }
}

/// A country as Fankai names it, in French, as a code and the language it
/// implies for the original.
fn country(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match fold(name).as_str() {
        "japon" | "japan" => ("JP", "ja"),
        "coree du sud" | "south korea" => ("KR", "ko"),
        "chine" | "china" => ("CN", "zh"),
        "taiwan" => ("TW", "zh"),
        "france" => ("FR", "fr"),
        "etats-unis" | "united states" => ("US", "en"),
        "royaume-uni" | "united kingdom" => ("GB", "en"),
        "canada" => ("CA", "en"),
        _ => return None,
    })
}

/// The TMDB person behind `https://www.themoviedb.org/person/81244`.
fn tmdb_person(url: &str) -> Option<i64> {
    let rest = url.split("themoviedb.org/person/").nth(1)?;
    rest.split(['/', '-', '?']).next()?.parse().ok()
}

/// A synopsis as Fankai writes it, minus the carriage returns their tooling
/// leaves in it — four before every newline — and the blank runs between.
fn clean(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut blank = false;

    for line in text.replace('\r', "").lines() {
        let line = line.trim();
        if line.is_empty() {
            blank = true;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank {
                out.push('\n');
            }
        }
        blank = false;
        out.push_str(line);
    }

    (!out.is_empty()).then_some(out)
}

fn image(cover_type: CoverType, url: &str, season_number: Option<i32>, slot: i32) -> Image {
    Image {
        id: new_id(),
        season_number,
        cover_type,
        url: url.to_string(),
        language: None,
        sort_order: slot,
        source: Some(names::FANKAI.to_string()),
        is_manual: false,
    }
}

/// The productions named by `needle`, best match first: a name that is the
/// term, then one that starts with it, then one that has it somewhere.
fn matching<'a>(productions: &'a [Production], needle: &str) -> Vec<&'a Production> {
    let mut found: Vec<(u8, &Production)> = productions
        .iter()
        .filter_map(|production| {
            [
                Some(production.title.as_str()),
                production.show_title.as_deref(),
                production.title_for_plex.as_deref(),
                production.original_title.as_deref(),
            ]
            .into_iter()
            .flatten()
            .map(fold)
            .filter_map(|name| {
                if name == needle {
                    Some(0)
                } else if name.starts_with(needle) {
                    Some(1)
                } else if name.contains(needle) {
                    Some(2)
                } else {
                    None
                }
            })
            .min()
            .map(|rank| (rank, production))
        })
        .collect();

    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.title.cmp(&b.1.title)));
    found
        .into_iter()
        .map(|(_, production)| production)
        .collect()
}

/// Text as a reader types it: lower case, accents gone, one space between
/// words. `Horimiya Kaï` and `horimiya kai` are the same name.
pub(crate) fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for c in text.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' | 'ā' => out.push('a'),
            'ç' => out.push('c'),
            'é' | 'è' | 'ê' | 'ë' | 'ē' => out.push('e'),
            'î' | 'ï' | 'í' | 'ì' | 'ī' => out.push('i'),
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' | 'ō' => out.push('o'),
            'ù' | 'û' | 'ü' | 'ú' | 'ū' => out.push('u'),
            'ÿ' | 'ý' => out.push('y'),
            'ñ' => out.push('n'),
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            'ß' => out.push_str("ss"),
            c if c.is_whitespace() => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn production() -> Production {
        serde_json::from_value(json!({
            "id": 12,
            "title": "Naruto Shippuden Yabai",
            "show_title": "Naruto Shippuden Yabai",
            "original_title": "ナルト 疾風伝",
            "title_for_plex": "Naruto Shippuden Yabai",
            "plot": "Après des années d'entraînement, Naruto revient.\r\r\r\r\n\r\r\r\r\nUne suite.",
            "genres": "Yabai,Action,Aventure,Animation,Anime,Comédie,Drame,Fantastique",
            "status": "Ended",
            "country": "Japon",
            "studio": "Fan-Kai",
            "year": 2007,
            "premiered": "2007-02-15",
            "rating": { "name": "themoviedb", "user": 0.0, "value": 8.6, "votes": 8104 },
            "images": {
                "banner": "https://metadata.fankai.fr/series/12/image/banner?t=1",
                "fanart": "https://metadata.fankai.fr/series/12/image/fanart?t=1",
                "logo": "https://metadata.fankai.fr/series/12/image/logo?t=1",
                "poster": "https://metadata.fankai.fr/series/12/image/poster?t=1"
            },
            "statistics": {
                "episodes_count": 36, "first_aired": "2022-11-06",
                "last_aired": "2024-11-16", "seasons_count": 4
            },
            "theme_music": "https://metadata.fankai.fr/series/12/theme?t=1"
        }))
        .unwrap()
    }

    fn seasons() -> Vec<(SeasonRecord, Vec<EpisodeRecord>)> {
        let first: SeasonRecord = serde_json::from_value(json!({
            "id": 203, "season_number": 1, "title": "Akatsuki",
            "plot": "La menace de l'Akatsuki plane.", "premiered": "2022-11-06",
            "images": {
                "fanart": "https://metadata.fankai.fr/seasons/203/image/fanart?t=1",
                "poster": "https://metadata.fankai.fr/seasons/203/image/poster?t=1"
            }
        }))
        .unwrap();
        let second: SeasonRecord = serde_json::from_value(json!({
            "id": 22, "season_number": 2, "title": "Itachi", "images": {}
        }))
        .unwrap();
        let specials: SeasonRecord = serde_json::from_value(json!({
            "id": 9, "season_number": 0, "title": "Films Officiels", "images": {}
        }))
        .unwrap();

        let films_first: Vec<EpisodeRecord> = serde_json::from_value(json!([
            {
                "episode_number": 7, "display_season": "1", "display_episode": "7",
                "formatted_name": "Naruto Shippuden Yabai.S01E07.MULTI.1080p.x264-FANKAI",
                "title": "Sasori", "plot": "Fin de l'arc.\r\n\r\nCe film couvre les épisodes 19 à 32.",
                "aired": "2023-03-03", "duration": 4743,
                "thumb_image": "https://metadata.fankai.fr/episodes/213/image?t=1"
            },
            {
                "episode_number": 1, "display_season": "1", "display_episode": "1",
                "formatted_name": "Naruto Shippuden Yabai.S01E01.MULTI.1080p.x264-FANKAI",
                "title": "Le retour", "aired": "2022-11-06", "duration": 3600
            }
        ]))
        .unwrap();
        let films_second: Vec<EpisodeRecord> = serde_json::from_value(json!([
            {
                "episode_number": 8, "display_season": "2", "display_episode": "8",
                "formatted_name": "Naruto Shippuden Yabai.S02E08.MULTI.1080p.x264-FANKAI",
                "title": "Les frères", "aired": "2023-03-19", "duration": 5065
            },
            {
                "episode_number": 9, "display_season": "2", "display_episode": "9",
                "title": "", "aired": "2023-03-28", "duration": 2502
            },
            {
                "episode_number": 10, "display_season": "2", "display_episode": "10",
                "formatted_name": "Naruto Shippuden Yabai.S02E10.MULTI.1080p.x264-FANKAI",
                "title": "Sans durée", "aired": "2023-04-26", "duration": 1
            },
            {
                "episode_number": 10, "display_season": "2", "display_episode": "10",
                "formatted_name": "Naruto Shippuden Yabai.S02E10.MULTI.1080p.x264-FANKAI",
                "title": "Le même, listé deux fois", "aired": "2023-04-27", "duration": 1
            }
        ]))
        .unwrap();
        let films_special: Vec<EpisodeRecord> = serde_json::from_value(json!([
            {
                "episode_number": 1, "display_season": "0", "display_episode": "1",
                "formatted_name": "Naruto Shippuden Yabai.S00E01.MULTI.1080p.x264-FANKAI",
                "title": "Naruto Shippuden : Le Film", "aired": "2007-08-04", "duration": 5400
            }
        ]))
        .unwrap();

        vec![
            (first, films_first),
            (second, films_second),
            (specials, films_special),
        ]
    }

    fn actors() -> Vec<ActorRecord> {
        serde_json::from_value(json!([
            {
                "id": 100, "name": "Triggerforce", "role": "Kaïeur",
                "thumb_url": "https://static.wikia.nocookie.net/fan-kai/images/e/e6/Site-logo.png",
                "total_appearances": 7
            },
            {
                "id": 172, "name": "Akira Ishida", "role": "Gaara (voice)",
                "profile_url": "https://www.themoviedb.org/person/81244",
                "thumb_url": "https://image.tmdb.org/t/p/h632/jnW2Gn2NlR2uwOCeyOuzypnTmkH.jpg"
            },
            { "id": 9, "name": "  ", "role": "Nobody" }
        ]))
        .unwrap()
    }

    #[test]
    fn a_film_is_numbered_as_its_file_is_named() {
        let item = to_item(&production(), &seasons(), &[], true);

        let numbers: Vec<(i32, i32, Option<i32>)> = item
            .episodes
            .iter()
            .map(|e| (e.season_number, e.episode_number, e.absolute_episode_number))
            .collect();
        // Sorted, and the second season carries on from the first.
        assert_eq!(
            numbers,
            vec![
                (0, 1, None),
                (1, 1, Some(1)),
                (1, 7, Some(7)),
                (2, 8, Some(8)),
                (2, 9, Some(9)),
                (2, 10, Some(10))
            ]
        );

        // The film listed twice at S02E10 is kept once, as first listed.
        let tenth = item
            .episodes
            .iter()
            .find(|e| e.episode_number == 10)
            .unwrap();
        assert_eq!(tenth.title, "Sans durée");

        // A length of a second — Fankai's placeholder for a film it has not
        // timed — is no length, and does not drag the mean down either.
        let tenth = item
            .episodes
            .iter()
            .find(|e| e.episode_number == 10)
            .unwrap();
        assert_eq!(tenth.runtime, None);

        // Without a file name, the fields say the same thing; without a title,
        // the film is called by its number.
        let ninth = item
            .episodes
            .iter()
            .find(|e| e.episode_number == 9)
            .unwrap();
        assert_eq!(ninth.title, "Film 9");
        assert_eq!(ninth.runtime, Some(42));
    }

    #[test]
    fn the_file_name_is_read_before_the_fields() {
        assert_eq!(
            numbers_in("Naruto Shippuden Yabai.S02E08.MULTI.1080p.x264-FANKAI"),
            Some((2, 8))
        );
        // A title beginning with S is not a placing.
        assert_eq!(numbers_in("Sword Art Online Kai.MULTI.1080p"), None);
        assert_eq!(numbers_in("Black Lagoon Henshū.S01E01"), Some((1, 1)));
        assert_eq!(numbers_in("X.S01E00"), None);
    }

    #[test]
    fn the_production_carries_its_identity() {
        let item = to_item(&production(), &seasons(), &actors(), true);

        assert_eq!(item.title, "Naruto Shippuden Yabai");
        // The export's folder, among other things, is named after it.
        assert_eq!(item.slug, "naruto-shippuden-yabai-2007");
        assert_eq!(item.original_title.as_deref(), Some("ナルト 疾風伝"));
        assert_eq!(item.external_ids.fankai, Some(12));
        assert_eq!(item.external_ids.tvdb, None);
        assert_eq!(item.external_ids.tmdb, None);
        assert_eq!(item.status.as_deref(), Some("ended"));
        assert_eq!(item.year, Some(2007));
        assert_eq!(item.first_aired.as_deref(), Some("2007-02-15"));
        assert_eq!(item.last_aired.as_deref(), Some("2024-11-16"));
        assert_eq!(item.network.as_deref(), Some("Fan-Kai"));
        assert_eq!(item.original_country.as_deref(), Some("JP"));
        assert_eq!(item.original_language.as_deref(), Some("ja"));
        // Series 12 here is not production 12 on fankai.fr: no address is
        // guessed from it.
        assert_eq!(item.homepage, None);
        assert_eq!(
            item.theme_music.as_deref(),
            Some("https://metadata.fankai.fr/series/12/theme?t=1")
        );
        assert_eq!(
            item.overview.as_deref(),
            Some("Après des années d'entraînement, Naruto revient.\n\nUne suite.")
        );
        // The mean of the films that have a length, in minutes.
        assert_eq!(item.runtime, Some(71));
        // Nothing of the production's own title is repeated as an alternative.
        assert!(item.alternative_titles.is_empty());
    }

    #[test]
    fn the_kind_of_recut_is_a_keyword_not_a_genre() {
        let item = to_item(&production(), &[], &[], true);

        assert!(!item.genres.iter().any(|g| g == "Yabai"));
        assert_eq!(item.genres[0], "Action");
        assert_eq!(item.keywords, vec!["Fan-Kai", "Yabai"]);
    }

    #[test]
    fn a_catalogue_in_another_language_gets_the_genres_in_english() {
        let item = to_item(&production(), &[], &[], false);

        assert_eq!(
            item.genres,
            vec![
                "Action",
                "Adventure",
                "Animation",
                "Anime",
                "Comedy",
                "Drama",
                "Fantasy"
            ]
        );
        // The kind is untouched either way.
        assert_eq!(item.keywords, vec!["Fan-Kai", "Yabai"]);
        assert_eq!(in_english("Science-Fiction"), "Science Fiction");
        assert_eq!(in_english("Arts Martiaux"), "Martial Arts");
        assert_eq!(in_english("Shonen"), "Shonen");
    }

    #[test]
    fn statuses_are_mapped_and_a_production_on_hold_still_continues() {
        assert_eq!(status(Some("Ended")), "ended");
        assert_eq!(status(Some("Canceled")), "ended");
        assert_eq!(status(Some("Terminé")), "ended");
        assert_eq!(status(Some("Continuing")), "continuing");
        assert_eq!(status(Some("En suspens")), "continuing");
        assert_eq!(status(None), "continuing");
    }

    #[test]
    fn artwork_is_filed_by_kind_and_season() {
        let item = to_item(&production(), &seasons(), &[], true);

        let covers: Vec<CoverType> = item.images.iter().map(|i| i.cover_type).collect();
        assert_eq!(
            covers,
            vec![
                CoverType::Poster,
                CoverType::Banner,
                CoverType::Fanart,
                CoverType::Clearlogo
            ]
        );
        assert!(
            item.images
                .iter()
                .all(|i| i.source.as_deref() == Some("fankai"))
        );

        let first = item.seasons.iter().find(|s| s.season_number == 1).unwrap();
        assert_eq!(first.title.as_deref(), Some("Akatsuki"));
        assert_eq!(first.images.len(), 2);
        assert!(first.images.iter().all(|i| i.season_number == Some(1)));
        let second = item.seasons.iter().find(|s| s.season_number == 2).unwrap();
        assert!(second.images.is_empty());

        let seventh = item
            .episodes
            .iter()
            .find(|e| e.episode_number == 7)
            .unwrap();
        assert_eq!(
            seventh.image.as_deref(),
            Some("https://metadata.fankai.fr/episodes/213/image?t=1")
        );
        assert_eq!(
            seventh.overview.as_deref(),
            Some("Fin de l'arc.\n\nCe film couvre les épisodes 19 à 32.")
        );
    }

    #[test]
    fn the_relayed_rating_is_filed_where_it_came_from() {
        let item = to_item(&production(), &[], &[], true);

        assert_eq!(item.ratings.len(), 1);
        assert_eq!(item.ratings[0].source, "tmdb");
        assert_eq!(item.ratings[0].value, Some(8.6));
        assert_eq!(item.ratings[0].votes, Some(8104));
    }

    #[test]
    fn people_keep_their_role_and_a_voice_actor_their_tmdb_page() {
        let item = to_item(&production(), &[], &actors(), true);

        assert_eq!(item.credits.len(), 2, "a blank name is not a person");
        let maker = &item.credits[0];
        assert_eq!(maker.person_name, "Triggerforce");
        assert_eq!(maker.character_name.as_deref(), Some("Kaïeur"));
        assert_eq!(maker.tmdb_person_id, None);
        assert_eq!(maker.image, None, "Fandom's logo is not shown elsewhere");
        let voice = &item.credits[1];
        assert_eq!(voice.character_name.as_deref(), Some("Gaara (voice)"));
        assert_eq!(voice.tmdb_person_id, Some(81244));
        assert_eq!(
            tmdb_person("https://www.themoviedb.org/person/81244-akira"),
            Some(81244)
        );
        assert_eq!(tmdb_person("https://example.com/person/1"), None);
    }

    #[test]
    fn a_name_is_found_however_it_is_typed() {
        // One row nobody can read does not take the others out of the search.
        let listed = listing(&json!([
            { "id": 3, "title": "Horimiya Kaï", "genres": "Kaï,Comédie" },
            { "id": 12, "title": "Naruto Shippuden Yabai", "genres": "Yabai" },
            { "id": "not-an-id", "title": "Broken" },
            { "id": 4, "title": "Naruto Kai", "genres": "Kaï" },
            { "id": 1, "title": "Black Lagoon Henshū", "original_title": "BLACK LAGOON", "genres": "Henshū" }
        ]))
        .unwrap();
        assert_eq!(listed.len(), 4);

        let titles = |needle: &str| -> Vec<i64> {
            matching(&listed, &fold(needle))
                .iter()
                .map(|p| p.id)
                .collect()
        };

        assert_eq!(titles("horimiya kai"), vec![3]);
        assert_eq!(titles("HORIMIYA  KAÏ"), vec![3]);
        // The exact name first, then the one that starts with it.
        assert_eq!(titles("Naruto Kai"), vec![4]);
        assert_eq!(titles("naruto"), vec![4, 12]);
        // The original's title finds the recut too.
        assert_eq!(titles("black lagoon"), vec![1]);
        assert_eq!(titles("one piece"), Vec::<i64>::new());
    }

    #[test]
    fn folding_settles_case_accents_and_spacing() {
        assert_eq!(fold("Horimiya Kaï"), "horimiya kai");
        assert_eq!(fold("  Black   Lagoon Henshū "), "black lagoon henshu");
        assert_eq!(fold("Cœur — Été"), "coeur — ete");
    }

    #[test]
    fn a_shallow_hit_is_enough_to_recognise_a_production() {
        let item = to_item(&production(), &[], &[], true);

        assert!(item.episodes.is_empty());
        assert!(item.seasons.is_empty());
        assert_eq!(item.external_ids.fankai, Some(12));
        assert!(
            item.images
                .iter()
                .any(|i| i.cover_type == CoverType::Poster)
        );
        assert_eq!(item.runtime, None);
    }
}

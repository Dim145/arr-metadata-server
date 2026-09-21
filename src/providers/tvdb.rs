//! TheTVDB v4.
//!
//! The one thing here that nothing else reliably supplies is **absolute episode
//! numbering**, which is what Sonarr matches anime releases on. TVDB also
//! carries air-order hints, the broadcast time of day, per-country content
//! ratings and its own artwork.
//!
//! Only series are fetched. TVDB indexes films too, but almost nothing hands us
//! a TVDB *movie* id, so the lookup would rarely fire; TMDB and Radarr's service
//! cover films between them.
//!
//! Authentication is a token from `/login`, good for about a month. It is
//! fetched on first use, kept, and refetched when the server rejects it.

use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{
    config,
    db::{new_id, now},
    domain::{
        CoverType, Episode, ExternalIds, Image, MediaItem, MediaKind, Rating, Season, Translation,
        make_slug,
    },
};

/// A search hit. Its fields are snake_case here where the rest of the API is
/// camelCase, which is TheTVDB's own inconsistency rather than a mistake.
#[derive(Debug, Deserialize)]
struct SearchHit {
    tvdb_id: Option<String>,
    name: Option<String>,
    year: Option<String>,
    overview: Option<String>,
    image_url: Option<String>,
    #[serde(default)]
    remote_ids: Vec<SearchRemoteId>,
}

#[derive(Debug, Deserialize)]
struct SearchRemoteId {
    id: Option<String>,
    #[serde(rename = "sourceName")]
    source_name: Option<String>,
}

/// A hit with no id is not a result: nothing can be fetched from it.
fn hit_to_item(hit: &SearchHit) -> Option<MediaItem> {
    let tvdb_id: i64 = hit.tvdb_id.as_deref()?.parse().ok()?;
    let title = non_empty(hit.name.as_deref())?;

    let mut item = MediaItem::empty(MediaKind::Series);

    item.title = title;
    item.year = hit.year.as_deref().and_then(|y| y.parse().ok());
    item.overview = non_empty(hit.overview.as_deref());
    item.external_ids.tvdb = Some(tvdb_id);
    item.external_ids.imdb = hit
        .remote_ids
        .iter()
        .find(|remote| remote.source_name.as_deref() == Some("IMDB"))
        .and_then(|remote| remote.id.as_deref())
        .and_then(crate::domain::ids::normalize_imdb_id);
    item.slug = make_slug(&item.title, item.year);

    if let Some(url) = non_empty(hit.image_url.as_deref()) {
        item.images.push(Image {
            id: new_id(),
            season_number: None,
            cover_type: CoverType::Poster,
            url,
            language: None,
            sort_order: 0,
            source: Some("tvdb".into()),
            is_manual: false,
        });
    }

    Some(item)
}

/// One episode's text in a single language.
pub struct TranslatedEpisode {
    pub season_number: i32,
    pub episode_number: i32,
    pub title: Option<String>,
    pub overview: Option<String>,
}

/// Artwork paths on episodes are relative to this.
const ARTWORK_BASE: &str = "https://artworks.thetvdb.com";

/// Re-authenticate this often even without being rejected. The token is good
/// for about a month; a day keeps a long-running server from ever reaching the
/// edge of that.
const TOKEN_MAX_AGE_HOURS: i64 = 24;

/// How many of each artwork kind to keep — TVDB lists hundreds.
const PER_KIND: usize = 5;

pub struct TvdbClient {
    http: reqwest::Client,
    base: String,
    api_key: Option<String>,
    pin: Option<String>,
    enabled: bool,
    /// The language the canonical entity is stored in, as TVDB spells it.
    language: String,
    token: Arc<Mutex<Option<Token>>>,
}

struct Token {
    value: String,
    obtained: chrono::DateTime<chrono::Utc>,
}

impl TvdbClient {
    pub fn new(http: reqwest::Client, cfg: &config::Tvdb, language: &str) -> Self {
        Self {
            http,
            base: cfg.upstream.clone(),
            api_key: cfg.api_key.clone(),
            pin: cfg.pin.clone(),
            enabled: cfg.enabled,
            language: crate::providers::lang::iso_639_1_to_3(
                crate::providers::lang::base_language(language),
            ),
            token: Arc::new(Mutex::new(None)),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled && self.is_configured()
    }

    /// A series and its episodes, by TVDB id.
    ///
    /// Two calls, made together. The series document carries the artwork, the
    /// seasons and every translated name; the episodes come from the language
    /// endpoint, because `name` on a TVDB record is whatever TVDB considers the
    /// primary — 進撃の巨人 and its Japanese episode titles — and a client
    /// asking in English should not be answered in Japanese.
    pub async fn series(&self, tvdb_id: i64) -> Result<Option<(Value, MediaItem)>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let document = format!("series/{tvdb_id}/extended?meta=translations&short=false");

        let (series, episodes) = tokio::join!(
            self.get(&document),
            self.episodes_in(tvdb_id, &self.language)
        );

        let Some(mut raw) = series? else {
            return Ok(None);
        };

        // An unknown language answers 200 with every name blank rather than an
        // error, so "did anything come back named" is the only real test of
        // whether TVDB holds this language at all.
        let mut episodes = match episodes {
            Ok(list) if list.iter().any(is_named) => list,
            Ok(_) => Vec::new(),
            Err(e) => {
                tracing::debug!(tvdb_id, error = %e, "TheTVDB episodes could not be fetched");
                Vec::new()
            }
        };

        if episodes.is_empty() {
            episodes = self.untranslated_episodes(tvdb_id).await;
        }

        // Splice everything into one document, so the snapshot holds what TVDB
        // said and a single deserialisation sees all of it.
        if let Some(data) = raw.get_mut("data").and_then(|d| d.as_object_mut()) {
            data.insert("episodes".into(), Value::Array(episodes));
        }

        let envelope: Envelope<SeriesExtended> = serde_json::from_value(raw.clone())
            .context("TheTVDB returned a series this server could not interpret")?;

        Ok(Some((raw, to_item(&envelope.data, &self.language))))
    }

    /// Series matching a term, as TheTVDB ranks them.
    ///
    /// Shallow: an id, a name, a year and a poster. Enough to recognise the
    /// work and to fetch it properly afterwards, which is what a search result
    /// is for. TheTVDB is the only provider here that knows a series TMDB has
    /// never heard of, and it was not being asked at all.
    pub async fn search(&self, term: &str, limit: usize) -> Result<Vec<MediaItem>> {
        if !self.is_enabled() {
            return Ok(Vec::new());
        }

        let path = format!(
            "search?query={}&type=series&limit={}",
            urlencoding::encode(term.trim()),
            limit.clamp(1, 50),
        );

        let Some(body) = self.get(&path).await? else {
            return Ok(Vec::new());
        };

        let envelope: Envelope<Vec<SearchHit>> = serde_json::from_value(body)
            .context("TheTVDB returned a search this server could not interpret")?;

        Ok(envelope.data.iter().filter_map(hit_to_item).collect())
    }

    /// Every episode's title and overview in one language.
    ///
    /// This answers a client asking in a language the entity is not stored in.
    /// TMDB is asked first because it is usually richer, but it has a handful of
    /// languages where TheTVDB has dozens, so this is what makes a French or
    /// Czech request work at all for most series.
    pub async fn episode_texts(
        &self,
        tvdb_id: i64,
        language: &str,
    ) -> Result<Vec<TranslatedEpisode>> {
        if !self.is_enabled() {
            return Ok(Vec::new());
        }

        let episodes = self.episodes_in(tvdb_id, language).await?;

        Ok(episodes
            .into_iter()
            .filter_map(|raw| {
                let record: EpisodeRecord = serde_json::from_value(raw).ok()?;

                Some(TranslatedEpisode {
                    season_number: record.season_number?,
                    episode_number: record.number?,
                    title: non_empty(record.name.as_deref()),
                    overview: non_empty(record.overview.as_deref()),
                })
            })
            .filter(|e| e.title.is_some() || e.overview.is_some())
            .collect())
    }

    /// Every episode in one language, following TVDB's paging.
    async fn episodes_in(&self, tvdb_id: i64, language: &str) -> Result<Vec<Value>> {
        let mut episodes = Vec::new();

        // TVDB pages at 500. The cap is a guard against a paging bug upstream
        // turning into an unbounded loop, not a real limit: it allows 10000
        // episodes, and the longest series ever made is far short of that.
        for page in 0..20 {
            let path = format!("series/{tvdb_id}/episodes/official/{language}?page={page}");

            let Some(body) = self.get(&path).await? else {
                break;
            };

            let batch = body
                .get("data")
                .and_then(|d| d.get("episodes"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            let exhausted = batch.is_empty()
                || body
                    .get("links")
                    .and_then(|l| l.get("next"))
                    .is_none_or(Value::is_null);

            episodes.extend(batch);

            if exhausted {
                break;
            }
        }

        Ok(episodes)
    }

    /// The episodes as TVDB stores them, for a language it does not hold.
    ///
    /// Titles in the wrong language beat no titles at all, and another provider
    /// may still fill them in.
    async fn untranslated_episodes(&self, tvdb_id: i64) -> Vec<Value> {
        let path = format!("series/{tvdb_id}/extended?meta=episodes&short=true");

        match self.get(&path).await {
            Ok(Some(body)) => body
                .get("data")
                .and_then(|d| d.get("episodes"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            Ok(None) => Vec::new(),
            Err(e) => {
                tracing::warn!(tvdb_id, error = %e, "TheTVDB episodes were not available");
                Vec::new()
            }
        }
    }

    /// A GET with the current token, retried once after re-authenticating.
    async fn get(&self, path: &str) -> Result<Option<Value>> {
        let token = self.token().await?;

        match self.try_get(path, &token).await? {
            Attempt::Body(value) => Ok(Some(value)),
            Attempt::Missing => Ok(None),
            Attempt::Unauthorized => {
                // The token outlived its welcome. One fresh attempt, then give up.
                tracing::debug!("TheTVDB rejected the token; re-authenticating");
                self.token.lock().await.take();

                let token = self.token().await?;

                match self.try_get(path, &token).await? {
                    Attempt::Body(value) => Ok(Some(value)),
                    Attempt::Missing => Ok(None),
                    Attempt::Unauthorized => {
                        anyhow::bail!("TheTVDB rejected a freshly issued token; check the API key")
                    }
                }
            }
        }
    }

    async fn try_get(&self, path: &str, token: &str) -> Result<Attempt> {
        let url = format!("{}/{path}", self.base);

        let response = self
            .http
            .get(&url)
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .with_context(|| format!("TheTVDB request failed: {url}"))?;

        match response.status() {
            reqwest::StatusCode::NOT_FOUND => Ok(Attempt::Missing),
            reqwest::StatusCode::UNAUTHORIZED => Ok(Attempt::Unauthorized),
            status if status.is_success() => {
                Ok(Attempt::Body(response.json().await.with_context(|| {
                    format!("TheTVDB returned a malformed body for {url}")
                })?))
            }
            status => anyhow::bail!("TheTVDB returned {status} for {url}"),
        }
    }

    /// The current token, obtaining one if there is none or it is old.
    async fn token(&self) -> Result<String> {
        let mut held = self.token.lock().await;

        let fresh = held.as_ref().is_some_and(|t| {
            chrono::Utc::now() - t.obtained < chrono::Duration::hours(TOKEN_MAX_AGE_HOURS)
        });

        if fresh {
            return Ok(held.as_ref().expect("checked").value.clone());
        }

        let key = self
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no TheTVDB API key configured"))?;

        let mut body = serde_json::json!({ "apikey": key });
        // A subscriber key needs a PIN; a project key must not send one.
        if let Some(pin) = &self.pin {
            body["pin"] = Value::String(pin.clone());
        }

        let response = self
            .http
            .post(format!("{}/login", self.base))
            .json(&body)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .context("TheTVDB login failed")?;

        if !response.status().is_success() {
            anyhow::bail!("TheTVDB login returned {}", response.status());
        }

        let envelope: Envelope<LoginData> = response
            .json()
            .await
            .context("TheTVDB login returned a body this server could not interpret")?;

        let value = envelope.data.token;
        *held = Some(Token {
            value: value.clone(),
            obtained: chrono::Utc::now(),
        });

        tracing::debug!("authenticated with TheTVDB");
        Ok(value)
    }
}

enum Attempt {
    Body(Value),
    Missing,
    Unauthorized,
}

// ─── response shapes ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct LoginData {
    token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Translations {
    #[serde(default)]
    name_translations: Vec<NameTranslation>,
    #[serde(default)]
    overview_translations: Vec<OverviewTranslation>,
}

#[derive(Debug, Deserialize)]
struct NameTranslation {
    name: Option<String>,
    language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OverviewTranslation {
    overview: Option<String>,
    language: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesExtended {
    id: i64,
    #[serde(default)]
    name: String,
    slug: Option<String>,
    overview: Option<String>,
    status: Option<Named>,
    first_aired: Option<String>,
    last_aired: Option<String>,
    year: Option<String>,
    average_runtime: Option<i32>,
    original_country: Option<String>,
    original_language: Option<String>,
    airs_time: Option<String>,
    original_network: Option<Named>,
    latest_network: Option<Named>,
    score: Option<f64>,
    #[serde(default)]
    genres: Vec<Named>,
    #[serde(default)]
    remote_ids: Vec<RemoteId>,
    #[serde(default)]
    content_ratings: Vec<ContentRating>,
    #[serde(default)]
    aliases: Vec<Alias>,
    #[serde(default)]
    seasons: Vec<SeasonRecord>,
    #[serde(default)]
    episodes: Vec<EpisodeRecord>,
    #[serde(default)]
    artworks: Vec<ArtworkRecord>,
    #[serde(default)]
    translations: Option<Translations>,
}

#[derive(Debug, Deserialize)]
struct Named {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteId {
    #[serde(default)]
    id: String,
    #[serde(default)]
    source_name: String,
}

#[derive(Debug, Deserialize)]
struct ContentRating {
    #[serde(default)]
    country: String,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct Alias {
    #[serde(default)]
    name: String,
    language: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeasonRecord {
    id: Option<i64>,
    number: Option<i32>,
    name: Option<String>,
    #[serde(rename = "type")]
    season_type: Option<SeasonType>,
}

#[derive(Debug, Deserialize)]
struct SeasonType {
    #[serde(default, rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeRecord {
    id: Option<i64>,
    season_number: Option<i32>,
    number: Option<i32>,
    absolute_number: Option<i32>,
    airs_before_season: Option<i32>,
    airs_before_episode: Option<i32>,
    name: Option<String>,
    overview: Option<String>,
    aired: Option<String>,
    runtime: Option<i32>,
    finale_type: Option<String>,
    image: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtworkRecord {
    #[serde(rename = "type")]
    kind: Option<i64>,
    image: Option<String>,
    language: Option<String>,
    score: Option<i64>,
    season: Option<i64>,
}

// ─── mapping ─────────────────────────────────────────────────────────────────

/// TVDB's numeric artwork types, from `/artwork/types`.
fn cover_type(kind: i64) -> Option<(CoverType, bool)> {
    // The flag says whether the artwork belongs to a season rather than the series.
    Some(match kind {
        1 => (CoverType::Banner, false),
        2 => (CoverType::Poster, false),
        3 => (CoverType::Fanart, false),
        22 => (CoverType::Clearart, false),
        23 => (CoverType::Clearlogo, false),
        6 => (CoverType::Banner, true),
        7 => (CoverType::Poster, true),
        8 => (CoverType::Fanart, true),
        _ => return None,
    })
}

/// Whether a raw episode record carries a name in the language asked for.
fn is_named(episode: &Value) -> bool {
    episode
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| !name.trim().is_empty())
}

/// Every language TheTVDB holds this series in.
///
/// It carries far more than TMDB — 48 languages for *Attack on Titan* against
/// TMDB's handful — and they are what answers a client asking in a language the
/// entity is not stored in. The two lists are keyed by language and joined
/// here, because TVDB may hold a name without an overview or the reverse.
fn all_translations(block: &Translations) -> Vec<Translation> {
    let mut by_language: BTreeMap<&str, Translation> = BTreeMap::new();

    for entry in &block.name_translations {
        let Some(language) = entry.language.as_deref() else {
            continue;
        };

        by_language
            .entry(language)
            .or_insert_with(|| blank_translation(language))
            .title = non_empty(entry.name.as_deref());
    }

    for entry in &block.overview_translations {
        let Some(language) = entry.language.as_deref() else {
            continue;
        };

        by_language
            .entry(language)
            .or_insert_with(|| blank_translation(language))
            .overview = non_empty(entry.overview.as_deref());
    }

    // One with neither field is noise.
    by_language
        .into_values()
        .filter(|t| t.title.is_some() || t.overview.is_some())
        .collect()
}

fn blank_translation(language: &str) -> Translation {
    Translation {
        language: language.to_string(),
        title: None,
        overview: None,
        is_manual: false,
    }
}

fn to_item(series: &SeriesExtended, language: &str) -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Series);

    let translated = series.translations.as_ref();

    item.title = translated
        .and_then(|t| {
            t.name_translations
                .iter()
                .find(|n| n.language.as_deref() == Some(language))
                .and_then(|n| non_empty(n.name.as_deref()))
        })
        .unwrap_or_else(|| series.name.clone());

    item.overview = translated
        .and_then(|t| {
            t.overview_translations
                .iter()
                .find(|o| o.language.as_deref() == Some(language))
                .and_then(|o| non_empty(o.overview.as_deref()))
        })
        .or_else(|| non_empty(series.overview.as_deref()));

    item.translations = translated.map(all_translations).unwrap_or_default();
    item.first_aired = non_empty(series.first_aired.as_deref());
    item.last_aired = non_empty(series.last_aired.as_deref());
    item.year = series.year.as_deref().and_then(|y| y.parse().ok());
    item.runtime = series.average_runtime.filter(|r| *r > 0);
    item.air_time = non_empty(series.airs_time.as_deref());
    item.status = Some(status(series.status.as_ref().map(|s| s.name.as_str())).to_string());
    item.genres = series.genres.iter().map(|g| g.name.clone()).collect();

    // TVDB already reports these in the forms the canonical model uses: ISO
    // 639-2/T for language and alpha-3 for country.
    item.original_language = non_empty(series.original_language.as_deref());
    item.original_country = non_empty(series.original_country.as_deref());

    item.network = series
        .original_network
        .as_ref()
        .or(series.latest_network.as_ref())
        .map(|n| n.name.clone())
        .filter(|n| !n.is_empty());

    item.slug = match non_empty(series.slug.as_deref()) {
        Some(slug) => slug,
        None => make_slug(&item.title, item.year),
    };

    item.external_ids = external_ids(series);
    item.content_rating_country = None;

    // Content ratings arrive per country, in alpha-3; the wire layer wants
    // alpha-2 uppercase, and the US entry is the one clients key on.
    if let Some(rating) = series
        .content_ratings
        .iter()
        .find(|r| r.country == "usa")
        .or_else(|| series.content_ratings.iter().find(|r| !r.name.is_empty()))
    {
        item.content_rating = non_empty(Some(&rating.name));
        item.content_rating_country = alpha3_to_alpha2(&rating.country);
    }

    if let Some(score) = series.score.filter(|s| *s > 0.0) {
        item.ratings = vec![Rating {
            source: "tvdb".to_string(),
            // TVDB's score is a popularity figure on its own scale, not a mark
            // out of ten, so it carries no vote count.
            value: Some(score),
            votes: None,
            rating_type: Some("user".to_string()),
        }];
    }

    item.alternative_titles = series
        .aliases
        .iter()
        .filter(|a| !a.name.trim().is_empty())
        .map(|a| crate::domain::AlternativeTitle {
            id: new_id(),
            title: a.name.clone(),
            title_type: None,
            language: a.language.clone(),
            is_manual: false,
        })
        .collect();

    item.images = artwork(series);
    item.seasons = seasons(series);
    item.episodes = episodes(series);
    item.updated_at = now();

    item
}

fn external_ids(series: &SeriesExtended) -> ExternalIds {
    let mut ids = ExternalIds {
        tvdb: Some(series.id),
        ..Default::default()
    };

    for remote in &series.remote_ids {
        match remote.source_name.as_str() {
            "IMDB" => {
                ids.imdb = crate::domain::ids::normalize_imdb_id(&remote.id);
            }
            "TheMovieDB.com" => ids.tmdb = remote.id.parse().ok(),
            "TV Maze" => ids.tvmaze = remote.id.parse().ok(),
            "TheTVDB.com" => {}
            _ => {}
        }
    }

    ids
}

fn artwork(series: &SeriesExtended) -> Vec<Image> {
    use std::collections::HashMap;

    let mut kept: HashMap<(CoverType, Option<i32>), usize> = HashMap::new();
    let mut ranked: Vec<&ArtworkRecord> = series.artworks.iter().collect();

    // TVDB's score is a community ranking; take the best few of each kind.
    ranked.sort_by_key(|a| -a.score.unwrap_or(0));

    let mut images = Vec::new();

    for record in ranked {
        let (Some(kind), Some(url)) = (record.kind, non_empty(record.image.as_deref())) else {
            continue;
        };
        let Some((cover_type, is_season)) = cover_type(kind) else {
            continue;
        };

        let season_number = if is_season {
            match record.season {
                Some(n) => Some(n as i32),
                // Season artwork with no season is unusable.
                None => continue,
            }
        } else {
            None
        };

        let slot = kept.entry((cover_type, season_number)).or_default();
        if *slot >= PER_KIND {
            continue;
        }

        images.push(Image {
            id: new_id(),
            season_number,
            cover_type,
            url: absolute(&url),
            language: record.language.clone().filter(|l| !l.is_empty()),
            sort_order: *slot as i32,
            source: Some(crate::providers::names::TVDB.to_string()),
            is_manual: false,
        });

        *slot += 1;
    }

    images
}

fn seasons(series: &SeriesExtended) -> Vec<Season> {
    let mut seasons: Vec<Season> = series
        .seasons
        .iter()
        // TVDB describes several orderings of the same series; only the aired
        // order matches what every client expects.
        .filter(|s| {
            s.season_type
                .as_ref()
                .is_none_or(|t| t.kind.is_empty() || t.kind == "official")
        })
        .filter_map(|s| {
            Some(Season {
                id: new_id(),
                season_number: s.number?,
                title: non_empty(s.name.as_deref()),
                overview: None,
                air_date: None,
                tmdb_id: None,
                tvdb_id: s.id,
                is_manual: false,
                images: Vec::new(),
            })
        })
        .collect();

    seasons.sort_by_key(|s| s.season_number);
    seasons.dedup_by_key(|s| s.season_number);
    seasons
}

fn episodes(series: &SeriesExtended) -> Vec<Episode> {
    series
        .episodes
        .iter()
        .filter_map(|record| {
            let season_number = record.season_number?;
            let episode_number = record.number?;
            let aired = non_empty(record.aired.as_deref());

            Some(Episode {
                id: new_id(),
                season_number,
                episode_number,
                // The field this provider is here for.
                absolute_episode_number: record.absolute_number.filter(|n| *n > 0),
                aired_after_season_number: None,
                aired_before_season_number: record.airs_before_season,
                aired_before_episode_number: record.airs_before_episode,
                title: record.name.clone().unwrap_or_default(),
                overview: non_empty(record.overview.as_deref()),
                air_date_utc: aired.as_deref().map(|d| format!("{d}T00:00:00Z")),
                air_date: aired,
                runtime: record.runtime.filter(|r| *r > 0),
                finale_type: non_empty(record.finale_type.as_deref()),
                image: non_empty(record.image.as_deref()).map(|i| absolute(&i)),
                tvdb_id: record.id,
                tmdb_id: None,
                rating: None,
                is_manual: false,
            })
        })
        .collect()
}

/// TVDB's own vocabulary mapped to Sonarr's.
fn status(status: Option<&str>) -> &'static str {
    match status.unwrap_or_default() {
        "Ended" | "Cancelled" | "Canceled" => "ended",
        "Upcoming" | "Planned" => "upcoming",
        _ => "continuing",
    }
}

/// Artwork paths on episodes are relative; those in `artworks` are not.
fn absolute(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{ARTWORK_BASE}{path}")
    }
}

/// TVDB reports rating countries in alpha-3; clients match on alpha-2.
fn alpha3_to_alpha2(code: &str) -> Option<String> {
    let pairs = [
        ("usa", "US"),
        ("gbr", "GB"),
        ("can", "CA"),
        ("aus", "AU"),
        ("nzl", "NZ"),
        ("irl", "IE"),
        ("fra", "FR"),
        ("deu", "DE"),
        ("esp", "ES"),
        ("ita", "IT"),
        ("nld", "NL"),
        ("bel", "BE"),
        ("swe", "SE"),
        ("nor", "NO"),
        ("dnk", "DK"),
        ("fin", "FI"),
        ("prt", "PT"),
        ("bra", "BR"),
        ("mex", "MX"),
        ("arg", "AR"),
        ("jpn", "JP"),
        ("kor", "KR"),
        ("chn", "CN"),
        ("ind", "IN"),
        ("rus", "RU"),
        ("pol", "PL"),
        ("cze", "CZ"),
        ("hun", "HU"),
        ("rou", "RO"),
        ("tur", "TR"),
        ("isr", "IL"),
        ("zaf", "ZA"),
    ];

    pairs
        .iter()
        .find(|(three, _)| *three == code.to_ascii_lowercase())
        .map(|(_, two)| (*two).to_string())
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

    fn fixture() -> SeriesExtended {
        serde_json::from_value(raw_fixture()).expect("fixture")
    }

    fn raw_fixture() -> serde_json::Value {
        serde_json::json!({
            "id": 81189,
            "name": "Breaking Bad",
            "slug": "breaking-bad",
            "overview": "A chemistry teacher.",
            "status": { "name": "Ended" },
            "firstAired": "2008-01-20",
            "lastAired": "2013-09-29",
            "year": "2008",
            "averageRuntime": 48,
            "originalCountry": "usa",
            "originalLanguage": "eng",
            "airsTime": "21:00",
            "originalNetwork": { "name": "AMC" },
            "score": 1234.0,
            "genres": [{ "name": "Drama" }, { "name": "Crime" }],
            "remoteIds": [
                { "id": "tt0903747", "sourceName": "IMDB" },
                { "id": "1396", "sourceName": "TheMovieDB.com" },
                { "id": "169", "sourceName": "TV Maze" },
                { "id": "EP01009396", "sourceName": "TMS (Zap2It)" }
            ],
            "contentRatings": [
                { "country": "gbr", "name": "18" },
                { "country": "usa", "name": "TV-MA" }
            ],
            "aliases": [{ "name": "Во все тяжкие", "language": "rus" }],
            "seasons": [
                { "id": 1, "number": 0, "type": { "type": "official" } },
                { "id": 2, "number": 1, "type": { "type": "official" } },
                { "id": 3, "number": 1, "type": { "type": "dvd" } }
            ],
            "episodes": [
                {
                    "id": 349232, "seasonNumber": 1, "number": 1, "absoluteNumber": 1,
                    "name": "Pilot", "overview": "It begins.", "aired": "2008-01-20",
                    "runtime": 58, "image": "/banners/episodes/81189/349232.jpg"
                },
                {
                    "id": 349233, "seasonNumber": 1, "number": 2, "absoluteNumber": 2,
                    "name": "Cat's in the Bag...", "aired": "2008-01-27", "runtime": 48
                },
                { "id": 9, "seasonNumber": null, "number": null, "name": "Broken" }
            ],
            "artworks": [
                { "type": 2, "image": "https://artworks.thetvdb.com/p1.jpg", "score": 100, "language": "eng" },
                { "type": 2, "image": "https://artworks.thetvdb.com/p2.jpg", "score": 900, "language": "eng" },
                { "type": 23, "image": "https://artworks.thetvdb.com/logo.png", "score": 10 },
                { "type": 7, "image": "https://artworks.thetvdb.com/s1.jpg", "score": 5, "season": 1 },
                { "type": 7, "image": "https://artworks.thetvdb.com/orphan.jpg", "score": 5 },
                { "type": 999, "image": "https://artworks.thetvdb.com/unknown.jpg", "score": 1 }
            ]
        })
    }

    /// The shape TVDB returns for `?meta=translations`.
    fn translated() -> SeriesExtended {
        let mut raw = raw_fixture();
        raw["translations"] = serde_json::json!({
            "nameTranslations": [
                { "name": "Útok titánů", "language": "ces" },
                { "name": "Attack on Titan", "language": "eng" }
            ],
            "overviewTranslations": [
                { "overview": "Humanity behind walls.", "language": "eng" }
            ]
        });
        serde_json::from_value(raw).unwrap()
    }

    #[test]
    fn the_requested_language_wins_over_the_primary_name() {
        // TVDB calls the series 進撃の巨人; a client asking in English must not
        // be answered in Japanese.
        let item = to_item(&translated(), "eng");

        assert_eq!(item.title, "Attack on Titan");
        assert_eq!(item.overview.as_deref(), Some("Humanity behind walls."));
    }

    #[test]
    fn an_untranslated_language_falls_back_to_the_primary_name() {
        let item = to_item(&translated(), "kor");

        assert_eq!(item.title, "Breaking Bad", "the name TVDB led with");
    }

    #[test]
    fn a_series_maps_to_the_canonical_model() {
        let item = to_item(&fixture(), "eng");

        assert_eq!(item.title, "Breaking Bad");
        assert_eq!(item.slug, "breaking-bad");
        assert_eq!(item.year, Some(2008));
        assert_eq!(item.runtime, Some(48));
        assert_eq!(item.status.as_deref(), Some("ended"));
        assert_eq!(item.network.as_deref(), Some("AMC"));
        assert_eq!(item.genres, vec!["Drama", "Crime"]);
        // TVDB reports these in the forms the canonical model already uses.
        assert_eq!(item.original_language.as_deref(), Some("eng"));
        assert_eq!(item.original_country.as_deref(), Some("usa"));
    }

    #[test]
    fn the_air_time_comes_across() {
        // Neither TMDB nor Radarr's service has this field at all.
        assert_eq!(
            to_item(&fixture(), "eng").air_time.as_deref(),
            Some("21:00")
        );
    }

    #[test]
    fn remote_ids_are_sorted_into_their_namespaces() {
        let ids = to_item(&fixture(), "eng").external_ids;

        assert_eq!(ids.tvdb, Some(81189));
        assert_eq!(ids.tmdb, Some(1396));
        assert_eq!(ids.imdb.as_deref(), Some("tt0903747"));
        assert_eq!(ids.tvmaze, Some(169));
    }

    #[test]
    fn absolute_numbering_is_carried() {
        // The reason this provider exists: Sonarr matches anime on it.
        let item = to_item(&fixture(), "eng");
        assert_eq!(item.episodes[0].absolute_episode_number, Some(1));
        assert_eq!(item.episodes[1].absolute_episode_number, Some(2));
    }

    #[test]
    fn an_episode_with_no_numbering_is_dropped() {
        // It cannot be addressed or merged, so it is noise.
        assert_eq!(to_item(&fixture(), "eng").episodes.len(), 2);
    }

    #[test]
    fn relative_artwork_paths_are_made_absolute() {
        let item = to_item(&fixture(), "eng");
        assert_eq!(
            item.episodes[0].image.as_deref(),
            Some("https://artworks.thetvdb.com/banners/episodes/81189/349232.jpg")
        );
        assert_eq!(
            absolute("https://already/absolute.jpg"),
            "https://already/absolute.jpg"
        );
    }

    #[test]
    fn the_us_rating_wins_and_its_country_is_converted() {
        // The fixture lists Great Britain first; clients match on `US`.
        let item = to_item(&fixture(), "eng");
        assert_eq!(item.content_rating.as_deref(), Some("TV-MA"));
        assert_eq!(item.content_rating_country.as_deref(), Some("US"));
    }

    #[test]
    fn artwork_is_ranked_by_score_and_unknown_kinds_are_skipped() {
        let item = to_item(&fixture(), "eng");

        let posters: Vec<&str> = item
            .images
            .iter()
            .filter(|i| i.cover_type == CoverType::Poster && i.season_number.is_none())
            .map(|i| i.url.as_str())
            .collect();
        assert_eq!(
            posters,
            vec![
                "https://artworks.thetvdb.com/p2.jpg",
                "https://artworks.thetvdb.com/p1.jpg"
            ]
        );

        assert!(
            item.images
                .iter()
                .any(|i| i.cover_type == CoverType::Clearlogo)
        );
        assert!(!item.images.iter().any(|i| i.url.contains("unknown")));
        // Season artwork with no season number cannot be placed.
        assert!(!item.images.iter().any(|i| i.url.contains("orphan")));

        let season = item
            .images
            .iter()
            .find(|i| i.season_number == Some(1))
            .unwrap();
        assert_eq!(season.cover_type, CoverType::Poster);
    }

    #[test]
    fn only_the_aired_season_ordering_is_taken() {
        // TVDB describes DVD and absolute orderings of the same series; taking
        // them all would list season 1 three times.
        let item = to_item(&fixture(), "eng");
        let numbers: Vec<i32> = item.seasons.iter().map(|s| s.season_number).collect();
        assert_eq!(numbers, vec![0, 1]);
    }

    #[test]
    fn statuses_map_to_sonarrs_vocabulary() {
        assert_eq!(status(Some("Ended")), "ended");
        assert_eq!(status(Some("Continuing")), "continuing");
        assert_eq!(status(Some("Upcoming")), "upcoming");
        assert_eq!(status(None), "continuing");
    }

    #[test]
    fn unknown_countries_are_left_out_rather_than_guessed() {
        assert_eq!(alpha3_to_alpha2("usa").as_deref(), Some("US"));
        assert_eq!(alpha3_to_alpha2("USA").as_deref(), Some("US"));
        assert_eq!(alpha3_to_alpha2("xyz"), None);
    }
}

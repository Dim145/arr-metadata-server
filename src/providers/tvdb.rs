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
        CountryRelease, CoverType, Credit, CreditType, Episode, EpisodeOrder, ExternalIds, Image,
        MediaItem, MediaKind, OrderKind, OrderRecord, PlacedEpisode, Season, Translation,
        make_slug, person::PersonRef,
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

/// How often, at most, a refused token is forgotten for a fresh one: TheTVDB
/// rate-limits sign-ins, and a refusal is not always the token's end.
const RENEW_AT_MOST_EVERY_SECS: u64 = 60;

/// How long a failed sign-in is remembered: every call meanwhile fails with
/// it rather than signing in again. A wrong key otherwise cost a sign-in per
/// call — a refresh's, and each relayed one's.
const SIGN_IN_RETRY_SECS: u64 = 60;

/// How many requests to TheTVDB are in flight at once.
const AT_ONCE: usize = 8;

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
    /// When a refused token was last forgotten for the relay's sake.
    forgotten: parking_lot::Mutex<Option<std::time::Instant>>,
    /// The last sign-in that failed, and why, while it is remembered.
    failed_sign_in: parking_lot::Mutex<Option<(std::time::Instant, String)>>,
    gate: crate::providers::Gate,
    /// This process, named on every call: `api4.thetvdb.com` is a name this
    /// server answers on too, and a resolver that sends it here would have
    /// it ask itself. See [`crate::providers::radarr::LOOP_HEADER`].
    instance: String,
}

struct Token {
    value: String,
    obtained: chrono::DateTime<chrono::Utc>,
}

impl TvdbClient {
    pub fn new(
        http: reqwest::Client,
        cfg: &config::Tvdb,
        language: &str,
        instance: String,
    ) -> Self {
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
            forgotten: parking_lot::Mutex::new(None),
            failed_sign_in: parking_lot::Mutex::new(None),
            gate: crate::providers::Gate::new("tvdb", "TheTVDB", AT_ONCE),
            instance,
        }
    }

    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    /// The operator's token, signed in for when there is none or it is old:
    /// what the relay asks TheTVDB with in a client's place.
    pub async fn bearer(&self) -> Result<String> {
        self.token().await
    }

    /// TheTVDB refused `token`. Whether asking again is worth it: the token
    /// held is already another — somebody signed in meanwhile — or it is the
    /// one refused, which is then forgotten so that the next call signs in
    /// anew. Once a minute at most: a second refusal within the minute was
    /// for another reason than the token's end — a path the operator's key
    /// may not read — and a caller repeating that request would otherwise
    /// sign the operator in on every call.
    pub async fn refused(&self, token: &str) -> bool {
        let mut held = self.token.lock().await;
        match held.as_ref() {
            None => true,
            Some(current) if current.value != token => true,
            Some(_) => {
                let mut forgotten = self.forgotten.lock();
                let lately = forgotten.is_some_and(|at| {
                    at.elapsed() < std::time::Duration::from_secs(RENEW_AT_MOST_EVERY_SECS)
                });
                if lately {
                    return false;
                }
                *forgotten = Some(std::time::Instant::now());
                held.take();
                true
            }
        }
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
    ///
    /// An error when the episodes could not be read at all, whatever came of
    /// the rest: a series answered without them was stored as having none —
    /// its list numbered by TMDB under a TheTVDB id, the way Sonarr files
    /// episodes under the wrong numbers. Failed, it is TheTVDB not answering,
    /// and what it gave before stands (`service::gather`).
    ///
    /// Its characters too, the first `cast` of them: the people TheTVDB
    /// files under the series, by its people ids.
    pub async fn series(&self, tvdb_id: i64, cast: usize) -> Result<Option<(Value, MediaItem)>> {
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
        let (named, unnamed) = match episodes {
            Ok(list) if list.iter().any(is_named) => (Some(list), None),
            Ok(list) => (None, Some(list)),
            Err(e) => {
                tracing::debug!(
                    tvdb_id,
                    error = format_args!("{e:#}"),
                    "TheTVDB episodes could not be fetched"
                );
                (None, None)
            }
        };

        let episodes = match named {
            Some(list) => list,
            None => match self.untranslated_episodes(tvdb_id).await {
                Ok(list) => list,
                // Numbered without names beats not at all: another provider
                // may name them.
                Err(_) if unnamed.as_ref().is_some_and(|list| !list.is_empty()) => {
                    unnamed.unwrap_or_default()
                }
                Err(e) => return Err(e.context("TheTVDB's episodes could not be read")),
            },
        };

        // Splice everything into one document, so the snapshot holds what TVDB
        // said and a single deserialisation sees all of it.
        if let Some(data) = raw.get_mut("data").and_then(|d| d.as_object_mut()) {
            data.insert("episodes".into(), Value::Array(episodes));
        }

        let envelope: Envelope<SeriesExtended> = serde_json::from_value(raw.clone())
            .context("TheTVDB returned a series this server could not interpret")?;

        let mut item = to_item(&envelope.data, &self.language);
        let (credits, known) = characters(&envelope.data, cast);
        item.credits = credits;
        item.people.by_credit = known;
        Ok(Some((raw, item)))
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

    /// Every episode in one language, as they aired, following TVDB's paging.
    async fn episodes_in(&self, tvdb_id: i64, language: &str) -> Result<Vec<Value>> {
        self.episodes_of(tvdb_id, "official", language).await
    }

    /// The other orders TVDB keeps this series' episodes in — the DVDs',
    /// straight through, an alternate or a regional one — each a sequence of
    /// episode records, by id, placed in seasons and numbers of its own. The
    /// aired order is not among them: it is the work's own.
    ///
    /// Each is read from the episodes endpoint of its type. One that answers
    /// empty for a type the series has seasons of is read season by season
    /// instead, from each season's own record (`/seasons/{id}/extended`, one
    /// call per season of the type), which is how TheTVDB's own site lists
    /// it: the endpoint answers an empty list for `altdvd` and `alttwo`
    /// (thetvdb/v4-api#340), and the real Skyhook serves both whole.
    ///
    /// Each place keeps the record TheTVDB gave of its episode — its name,
    /// date, runtime, still — since an order's episode may be one of its own:
    /// a DVD's hour-long cut of two aired parts, an extended cut. Each order
    /// is named by the series' own name for it where TheTVDB keeps one
    /// (`alternateName`: "Superfan Order", "BluRay Order"), and by TheTVDB's
    /// for its type otherwise ("DVD Order").
    pub async fn orders(&self, tvdb_id: i64) -> Result<Vec<EpisodeOrder>> {
        if !self.is_enabled() {
            return Ok(Vec::new());
        }

        let Some(body) = self
            .get(&format!("series/{tvdb_id}/extended?short=true"))
            .await?
        else {
            return Ok(Vec::new());
        };

        // The seasons of each other order, by type: their ids are the way
        // round an endpoint that answers empty.
        let mut seasons_of: BTreeMap<String, Vec<SeasonRecord>> = BTreeMap::new();
        for season in body
            .pointer("/data/seasons")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Ok(record) = serde_json::from_value::<SeasonRecord>(season.clone()) else {
                continue;
            };
            let Some(kind) = record.season_type.as_ref().map(|t| t.kind.as_str()) else {
                continue;
            };
            if OTHER_ORDERS.contains(&kind) {
                seasons_of.entry(kind.to_string()).or_default().push(record);
            }
        }

        // No falling back to the untranslated episodes here, as the aired
        // fetch does: a language TVDB does not hold blanks the names, and a
        // place is told by its id and its numbers first.
        let mut orders = Vec::with_capacity(seasons_of.len());
        for (kind, mut seasons) in seasons_of {
            let label = order_label(&kind, &seasons);
            let mut episodes: Vec<PlacedEpisode> = self
                .episodes_of(tvdb_id, &kind, &self.language)
                .await?
                .into_iter()
                .filter_map(|raw| placed(&raw))
                .collect();
            if episodes.is_empty() {
                seasons.sort_by_key(|season| season.number);
                episodes = self.placed_by_season(&seasons).await?;
            }
            if !episodes.is_empty() {
                orders.push(EpisodeOrder {
                    kind: OrderKind::of_tvdb(&kind),
                    key: kind,
                    source: crate::providers::names::TVDB.to_string(),
                    source_ref: None,
                    label,
                    is_default: false,
                    is_frozen: false,
                    announced: None,
                    updated_at: now(),
                    episodes,
                });
            }
        }

        Ok(orders)
    }

    /// An order's episodes read season by season, each from its season's
    /// own record: where one stands is the season's number and the episode's
    /// within it, as the record gives it. A season answered 404 places
    /// nothing.
    async fn placed_by_season(&self, seasons: &[SeasonRecord]) -> Result<Vec<PlacedEpisode>> {
        let mut placed_all = Vec::new();
        for season in seasons {
            let (Some(id), Some(number)) = (season.id, season.number) else {
                continue;
            };
            let Some(body) = self.get(&format!("seasons/{id}/extended")).await? else {
                continue;
            };
            let mut of_season: Vec<PlacedEpisode> = body
                .pointer("/data/episodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(placed)
                .map(|mut place| {
                    place.season_number = number;
                    place
                })
                .collect();
            of_season.sort_by_key(|place| place.episode_number);
            placed_all.extend(of_season);
        }
        Ok(placed_all)
    }

    /// Every episode in one language and one order, following TVDB's paging.
    async fn episodes_of(&self, tvdb_id: i64, order: &str, language: &str) -> Result<Vec<Value>> {
        let mut episodes = Vec::new();
        // Each one segment of the path, whatever it holds: a language comes
        // from a caller, and `../` in it would be another endpoint read with
        // the operator's token.
        let (order, language) = (
            crate::providers::segment(order),
            crate::providers::segment(language),
        );

        // TVDB pages at 500. The cap is a guard against a paging bug upstream
        // turning into an unbounded loop, not a real limit: it allows 10000
        // episodes, and the longest series ever made is far short of that.
        for page in 0..20 {
            let path = format!("series/{tvdb_id}/episodes/{order}/{language}?page={page}");

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
    async fn untranslated_episodes(&self, tvdb_id: i64) -> Result<Vec<Value>> {
        let path = format!("series/{tvdb_id}/extended?meta=episodes&short=true");

        match self.get(&path).await {
            Ok(Some(body)) => Ok(body
                .get("data")
                .and_then(|d| d.get("episodes"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()),
            Ok(None) => Ok(Vec::new()),
            Err(e) => {
                tracing::warn!(
                    tvdb_id,
                    error = format_args!("{e:#}"),
                    "TheTVDB episodes were not available"
                );
                Err(e)
            }
        }
    }

    /// [`fetch`](Self::fetch), noting for a `providers::noting` caller when
    /// TheTVDB did not answer. Here besides at its door: a sign-in that
    /// failed within the minute is refused without a request, and a body
    /// that could not be read is no word about the series either.
    async fn get(&self, path: &str) -> Result<Option<Value>> {
        let got = self.fetch(path).await;
        if got.is_err() {
            crate::providers::went_unanswered(crate::providers::names::TVDB);
        }
        got
    }

    /// A GET with the current token, retried once after re-authenticating —
    /// at most once a minute: a path the operator's key may not read is
    /// refused whatever the token, and asking for a new one on every such
    /// call would sign the operator in on every call.
    async fn fetch(&self, path: &str) -> Result<Option<Value>> {
        let token = self.token().await?;

        match self.try_get(path, &token).await? {
            Attempt::Body(value) => Ok(Some(value)),
            Attempt::Missing => Ok(None),
            Attempt::Unauthorized => {
                if !self.refused(&token).await {
                    anyhow::bail!(
                        "TheTVDB refused a token issued within the minute; check the API key"
                    );
                }
                // The token outlived its welcome. One fresh attempt, then give up.
                tracing::debug!("TheTVDB rejected the token; re-authenticating");

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

        let (response, _permit) = self
            .gate
            .send(|| {
                self.http
                    .get(&url)
                    .bearer_auth(token)
                    .header(crate::providers::radarr::LOOP_HEADER, &self.instance)
                    .timeout(std::time::Duration::from_secs(20))
            })
            .await
            .map_err(|e| anyhow::anyhow!("TheTVDB request failed: {url}: {e}"))?;

        match response.status() {
            reqwest::StatusCode::NOT_FOUND => Ok(Attempt::Missing),
            reqwest::StatusCode::UNAUTHORIZED => Ok(Attempt::Unauthorized),
            status if status.is_success() => Ok(Attempt::Body(
                crate::providers::read_json(response)
                    .await
                    .with_context(|| {
                        format!("TheTVDB returned a body this server could not read: {url}")
                    })?,
            )),
            status => {
                let reason = crate::providers::error_text(response).await;
                anyhow::bail!("TheTVDB returned {status} for {url}: {reason}")
            }
        }
    }

    /// The current token, obtaining one if there is none or it is old.
    ///
    /// One sign-in at a time — the token's lock is held across it — and one
    /// that failed is remembered for a minute: the calls meanwhile fail with
    /// its reason instead of signing in again.
    async fn token(&self) -> Result<String> {
        let mut held = self.token.lock().await;

        let fresh = held.as_ref().is_some_and(|t| {
            chrono::Utc::now() - t.obtained < chrono::Duration::hours(TOKEN_MAX_AGE_HOURS)
        });

        if fresh {
            return Ok(held.as_ref().expect("checked").value.clone());
        }

        if let Some((at, why)) = self.failed_sign_in.lock().as_ref()
            && at.elapsed() < std::time::Duration::from_secs(SIGN_IN_RETRY_SECS)
        {
            anyhow::bail!("{why} (not asked again for a minute)");
        }

        match self.sign_in().await {
            Ok(value) => {
                self.failed_sign_in.lock().take();
                *held = Some(Token {
                    value: value.clone(),
                    obtained: chrono::Utc::now(),
                });
                tracing::debug!("authenticated with TheTVDB");
                Ok(value)
            }
            Err(e) => {
                *self.failed_sign_in.lock() = Some((std::time::Instant::now(), format!("{e:#}")));
                Err(e)
            }
        }
    }

    /// Ask TheTVDB for a token.
    async fn sign_in(&self) -> Result<String> {
        let key = self
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no TheTVDB API key configured"))?;

        let mut body = serde_json::json!({ "apikey": key });
        // A subscriber key needs a PIN; a project key must not send one.
        if let Some(pin) = &self.pin {
            body["pin"] = Value::String(pin.clone());
        }

        let url = format!("{}/login", self.base);
        let (response, _permit) = self
            .gate
            .send(|| {
                self.http
                    .post(&url)
                    .json(&body)
                    .header(crate::providers::radarr::LOOP_HEADER, &self.instance)
                    .timeout(std::time::Duration::from_secs(20))
            })
            .await
            .map_err(|e| anyhow::anyhow!("TheTVDB login failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let reason = crate::providers::error_text(response).await;
            anyhow::bail!("TheTVDB login returned {status}: {reason}");
        }

        let envelope: Envelope<LoginData> = serde_json::from_value(
            crate::providers::read_json(response)
                .await
                .context("TheTVDB login returned a body this server could not read")?,
        )
        .context("TheTVDB login returned a body this server could not interpret")?;

        Ok(envelope.data.token)
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
    /// The people of the series and the parts they play.
    #[serde(default)]
    characters: Vec<CharacterRecord>,
}

/// One of the people of a series, and their part.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CharacterRecord {
    /// The part: `Skyler White`.
    name: Option<String>,
    people_id: Option<i64>,
    person_name: Option<String>,
    /// TheTVDB's kind of credit: 3 a part in the cast, 4 a guest star's.
    #[serde(rename = "type")]
    kind: Option<i64>,
    /// Set on a part in one episode alone.
    episode_id: Option<i64>,
    /// The person's portrait.
    #[serde(rename = "personImgURL")]
    person_image: Option<String>,
    /// The part's picture: the actor in character, or the character drawn.
    image: Option<String>,
    /// Where TheTVDB puts it among the others; 0 where it puts it nowhere.
    sort: Option<i64>,
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
#[serde(rename_all = "camelCase")]
struct SeasonType {
    #[serde(default, rename = "type")]
    kind: String,
    /// TheTVDB's name for the type: "DVD Order", "Alternate Order 2".
    #[serde(default)]
    name: Option<String>,
    /// The series' own name for its order of this type: "Superfan Order",
    /// "BluRay Order". The slots `alternate` to `alttwo` mean what this says.
    #[serde(default)]
    alternate_name: Option<String>,
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
    /// The season record a season's artwork belongs to — of any order, the
    /// DVD's and the absolute one included: what v4 sends.
    season_id: Option<i64>,
    /// A season's number, which v4 does not send; read when it does.
    season: Option<i64>,
}

// ─── mapping ─────────────────────────────────────────────────────────────────

/// The season types TVDB numbers a series by besides the aired order, as
/// they are named in a season's `type` and in the episodes path. The last
/// two are the further alternate orders — a Blu-ray's, a streaming
/// service's — that the real Skyhook offers Sonarr v5 and TheTVDB's own
/// episodes endpoint answers empty for (see [`TvdbClient::orders`]).
const OTHER_ORDERS: &[&str] = &[
    "dvd",
    "absolute",
    "alternate",
    "regional",
    "altdvd",
    "alttwo",
];

/// Where one episode record stands in the order it was read from, with the
/// record itself: what is served for the place when the work holds no
/// episode of that id.
fn placed(raw: &Value) -> Option<PlacedEpisode> {
    let record: EpisodeRecord = serde_json::from_value(raw.clone()).ok()?;
    let mut place = PlacedEpisode::of_tvdb(record.id?, record.season_number?, record.number?);
    place.absolute_number = record.absolute_number.filter(|n| *n > 0);
    place.record = Some(OrderRecord {
        title: record
            .name
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .to_string(),
        overview: non_empty(record.overview.as_deref()),
        aired: non_empty(record.aired.as_deref()),
        runtime: record.runtime.filter(|r| *r > 0),
        image: non_empty(record.image.as_deref()).map(|i| absolute(&i)),
        finale_type: non_empty(record.finale_type.as_deref()),
    });
    Some(place)
}

/// What an order is called: the series' own name for it where TheTVDB keeps
/// one, else TheTVDB's name for its type, else one made of the type.
fn order_label(kind: &str, seasons: &[SeasonRecord]) -> String {
    let named = |pick: fn(&SeasonType) -> Option<&str>| {
        seasons
            .iter()
            .filter_map(|season| season.season_type.as_ref())
            .find_map(|t| pick(t).map(str::trim).filter(|n| !n.is_empty()))
            .map(str::to_string)
    };
    named(|t| t.alternate_name.as_deref())
        .or_else(|| named(|t| t.name.as_deref()))
        .unwrap_or_else(|| match kind {
            "dvd" => "DVD Order".to_string(),
            "absolute" => "Absolute Order".to_string(),
            "alternate" => "Alternate Order".to_string(),
            "regional" => "Regional Order".to_string(),
            "altdvd" => "Alternate DVD Order".to_string(),
            "alttwo" => "Alternate Order 2".to_string(),
            other => {
                let mut letters = other.chars();
                let first = letters.next().map(|c| c.to_uppercase().collect::<String>());
                format!("{}{} Order", first.unwrap_or_default(), letters.as_str())
            }
        })
}

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
        tagline: None,
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
    // alpha-2 uppercase, and the US entry is the one clients key on. Every
    // country's is kept besides, for a reader or a Radarr of another.
    if let Some(rating) = series
        .content_ratings
        .iter()
        .find(|r| r.country == "usa")
        .or_else(|| series.content_ratings.iter().find(|r| !r.name.is_empty()))
    {
        item.content_rating = non_empty(Some(&rating.name));
        item.content_rating_country = alpha3_to_alpha2(&rating.country);
    }
    item.releases = ratings_by_country(&series.content_ratings);

    // TheTVDB's `score` is not taken as a rating. It is a popularity figure —
    // 3 776 757 for Breaking Bad — on no scale a rating is read on, and filed as
    // one it did worse than mean nothing: it held the `tvdb` slot, so Skyhook's
    // rating, which Skyhook also called that, was dropped as a duplicate.

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

/// The kind of credit TheTVDB files a part in a series' cast under.
const ACTOR: i64 = 3;

/// How many of TheTVDB's characters a series keeps when the `tvdb.castLimit`
/// setting says nothing: as many as of TMDB's cast.
pub const DEFAULT_CAST: usize = 25;

/// How many of TheTVDB's characters a series keeps, as the `tvdb.castLimit`
/// setting says: from one to as many as `tmdb.castLimit` allows.
pub fn cast_limit(settings: &crate::settings::Store) -> usize {
    settings
        .int_at("tvdb.castLimit", None, None)
        .and_then(|n| usize::try_from(n).ok())
        .map_or(DEFAULT_CAST, |n| {
            n.clamp(1, crate::providers::tmdb::map::MAX_CAST)
        })
}

/// The series' cast, as TheTVDB lists its characters: the first `limit` in
/// TheTVDB's order — those it puts nowhere after — each person once, the
/// parts of one who plays several put together, with TheTVDB's id of each
/// person. Not a guest of one episode, whom the series' cast leaves out.
fn characters(series: &SeriesExtended, limit: usize) -> (Vec<Credit>, BTreeMap<String, PersonRef>) {
    let mut parts: Vec<(usize, &CharacterRecord)> = series
        .characters
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == Some(ACTOR) && c.episode_id.is_none())
        .filter(|(_, c)| {
            c.person_name
                .as_deref()
                .is_some_and(|n| !n.trim().is_empty())
        })
        .collect();
    parts.sort_by_key(|(at, c)| {
        let sort = c.sort.unwrap_or(0);
        (sort <= 0, sort, *at)
    });

    let mut credits: Vec<Credit> = Vec::new();
    let mut known: BTreeMap<String, PersonRef> = BTreeMap::new();
    let mut people: Vec<(Option<i64>, String)> = Vec::new();
    for (_, part) in parts {
        let name = part.person_name.as_deref().unwrap_or_default().trim();
        let character = non_empty(part.name.as_deref());
        let held = people
            .iter()
            .position(|(id, n)| match (id, part.people_id) {
                (Some(a), Some(b)) => *a == b,
                _ => n == name,
            });
        if let Some(at) = held {
            // Another part of somebody already listed: on their one credit.
            if let Some(character) = character {
                let credit = &mut credits[at];
                credit.character_name = Some(match credit.character_name.take() {
                    Some(parts) => format!("{parts} / {character}"),
                    None => character,
                });
            }
            continue;
        }
        if credits.len() == limit.clamp(1, crate::providers::tmdb::map::MAX_CAST) {
            continue;
        }
        let credit = Credit {
            id: new_id(),
            credit_type: CreditType::Actor,
            person_name: name.to_string(),
            character_name: character,
            image: part
                .person_image
                .as_deref()
                .or(part.image.as_deref())
                .map(str::trim)
                .filter(|i| !i.is_empty())
                .map(absolute),
            tmdb_person_id: None,
            credit_tmdb_id: None,
            sort_order: i32::try_from(credits.len()).unwrap_or(i32::MAX),
            is_manual: false,
        };
        if let Some(id) = part.people_id.filter(|id| *id > 0) {
            known.insert(
                credit.id.clone(),
                PersonRef {
                    tvdb_id: Some(id),
                    ..PersonRef::default()
                },
            );
        }
        people.push((part.people_id, name.to_string()));
        credits.push(credit);
    }
    (credits, known)
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

    // A season's artwork names its season record by id. Only the aired
    // order's are the work's seasons: a DVD season 1 is not the season 1
    // every client means, and its pictures are not that season's.
    let aired: HashMap<i64, i32> = series
        .seasons
        .iter()
        .filter(|s| is_aired_order(s))
        .filter_map(|s| Some((s.id?, s.number?)))
        .collect();

    let mut kept: HashMap<(CoverType, Option<i32>), usize> = HashMap::new();
    let mut ranked: Vec<&ArtworkRecord> = series.artworks.iter().collect();

    // TVDB's score is a community ranking; take the best few of each kind.
    // `Reverse` rather than a negation, which overflows on `i64::MIN`.
    ranked.sort_by_key(|a| std::cmp::Reverse(a.score.unwrap_or(0)));

    let mut images = Vec::new();

    for record in ranked {
        let (Some(kind), Some(url)) = (record.kind, non_empty(record.image.as_deref())) else {
            continue;
        };
        let Some((cover_type, is_season)) = cover_type(kind) else {
            continue;
        };

        let season_number = if is_season {
            let number = match record.season_id {
                Some(id) => aired.get(&id).copied(),
                // `try_from`, not `as`: a value past `i32` wraps to a negative
                // season number and files the artwork under a season that does
                // not exist, rather than being recognised as nonsense and
                // dropped.
                None => record.season.and_then(|n| i32::try_from(n).ok()),
            };
            match number {
                Some(n) => Some(n),
                // Season artwork with no usable season — or another order's
                // — is unusable.
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

/// Whether a season record is one of the aired order's: the seasons every
/// client means.
fn is_aired_order(season: &SeasonRecord) -> bool {
    season
        .season_type
        .as_ref()
        .is_none_or(|t| t.kind.is_empty() || t.kind == "official")
}

fn seasons(series: &SeriesExtended) -> Vec<Season> {
    let mut seasons: Vec<Season> = series
        .seasons
        .iter()
        // TVDB describes several orderings of the same series; only the aired
        // order matches what every client expects.
        .filter(|s| is_aired_order(s))
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
                primary_images: Default::default(),
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
                // A date and no time: see the note in `tmdb::map`.
                air_date_utc: None,
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
    crate::providers::lang::iso_3166_3_to_2(code).map(str::to_string)
}

/// Every country's rating, once a country, in the order TheTVDB lists them:
/// one of a country this server cannot name is left out rather than filed
/// under a guess.
fn ratings_by_country(ratings: &[ContentRating]) -> Vec<CountryRelease> {
    let mut out: Vec<CountryRelease> = Vec::new();
    for rating in ratings {
        let (Some(country), Some(certification)) = (
            alpha3_to_alpha2(&rating.country),
            non_empty(Some(&rating.name)),
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
            source: Some(crate::providers::names::TVDB.to_string()),
        });
    }
    out
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

    #[test]
    fn an_episode_is_placed_by_what_the_order_says_of_it() {
        let raw = serde_json::json!({ "id": 297989, "seasonNumber": 1, "number": 2, "absoluteNumber": 2, "name": "The Train Job" });
        let placed = placed(&raw).expect("placed");
        assert_eq!(
            (placed.tvdb_id, placed.season_number, placed.episode_number),
            (Some(297989), 1, 2)
        );
        assert_eq!(placed.absolute_number, Some(2));
        // With the record, served where the work holds no such episode.
        assert_eq!(
            placed.record.as_ref().map(|r| r.title.as_str()),
            Some("The Train Job")
        );
        // Without an id, or a place, there is nothing to keep.
        assert!(placed_is_none(
            &serde_json::json!({ "seasonNumber": 1, "number": 2 })
        ));
        assert!(placed_is_none(
            &serde_json::json!({ "id": 1, "seasonNumber": 1 })
        ));
    }

    fn placed_is_none(raw: &Value) -> bool {
        placed(raw).is_none()
    }

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

    /// A season's artwork as TheTVDB v4 sends it — by `seasonId`, never by
    /// `season` — taken from a stored answer for *Rurouni Kenshin* (70863):
    /// filed under the aired order's season it names, and dropped when it
    /// names a DVD or absolute season.
    #[test]
    fn season_artwork_is_filed_under_the_aired_season_its_id_names() {
        let season_type = |id: i64, name: &str, kind: &str| serde_json::json!({ "id": id, "name": name, "type": kind, "alternateName": null });
        let aired = season_type(1, "Aired Order", "official");
        let raw = serde_json::json!({
            "id": 70863,
            "name": "るろうに剣心 明治剣客浪漫譚",
            "seasons": [
                { "id": 1210, "seriesId": 70863, "type": aired, "number": 1, "name": "東京編",
                  "image": "https://artworks.thetvdb.com/banners/v4/season/1210/posters/6a156459154c1.jpg",
                  "imageType": 7, "lastUpdated": "2026-08-25 10:47:42", "year": null },
                { "id": 1211, "seriesId": 70863, "type": aired, "number": 2, "name": "京都編",
                  "image": null, "imageType": null, "lastUpdated": "2026-09-29 00:48:32", "year": null },
                { "id": 18331, "seriesId": 70863, "type": aired, "number": 0, "name": null,
                  "image": null, "imageType": 7, "lastUpdated": "2026-05-26 09:13:43", "year": null },
                { "id": 1698153, "seriesId": 70863, "type": season_type(3, "Absolute Order", "absolute"),
                  "number": 1, "name": null, "image": null, "imageType": 7, "lastUpdated": null, "year": null },
                { "id": 1698154, "seriesId": 70863, "type": season_type(2, "DVD Order", "dvd"),
                  "number": 0, "name": null, "image": null, "imageType": 6, "lastUpdated": null, "year": null }
            ],
            "artworks": [
                { "id": 60942446, "image": "https://artworks.thetvdb.com/banners/seasons/70863-0-3.jpg",
                  "thumbnail": "https://artworks.thetvdb.com/banners/seasons/70863-0-3_t.jpg",
                  "language": "jpn", "type": 7, "score": 100000, "width": 400, "height": 578,
                  "includesText": true, "thumbnailWidth": 0, "thumbnailHeight": 0, "updatedAt": 0,
                  "seasonId": 18331, "status": { "id": 0, "name": null }, "tagOptions": null },
                { "id": 60942468, "image": "https://artworks.thetvdb.com/banners/seasons/dvd-0.jpg",
                  "language": "jpn", "type": 7, "score": 100000, "seasonId": 1698154 },
                { "id": 64740947, "image": "https://artworks.thetvdb.com/banners/v4/season/1210/posters/6a156459154c1.jpg",
                  "language": "eng", "type": 7, "score": 0, "seasonId": 1210 },
                { "id": 62813919, "image": "https://artworks.thetvdb.com/banners/v4/season/1698153/posters/613ec075c2405.jpg",
                  "language": "eng", "type": 7, "score": 0, "seasonId": 1698153 },
                { "id": 60942447, "image": "https://artworks.thetvdb.com/banners/seasonswide/70863-0.jpg",
                  "language": "eng", "type": 6, "score": 0, "seasonId": 18331 },
                { "id": 60942469, "image": "https://artworks.thetvdb.com/banners/seasonswide/dvd-0.jpg",
                  "language": "eng", "type": 6, "score": 0, "seasonId": 1698154 },
                { "id": 1, "image": "https://artworks.thetvdb.com/banners/seasons/nowhere.jpg",
                  "language": "eng", "type": 7, "score": 0, "seasonId": 999 }
            ]
        });
        let series: SeriesExtended = serde_json::from_value(raw).expect("a real-shaped answer");

        let mut filed: Vec<(Option<i32>, CoverType, &str)> = Vec::new();
        let images = artwork(&series);
        for image in &images {
            let name = image.url.rsplit('/').next().unwrap_or_default();
            filed.push((image.season_number, image.cover_type, name));
        }
        filed.sort_by_key(|(season, kind, name)| (*season, kind.as_str(), name.to_string()));

        assert_eq!(
            filed,
            [
                (Some(0), CoverType::Banner, "70863-0.jpg"),
                (Some(0), CoverType::Poster, "70863-0-3.jpg"),
                (Some(1), CoverType::Poster, "6a156459154c1.jpg"),
            ],
            "the aired seasons' pictures, and no other order's"
        );
    }

    #[test]
    fn only_the_aired_season_ordering_is_taken() {
        // TVDB describes DVD and absolute orderings of the same series; taking
        // them all would list season 1 three times.
        let item = to_item(&fixture(), "eng");
        let numbers: Vec<i32> = item.seasons.iter().map(|s| s.season_number).collect();
        assert_eq!(numbers, vec![0, 1]);
    }

    /// A server answering every request with `answer`, counting them, and the
    /// path each asked for.
    async fn answering(
        answer: &'static str,
    ) -> (String, std::sync::Arc<parking_lot::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let asked = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let seen = asked.clone();
        tokio::spawn(async move {
            while let Ok((mut connection, _)) = listener.accept().await {
                let mut request = vec![0u8; 8192];
                let read = connection.read(&mut request).await.unwrap_or(0);
                let line = String::from_utf8_lossy(&request[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                seen.lock().push(line);
                let _ = connection.write_all(answer.as_bytes()).await;
                let _ = connection.shutdown().await;
            }
        });
        (at, asked)
    }

    fn client_at(upstream: &str) -> TvdbClient {
        TvdbClient::new(
            reqwest::Client::new(),
            &config::Tvdb {
                upstream: upstream.into(),
                api_key: Some("wrong".into()),
                pin: None,
                enabled: true,
                passthrough: false,
            },
            "en",
            "test".into(),
        )
    }

    #[tokio::test]
    async fn a_failed_sign_in_is_remembered_rather_than_asked_again_on_every_call() {
        const REFUSED: &str = "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 46\r\nConnection: close\r\n\r\n{\"status\":\"failure\",\"message\":\"InvalidAPIKey\"}";
        let (at, asked) = answering(REFUSED).await;
        let tvdb = client_at(&at);

        let first = tvdb.bearer().await.unwrap_err();
        assert!(format!("{first:#}").contains("401"), "{first:#}");
        assert!(format!("{first:#}").contains("InvalidAPIKey"), "{first:#}");
        // The relay's next call, and a refresh's: no second sign-in.
        assert!(tvdb.bearer().await.is_err());
        assert!(tvdb.series(81189, DEFAULT_CAST).await.is_err());
        let lines = asked.lock().clone();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("POST /login"), "{lines:?}");
    }

    #[tokio::test]
    async fn a_language_is_one_segment_of_the_path_whatever_it_holds() {
        const EMPTY: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 47\r\nConnection: close\r\n\r\n{\"data\":{\"token\":\"t\",\"episodes\":[]},\"links\":{}}";
        let (at, asked) = answering(EMPTY).await;
        let tvdb = client_at(&at);

        let texts = tvdb.episode_texts(81189, "../../login?x=1#").await.unwrap();
        assert!(texts.is_empty());
        let lines = asked.lock().clone();
        assert!(
            lines.iter().any(|l| l.starts_with(
                "GET /series/81189/episodes/official/..%2F..%2Flogin%3Fx%3D1%23?page=0 "
            )),
            "{lines:?}"
        );
    }

    /// A server answering each request by the start of its path — the first
    /// route that matches, 404 for none — and the request line of each.
    async fn answering_by_path(
        routes: Vec<(&'static str, String)>,
    ) -> (String, std::sync::Arc<parking_lot::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        const MISSING: &str =
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let asked = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let seen = asked.clone();
        tokio::spawn(async move {
            while let Ok((mut connection, _)) = listener.accept().await {
                let mut request = vec![0u8; 8192];
                let read = connection.read(&mut request).await.unwrap_or(0);
                let line = String::from_utf8_lossy(&request[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let path = line.split(' ').nth(1).unwrap_or_default().to_string();
                seen.lock().push(line);
                let answer = routes
                    .iter()
                    .find(|(prefix, _)| path.starts_with(prefix))
                    .map_or(MISSING, |(_, answer)| answer.as_str());
                let _ = connection.write_all(answer.as_bytes()).await;
                let _ = connection.shutdown().await;
            }
        });
        (at, asked)
    }

    fn json_reply(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// An episode's id and where it stands: its season, its number, its
    /// absolute number.
    type Placement = (Option<i64>, i32, i32, Option<i32>);

    /// The Office (US), as TheTVDB answers for it: a DVD order the episodes
    /// endpoint lists, and a second alternate order it answers empty for
    /// (thetvdb/v4-api#340) whose seasons' own records list it — read
    /// season by season, one call each, and placed by the season's number
    /// whatever the record says.
    #[tokio::test]
    async fn an_order_the_endpoint_answers_empty_is_read_season_by_season() {
        let (at, asked) = answering_by_path(vec![
            ("/login", json_reply(r#"{"data":{"token":"t"}}"#)),
            (
                "/series/73244/extended",
                json_reply(
                    r#"{"data":{"id":73244,"seasons":[
                        {"id":10,"number":1,"type":{"type":"official","name":"Aired Order"}},
                        {"id":20,"number":1,"type":{"type":"dvd","name":"DVD Order"}},
                        {"id":31,"number":2,"type":{"type":"alttwo","name":"Alternate Order 2","alternateName":"Superfan Order"}},
                        {"id":30,"number":1,"type":{"type":"alttwo","name":"Alternate Order 2","alternateName":"Superfan Order"}}
                    ]}}"#,
                ),
            ),
            (
                "/series/73244/episodes/dvd/eng",
                json_reply(
                    r#"{"data":{"episodes":[{"id":110413,"seasonNumber":1,"number":1}]},"links":{"next":null}}"#,
                ),
            ),
            (
                "/series/73244/episodes/alttwo/eng",
                json_reply(r#"{"data":{"episodes":[]},"links":{}}"#),
            ),
            (
                "/seasons/30/extended",
                json_reply(
                    r#"{"data":{"id":30,"number":1,"type":{"type":"alttwo"},"episodes":[
                        {"id":10718410,"seasonNumber":99,"number":2,"absoluteNumber":2,"name":"Diversity Day (Extended Cut)","aired":"2005-03-29","runtime":31},
                        {"id":10718411,"seasonNumber":99,"number":1,"name":"Pilot (Extended Cut)","absoluteNumber":0}
                    ]}}"#,
                ),
            ),
            (
                "/seasons/31/extended",
                json_reply(
                    r#"{"data":{"id":31,"number":2,"episodes":[{"id":10718412,"seasonNumber":2,"number":1}]}}"#,
                ),
            ),
        ])
        .await;
        let tvdb = client_at(&at);

        let orders = tvdb.orders(73244).await.expect("the orders");
        let placed: Vec<(&str, Vec<Placement>)> = orders
            .iter()
            .map(|order| {
                (
                    order.key.as_str(),
                    order
                        .episodes
                        .iter()
                        .map(|p| {
                            (
                                p.tvdb_id,
                                p.season_number,
                                p.episode_number,
                                p.absolute_number,
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            placed,
            [
                (
                    "alttwo",
                    vec![
                        (Some(10718411), 1, 1, None),
                        (Some(10718410), 1, 2, Some(2)),
                        (Some(10718412), 2, 1, None)
                    ]
                ),
                ("dvd", vec![(Some(110413), 1, 1, None)]),
            ]
        );
        // Named by the series' own name for the order, where TheTVDB keeps
        // one; each place with the record TheTVDB gave of its episode.
        assert_eq!(
            orders
                .iter()
                .map(|o| (o.label.as_str(), o.kind, o.source.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Superfan Order", OrderKind::TvdbAlt, "tvdb"),
                ("DVD Order", OrderKind::Dvd, "tvdb")
            ]
        );
        let extended = &orders[0].episodes[1];
        let record = extended.record.as_ref().expect("its record");
        assert_eq!(record.title, "Diversity Day (Extended Cut)");
        assert_eq!(record.aired.as_deref(), Some("2005-03-29"));
        assert_eq!(record.runtime, Some(31));

        // One call per season of the order answered empty, and none for the
        // order the endpoint listed.
        let lines = asked.lock().clone();
        let seasons_read: Vec<&str> = lines
            .iter()
            .filter(|line| line.starts_with("GET /seasons/"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            seasons_read,
            [
                "GET /seasons/30/extended HTTP/1.1",
                "GET /seasons/31/extended HTTP/1.1"
            ],
            "{lines:?}"
        );
    }

    /// Shaped as `/series/{id}/extended` lists them: the people of the
    /// series, each with TheTVDB's id, the part, the person's portrait.
    #[test]
    fn the_characters_are_the_cast_in_thetvdb_s_order_up_to_the_cap() {
        let series: SeriesExtended = serde_json::from_value(serde_json::json!({
            "id": 81189,
            "name": "Breaking Bad",
            "characters": [
                { "name": "Skyler White", "peopleId": 269404, "personName": "Anna Gunn", "type": 3,
                  "sort": 2, "personImgURL": "https://artworks.thetvdb.com/p/anna.jpg",
                  "image": "/banners/actors/75476.jpg" },
                { "name": "Walter White", "peopleId": 17419, "personName": "Bryan Cranston",
                  "type": 3, "sort": 1, "image": "/banners/actors/walt.jpg" },
                { "name": "Heisenberg", "peopleId": 17419, "personName": "Bryan Cranston",
                  "type": 3, "sort": 4 },
                { "name": "Somebody Unsorted", "peopleId": 5, "personName": "Late Comer",
                  "type": 3, "sort": 0 },
                { "name": "A Guest", "peopleId": 6, "personName": "Guest Of One", "type": 4, "sort": 3 },
                { "name": "In One Episode", "peopleId": 7, "personName": "Episode Only", "type": 3,
                  "sort": 3, "episodeId": 349232 },
                { "name": "Nobody", "peopleId": 8, "personName": " ", "type": 3, "sort": 5 }
            ]
        }))
        .expect("a series");

        let (cast, known) = characters(&series, DEFAULT_CAST);
        let read: Vec<(&str, Option<&str>, i32)> = cast
            .iter()
            .map(|c| {
                (
                    c.person_name.as_str(),
                    c.character_name.as_deref(),
                    c.sort_order,
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                ("Bryan Cranston", Some("Walter White / Heisenberg"), 0),
                ("Anna Gunn", Some("Skyler White"), 1),
                ("Late Comer", Some("Somebody Unsorted"), 2),
            ],
            "TheTVDB's order, the unsorted after; one credit for two parts; no \
             guest, no part in one episode, nobody nameless"
        );
        assert!(cast.iter().all(|c| c.credit_type == CreditType::Actor));
        assert_eq!(
            cast[1].image.as_deref(),
            Some("https://artworks.thetvdb.com/p/anna.jpg"),
            "the person's portrait before the part's picture"
        );
        assert_eq!(
            cast[0].image.as_deref(),
            Some("https://artworks.thetvdb.com/banners/actors/walt.jpg")
        );
        assert_eq!(known[&cast[0].id].tvdb_id, Some(17419));
        assert_eq!(known[&cast[1].id].tvdb_id, Some(269404));

        let (cut, _) = characters(&series, 2);
        assert_eq!(cut.len(), 2, "the cap is the setting's");
    }

    #[test]
    fn statuses_map_to_sonarrs_vocabulary() {
        assert_eq!(status(Some("Ended")), "ended");
        assert_eq!(status(Some("Continuing")), "continuing");
        assert_eq!(status(Some("Upcoming")), "upcoming");
        assert_eq!(status(None), "continuing");
    }

    #[test]
    fn every_country_s_rating_is_kept_in_alpha_2() {
        let item = to_item(&fixture(), "eng");
        let rated: Vec<(&str, Option<&str>)> = item
            .releases
            .iter()
            .map(|r| (r.country.as_str(), r.certification.as_deref()))
            .collect();
        assert_eq!(rated, [("GB", Some("18")), ("US", Some("TV-MA"))]);
        assert!(
            item.releases
                .iter()
                .all(|r| r.source.as_deref() == Some("tvdb") && r.release_type.is_none())
        );
        assert_eq!(item.certification_in("GB"), Some("18"));

        // A country this server cannot name is left out, and one rated
        // twice keeps its first.
        let rating = |country: &str, name: &str| ContentRating {
            country: country.into(),
            name: name.into(),
        };
        let kept = ratings_by_country(&[
            rating("xyz", "X"),
            rating("fra", "-12"),
            rating("fra", "-16"),
            rating("deu", " "),
        ]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].country, "FR");
        assert_eq!(kept[0].certification.as_deref(), Some("-12"));
    }

    #[test]
    fn unknown_countries_are_left_out_rather_than_guessed() {
        assert_eq!(alpha3_to_alpha2("usa").as_deref(), Some("US"));
        assert_eq!(alpha3_to_alpha2("USA").as_deref(), Some("US"));
        assert_eq!(alpha3_to_alpha2("xyz"), None);
    }
}

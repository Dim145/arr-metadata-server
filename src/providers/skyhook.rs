//! Sonarr's own Skyhook, as a second opinion and as a fallback.
//!
//! Used when this server cannot answer from its own store or from TMDB — most
//! often for a TVDB-only series, or one whose TVDB↔TMDB link is missing — and
//! beside the others while enrichment is on. The response is absorbed into
//! the canonical model, so the next request is served locally and the entry
//! becomes editable like any other.
//!
//! Its second edition is asked first, what Sonarr v5 reads:
//! `/v2/tvdb/shows/eng/{id}/official` and `/v2/tvdb/search?language=eng`. It
//! says what the first never did — every language the series is held in, a
//! season's own title, the title in the series' own language — and names an
//! episode's ids `metadataId` and `metadataShowId` where the first says
//! `tvdbId` and `tvdbShowId`. Both names are read, either or both being
//! there, since a mirror of this server writes the two pairs. The first
//! edition, `/v1/tvdb/shows/en/{id}`, is asked when the second answers with
//! an error, a 404 among them: a mirror that only knows the first edition
//! answers the second with a 404 for every series, and a series Skyhook does
//! not have costs one request more.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    config,
    domain::{MediaItem, Translation},
    wire::{readable_entries, sonarr::ShowResource, string_or_null},
};

/// The language the first edition is asked in, the only one it accepts:
/// `en-US`, `fr` and `es` are each a 400 or a 404 there. What it is asked
/// for is structural anyway — absolute numbering, air-order hints, ids —
/// and the prose is translated by the language overlay, from providers that
/// do speak other languages.
const LANGUAGE_V1: &str = "en";

/// The second edition's, in three letters as Sonarr v5 writes them. English
/// too, for the same reason: the other languages come in `translations`.
const LANGUAGE_V2: &str = "eng";

/// The order the second edition numbers episodes by: TheTVDB's aired order,
/// the one Sonarr numbers a series by unless told otherwise.
const SEASON_TYPE: &str = "official";

/// The client only speaks HTTP. Whether it is spoken to at all is a setting,
/// read by the caller — keeping the switch in one place rather than half here
/// and half there, which is how `skyhook.fallback` came to be a setting that
/// changed nothing.
pub struct SkyhookClient {
    http: reqwest::Client,
    base: String,
    instance: String,
    gate: crate::providers::Gate,
}

/// How many requests to Skyhook are in flight at once: a Sonarr library
/// refresh asks for every series at the same moment.
const AT_ONCE: usize = 8;

/// A series as Skyhook answered it: what both editions say, read as the
/// first one says it, and what only the second adds.
#[derive(Clone, Debug)]
pub struct Show {
    resource: ShowResource,
    /// The series in other languages, by their three letters.
    translations: Vec<Translation>,
    /// Each season's own title, by its number.
    season_titles: HashMap<i32, String>,
    /// The title in the series' own language.
    original_title: Option<String>,
}

impl Show {
    /// The series' title, empty when Skyhook names nothing.
    pub fn title(&self) -> &str {
        &self.resource.title
    }
}

/// This server answered itself: Skyhook's name resolves here.
#[derive(Debug, thiserror::Error)]
#[error(
    "skyhook.sonarr.tv resolves to this server — set AMS_SKYHOOK_UPSTREAM to somewhere else, or \
     turn the provider off"
)]
struct Looped;

impl SkyhookClient {
    pub fn new(http: reqwest::Client, cfg: &config::Skyhook, instance: String) -> Self {
        Self {
            http,
            base: cfg.upstream.clone(),
            instance,
            gate: crate::providers::Gate::new("skyhook", "Skyhook", AT_ONCE),
        }
    }

    /// One show by TVDB id, from the second edition or else the first.
    /// Returns the raw body alongside the one read so the caller can
    /// snapshot it.
    pub async fn show(&self, tvdb_id: i64) -> Result<Option<(Value, Show)>> {
        let second = format!(
            "{}/v2/tvdb/shows/{LANGUAGE_V2}/{tvdb_id}/{SEASON_TYPE}",
            self.base
        );
        match self.fetch(&second, &[]).await {
            Ok(Some(value)) => match read(&value) {
                Ok(show) => return Ok(Some((value, show))),
                Err(e) => tracing::warn!(
                    tvdb_id,
                    error = format_args!("{e:#}"),
                    "Skyhook's second edition answered with a show this server could not read; \
                     asking the first"
                ),
            },
            Ok(None) => {}
            Err(e) if e.is::<Looped>() => return Err(e),
            Err(e) => tracing::debug!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "Skyhook's second edition failed; asking the first"
            ),
        }

        let first = format!("{}/v1/tvdb/shows/{LANGUAGE_V1}/{tvdb_id}", self.base);
        let Some(value) = self.fetch(&first, &[]).await? else {
            return Ok(None);
        };
        let show =
            read(&value).context("Skyhook returned a show this server could not interpret")?;
        Ok(Some((value, show)))
    }

    /// Series by title, or by a `prefix:id` lookup, from the second edition
    /// or else the first.
    pub async fn search(&self, term: &str) -> Result<Vec<Show>> {
        let second = format!("{}/v2/tvdb/search", self.base);
        let answered = match self
            .fetch(&second, &[("language", LANGUAGE_V2), ("term", term)])
            .await
        {
            Ok(Some(value)) => Some(value),
            Ok(None) => None,
            Err(e) if e.is::<Looped>() => return Err(e),
            Err(e) => {
                tracing::debug!(
                    error = format_args!("{e:#}"),
                    "Skyhook's second edition failed a search; asking the first"
                );
                None
            }
        };
        let value = match answered {
            Some(value) => value,
            None => {
                let first = format!("{}/v1/tvdb/search/{LANGUAGE_V1}", self.base);
                let Some(value) = self.fetch(&first, &[("term", term)]).await? else {
                    return Ok(Vec::new());
                };
                value
            }
        };

        // A single 404-ish payload or an unexpected shape is an empty result,
        // not an error: the caller has already tried everything else. A show
        // it names nothing would be a result with nothing to pick it by.
        let Value::Array(entries) = value else {
            return Ok(Vec::new());
        };
        Ok(entries
            .iter()
            .filter_map(|entry| read(entry).ok())
            .filter(|show| !show.title().trim().is_empty())
            .collect())
    }

    async fn fetch(&self, url: &str, query: &[(&str, &str)]) -> Result<Option<Value>> {
        let (response, _permit) = self
            .gate
            .send(|| {
                self.http
                    .get(url)
                    .query(query)
                    // See `providers::radarr::LOOP_HEADER`: this hostname is one
                    // we also answer on, so a resolver-level redirect would have
                    // us call ourselves.
                    .header(crate::providers::radarr::LOOP_HEADER, &self.instance)
                    .timeout(std::time::Duration::from_secs(20))
            })
            .await
            .map_err(|e| anyhow::anyhow!("Skyhook request failed: {url}: {e}"))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if response.status() == reqwest::StatusCode::LOOP_DETECTED {
            return Err(Looped.into());
        }

        let status = response.status();
        if !status.is_success() {
            let reason = crate::providers::error_text(response).await;
            anyhow::bail!("Skyhook returned {status} for {url}: {reason}");
        }

        crate::providers::read_json(response)
            .await
            .map(Some)
            .with_context(|| format!("Skyhook returned a body this server could not read: {url}"))
    }
}

/// What only the second edition says, read leniently: an entry that cannot
/// be read is left out, and none of it fails the show.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Additions {
    #[serde(default, deserialize_with = "string_or_null")]
    original_title: String,
    #[serde(default, deserialize_with = "readable_entries")]
    translations: Vec<TranslationResource>,
    #[serde(default, deserialize_with = "readable_entries")]
    seasons: Vec<SeasonTitle>,
}

#[derive(Deserialize)]
struct TranslationResource {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    overview: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeasonTitle {
    season_number: i32,
    #[serde(default)]
    title: Option<String>,
}

/// A show of either edition, read.
fn read(value: &Value) -> Result<Show> {
    let mut first = value.clone();
    as_first_edition(&mut first);
    let resource: ShowResource = serde_json::from_value(first)?;
    let additions = Additions::deserialize(value).unwrap_or_default();

    let mut translations: Vec<Translation> = Vec::new();
    for entry in additions.translations {
        let Some(language) = entry
            .language
            .map(|l| l.trim().to_ascii_lowercase())
            .filter(|l| !l.is_empty())
        else {
            continue;
        };
        let title = non_empty(entry.title);
        let overview = non_empty(entry.overview);
        // One with neither field is noise; a language twice is the first.
        if (title.is_none() && overview.is_none())
            || translations.iter().any(|t| t.language == language)
        {
            continue;
        }
        translations.push(Translation {
            language,
            title,
            overview,
            is_manual: false,
        });
    }

    let season_titles = additions
        .seasons
        .into_iter()
        .filter_map(|s| Some((s.season_number, non_empty(s.title)?)))
        .collect();

    Ok(Show {
        resource,
        translations,
        season_titles,
        original_title: non_empty(Some(additions.original_title)),
    })
}

/// The second edition's names for an episode's ids, read as the first's: an
/// episode that says `metadataId` and not `tvdbId` is given its `tvdbId`
/// from it, and the same for the show's. One that says both is left as it is.
fn as_first_edition(value: &mut Value) {
    let Some(episodes) = value.get_mut("episodes").and_then(Value::as_array_mut) else {
        return;
    };
    for episode in episodes {
        let Some(fields) = episode.as_object_mut() else {
            continue;
        };
        for (first, second) in [("tvdbId", "metadataId"), ("tvdbShowId", "metadataShowId")] {
            if fields.get(first).is_none_or(Value::is_null)
                && let Some(id) = fields.get(second).cloned()
            {
                fields.insert(first.to_string(), id);
            }
        }
    }
}

fn non_empty(text: Option<String>) -> Option<String> {
    text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// A show absorbed into the canonical model: what both editions say, as the
/// first is absorbed, with the languages and the seasons' titles the second
/// adds.
pub fn to_item(show: &Show) -> MediaItem {
    let mut item = crate::wire::sonarr::to_item(&show.resource);
    if show.original_title.is_some() {
        item.original_title.clone_from(&show.original_title);
    }
    item.translations.clone_from(&show.translations);
    for season in &mut item.seasons {
        if let Some(title) = show.season_titles.get(&season.season_number) {
            season.title = Some(title.clone());
        }
    }
    item
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "v1" => include_str!("fixtures/skyhook/v1_show_305288.json"),
            "v2" => include_str!("fixtures/skyhook/v2_show_305288.json"),
            "ours" => include_str!("fixtures/skyhook/ours_show_305288.json"),
            "search" => include_str!("fixtures/skyhook/v2_search_stranger_things.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(text).expect("a fixture")
    }

    #[test]
    fn the_second_edition_is_read_with_its_languages_and_its_seasons_titles() {
        let show = read(&fixture("v2")).expect("read");
        let item = to_item(&show);

        assert_eq!(item.title, "Stranger Things");
        assert_eq!(item.external_ids.tvdb, Some(305288));
        assert_eq!(item.original_title.as_deref(), Some("Stranger Things"));
        // Every episode keeps TheTVDB's id, read from `metadataId`.
        assert!(!item.episodes.is_empty());
        assert!(item.episodes.iter().all(|e| e.tvdb_id.is_some()));
        let first = item
            .episodes
            .iter()
            .find(|e| e.season_number == 1 && e.episode_number == 1)
            .expect("S01E01");
        assert_eq!(first.tvdb_id, Some(5468124));
        assert_eq!(first.absolute_episode_number, Some(1));

        let french = item
            .translations
            .iter()
            .find(|t| t.language == "fra")
            .expect("French");
        assert!(
            french
                .overview
                .as_deref()
                .is_some_and(|o| o.starts_with("Quand un jeune garçon"))
        );
        assert!(item.translations.len() > 20, "{}", item.translations.len());

        let second = item
            .seasons
            .iter()
            .find(|s| s.season_number == 2)
            .expect("season 2");
        assert_eq!(second.title.as_deref(), Some("Stranger Things 2"));
        // The specials have no title of their own, and are given none.
        let specials = item.seasons.iter().find(|s| s.season_number == 0).unwrap();
        assert_eq!(specials.title, None);
    }

    #[test]
    fn the_first_edition_reads_as_it_did() {
        let item = to_item(&read(&fixture("v1")).expect("read"));
        assert_eq!(item.external_ids.tvdb, Some(305288));
        assert!(item.episodes.iter().all(|e| e.tvdb_id.is_some()));
        assert!(item.translations.is_empty());
        assert!(item.seasons.iter().all(|s| s.title.is_none()));
    }

    #[test]
    fn a_mirror_that_writes_both_names_is_read_once() {
        // This server's own `/v2/`, which writes `tvdbId` and `metadataId`
        // both: neither a duplicate nor a refusal.
        let item = to_item(&read(&fixture("ours")).expect("read"));
        assert!(item.episodes.iter().all(|e| e.tvdb_id.is_some()));
        assert_eq!(
            item.episodes
                .iter()
                .find(|e| e.season_number == 0 && e.episode_number == 1)
                .and_then(|e| e.tvdb_id),
            Some(6382653)
        );
    }

    #[test]
    fn the_names_of_the_second_edition_are_taken_only_where_the_first_are_missing() {
        let mut value = serde_json::json!({
            "tvdbId": 1,
            "episodes": [
                { "seasonNumber": 1, "episodeNumber": 1, "metadataId": 11, "metadataShowId": 1 },
                { "seasonNumber": 1, "episodeNumber": 2, "tvdbId": 12, "metadataId": 99 },
                { "seasonNumber": 1, "episodeNumber": 3, "tvdbId": null, "metadataId": 13 },
                "not an episode"
            ]
        });
        as_first_edition(&mut value);
        let ids: Vec<Value> = value["episodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.get("tvdbId").cloned().unwrap_or(Value::Null))
            .collect();
        assert_eq!(
            ids,
            [11, 12, 13]
                .map(Value::from)
                .into_iter()
                .chain([Value::Null])
                .collect::<Vec<_>>()
        );
        assert_eq!(value["episodes"][0]["tvdbShowId"], 1);
    }

    #[test]
    fn a_search_of_the_second_edition_reads_each_result() {
        let Value::Array(entries) = fixture("search") else {
            panic!("a list");
        };
        let shows: Vec<Show> = entries.iter().filter_map(|e| read(e).ok()).collect();
        assert_eq!(shows.len(), 2);
        assert_eq!(shows[0].resource.tvdb_id, 305288);
        assert!(!to_item(&shows[0]).translations.is_empty());
    }

    /// A Skyhook small enough to read: each path answered with a status and
    /// a body, every request line kept.
    async fn skyhook(
        answers: Vec<(&'static str, u16, String)>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&asked);
        tokio::spawn(async move {
            while let Ok((mut connection, _)) = listener.accept().await {
                let mut request = vec![0u8; 8192];
                let read = connection.read(&mut request).await.unwrap_or(0);
                let line = String::from_utf8_lossy(&request[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                seen.lock().push(path.clone());
                let (status, body) = answers
                    .iter()
                    .find(|(prefix, _, _)| path.starts_with(prefix))
                    .map(|(_, status, body)| (*status, body.clone()))
                    .unwrap_or((404, String::new()));
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = connection.write_all(reply.as_bytes()).await;
                let _ = connection.shutdown().await;
            }
        });
        (at, asked)
    }

    fn client(at: &str) -> SkyhookClient {
        SkyhookClient::new(
            reqwest::Client::new(),
            &config::Skyhook {
                upstream: at.to_string(),
                fallback: true,
                enrich: true,
            },
            "test-instance".into(),
        )
    }

    #[tokio::test]
    async fn the_second_edition_is_asked_first_and_the_first_when_it_fails() {
        let v2 = fixture("v2").to_string();
        let v1 = fixture("v1").to_string();

        let (at, asked) = skyhook(vec![
            ("/v2/tvdb/shows/eng/305288/official", 200, v2),
            ("/v1/", 200, v1.clone()),
        ])
        .await;
        let (_, show) = client(&at).show(305288).await.unwrap().expect("found");
        assert!(!to_item(&show).translations.is_empty());
        assert_eq!(
            asked.lock().as_slice(),
            ["/v2/tvdb/shows/eng/305288/official"]
        );

        let (at, asked) = skyhook(vec![
            ("/v2/", 500, r#"{"message":"down"}"#.into()),
            ("/v1/tvdb/shows/en/305288", 200, v1),
        ])
        .await;
        let (_, show) = client(&at).show(305288).await.unwrap().expect("found");
        assert_eq!(show.resource.tvdb_id, 305288);
        assert!(to_item(&show).translations.is_empty());
        assert_eq!(
            asked.lock().as_slice(),
            [
                "/v2/tvdb/shows/eng/305288/official",
                "/v1/tvdb/shows/en/305288"
            ]
        );

        // Neither has it.
        let (at, _) = skyhook(Vec::new()).await;
        assert!(client(&at).show(305288).await.unwrap().is_none());
    }
}

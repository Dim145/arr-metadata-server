//! What IMDb's titles and episodes give a work, as the clients are given it.
//!
//! The titles a film goes by in the regions a caller is set to are given to
//! Radarr among its alternative titles, and to nothing else; each episode of
//! a series is matched to IMDb's by its season and its number, for its page
//! and its `.nfo` document. Each only while its switch is on — `imdb.akas`,
//! `imdb.episodes` — whatever the tables still hold from when it was.

use std::collections::HashMap;

use crate::{
    db::repo::{self, imdb::EpisodeMatch},
    domain::MediaItem,
    state::AppState,
    wire::radarr::RegionalTitle,
};

/// How many regions `imdb.regions` may name: more would be asking Radarr to
/// match a film's releases against the whole world's titles.
pub const MOST_REGIONS: usize = 24;

/// The regions a value of `imdb.regions` names — `FR, BE CA` is FR, BE and
/// CA — each as IMDb writes one: ISO 3166-1's two letters, or a code of
/// IMDb's own of three or four (`XWW`, the world in English).
pub fn parse_regions(text: &str) -> Result<Vec<String>, String> {
    let mut regions: Vec<String> = Vec::new();
    for token in text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|token| !token.is_empty())
    {
        let region = token.to_ascii_uppercase();
        if !(2..=4).contains(&region.len()) || !region.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(format!(
                "{token} is not a region: two letters, as FR or US, or one of IMDb's own codes, as XWW"
            ));
        }
        if !regions.contains(&region) {
            regions.push(region);
        }
    }
    if regions.is_empty() {
        return Err("names no region".into());
    }
    if regions.len() > MOST_REGIONS {
        return Err(format!("names more than {MOST_REGIONS} regions"));
    }
    Ok(regions)
}

/// The regions whose titles a caller's Radarr is given: those `imdb.regions`
/// names for it — on the server, on its key or on the rule of its address —
/// else the country of the language it is answered in (`fr-FR` is FR), else
/// the United States, as TMDB's answers default to.
pub fn regions(state: &AppState, client: Option<&str>, peer: Option<&str>) -> Vec<String> {
    if let Some(set) = state.settings.resolve("imdb.regions", client, peer)
        && let Ok(regions) = parse_regions(&set)
    {
        return regions;
    }
    vec![region_of(&state.language(client, peer)).unwrap_or_else(|| "US".to_string())]
}

/// The country a language's tag names: `fr-FR` is FR, `fr` none.
fn region_of(tag: &str) -> Option<String> {
    tag.split(['-', '_'])
        .nth(1)
        .map(str::to_ascii_uppercase)
        .filter(|region| region.len() == 2 && region.bytes().all(|b| b.is_ascii_uppercase()))
}

/// The titles IMDb gives each of these films in these regions, in IMDb's
/// order: one list a film, in the films' order. Empty lists while the
/// titles are switched off, and when they could not be read — a title is
/// not worth failing Radarr's refresh of a film over.
pub async fn regional_titles(
    state: &AppState,
    items: &[MediaItem],
    regions: &[String],
) -> Vec<Vec<RegionalTitle>> {
    let none = || vec![Vec::new(); items.len()];
    if !state.flag("imdb.akas", false) || regions.is_empty() {
        return none();
    }

    let tconsts: Vec<String> = items
        .iter()
        .filter_map(|item| item.external_ids.imdb.clone())
        .collect();
    let titles = match repo::imdb::titles_in(&state.db, &tconsts, regions).await {
        Ok(titles) => titles,
        Err(e) => {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "could not read IMDb's titles by region"
            );
            return none();
        }
    };

    let mut by_work: HashMap<String, Vec<RegionalTitle>> = HashMap::new();
    for title in titles {
        by_work
            .entry(title.tconst)
            .or_default()
            .push(RegionalTitle {
                title: title.title,
                language: title.language,
            });
    }

    items
        .iter()
        .map(|item| {
            item.external_ids
                .imdb
                .as_ref()
                .and_then(|tconst| by_work.get(tconst))
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

/// Each episode of a work IMDb numbers the same, of one season or of all,
/// with IMDb's rating of it while IMDb's ratings are on. Nothing while
/// `imdb.episodes` is off.
pub async fn episodes(
    state: &AppState,
    media_id: &str,
    season: Option<i32>,
) -> anyhow::Result<Vec<EpisodeMatch>> {
    if !state.flag("imdb.episodes", false) {
        return Ok(Vec::new());
    }
    let mut found = repo::imdb::episodes_of(&state.db, media_id, season).await?;
    // A list of ratings no longer read is no longer current.
    if !state.flag("imdb.enabled", false) {
        for episode in &mut found {
            episode.rating = None;
            episode.votes = None;
        }
    }
    Ok(found)
}

/// The IMDb id each of a work's episodes is matched to, by the episode's id,
/// for its `.nfo` document. Nothing while `imdb.episodes` is off, or when
/// they could not be read: a document goes without rather than not at all.
pub async fn episode_ids(state: &AppState, media_id: &str) -> HashMap<String, String> {
    if !state.flag("imdb.episodes", false) {
        return HashMap::new();
    }
    match repo::imdb::episode_tconsts(&state.db, media_id).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(
                media_id,
                error = format_args!("{e:#}"),
                "could not read which of IMDb's episodes a work's are"
            );
            HashMap::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_names_regions_as_imdb_writes_them() {
        assert_eq!(
            parse_regions("fr, BE  ca,XWW,FR").unwrap(),
            ["FR", "BE", "CA", "XWW"]
        );
        for refused in ["", " , ", "France", "F", "F1", "FR;BE"] {
            assert!(parse_regions(refused).is_err(), "{refused:?}");
        }
        let many: Vec<String> = (b'A'..=b'Z').map(|c| format!("A{}", c as char)).collect();
        assert!(parse_regions(&many[..MOST_REGIONS].join(",")).is_ok());
        assert!(parse_regions(&many.join(",")).is_err());
    }

    #[test]
    fn a_language_names_its_country_when_its_tag_does() {
        assert_eq!(region_of("fr-FR").as_deref(), Some("FR"));
        assert_eq!(region_of("en_us").as_deref(), Some("US"));
        assert_eq!(region_of("pt-BR").as_deref(), Some("BR"));
        assert_eq!(region_of("fr"), None);
        assert_eq!(region_of("zh-Hant"), None);
    }

    #[tokio::test]
    async fn a_callers_regions_are_set_or_else_its_languages_country() {
        let state = crate::api::native::testing::server().await;
        let set = |key: &'static str, value: &'static str| {
            let state = state.clone();
            async move {
                state
                    .settings
                    .set(
                        crate::settings::registry::Scope::Server,
                        "",
                        key,
                        value,
                        None,
                    )
                    .await
                    .unwrap();
            }
        };

        set("tmdb.language", "fr-FR").await;
        assert_eq!(regions(&state, None, None), ["FR"]);
        set("tmdb.language", "de").await;
        assert_eq!(regions(&state, None, None), ["US"], "TMDB's default");
        set("imdb.regions", "be, ca, XWW").await;
        assert_eq!(regions(&state, None, None), ["BE", "CA", "XWW"]);
    }
}

//! One id of a work's to its others, from Wikidata's links kept here (wave P,
//! P5).
//!
//! What [`crate::jobs::wikidata`] kept at its last reading, and nothing else:
//! a request never waits on Wikidata. Nothing at all while `wikidata.enabled`
//! is off, whatever the links kept still hold — as IMDb's lists are read only
//! while their switches are on.
//!
//! The links only ever find ids. A work is then answered from what is held
//! here, or fetched from the providers by the id found; nothing is written of
//! a work from the links themselves.
//!
//! Nor do they ever change the id a client holds a work by. A work held is
//! answered as it is held, under the id its clients know: a series TMDB
//! alone names keeps the number made of TMDB's id, and its numbering, even
//! where the links know its TheTVDB id ([`held`]). A work is fetched by the
//! ids they give only where nothing is held under the id asked, nor under
//! the ids they give ([`fetchable`]): the work fetched is a new one, which
//! no client holds yet.

use anyhow::Result;

use crate::{
    db::repo::{self, wikidata::Link},
    domain::{MediaItem, MediaKind},
    service::{self, ids},
    state::AppState,
};

pub use crate::providers::wikidata::Site;

/// One of Wikidata's items an id is on, and every id the item carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Wikidata's id for the item: `Q1079`.
    pub id: String,
    /// Each id the item carries, by site, as the site writes it — the one
    /// asked by among them — in the order of [`Site::ALL`], then of the ids.
    pub ids: Vec<(Site, String)>,
}

impl Item {
    /// The ids the item carries of one site's.
    pub fn of(&self, site: Site) -> impl Iterator<Item = &str> {
        self.ids
            .iter()
            .filter(move |(of, _)| *of == site)
            .map(|(_, id)| id.as_str())
    }
}

/// Whether the links are read at all: `wikidata.enabled`.
pub fn enabled(state: &AppState) -> bool {
    state.flag("wikidata.enabled", false)
}

/// The items an id of `site`'s is on, each with every id it carries: one,
/// nearly always — Wikidata has its duplicates. None while the links are not
/// read, and for what is not an id of the site's as the site writes it.
pub async fn linked(state: &AppState, site: Site, id: &str) -> Result<Vec<Item>> {
    if !enabled(state) {
        return Ok(Vec::new());
    }
    let Some(number) = site.number(id.trim()) else {
        return Ok(Vec::new());
    };
    let links = repo::wikidata::of_id(&state.db, site.property(), number).await?;
    Ok(items(&links))
}

/// The item Wikidata knows by `id` — `Q1079` — with every id it carries;
/// none while the links are not read, or for an item they do not hold.
pub async fn item(state: &AppState, id: &str) -> Result<Option<Item>> {
    if !enabled(state) {
        return Ok(None);
    }
    let Some(number) = item_number(id) else {
        return Ok(None);
    };
    let links = repo::wikidata::of_item(&state.db, number).await?;
    Ok(items(&links).pop())
}

/// The ids of `to`'s the items an id of `from`'s is on carry, each once, in
/// the order of the items.
pub async fn others(state: &AppState, from: Site, id: &str, to: Site) -> Result<Vec<String>> {
    let mut found: Vec<String> = Vec::new();
    for item in linked(state, from, id).await? {
        for other in item.of(to) {
            if !found.iter().any(|known| known == other) {
                found.push(other.to_string());
            }
        }
    }
    Ok(found)
}

/// [`others`], the first of them as the number a provider is asked by.
pub async fn other_number(state: &AppState, from: Site, id: &str, to: Site) -> Result<Option<i64>> {
    Ok(others(state, from, id, to)
        .await?
        .into_iter()
        .find_map(|other| other.parse().ok()))
}

/// The works held here an id of `site`'s leads to through the links, for
/// where the catalogue holds none under that id itself: those held under
/// another id of an item it is on. A work of another kind than the site's
/// ids name is not taken, nor one carrying an id of `site`'s that is not
/// this one — the links do not overrule what a work says. Each once, in the
/// order the items' ids come: TMDB's first, then TheTVDB's, IMDb's, the
/// anime sites', TVmaze's.
pub async fn held(state: &AppState, site: Site, id: &str) -> Result<Vec<MediaItem>> {
    let Some(asked) = site.number(id.trim()).map(|number| site.id(number)) else {
        return Ok(Vec::new());
    };
    let items = linked(state, site, &asked).await?;
    let found = held_among(state, &items, Some((site, &asked)), site.kind()).await?;
    Ok(found
        .into_iter()
        .filter(|work| !contradicts(work, site, &asked))
        .collect())
}

/// [`held`], by Wikidata's own id for the item: `Q1079`. A work carrying
/// another item's id is not taken.
pub async fn held_by_item(state: &AppState, id: &str) -> Result<Vec<MediaItem>> {
    let Some(item) = item(state, id).await? else {
        return Ok(Vec::new());
    };
    let found = held_among(state, std::slice::from_ref(&item), None, None).await?;
    Ok(found
        .into_iter()
        .filter(|work| {
            work.external_ids
                .wikidata
                .as_deref()
                .is_none_or(|held| held.eq_ignore_ascii_case(&item.id))
        })
        .collect())
}

/// The works held here a `/3/find` by another site's id leads to through
/// the links, the site as TMDB's `external_source` names it: `imdb_id`;
/// `tvdb_id`, TheTVDB's own numbers alone — one this server made leads to a
/// work held, or to none; `wikidata_id`. For where the catalogue holds no
/// work under the id itself: TMDB's relay asks it in place of TMDB, where it
/// answers from the catalogue first, and where TMDB knows no work by the id.
pub async fn held_for_find(
    state: &AppState,
    external_source: &str,
    external_id: &str,
) -> Result<Vec<MediaItem>> {
    let external_id = external_id.trim();
    match external_source {
        "imdb_id" => match crate::domain::ids::normalize_imdb_id(external_id) {
            Some(imdb) => held(state, Site::Imdb, &imdb).await,
            None => Ok(Vec::new()),
        },
        "tvdb_id" => match external_id.parse::<i64>() {
            Ok(id)
                if !ids::is_own(id)
                    && ids::from_fankai(id).is_none()
                    && ids::from_synthetic(id).is_none() =>
            {
                held(state, Site::TvdbSeries, &id.to_string()).await
            }
            _ => Ok(Vec::new()),
        },
        "wikidata_id" => held_by_item(state, external_id).await,
        _ => Ok(Vec::new()),
    }
}

/// The ids a work may be fetched by, from the links: TMDB's and TheTVDB's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fetchable {
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
}

/// The ids a work of `kind` asked by `site`'s `id` may be fetched by, for
/// where no provider says which work it is: the first TMDB id and the first
/// TheTVDB id of `kind` the links give.
///
/// None at all as soon as a work is held here under any id of the items the
/// links find — the one asked, or another — or a merge folded one of them
/// into a work: the links add nothing to a work held, and a fetch by one of
/// their ids would land in it. A fetch by TheTVDB's id, say, reads the TMDB
/// id TheTVDB names, finds the work held under it — a series TMDB alone
/// names, which another IMDb id kept [`held`] from — and writes TheTVDB's
/// id and numbering onto it: the id its clients hold it by would change.
/// The work fetched is always a new one, which no client holds yet.
pub async fn fetchable(
    state: &AppState,
    site: Site,
    id: &str,
    kind: MediaKind,
) -> Result<Fetchable> {
    let mut fetchable = Fetchable::default();
    let Some(asked) = site.number(id.trim()).map(|number| site.id(number)) else {
        return Ok(fetchable);
    };
    if !enabled(state) || taken(state, kind, site, &asked).await? {
        return Ok(fetchable);
    }

    let items = linked(state, site, &asked).await?;
    for item in &items {
        for (of, other) in &item.ids {
            if taken(state, kind, *of, other).await? {
                return Ok(Fetchable::default());
            }
        }
    }
    let (tmdb, tvdb) = match kind {
        MediaKind::Series => (Site::TmdbTv, Site::TvdbSeries),
        MediaKind::Movie => (Site::TmdbMovie, Site::TvdbMovie),
    };
    for (of, slot) in [(tmdb, &mut fetchable.tmdb), (tvdb, &mut fetchable.tvdb)] {
        *slot = items
            .iter()
            .flat_map(|item| item.of(of))
            .find_map(|other| other.parse::<i64>().ok());
    }
    Ok(fetchable)
}

/// Whether a work is held here under `site`'s `id`, or a merge folded the
/// id into one.
async fn taken(state: &AppState, kind: MediaKind, site: Site, id: &str) -> Result<bool> {
    let Some(source) = site.source() else {
        return Ok(false);
    };
    Ok(repo::item::find_id_by_external(&state.db, source, id)
        .await?
        .is_some()
        || service::merge::aliased(state, kind, source, id)
            .await?
            .is_some())
}

/// The works held under any id of `items` but `asked`, of `kind` where one
/// is given.
async fn held_among(
    state: &AppState,
    items: &[Item],
    asked: Option<(Site, &str)>,
    kind: Option<MediaKind>,
) -> Result<Vec<MediaItem>> {
    let mut seen: Vec<String> = Vec::new();
    let mut found = Vec::new();
    for item in items {
        for (site, id) in &item.ids {
            if asked == Some((*site, id.as_str())) {
                continue;
            }
            let Some(source) = site.source() else {
                continue;
            };
            let Some(media) = repo::item::find_id_by_external(&state.db, source, id).await? else {
                continue;
            };
            if seen.contains(&media) {
                continue;
            }
            seen.push(media.clone());
            if let Some(work) = service::load(state, &media).await?
                && kind.is_none_or(|kind| work.kind == kind)
                && !found.iter().any(|known: &MediaItem| known.id == work.id)
            {
                found.push(work);
            }
        }
    }
    Ok(found)
}

/// Whether a work carries an id of `site`'s other than `id`. A series split
/// into seasons is several entries of AniList's and MyAnimeList's, so one
/// more of those is no contradiction; the catalogue keeps none of AniDB's.
fn contradicts(work: &MediaItem, site: Site, id: &str) -> bool {
    let ids = &work.external_ids;
    let another = |held: Option<i64>| held.is_some_and(|held| held.to_string() != id);
    match site {
        Site::TmdbMovie | Site::TmdbTv => another(ids.tmdb),
        Site::TvdbSeries | Site::TvdbMovie => another(ids.tvdb),
        Site::TvMaze => another(ids.tvmaze),
        Site::Imdb => ids.imdb.as_deref().is_some_and(|held| held != id),
        Site::AniList | Site::Mal | Site::AniDb => false,
    }
}

/// The links, item by item, as items.
fn items(links: &[Link]) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut current = None;
    for link in links {
        let Some(site) = Site::of_property(link.property) else {
            continue;
        };
        if current != Some(link.item) {
            current = Some(link.item);
            items.push(Item {
                id: format!("Q{}", link.item),
                ids: Vec::new(),
            });
        }
        if let Some(item) = items.last_mut() {
            item.ids.push((site, site.id(link.value)));
        }
    }
    // By site, in a stable sort: each site's ids stay in their order.
    for item in &mut items {
        item.ids.sort_by_key(|(site, _)| *site);
    }
    items
}

/// The number of Wikidata's id for an item: `Q1079` is 1079.
fn item_number(id: &str) -> Option<u32> {
    let id = id.trim();
    let digits = id.strip_prefix('Q').or_else(|| id.strip_prefix('q'))?;
    if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<u32>()
        .ok()
        .filter(|&number| number > 0 && i32::try_from(number).is_ok())
}

/// Keep `links` as the links Wikidata gave, as a reading would: for the
/// tests of what reads them.
#[cfg(test)]
pub async fn keep_for_tests(state: &AppState, links: &[(Site, &str, u32)]) {
    let mut kept: Vec<Link> = links
        .iter()
        .map(|(site, id, item)| Link {
            property: site.property(),
            value: site.number(id).expect("an id of the site's"),
            item: *item,
        })
        .collect();
    kept.sort_unstable();
    kept.dedup();
    let change = repo::wikidata::compare(&state.db, &kept)
        .await
        .expect("compared");
    repo::wikidata::apply(&state.db, &change)
        .await
        .expect("kept");
    state
        .settings
        .set(
            crate::settings::Scope::Server,
            "",
            "wikidata.enabled",
            "true",
            None,
        )
        .await
        .expect("switched on");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::ExternalIds,
        service::testing::{overdue, server},
    };

    /// *Breaking Bad* and *Inception*, as Wikidata files them, and a TMDB
    /// film Wikidata has on two items.
    async fn links(state: &AppState) {
        keep_for_tests(
            state,
            &[
                (Site::TmdbTv, "1396", 1079),
                (Site::TvdbSeries, "81189", 1079),
                (Site::Imdb, "tt0903747", 1079),
                (Site::TvMaze, "169", 1079),
                (Site::TmdbMovie, "27205", 25188),
                (Site::TvdbMovie, "113", 25188),
                (Site::Imdb, "tt1375666", 25188),
                (Site::TmdbMovie, "13635", 2_063_364),
                (Site::Imdb, "tt1259014", 2_063_364),
                (Site::TmdbMovie, "13635", 138_841_612),
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn an_id_leads_to_the_others_of_its_item_and_nothing_while_switched_off() {
        let (state, _) = server().await;
        links(&state).await;

        let found = linked(&state, Site::Imdb, "tt0903747").await.expect("read");
        assert_eq!(
            found,
            vec![Item {
                id: "Q1079".into(),
                ids: vec![
                    (Site::TmdbTv, "1396".into()),
                    (Site::TvdbSeries, "81189".into()),
                    (Site::Imdb, "tt0903747".into()),
                    (Site::TvMaze, "169".into()),
                ],
            }]
        );
        assert_eq!(
            others(&state, Site::TvdbSeries, "81189", Site::TmdbTv)
                .await
                .expect("read"),
            vec!["1396"]
        );
        assert_eq!(
            other_number(&state, Site::TmdbTv, "1396", Site::TvdbSeries)
                .await
                .expect("read"),
            Some(81189)
        );
        assert_eq!(
            item(&state, "Q25188").await.expect("read").map(|item| item
                .of(Site::TvdbMovie)
                .map(str::to_string)
                .collect::<Vec<_>>()),
            Some(vec!["113".to_string()])
        );
        // Both items a duplicated id is on.
        assert_eq!(
            linked(&state, Site::TmdbMovie, "13635")
                .await
                .expect("read")
                .len(),
            2
        );
        // Nothing for what is no id of the site's, nor an id nobody files.
        for (site, id) in [
            (Site::Imdb, "903747"),
            (Site::TmdbTv, "breaking-bad"),
            (Site::TmdbTv, "1397"),
        ] {
            assert!(linked(&state, site, id).await.expect("read").is_empty());
        }
        assert_eq!(item(&state, "P345").await.expect("read"), None);

        state
            .settings
            .set(
                crate::settings::Scope::Server,
                "",
                "wikidata.enabled",
                "false",
                None,
            )
            .await
            .expect("switched off");
        assert!(
            linked(&state, Site::Imdb, "tt0903747")
                .await
                .expect("read")
                .is_empty()
        );
        assert_eq!(item(&state, "Q1079").await.expect("read"), None);
    }

    #[tokio::test]
    async fn a_held_work_is_found_by_another_id_of_its_item_unless_it_says_otherwise() {
        let (state, _) = server().await;
        links(&state).await;

        // Held under TheTVDB's id alone: found by IMDb's, TMDB's, the item's.
        let series = overdue(
            &state.db,
            MediaKind::Series,
            "Bridged Breaking Bad",
            ExternalIds {
                tvdb: Some(81189),
                ..Default::default()
            },
            true,
        )
        .await;
        for (site, id) in [(Site::Imdb, "tt0903747"), (Site::TmdbTv, "1396")] {
            let found = held(&state, site, id).await.expect("read");
            assert_eq!(
                found.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
                vec![series.id.as_str()],
                "{site:?}"
            );
        }
        assert_eq!(
            held_by_item(&state, "Q1079")
                .await
                .expect("read")
                .iter()
                .map(|w| w.id.as_str())
                .collect::<Vec<_>>(),
            vec![series.id.as_str()]
        );
        // Not by TMDB's id of a film, whatever the item says.
        assert!(
            held(&state, Site::TmdbMovie, "1396")
                .await
                .expect("read")
                .is_empty()
        );

        // A film held with another IMDb id than the one asked: the links do
        // not overrule it.
        overdue(
            &state.db,
            MediaKind::Movie,
            "Bridged Inception",
            ExternalIds {
                tmdb: Some(27205),
                imdb: Some("tt9999999".into()),
                ..Default::default()
            },
            true,
        )
        .await;
        assert!(
            held(&state, Site::Imdb, "tt1375666")
                .await
                .expect("read")
                .is_empty()
        );
        let found = held(&state, Site::TvdbMovie, "113").await.expect("read");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Bridged Inception");
    }

    #[tokio::test]
    async fn a_find_by_another_sites_id_reaches_the_work_held_through_the_links() {
        let (state, _) = server().await;
        links(&state).await;
        // Held under TMDB's id alone.
        let series = overdue(
            &state.db,
            MediaKind::Series,
            "Found Breaking Bad P5",
            ExternalIds {
                tmdb: Some(1396),
                ..Default::default()
            },
            true,
        )
        .await;
        for (source, id) in [
            ("imdb_id", "tt0903747"),
            ("imdb_id", "0903747"),
            ("tvdb_id", "81189"),
            ("wikidata_id", "Q1079"),
        ] {
            let found = held_for_find(&state, source, id).await.expect("read");
            assert_eq!(
                found.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
                [series.id.as_str()],
                "{source} {id}"
            );
        }
        // A number this server made is no id of TheTVDB's; TVRage's is not
        // read.
        let made = ids::to_synthetic(1396).expect("a number").to_string();
        for (source, id) in [("tvdb_id", made.as_str()), ("tvrage_id", "18164")] {
            assert!(
                held_for_find(&state, source, id)
                    .await
                    .expect("read")
                    .is_empty(),
                "{source} {id}"
            );
        }
    }

    #[tokio::test]
    async fn the_ids_to_fetch_by_are_never_those_of_a_work_held() {
        let (state, _) = server().await;
        keep_for_tests(
            &state,
            &[
                (Site::Imdb, "tt9906010", 990_610),
                (Site::TmdbTv, "990611", 990_610),
                (Site::TvdbSeries, "990612", 990_610),
                (Site::Imdb, "tt9906020", 990_620),
                (Site::TmdbTv, "990621", 990_620),
                (Site::TvdbSeries, "990622", 990_620),
            ],
        )
        .await;
        let both = Fetchable {
            tmdb: Some(990_611),
            tvdb: Some(990_612),
        };
        assert_eq!(
            fetchable(&state, Site::Imdb, "tt9906010", MediaKind::Series)
                .await
                .expect("read"),
            both
        );
        // No film's.
        assert_eq!(
            fetchable(&state, Site::Imdb, "tt9906010", MediaKind::Movie)
                .await
                .expect("read"),
            Fetchable::default()
        );

        // A series held under TMDB's id alone: nothing at all — a fetch by
        // TheTVDB's id would read TMDB's on TheTVDB's record, land in that
        // series and give it TheTVDB's id.
        overdue(
            &state.db,
            MediaKind::Series,
            "Fetchable TMDB alone P5",
            ExternalIds {
                tmdb: Some(990_621),
                ..Default::default()
            },
            true,
        )
        .await;
        assert_eq!(
            fetchable(&state, Site::Imdb, "tt9906020", MediaKind::Series)
                .await
                .expect("read"),
            Fetchable::default()
        );

        // A work held under the id asked: the links add nothing to it.
        overdue(
            &state.db,
            MediaKind::Series,
            "Fetchable held by IMDb P5",
            ExternalIds {
                imdb: Some("tt9906010".into()),
                ..Default::default()
            },
            true,
        )
        .await;
        assert_eq!(
            fetchable(&state, Site::Imdb, "tt9906010", MediaKind::Series)
                .await
                .expect("read"),
            Fetchable::default()
        );

        // Switched off: nothing.
        state
            .settings
            .set(
                crate::settings::Scope::Server,
                "",
                "wikidata.enabled",
                "false",
                None,
            )
            .await
            .expect("switched off");
        assert_eq!(
            fetchable(&state, Site::Imdb, "tt9906020", MediaKind::Series)
                .await
                .expect("read"),
            Fetchable::default()
        );
    }
}

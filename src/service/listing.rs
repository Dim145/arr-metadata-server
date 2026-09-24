//! What each work is listed by, kept current.
//!
//! A list is narrowed and ordered by a work's values as the catalogue shows
//! them — its locks applied, IMDb's list laid over its scores, the score its
//! card leads with — and those are not what the providers' columns hold. So
//! each work carries them beside its own (see [`repo::item::Listed`]), written
//! here from the work read the way its page reads it, by the same functions.
//!
//! Anything that changes one of them counts a change to the work: a write to
//! it, a lock set or lifted, IMDb's list replaced. A write lists its work again
//! straight away; `jobs::listing` lists whatever is left behind — every work
//! after an upgrade that changed the way, the works IMDb's daily list rescored,
//! all of them when IMDb is switched on or off, and any a write could not.

use anyhow::Result;

use crate::{
    db::{Db, repo},
    domain::fields,
    state::AppState,
};

/// The way works are listed. Raise it when [`repo::item::Listed::of`] changes
/// what it derives, and every work is listed again.
pub const VERSION: i64 = 1;

/// How many works are read and written at a time.
const BATCH: usize = 200;

/// Whether IMDb's list is laid over the scores, as it is on every page.
pub fn imdb_on(state: &AppState) -> bool {
    state.flag("imdb.enabled", false)
}

/// List these works again, each as it reads now. Returns how many were
/// written: a work changed again while it was being read is left for the next
/// pass rather than written with what it held before.
pub async fn relist(db: &Db, imdb: bool, ids: &[String]) -> Result<u64> {
    let mut written = 0;

    for chunk in ids.chunks(BATCH) {
        match relist_batch(db, imdb, chunk).await {
            Ok(n) => written += n,
            // One work that cannot be read or written fails the batch it is in:
            // the others are listed one by one, and that one is named and
            // passed over rather than allowed to hold them all back.
            Err(first) if chunk.len() > 1 => {
                let mut failed = None;
                let mut any = false;

                for id in chunk {
                    match relist_batch(db, imdb, std::slice::from_ref(id)).await {
                        Ok(n) => {
                            written += n;
                            any = true;
                        }
                        Err(e) => {
                            tracing::warn!(
                                id,
                                error = format_args!("{e:#}"),
                                "a work could not be listed"
                            );
                            failed = Some(e);
                        }
                    }
                }

                // Not one of them could be: the database, not a work.
                if !any {
                    return Err(failed.unwrap_or(first));
                }
            }
            Err(e) => return Err(e),
        }
    }

    Ok(written)
}

async fn relist_batch(db: &Db, imdb: bool, chunk: &[String]) -> Result<u64> {
    // The changes first, then the works: a change landing between the two is
    // then not taken for one that was listed.
    let changes = repo::item::listed_changes(db, chunk).await?;
    let mut items = repo::item::by_ids(db, chunk).await?;
    repo::item::attach_ratings(db, &mut items).await?;

    let found: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let locks = repo::override_field::list_for_many(db, &found).await?;

    let imdb_listed = if imdb {
        let tconsts: Vec<String> = items
            .iter()
            .filter_map(|i| i.external_ids.imdb.clone())
            .collect();
        repo::imdb::get_many(db, &tconsts).await?
    } else {
        Default::default()
    };

    let mut listed = Vec::with_capacity(items.len());
    for mut item in items {
        let Some(change) = changes.get(&item.id).copied() else {
            continue;
        };

        // As `service::load` and `apply_overrides` read it, so the list and
        // the page cannot disagree about what a work is.
        if let Some(locks) = locks.get(&item.id)
            && let Err(e) = fields::apply(&mut item, locks)
        {
            tracing::warn!(
                id = %item.id,
                error = %e,
                "a lock could not be applied; listing the work by its providers' values"
            );
        }
        if let Some(rating) = item
            .external_ids
            .imdb
            .as_deref()
            .and_then(|t| imdb_listed.get(t))
        {
            super::take_newer_imdb(&mut item.ratings, rating);
        }

        listed.push((item.id.clone(), change, repo::item::Listed::of(&item)));
    }

    repo::item::write_listed(db, &listed, imdb, VERSION).await
}

/// List again every work whose listed values are behind, a batch at a time,
/// up to about `most` of them, walking them in id order from the start.
/// Returns how many were listed, and whether it stopped with more to see.
pub async fn relist_stale(db: &Db, imdb: bool, most: usize) -> Result<(u64, bool)> {
    let mut listed = 0;
    let mut seen = 0;
    let mut after: Option<String> = None;

    while seen < most {
        let ids = repo::item::stale_ids(db, VERSION, imdb, after.as_deref(), BATCH as i64).await?;
        let Some(last) = ids.last().cloned() else {
            return Ok((listed, false));
        };

        seen += ids.len();
        listed += relist(db, imdb, &ids).await?;
        // Onwards whatever became of these: one changed again while it was
        // read is met on the next pass, one that cannot be written is not met
        // again on this one.
        after = Some(last);
    }

    Ok((listed, true))
}

/// List a work again straight after a write to it, so its page and its place
/// in the list agree at once.
///
/// Never fails the write: the write counted a change, so a work this could
/// not list is listed by `jobs::listing` shortly after.
pub async fn after_write(state: &AppState, id: &str) {
    if let Err(e) = relist(&state.db, imdb_on(state), &[id.to_string()]).await {
        tracing::warn!(
            id,
            error = format_args!("{e:#}"),
            "could not list a work again after a write; it will be shortly"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config,
        db::repo::item::{Query, Sort},
        domain::{ExternalIds, MediaItem, MediaKind, Rating, fields::Scope},
    };

    async fn db() -> Db {
        let db = Db::connect(&config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .expect("in-memory database");

        db.migrate().await.expect("migrations");
        db
    }

    async fn stored(db: &Db, adjust: impl FnOnce(&mut MediaItem)) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.id = crate::db::new_id();
        item.title = "Untitled".into();
        item.created_at = crate::db::now();
        item.updated_at = crate::db::now();
        adjust(&mut item);
        item.slug = crate::domain::make_slug(&item.title, item.year);

        repo::item::upsert(
            db,
            repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");

        item
    }

    async fn lock(db: &Db, id: &str, field: &str, value: serde_json::Value) {
        repo::override_field::set(db, id, Scope::Item, field, Some(&value), None)
            .await
            .expect("locked");
    }

    async fn titles(db: &Db, query: Query) -> Vec<String> {
        // A page, not the one row a zero limit is clamped to.
        let query = Query { limit: 50, ..query };
        repo::item::search(db, &query)
            .await
            .expect("searched")
            .into_iter()
            .map(|i| i.title)
            .collect()
    }

    fn rating(source: &str, value: f64, votes: i64) -> Rating {
        Rating {
            source: source.into(),
            value: Some(value),
            votes: Some(votes),
            rating_type: Some("user".into()),
        }
    }

    #[tokio::test]
    async fn a_new_work_is_in_every_filter_before_it_is_listed_again() {
        let db = db().await;
        stored(&db, |i| {
            i.title = "Fresh".into();
            i.genres = vec!["Drama".into()];
            i.year = Some(2008);
        })
        .await;

        let found = titles(
            &db,
            Query {
                genres: vec!["Drama".into()],
                year_from: Some(2008),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            found,
            ["Fresh"],
            "listed by its own values from its first write"
        );
    }

    #[tokio::test]
    async fn a_locked_genre_and_year_are_what_the_list_filters_and_counts_by() {
        let db = db().await;
        let work = stored(&db, |i| {
            i.title = "Relabelled".into();
            i.genres = vec!["Animation".into(), "Comedy".into()];
            i.year = Some(2008);
        })
        .await;

        lock(
            &db,
            &work.id,
            "genres",
            serde_json::json!(["Anime", "Comedy"]),
        )
        .await;
        lock(&db, &work.id, "year", serde_json::json!(2010)).await;

        // A lock counts a change, so the work is behind until it is listed.
        let behind = repo::item::stale_ids(&db, VERSION, false, None, 10)
            .await
            .unwrap();
        assert!(behind.contains(&work.id));
        assert_eq!(relist_stale(&db, false, 100).await.unwrap().0, 1);

        let by = |genres: &[&str], year: Option<i32>| Query {
            genres: genres.iter().map(|g| g.to_string()).collect(),
            year_from: year,
            year_to: year,
            ..Default::default()
        };
        assert_eq!(titles(&db, by(&["Anime"], None)).await, ["Relabelled"]);
        assert!(titles(&db, by(&["Animation"], None)).await.is_empty());
        assert_eq!(titles(&db, by(&[], Some(2010))).await, ["Relabelled"]);
        assert!(titles(&db, by(&[], Some(2008))).await.is_empty());

        let facets = repo::item::facets(&db, &Query::default()).await.unwrap();
        assert!(facets.genres.iter().any(|g| g.value == "Anime"));
        assert!(facets.genres.iter().all(|g| g.value != "Animation"));
        assert_eq!((facets.year_min, facets.year_max), (Some(2010), Some(2010)));

        // Nothing left to do.
        assert_eq!(relist_stale(&db, false, 100).await.unwrap().0, 0);
    }

    #[tokio::test]
    async fn a_locked_title_orders_the_list() {
        let db = db().await;
        stored(&db, |i| i.title = "Middle".into()).await;
        let renamed = stored(&db, |i| i.title = "Zebra".into()).await;
        stored(&db, |i| i.title = "Étoile".into()).await;

        lock(&db, &renamed.id, "title", serde_json::json!("Aardvark")).await;
        relist_stale(&db, false, 100).await.unwrap();

        let ordered = titles(
            &db,
            Query {
                sort: Sort::Title,
                ..Default::default()
            },
        )
        .await;
        // By the title each is shown under: Zebra is Aardvark now, and the
        // accent no longer sends Étoile past every letter. (A search returns
        // the stored titles; the order is what is under test.)
        assert_eq!(ordered, ["Zebra", "Étoile", "Middle"]);
    }

    #[tokio::test]
    async fn imdbs_list_scores_a_work_while_it_is_switched_on() {
        let db = db().await;
        stored(&db, |i| {
            i.title = "Rescored".into();
            i.external_ids = ExternalIds {
                imdb: Some("tt0000001".into()),
                ..Default::default()
            };
            i.ratings = vec![rating("tmdb", 7.2, 3_000)];
        })
        .await;
        stored(&db, |i| {
            i.title = "Steady".into();
            i.ratings = vec![rating("tmdb", 6.8, 900)];
        })
        .await;

        repo::imdb::replace_all(
            &db,
            &[repo::imdb::Rating {
                tconst: "tt0000001".into(),
                rating: 6.1,
                votes: 40_000,
            }],
        )
        .await
        .unwrap();
        repo::item::mark_changed_by_imdb(&db, &["tt0000001".to_string()])
            .await
            .unwrap();

        let rated = |min: f64| Query {
            min_rating: Some(min),
            sort: Sort::Rating,
            ..Default::default()
        };

        // Off: the stored 7.2 stands, as on the work's card.
        relist_stale(&db, false, 100).await.unwrap();
        assert_eq!(titles(&db, rated(7.0)).await, ["Rescored"]);

        // On: every work is behind until listed with IMDb's figure, which
        // is the card's figure then, and is ranked by it.
        assert!(
            !repo::item::stale_ids(&db, VERSION, true, None, 10)
                .await
                .unwrap()
                .is_empty()
        );
        relist_stale(&db, true, 100).await.unwrap();
        assert!(titles(&db, rated(7.0)).await.is_empty());
        assert_eq!(titles(&db, rated(6.0)).await, ["Steady", "Rescored"]);
    }

    #[test]
    fn a_nul_in_a_lock_is_left_out_of_what_the_work_is_listed_by() {
        // PostgreSQL will not store one; written as it came, it failed every
        // batch the work was in.
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Na\0me".into();
        item.network = Some("A\0B".into());
        item.genres = vec!["Dra\0ma".into()];

        let listed = repo::item::Listed::of(&item);
        assert_eq!(listed.network.as_deref(), Some("AB"));
        assert_eq!(listed.genres, ["Drama"]);
        assert!(!listed.title.contains('\0'));
    }

    #[tokio::test]
    async fn a_work_that_loses_its_imdb_id_to_another_is_listed_again() {
        let db = db().await;
        let imdb = || ExternalIds {
            imdb: Some("tt0000002".into()),
            ..Default::default()
        };
        let first = stored(&db, |i| {
            i.title = "First".into();
            i.external_ids = imdb();
        })
        .await;
        relist_stale(&db, true, 100).await.unwrap();
        assert!(
            repo::item::stale_ids(&db, VERSION, true, None, 10)
                .await
                .unwrap()
                .is_empty()
        );

        // Another entry for the same title claims the id; the first loses
        // IMDb's figure with it, and has to be listed without it.
        stored(&db, |i| {
            i.title = "Second".into();
            i.external_ids = imdb();
        })
        .await;

        let behind = repo::item::stale_ids(&db, VERSION, true, None, 10)
            .await
            .unwrap();
        assert!(behind.contains(&first.id));
    }

    #[tokio::test]
    async fn stale_works_are_walked_from_a_cursor_in_id_order() {
        let db = db().await;
        let mut ids = Vec::new();
        for n in 0..5 {
            ids.push(stored(&db, |i| i.title = format!("Work {n}")).await.id);
        }
        ids.sort();

        let first = repo::item::stale_ids(&db, VERSION, false, None, 2)
            .await
            .unwrap();
        assert_eq!(first, ids[..2]);
        let next = repo::item::stale_ids(&db, VERSION, false, Some(&first[1]), 2)
            .await
            .unwrap();
        assert_eq!(next, ids[2..4]);

        // A pass stops at its limit, having listed a whole batch, and says
        // there may be more behind it; the next finds the rest.
        assert_eq!(relist_stale(&db, false, 1).await.unwrap(), (5, true));
        assert_eq!(relist_stale(&db, false, 100).await.unwrap(), (0, false));
    }

    #[tokio::test]
    async fn a_change_made_while_a_work_was_read_leaves_it_to_be_listed_again() {
        let db = db().await;
        let work = stored(&db, |i| {
            i.title = "Busy".into();
            i.genres = vec!["Drama".into()];
        })
        .await;

        let changes = repo::item::listed_changes(&db, std::slice::from_ref(&work.id))
            .await
            .unwrap();
        let read = changes[&work.id];

        // Somebody locks a genre after the work was read, before it is written.
        lock(&db, &work.id, "genres", serde_json::json!(["Crime"])).await;

        let stale = repo::item::Listed {
            title: "busy".into(),
            genres: vec!["Drama".into()],
            ..Default::default()
        };
        let written =
            repo::item::write_listed(&db, &[(work.id.clone(), read, stale)], false, VERSION)
                .await
                .unwrap();

        assert_eq!(written, 0, "what it held before the lock is not written");
        assert!(
            repo::item::stale_ids(&db, VERSION, false, None, 10)
                .await
                .unwrap()
                .contains(&work.id)
        );

        relist_stale(&db, false, 100).await.unwrap();
        assert_eq!(
            titles(
                &db,
                Query {
                    genres: vec!["Crime".into()],
                    ..Default::default()
                }
            )
            .await,
            ["Busy"]
        );
    }
}

//! The popularity of the works held here, from TVmaze's weight and the change
//! in IMDb's votes (wave P, P6): worked out again once IMDb's ratings or
//! TVmaze's index have landed, and once a day besides, by the lists'
//! scheduler (`jobs::datasets`), on the instance that leads. See
//! `service::popularity` for what the figure is.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;

use crate::{
    db::{
        now,
        repo::{
            self,
            popularity::{Held, WeightFrom, Worked},
            tvmaze::Key,
        },
    },
    domain::MediaKind,
    service::popularity::{Scale, Signals, figure, votes_per_day},
    state::AppState,
};

/// How often the popularity is worked out again, besides after the lists
/// that move it.
pub const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Work the popularity out again, saying in the log how it went: what moves
/// it never fails over it.
pub async fn work_out_logged(state: &AppState) {
    match work_out(state).await {
        Ok(summary) => tracing::info!(summary, "the popularity of the works held was worked out"),
        Err(e) => tracing::warn!(
            error = format_args!("{e:#}"),
            "could not work the popularity of the works held out; the figures kept stay"
        ),
    }
}

/// Work the popularity of every work held out again, from what is switched
/// on — TVmaze's index (`tvmaze.index`), TVmaze's answers (`tvmaze.enabled`),
/// IMDb's list (`imdb.enabled`) — and keep it: a work none of them says
/// anything of has none. What it came to.
pub async fn work_out(state: &AppState) -> Result<String> {
    let held = repo::popularity::held(&state.db).await?;
    let before = repo::popularity::worked(&state.db).await?;
    let series: Vec<&Held> = held
        .iter()
        .filter(|work| work.kind == MediaKind::Series.as_str())
        .collect();

    // TVmaze's weight of each series: the index's, while it is kept — by
    // TVmaze's id, else TheTVDB's, else IMDb's.
    let mut weights: HashMap<&str, (i32, WeightFrom)> = HashMap::new();
    if state.flag("tvmaze.index", false) {
        let ids = |of: fn(&Held) -> Option<i64>| -> Vec<i64> {
            series.iter().filter_map(|work| of(work)).collect()
        };
        let by_tvmaze = repo::tvmaze::by(&state.db, Key::Tvmaze, &ids(|w| w.tvmaze)).await?;
        let by_tvdb = repo::tvmaze::by(&state.db, Key::Tvdb, &ids(|w| w.tvdb)).await?;
        let tconsts: Vec<String> = series.iter().filter_map(|w| w.imdb.clone()).collect();
        let by_imdb = repo::tvmaze::by_imdb(&state.db, &tconsts).await?;
        for work in &series {
            let show = work
                .tvmaze
                .and_then(|id| by_tvmaze.get(&id))
                .or_else(|| work.tvdb.and_then(|id| by_tvdb.get(&id)))
                .or_else(|| work.imdb.as_ref().and_then(|id| by_imdb.get(id)));
            if let Some(weight) = show.and_then(|show| show.weight) {
                weights.insert(&work.id, (weight, WeightFrom::Index));
            }
        }
    }

    // Else the one in TVmaze's own answer for the series, while TVmaze is
    // asked: read again only from an answer fetched since it was last read.
    let mut answers: HashMap<String, (Option<i32>, String)> = HashMap::new();
    if state.flag("tvmaze.enabled", false) {
        let fetched = repo::popularity::answered_at(&state.db).await?;
        let mut unread: Vec<String> = Vec::new();
        for work in &series {
            if weights.contains_key(work.id.as_str()) {
                continue;
            }
            let Some(at) = fetched.get(&work.id) else {
                continue;
            };
            match before.get(&work.id) {
                Some(kept) if kept.answer_at.as_deref() == Some(at.as_str()) => {
                    answers.insert(work.id.clone(), (kept.answer_weight, at.clone()));
                }
                _ => unread.push(work.id.clone()),
            }
        }
        answers.extend(repo::popularity::answer_weights(&state.db, &unread).await?);
        for (id, (weight, _)) in &answers {
            if let Some(weight) = weight {
                weights
                    .entry(id.as_str())
                    .or_insert((*weight, WeightFrom::Answer));
            }
        }
    }

    // The votes IMDb's list gave each work a day, while the list is read.
    let votes: HashMap<String, f64> = if state.flag("imdb.enabled", false) {
        repo::imdb::votes(&state.db)
            .await?
            .into_iter()
            .filter_map(|(tconst, days)| Some((tconst, votes_per_day(&days)?)))
            .collect()
    } else {
        HashMap::new()
    };

    // Each work's figure, then each figure on TMDB's scale.
    let asked = state.flag("tvmaze.enabled", false);
    let mut worked: Vec<(Worked, Option<f64>, MediaKind)> = Vec::new();
    for work in &held {
        let kind = work.kind.parse().unwrap_or(MediaKind::Series);
        let weight = weights.get(work.id.as_str()).copied();
        let votes_per_day = work.imdb.as_ref().and_then(|t| votes.get(t)).copied();
        // The weight of TVmaze's answer, kept for when the index has none:
        // read now, or as it was last read while the index gave it.
        let (answer_weight, answer_at) = match answers.get(&work.id) {
            Some((weight, at)) => (*weight, Some(at.clone())),
            None if asked => before
                .get(&work.id)
                .map(|kept| (kept.answer_weight, kept.answer_at.clone()))
                .unwrap_or_default(),
            None => (None, None),
        };
        if weight.is_none() && votes_per_day.is_none() && answer_at.is_none() {
            continue;
        }
        let signals = Signals {
            weight: weight.map(|(weight, _)| weight),
            votes_per_day,
        };
        worked.push((
            Worked {
                media_id: work.id.clone(),
                weight,
                answer_weight,
                answer_at,
                votes_per_day,
            },
            figure(signals),
            kind,
        ));
    }
    let scale = Scale::new(
        worked
            .iter()
            .filter_map(|(_, figure, kind)| Some((*kind, (*figure)?))),
        held.iter().filter_map(|work| {
            Some((
                work.kind.parse().unwrap_or(MediaKind::Series),
                work.popularity?,
            ))
        }),
    );

    // Where it moved; kept only for the works TMDB gives none, the others
    // sorting on TMDB's.
    let placed: HashMap<&str, f64> = worked
        .iter()
        .filter_map(|(work, figure, kind)| {
            Some((work.media_id.as_str(), scale.place(*kind, (*figure)?)))
        })
        .collect();
    let moved: Vec<(String, Option<f64>)> = held
        .iter()
        .filter_map(|work| {
            let now = placed
                .get(work.id.as_str())
                .copied()
                .filter(|_| work.popularity.is_none());
            let same = match (work.local, now) {
                (Some(a), Some(b)) => (a - b).abs() < 1e-9,
                (None, None) => true,
                _ => false,
            };
            (!same).then(|| (work.id.clone(), now))
        })
        .collect();
    let figured = placed.len();

    let rows: Vec<Worked> = worked.into_iter().map(|(work, _, _)| work).collect();
    let moved = repo::popularity::write(&state.db, &rows, &moved, &now()).await?;
    if moved > 0 {
        // A work kept in memory carries the figure it was read with, and a
        // list kept the order it was in.
        state.caches.invalidate_all().await;
    }

    let by_weight = rows.iter().filter(|work| work.weight.is_some()).count();
    let by_votes = rows
        .iter()
        .filter(|work| work.votes_per_day.is_some())
        .count();
    let without_tmdb = held.iter().filter(|work| work.popularity.is_none()).count();
    Ok(format!(
        "{figured} of {} works: {by_weight} by TVmaze's weight, {by_votes} by IMDb's votes; \
         {moved} of the {without_tmdb} without TMDB's moved",
        held.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::RowExt,
        domain::{MediaItem, MediaKind},
        providers::tvmaze::IndexShow,
        settings::registry::Scope,
    };

    /// A work held, shaped as the test needs it.
    async fn held(
        state: &AppState,
        kind: MediaKind,
        title: &str,
        shape: impl FnOnce(&mut MediaItem),
    ) -> MediaItem {
        let mut item = MediaItem::empty(kind);
        item.id = crate::db::new_id();
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        item.created_at = now();
        item.updated_at = now();
        shape(&mut item);
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        item
    }

    async fn switch(state: &AppState, keys: &[&str], on: bool) {
        for key in keys {
            state
                .settings
                .set(
                    Scope::Server,
                    "",
                    key,
                    if on { "true" } else { "false" },
                    None,
                )
                .await
                .expect("switched");
        }
    }

    #[tokio::test]
    async fn a_work_tmdb_gives_no_popularity_is_given_one_here_and_sorted_by_it() {
        let state = crate::api::native::testing::server().await;
        switch(&state, &["tvmaze.index", "imdb.enabled"], true).await;
        switch(&state, &["tvmaze.enabled"], false).await;

        let followed = held(&state, MediaKind::Series, "Kestrel Moor Watch", |work| {
            work.external_ids.tvmaze = Some(880_001);
        })
        .await;
        let known = held(&state, MediaKind::Series, "Kestrel Moor Known", |work| {
            work.popularity = Some(50.0);
            work.external_ids.tvdb = Some(990_002);
        })
        .await;
        let voted = held(&state, MediaKind::Movie, "Kestrel Moor Voted", |work| {
            work.external_ids.imdb = Some("tt8810003".into());
        })
        .await;
        let quiet = held(&state, MediaKind::Movie, "Kestrel Moor Quiet", |work| {
            work.popularity = Some(5.0);
        })
        .await;

        repo::tvmaze::write(
            &state.db,
            &[
                IndexShow {
                    id: 880_001,
                    name: "Kestrel Moor Watch".into(),
                    weight: Some(90),
                    ..IndexShow::default()
                },
                IndexShow {
                    id: 880_002,
                    name: "Kestrel Moor Known".into(),
                    tvdb_id: Some(990_002),
                    weight: Some(10),
                    ..IndexShow::default()
                },
            ],
        )
        .await
        .unwrap();
        repo::imdb::keep_votes(&state.db, "2026-10-26", &[("tt8810003".into(), 1_000)])
            .await
            .unwrap();
        repo::imdb::keep_votes(&state.db, "2026-11-02", &[("tt8810003".into(), 1_700)])
            .await
            .unwrap();

        assert_eq!(
            work_out(&state).await.unwrap(),
            "3 of 4 works: 2 by TVmaze's weight, 1 by IMDb's votes; \
             2 of the 2 without TMDB's moved"
        );

        // Too few of TMDB's figures to go by: each figure times ten — the
        // weight of 90, the hundred votes a day. A work with TMDB's keeps it.
        let ids = [&followed, &known, &voted, &quiet].map(|work| work.id.clone());
        let local = repo::popularity::local_of(&state.db, &ids).await.unwrap();
        assert_eq!(local.get(&followed.id), Some(&90.0));
        let by_votes = local[&voted.id];
        assert!((46.0..46.3).contains(&by_votes), "{by_votes}");
        assert_eq!(local.len(), 2);
        let worked = repo::popularity::worked(&state.db).await.unwrap();
        assert_eq!(
            worked[&known.id].weight,
            Some((10, repo::popularity::WeightFrom::Index))
        );
        assert_eq!(worked[&voted.id].votes_per_day, Some(100.0));

        // Served with TMDB's where it has one, else this one.
        let read = |id: String| {
            let state = state.clone();
            async move {
                crate::service::load(&state, &id)
                    .await
                    .unwrap()
                    .unwrap()
                    .popularity
            }
        };
        assert_eq!(read(followed.id.clone()).await, Some(90.0));
        assert_eq!(read(known.id.clone()).await, Some(50.0));
        let mut listed = vec![voted.clone(), quiet.clone()];
        listed.iter_mut().for_each(|work| work.popularity = None);
        listed[1].popularity = Some(5.0);
        crate::service::overlay_imdb_many(&state, &mut listed).await;
        assert_eq!(listed[0].popularity, Some(by_votes));
        assert_eq!(listed[1].popularity, Some(5.0));

        // The catalogue in the order of whichever each has.
        let order: Vec<String> = repo::item::search(
            &state.db,
            &repo::item::Query {
                include_disabled: true,
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .into_iter()
        .map(|work| work.title)
        .collect();
        assert_eq!(
            order,
            [
                "Kestrel Moor Watch",
                "Kestrel Moor Known",
                "Kestrel Moor Voted",
                "Kestrel Moor Quiet"
            ]
        );

        // And the search index ranks a work by it.
        repo::search::catch_up(&state.db, 100).await.unwrap();
        let row =
            sqlx::query(state.db.sql(
                "SELECT popularity FROM search_doc WHERE work_id = ? AND kind = 'work' LIMIT 1",
            ))
            .bind(&followed.id)
            .fetch_one(state.db.pool())
            .await
            .unwrap();
        assert_eq!(row.opt_real("popularity").unwrap(), Some(90.0));

        // Switched off, nothing stands in for TMDB's.
        switch(&state, &["tvmaze.index", "imdb.enabled"], false).await;
        assert_eq!(
            work_out(&state).await.unwrap(),
            "0 of 4 works: 0 by TVmaze's weight, 0 by IMDb's votes; \
             2 of the 2 without TMDB's moved"
        );
        assert!(
            repo::popularity::local_of(&state.db, &ids)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(read(followed.id.clone()).await, None);
    }
}

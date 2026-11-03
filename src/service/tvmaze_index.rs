//! A search of TVmaze's index, for when the providers do not answer (wave P,
//! P3).
//!
//! The index is TVmaze's list of every series it knows — ninety thousand and
//! more — kept by each instance once `tvmaze.index` is on (see
//! `jobs::tvmaze_index`). Searched here by name, as the catalogue's own search
//! folds and ranks names (`crate::search`), and by the ids TVmaze files each
//! series under elsewhere: what a series search can still answer with, its
//! TheTVDB and IMDb ids among it, when TheTVDB and Skyhook cannot. Read from
//! the database alone; TVmaze is never asked while a client waits.

use std::collections::HashSet;

use anyhow::Result;

use crate::{
    db::repo::tvmaze::{self, Key},
    providers::tvmaze::IndexShow,
    search::{Term, rank},
    state::AppState,
};

pub use crate::providers::tvmaze::IndexShow as Show;

/// How many series each read of the index brings back to be ranked: enough
/// that the one a term means is among them, however many share its first
/// letters.
const POOL: i64 = 200;

/// Whether the index is kept, and so searched: `tvmaze.index`. Switched off,
/// what was kept is no longer read, whatever the table still holds.
pub fn is_on(state: &AppState) -> bool {
    state.flag("tvmaze.index", false)
}

/// The series of TVmaze's index whose name answers `term`, the best first,
/// `limit` at most: a name that is the term, then one that starts with it,
/// then one with a word that does — TVmaze's weight deciding between names
/// that answer alike, and a term that ends with a year, `dune 2021`, read as
/// the name and the year the series first aired too. Nothing while the index
/// is off, or before it was first read.
pub async fn search(state: &AppState, term: &str, limit: usize) -> Result<Vec<Show>> {
    if !is_on(state) || limit == 0 {
        return Ok(Vec::new());
    }
    let term = Term::parse(term);
    if term.is_empty() {
        return Ok(Vec::new());
    }

    let mut seen: HashSet<i64> = HashSet::new();
    let mut found: Vec<IndexShow> = Vec::new();
    for form in term.forms() {
        let starting = tvmaze::starting(&state.db, form, POOL).await?;
        let inside = tvmaze::with_word(&state.db, form, POOL).await?;
        for show in starting.into_iter().chain(inside) {
            if seen.insert(show.id) {
                found.push(show);
            }
        }
    }

    Ok(ranked(&term, found, limit))
}

/// Series in the order they answer a term, best first, `limit` at most; those
/// that do not answer it at all left out.
fn ranked(term: &Term, shows: Vec<IndexShow>, limit: usize) -> Vec<IndexShow> {
    let mut scored: Vec<(f64, IndexShow)> = shows
        .into_iter()
        .filter_map(|show| {
            let year = show
                .premiered
                .as_deref()
                .and_then(|date| date.get(..4))
                .and_then(|year| year.parse().ok());
            let weight = show.weight.map(f64::from);
            let score = rank::score(term, &crate::search::norm(&show.name), year, weight, true)?;
            Some((score, show))
        })
        .collect();
    scored.sort_by(|(a, first), (b, second)| {
        b.total_cmp(a)
            .then(second.weight.cmp(&first.weight))
            .then(first.id.cmp(&second.id))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, show)| show)
        .collect()
}

/// The series of the index TVmaze files under this TheTVDB id — the better
/// known, should it file two there. Nothing while the index is off.
pub async fn by_tvdb(state: &AppState, tvdb_id: i64) -> Result<Option<Show>> {
    if !is_on(state) {
        return Ok(None);
    }
    Ok(tvmaze::by(&state.db, Key::Tvdb, &[tvdb_id])
        .await?
        .remove(&tvdb_id))
}

/// The series of the index TVmaze files under this IMDb id, `tt0903747`.
/// Nothing while the index is off.
pub async fn by_imdb(state: &AppState, tconst: &str) -> Result<Option<Show>> {
    if !is_on(state) {
        return Ok(None);
    }
    Ok(tvmaze::by_imdb(&state.db, &[tconst.to_string()])
        .await?
        .remove(tconst))
}

/// The series of the index under its TVmaze id. Nothing while the index is
/// off.
pub async fn by_id(state: &AppState, id: i64) -> Result<Option<Show>> {
    if !is_on(state) {
        return Ok(None);
    }
    Ok(tvmaze::by(&state.db, Key::Tvmaze, &[id]).await?.remove(&id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::registry::Scope;

    fn listed(id: i64, name: &str, weight: i32, premiered: &str) -> IndexShow {
        IndexShow {
            id,
            name: name.into(),
            weight: Some(weight),
            premiered: Some(premiered.into()),
            tvdb_id: Some(900_000 + id),
            imdb_id: Some(format!("tt{:07}", 9_000_000 + id)),
            ..IndexShow::default()
        }
    }

    #[test]
    fn a_name_that_is_the_term_comes_first_and_weight_decides_between_alike() {
        let term = Term::parse("the wandering lantern");
        let order: Vec<i64> = ranked(
            &term,
            vec![
                listed(1, "The Wandering Lantern Chronicles", 90, "2015-01-01"),
                listed(2, "The Wandering Lantern", 10, "2001-01-01"),
                listed(3, "Tales of the Wandering Lantern", 99, "2019-01-01"),
                listed(4, "The Wandering Lanterns", 80, "2011-01-01"),
                listed(5, "Something Else Entirely", 100, "2020-01-01"),
            ],
            10,
        )
        .into_iter()
        .map(|show| show.id)
        .collect();
        assert_eq!(order[0], 2, "{order:?}");
        assert_eq!(&order[1..3], [4, 1], "{order:?}");
        assert_eq!(order[3], 3, "{order:?}");
        assert!(!order.contains(&5), "a name the term is not in: {order:?}");

        // A year read as the year it first aired.
        let term = Term::parse("the wandering lantern 2015");
        let first = ranked(
            &term,
            vec![
                listed(2, "The Wandering Lantern", 10, "2001-01-01"),
                listed(6, "The Wandering Lantern", 10, "2015-03-01"),
            ],
            1,
        );
        assert_eq!(first[0].id, 6);
    }

    #[tokio::test]
    async fn the_index_is_searched_while_it_is_on_and_by_its_ids_elsewhere() {
        let state = crate::api::native::testing::server().await;
        tvmaze::write(
            &state.db,
            &[
                listed(71, "Quillon Harbour", 30, "2012-04-01"),
                listed(72, "Quillon Harbour Nights", 70, "2016-04-01"),
                listed(73, "Return to Quillon", 50, "2020-04-01"),
            ],
        )
        .await
        .unwrap();

        // Off by default: nothing, whatever the table holds.
        assert!(search(&state, "quillon", 10).await.unwrap().is_empty());
        assert_eq!(by_tvdb(&state, 900_071).await.unwrap(), None);

        state
            .settings
            .set(Scope::Server, "", "tvmaze.index", "true", None)
            .await
            .unwrap();
        let found: Vec<(i64, Option<i64>, Option<String>)> = search(&state, "quillon", 10)
            .await
            .unwrap()
            .into_iter()
            .map(|show| (show.id, show.tvdb_id, show.imdb_id))
            .collect();
        assert_eq!(
            found,
            [
                (72, Some(900_072), Some("tt9000072".into())),
                (71, Some(900_071), Some("tt9000071".into())),
                (73, Some(900_073), Some("tt9000073".into())),
            ]
        );
        assert_eq!(
            search(&state, "Quillon Harbour", 1).await.unwrap()[0].id,
            71
        );
        assert!(search(&state, "  ", 10).await.unwrap().is_empty());

        assert_eq!(by_tvdb(&state, 900_072).await.unwrap().unwrap().id, 72);
        assert_eq!(by_imdb(&state, "tt9000073").await.unwrap().unwrap().id, 73);
        assert_eq!(
            by_id(&state, 71).await.unwrap().unwrap().name,
            "Quillon Harbour"
        );
        assert_eq!(by_id(&state, 70).await.unwrap(), None);
    }
}

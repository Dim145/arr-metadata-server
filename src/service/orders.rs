//! The other orders a series' episodes come in, read from TheTVDB when a
//! series is fetched and kept beside its aired order — never in it — and
//! where each one places the work's episodes, for every client that numbers
//! them by one: Sonarr v5 by its season type, Plex by its episode order.

use std::collections::{HashMap, HashSet};

use crate::{
    db::repo,
    domain::{Episode, EpisodeOrder, MediaItem, MediaKind, PlacedEpisode},
    state::AppState,
};

/// What TheTVDB, and Sonarr after it, call the aired order: the work's own,
/// the one every series has, rather than one kept beside it.
pub const AIRED: &str = "official";

/// The order `kind` names among those the catalogue keeps for a work. None
/// for the aired order, and none for a kind the work has no order of: a
/// client asks in the order a person chose, and is answered in the aired one
/// rather than refused — a refusal would fail its refresh.
pub fn named<'a>(orders: &'a [EpisodeOrder], kind: &str) -> Option<&'a EpisodeOrder> {
    let wanted = kind.trim();
    if wanted.eq_ignore_ascii_case(AIRED) {
        return None;
    }
    orders
        .iter()
        .find(|order| order.kind.eq_ignore_ascii_case(wanted) && !order.episodes.is_empty())
}

/// [`named`], when the order places an episode the work still has. One in
/// name only — its places all name episodes the work no longer holds, as
/// after a refresh without TheTVDB — is no order to answer in: a client set
/// to it would be sent no episode at all. The aired order stands in.
pub fn in_use<'a>(
    item: &'a MediaItem,
    orders: &'a [EpisodeOrder],
    kind: &str,
) -> Option<&'a EpisodeOrder> {
    named(orders, kind).filter(|order| !placements(item, order).is_empty())
}

/// The work's episodes placed in one of its other orders, each with the
/// aired episode it stands for. A place that names an episode the work no
/// longer has is left out; a place given twice is kept once — Sonarr threw
/// on a season and number it met twice, and Plex keys an episode by them.
pub fn placements<'a>(
    item: &'a MediaItem,
    order: &'a EpisodeOrder,
) -> Vec<(&'a PlacedEpisode, &'a Episode)> {
    let by_tvdb: HashMap<i64, &Episode> = item
        .episodes
        .iter()
        .filter_map(|episode| Some((episode.tvdb_id?, episode)))
        .collect();

    let mut seen = HashSet::new();
    order
        .episodes
        .iter()
        .filter_map(|place| {
            let episode = *by_tvdb.get(&place.tvdb_id)?;
            seen.insert((place.season_number, place.episode_number))
                .then_some((place, episode))
        })
        .collect()
}

/// Ask TheTVDB how else it numbers this series, and keep the answer. Best
/// effort: a failure is logged, and what was kept before stays.
pub async fn gather(state: &AppState, item: &MediaItem) {
    if item.kind != MediaKind::Series || !state.tvdb.is_enabled() {
        return;
    }
    let Some(tvdb_id) = item.external_ids.tvdb else {
        return;
    };

    let orders = match state.tvdb.orders(tvdb_id).await {
        Ok(orders) => orders,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "TheTVDB's other episode orders could not be read"
            );
            return;
        }
    };

    if let Err(e) = repo::order::replace(&state.db, &item.id, &orders).await {
        tracing::warn!(id = %item.id, error = %e, "could not keep the episode orders");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::child::blank_episode;

    fn place(tvdb_id: i64, season: i32, number: i32) -> PlacedEpisode {
        PlacedEpisode {
            tvdb_id,
            season_number: season,
            episode_number: number,
            absolute_number: None,
        }
    }

    fn series() -> MediaItem {
        let mut it = MediaItem::empty(MediaKind::Series);
        it.episodes = [(1, 1, 11), (1, 2, 12)]
            .into_iter()
            .map(|(season, number, tvdb_id)| {
                let mut episode = blank_episode(season, number);
                episode.tvdb_id = Some(tvdb_id);
                episode
            })
            .collect();
        it
    }

    #[test]
    fn an_order_places_the_episodes_the_work_has_once_each() {
        let it = series();
        let dvd = EpisodeOrder {
            kind: "dvd".into(),
            // Swapped; an episode the work no longer has; a place given twice.
            episodes: vec![
                place(12, 1, 1),
                place(11, 1, 2),
                place(99, 1, 3),
                place(11, 1, 2),
            ],
        };
        let placed: Vec<(i32, i64)> = placements(&it, &dvd)
            .into_iter()
            .map(|(place, episode)| (place.episode_number, episode.tvdb_id.unwrap()))
            .collect();
        assert_eq!(placed, [(1, 12), (2, 11)]);

        let orders = vec![dvd];
        assert_eq!(
            named(&orders, " DVD ").map(|o| o.kind.as_str()),
            Some("dvd")
        );
        assert!(named(&orders, AIRED).is_none());
        assert!(named(&orders, "absolute").is_none());
        assert!(in_use(&it, &orders, "dvd").is_some());
    }

    #[test]
    fn an_order_in_name_only_is_none() {
        let it = series();
        let orders = vec![EpisodeOrder {
            kind: "regional".into(),
            episodes: vec![place(99, 1, 1)],
        }];
        assert!(named(&orders, "regional").is_some());
        assert!(in_use(&it, &orders, "regional").is_none());
    }
}

//! Episodes TheTVDB moved, told.
//!
//! TheTVDB renumbers: a special becomes the first episode of a season, a
//! season is cut in two, an anime's absolute numbers shift by one. Sonarr
//! follows at its next refresh, and the files it had matched by the old
//! numbers then stand for other episodes, or for none — without anybody
//! having been told. Here, a write that moves episodes of a series — the same
//! TheTVDB episode at another season, number or absolute number — says so in
//! the trail, `item.renumbered`, with how many moved and the first of them,
//! `S01E05 → S02E01`; the webhooks hear it as they hear the rest.
//!
//! Compared by TheTVDB's own id for each episode, on both sides, and only
//! for the episodes a provider gave: one added or gone is no move, nor one
//! a person made by hand. A list kept as it was moves nothing, and that is
//! what a refresh or a sync keeps when the source that numbers the series did
//! not answer (`merge::provenance::protect_numbering`): no alert is raised for
//! the numbering of a provider that was only standing in.

use std::collections::HashMap;

use crate::{
    db::repo::audit::Action,
    domain::{Episode, MediaItem, MediaKind},
    state::AppState,
};

/// How many moves the trail's line names before it says how many more.
const NAMED: usize = 3;

/// Where an episode stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub season: i32,
    pub episode: i32,
    pub absolute: Option<i32>,
}

impl Place {
    fn of(episode: &Episode) -> Self {
        Self {
            season: episode.season_number,
            episode: episode.episode_number,
            absolute: episode.absolute_episode_number,
        }
    }

    /// `S01E05`, with the absolute number when it is what changed.
    fn label(self, with_absolute: bool) -> String {
        let place = format!("S{:02}E{:02}", self.season, self.episode);
        match self.absolute {
            Some(absolute) if with_absolute => format!("{place} (#{absolute})"),
            _ => place,
        }
    }
}

/// One episode, moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    pub tvdb_id: i64,
    pub from: Place,
    pub to: Place,
}

impl Move {
    fn label(&self) -> String {
        // An absolute number gained or lost is no move: a source that gives
        // none is not saying the episode has none.
        let absolute_moved = matches!(
            (self.from.absolute, self.to.absolute),
            (Some(a), Some(b)) if a != b
        );
        format!(
            "{} → {}",
            self.from.label(absolute_moved),
            self.to.label(absolute_moved)
        )
    }
}

/// The episodes in `after` that `before` holds elsewhere, by TheTVDB's id,
/// in the order they stood.
pub fn moves(before: &[Episode], after: &[Episode]) -> Vec<Move> {
    let was: HashMap<i64, Place> = before
        .iter()
        .filter(|e| !e.is_manual)
        .filter_map(|e| Some((e.tvdb_id?, Place::of(e))))
        .collect();

    let mut moved: Vec<Move> = after
        .iter()
        .filter(|e| !e.is_manual)
        .filter_map(|e| {
            let tvdb_id = e.tvdb_id?;
            let from = *was.get(&tvdb_id)?;
            let to = Place::of(e);
            let absolute_moved =
                matches!((from.absolute, to.absolute), (Some(a), Some(b)) if a != b);
            (from.season != to.season || from.episode != to.episode || absolute_moved)
                .then_some(Move { tvdb_id, from, to })
        })
        .collect();
    moved.sort_by_key(|m| (m.from.season, m.from.episode, m.tvdb_id));
    moved
}

/// What the trail says: how many, and the first few.
pub fn describe(title: &str, moved: &[Move]) -> String {
    let named: Vec<String> = moved.iter().take(NAMED).map(Move::label).collect();
    let more = moved.len().saturating_sub(NAMED);
    format!(
        "“{title}”: {} renumbered — {}{}",
        if moved.len() == 1 {
            "one episode".to_string()
        } else {
            format!("{} episodes", moved.len())
        },
        named.join(", "),
        if more > 0 {
            format!(", and {more} more")
        } else {
            String::new()
        }
    )
}

/// Say in the trail that a series' episodes moved, when they did.
pub async fn tell(state: &AppState, item: &MediaItem, moved: &[Move]) {
    if moved.is_empty() || item.kind != MediaKind::Series {
        return;
    }
    tracing::info!(
        id = %item.id,
        title = %item.title,
        moved = moved.len(),
        "TheTVDB moved episodes of a series"
    );
    crate::api::audit::record_system(
        state,
        Action::ItemRenumbered,
        &item.id,
        &describe(&item.title, moved),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(season: i32, number: i32, absolute: Option<i32>, tvdb: i64) -> Episode {
        let mut episode = crate::db::repo::child::blank_episode(season, number);
        episode.id = crate::db::new_id();
        episode.absolute_episode_number = absolute;
        episode.tvdb_id = Some(tvdb);
        episode.is_manual = false;
        episode
    }

    #[test]
    fn a_season_cut_in_two_is_said_episode_by_episode() {
        let before = vec![
            episode(1, 4, Some(4), 104),
            episode(1, 5, Some(5), 105),
            episode(1, 6, Some(6), 106),
        ];
        let after = vec![
            episode(1, 4, Some(4), 104),
            episode(2, 1, Some(5), 105),
            episode(2, 2, Some(6), 106),
            // New: no move.
            episode(2, 3, Some(7), 107),
        ];

        let moved = moves(&before, &after);
        assert_eq!(moved.len(), 2);
        assert_eq!(moved[0].label(), "S01E05 → S02E01");
        assert_eq!(
            describe("Stranger Things", &moved),
            "“Stranger Things”: 2 episodes renumbered — S01E05 → S02E01, S01E06 → S02E02"
        );
    }

    #[test]
    fn an_absolute_number_shifting_is_a_move_and_one_given_or_lost_is_not() {
        let before = vec![
            episode(1, 1, Some(12), 1),
            episode(1, 2, None, 2),
            episode(1, 3, Some(14), 3),
        ];
        let after = vec![
            episode(1, 1, Some(13), 1),
            episode(1, 2, Some(13), 2),
            episode(1, 3, None, 3),
        ];

        let moved = moves(&before, &after);
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].label(), "S01E01 (#12) → S01E01 (#13)");
    }

    #[test]
    fn a_list_kept_as_it_was_moves_nothing_and_a_hand_made_episode_is_left_out() {
        let before = vec![episode(1, 1, None, 1), episode(1, 2, None, 2)];
        // Kept by the refresh, under fresh row ids: protected numbering.
        let kept = vec![episode(1, 1, None, 1), episode(1, 2, None, 2)];
        assert!(moves(&before, &kept).is_empty());

        // Another provider's list, with no TheTVDB ids: nothing to compare.
        let mut theirs = vec![episode(1, 1, None, 1)];
        theirs[0].tvdb_id = None;
        theirs[0].season_number = 2;
        assert!(moves(&before, &theirs).is_empty());

        let mut by_hand = episode(3, 1, None, 2);
        by_hand.is_manual = true;
        assert!(moves(&before, &[by_hand]).is_empty());
    }

    #[test]
    fn a_long_list_names_three_and_counts_the_rest() {
        let before: Vec<Episode> = (1..=6).map(|n| episode(1, n, None, i64::from(n))).collect();
        let after: Vec<Episode> = (1..=6).map(|n| episode(2, n, None, i64::from(n))).collect();
        let said = describe("Show", &moves(&before, &after));
        assert_eq!(
            said,
            "“Show”: 6 episodes renumbered — S01E01 → S02E01, S01E02 → S02E02, S01E03 → \
             S02E03, and 3 more"
        );
        let one = describe("Show", &moves(&before[..1], &after[..1]));
        assert!(one.starts_with("“Show”: one episode renumbered"), "{one}");
    }
}

//! The popularity of a work held here, worked out here (wave P, P6).
//!
//! TMDB gives a popularity to every work it lists, and the catalogue's lists
//! are in its order. A work TMDB does not list — one entered by hand, or every
//! work of a catalogue TMDB does not serve — has none, and came last. Its
//! popularity is worked out here instead, for the works held alone, from two
//! things the server reads anyway:
//!
//! - TVmaze's weight of a series, from 0 to 100: the index's
//!   (`tvmaze.index`), else the one in TVmaze's own answer for the series
//!   (`tvmaze.enabled`);
//! - how many votes IMDb's daily list sees a work gather a day, over the week
//!   kept (`imdb.enabled`).
//!
//! Each is put between 0 and 10 — the weight in tenths, the votes by their
//! logarithm, ten for twenty-two thousand a day — and a work with both takes
//! their mean ([`figure`]). Then each work is placed on TMDB's scale: where
//! its rank among the works' figures falls among TMDB's popularities of the
//! works held of its kind ([`Scale`]), so that a list in TMDB's order takes a
//! work without one where it belongs. With too few of TMDB's to go by, the
//! figure times ten.
//!
//! It never stands for TMDB's (D4): laid over a work only where the work has
//! none of TMDB's ([`overlay`]), and the lists sort on whichever a work has.
//! A TMDB- or AniList-shaped document of a work TMDB gave a popularity shows
//! TMDB's. Worked out by `jobs::popularity`.

use std::collections::HashMap;

use crate::{domain::MediaKind, state::AppState};

/// What a work's popularity is worked out from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Signals {
    /// TVmaze's weight of the series, from 0 to 100.
    pub weight: Option<i32>,
    /// The votes IMDb's list gave it a day, on average over the days kept.
    pub votes_per_day: Option<f64>,
}

/// The votes a day that make a figure of ten: a film's first days in
/// cinemas, a finale's week.
const VOTES_FOR_TEN: f64 = 22_026.0;

/// A work's popularity before it is placed on TMDB's scale, from 0 to 10:
/// TVmaze's weight in tenths, the logarithm of its votes a day, the mean of
/// the two where it has both; nothing without either.
pub fn figure(signals: Signals) -> Option<f64> {
    let weight = signals
        .weight
        .map(|weight| f64::from(weight.clamp(0, 100)) / 10.0);
    let votes = signals
        .votes_per_day
        .filter(|votes| votes.is_finite())
        .map(|votes| 10.0 * (1.0 + votes.max(0.0)).ln() / (1.0 + VOTES_FOR_TEN).ln())
        .map(|votes| votes.min(10.0));
    match (weight, votes) {
        (Some(weight), Some(votes)) => Some((weight + votes) / 2.0),
        (weight, votes) => weight.or(votes),
    }
}

/// How many votes a work gathered a day, from the counts kept of it, the
/// oldest day first: from the first count to the last, over the days between
/// them. Nothing from fewer than two days, nor from a day that does not read;
/// a count that fell — IMDb takes votes away now and then — gathered none.
pub fn votes_per_day(days: &[(String, i64)]) -> Option<f64> {
    let (first, last) = (days.first()?, days.last()?);
    let day = |text: &str| chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok();
    let between = (day(&last.0)? - day(&first.0)?).num_days();
    (between > 0).then(|| (last.1 - first.1).max(0) as f64 / between as f64)
}

/// The fewest of TMDB's popularities a kind needs for its works to be placed
/// on them; under it, those of every kind are gone by.
const ENOUGH: usize = 20;

/// TMDB's popularities of the works held, by kind, and the figures worked
/// out here they are matched with: what places a figure on TMDB's scale.
pub struct Scale {
    /// The figures worked out here, of series and of films, in order.
    figures: [Vec<f64>; 2],
    /// TMDB's popularities, of series, of films, and of both, in order.
    tmdb: [Vec<f64>; 3],
}

fn slot(kind: MediaKind) -> usize {
    match kind {
        MediaKind::Series => 0,
        MediaKind::Movie => 1,
    }
}

impl Scale {
    /// From every work's figure worked out here, and every popularity TMDB
    /// gave a work held, each with the work's kind.
    pub fn new(
        figures: impl IntoIterator<Item = (MediaKind, f64)>,
        tmdb: impl IntoIterator<Item = (MediaKind, f64)>,
    ) -> Self {
        let mut scale = Self {
            figures: [Vec::new(), Vec::new()],
            tmdb: [Vec::new(), Vec::new(), Vec::new()],
        };
        for (kind, figure) in figures {
            if figure.is_finite() {
                scale.figures[slot(kind)].push(figure);
            }
        }
        for (kind, popularity) in tmdb {
            if popularity.is_finite() && popularity >= 0.0 {
                scale.tmdb[slot(kind)].push(popularity);
                scale.tmdb[2].push(popularity);
            }
        }
        for list in scale.figures.iter_mut().chain(scale.tmdb.iter_mut()) {
            list.sort_by(f64::total_cmp);
        }
        scale
    }

    /// A work's figure on TMDB's scale: TMDB's popularity at the same rank
    /// among TMDB's as the figure's among the figures of its kind, read
    /// between the two nearest; the figure times ten without enough of
    /// TMDB's. Rounded to the thousandth, as TMDB gives its own.
    pub fn place(&self, kind: MediaKind, figure: f64) -> f64 {
        let figures = &self.figures[slot(kind)];
        let tmdb = [&self.tmdb[slot(kind)], &self.tmdb[2]]
            .into_iter()
            .find(|tmdb| tmdb.len() >= ENOUGH);
        let placed = match tmdb {
            Some(tmdb) if !figures.is_empty() => {
                // The figure's rank, a tie counting half below it, as a share.
                let below = figures.partition_point(|other| *other < figure);
                let to = figures.partition_point(|other| *other <= figure);
                let share = (below + to) as f64 / 2.0 / figures.len() as f64;
                let at = share.clamp(0.0, 1.0) * (tmdb.len() - 1) as f64;
                let (low, high) = (at.floor() as usize, at.ceil() as usize);
                tmdb[low] + (tmdb[high] - tmdb[low]) * (at - low as f64)
            }
            _ => figure * 10.0,
        };
        (placed * 1000.0).round() / 1000.0
    }
}

/// Lay the popularity worked out here over the works that have none of
/// TMDB's — a work entered by hand, every work of a catalogue TMDB does not
/// serve — in one query; leave every other as it is. A failure costs the
/// figure and nothing else.
pub async fn overlay(state: &AppState, items: &mut [crate::domain::MediaItem]) {
    let without: Vec<String> = items
        .iter()
        .filter(|item| item.popularity.is_none())
        .map(|item| item.id.clone())
        .collect();
    if without.is_empty() {
        return;
    }
    let local: HashMap<String, f64> =
        match crate::db::repo::popularity::local_of(&state.db, &without).await {
            Ok(local) => local,
            Err(e) => {
                tracing::warn!(
                    error = format_args!("{e:#}"),
                    "could not read the popularity worked out here"
                );
                return;
            }
        };
    for item in items.iter_mut() {
        if item.popularity.is_none() {
            item.popularity = local.get(&item.id).copied();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_figure_is_the_weight_in_tenths_and_the_logarithm_of_the_votes() {
        let of = |weight: Option<i32>, votes_per_day: Option<f64>| {
            figure(Signals {
                weight,
                votes_per_day,
            })
        };
        assert_eq!(of(None, None), None);
        assert_eq!(of(Some(97), None), Some(9.7));
        assert_eq!(of(Some(140), None), Some(10.0));
        assert_eq!(of(None, Some(0.0)), Some(0.0));
        assert_eq!(of(None, Some(VOTES_FOR_TEN)).map(|f| f.round()), Some(10.0));
        assert_eq!(of(None, Some(1e9)), Some(10.0));
        let some = of(None, Some(150.0)).unwrap();
        assert!((4.9..5.1).contains(&some), "{some}");
        // Both: their mean.
        assert_eq!(of(Some(60), Some(0.0)), Some(3.0));
        assert_eq!(of(Some(50), Some(f64::NAN)), Some(5.0));
    }

    #[test]
    fn the_votes_a_day_run_from_the_first_count_kept_to_the_last() {
        let day = |d: &str, votes: i64| (d.to_string(), votes);
        assert_eq!(
            votes_per_day(&[day("2026-10-26", 1000), day("2026-11-02", 1700)]),
            Some(100.0)
        );
        assert_eq!(votes_per_day(&[day("2026-11-02", 1700)]), None);
        assert_eq!(votes_per_day(&[]), None);
        assert_eq!(
            votes_per_day(&[day("2026-10-30", 1000), day("2026-11-02", 900)]),
            Some(0.0),
            "a count that fell gathered none"
        );
        assert_eq!(
            votes_per_day(&[day("yesterday", 1), day("2026-11-02", 9)]),
            None
        );
    }

    #[test]
    fn a_figure_is_placed_where_its_rank_falls_among_tmdbs() {
        let tmdb: Vec<(MediaKind, f64)> = (1..=100)
            .map(|n| (MediaKind::Series, f64::from(n)))
            .collect();
        let figures = [1.0, 2.0, 3.0, 4.0].map(|f| (MediaKind::Series, f));
        let scale = Scale::new(figures, tmdb.clone());

        let placed: Vec<f64> = [1.0, 2.0, 3.0, 4.0]
            .into_iter()
            .map(|f| scale.place(MediaKind::Series, f))
            .collect();
        // An eighth, three eighths, five and seven of the way up TMDB's.
        assert_eq!(placed, [13.375, 38.125, 62.875, 87.625]);
        assert!(placed.windows(2).all(|pair| pair[0] < pair[1]));
        // Above every figure, at the top of TMDB's.
        assert_eq!(scale.place(MediaKind::Series, 9.0), 100.0);

        // Too few of TMDB's for films: those of every kind are gone by.
        let films = Scale::new(
            [(MediaKind::Movie, 2.0), (MediaKind::Movie, 8.0)],
            tmdb.into_iter().chain([(MediaKind::Movie, 500.0)]),
        );
        assert!(films.place(MediaKind::Movie, 8.0) > films.place(MediaKind::Movie, 2.0));

        // None at all: the figure times ten.
        let alone = Scale::new([(MediaKind::Movie, 4.25)], []);
        assert_eq!(alone.place(MediaKind::Movie, 4.25), 42.5);
    }
}

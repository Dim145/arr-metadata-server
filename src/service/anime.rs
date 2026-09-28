//! Which AniList and MyAnimeList entry a work is.
//!
//! Both sites split a series into one entry per season or cour, so "the
//! AniList entry for *Attack on Titan*" is really seven entries. The one asked
//! about is the one that stands for the whole: the first season's, as the
//! anime identifier list places it. Its score and its titles are what the
//! series is known by; the later seasons' belong to those seasons.
//!
//! Looked up by identifier from the list, never by title. Where the list has
//! not been downloaded yet, or does not know the series, the ids Skyhook
//! republishes stand in.

use anyhow::Result;

use crate::{
    db::repo::{self, anime::Entry},
    domain::{ExternalIds, ExternalSource},
    state::AppState,
};

/// The entry to ask each site about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chosen {
    pub mal: Option<i64>,
    pub anilist: Option<i64>,
}

impl Chosen {
    pub fn is_empty(&self) -> bool {
        self.mal.is_none() && self.anilist.is_none()
    }
}

/// The entries to ask about, site by site: the one a person set on the work
/// when they locked its identifiers, then the identifier list's, then the
/// one the work already goes by — the oldest of several, which is the first
/// season far more often than not.
pub fn choose(mapped: Option<Chosen>, pinned: Option<&ExternalIds>, own: &ExternalIds) -> Chosen {
    let first = |ids: &[i64]| ids.iter().copied().min();
    let mapped = mapped.unwrap_or_default();
    Chosen {
        mal: pinned
            .and_then(|p| first(&p.mal))
            .or(mapped.mal)
            .or_else(|| first(&own.mal)),
        anilist: pinned
            .and_then(|p| first(&p.anilist))
            .or(mapped.anilist)
            .or_else(|| first(&own.anilist)),
    }
}

/// Whether either anime source is switched on, which is also what keeps the
/// identifier list downloaded.
pub fn enabled(state: &AppState) -> bool {
    crate::jobs::datasets::anime_wanted(state)
}

/// The entry that stands for a TheTVDB series.
///
/// `known` is what the other providers already said — Skyhook lists every
/// MyAnimeList and AniList entry it files under the series.
pub async fn for_series(state: &AppState, tvdb_id: i64, known: &ExternalIds) -> Chosen {
    let entries = match repo::anime::for_series(&state.db, tvdb_id).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "could not read the anime identifier list"
            );
            Vec::new()
        }
    };

    match primary_of_series(&entries) {
        Some(entry) => Chosen {
            mal: entry.mal_id,
            anilist: entry.anilist_id,
        },
        // The oldest entry is the first season far more often than not, and a
        // guess at the right series beats nothing at all.
        None => Chosen {
            mal: known.mal.iter().copied().min(),
            anilist: known.anilist.iter().copied().min(),
        },
    }
}

/// The entry that is a TMDB film. Nothing stands in when the list does not
/// know it: no other provider says which anime a film is.
pub async fn for_movie(state: &AppState, tmdb_id: i64) -> Chosen {
    let entries = match repo::anime::for_movie(&state.db, tmdb_id).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                tmdb_id,
                error = format_args!("{e:#}"),
                "could not read the anime identifier list"
            );
            Vec::new()
        }
    };

    primary_of_movie(&entries)
        .map(|entry| Chosen {
            mal: entry.mal_id,
            anilist: entry.anilist_id,
        })
        .unwrap_or_default()
}

/// The TheTVDB series a MyAnimeList or AniList entry belongs to, for Sonarr's
/// `mal:` and `anilist:` searches — which is how its AniList and MyAnimeList
/// import lists find a series.
pub async fn series_for(state: &AppState, source: ExternalSource, id: i64) -> Result<Option<i64>> {
    if !enabled(state) {
        return Ok(None);
    }

    match source {
        ExternalSource::Mal => repo::anime::tvdb_for_mal(&state.db, id).await,
        ExternalSource::AniList => repo::anime::tvdb_for_anilist(&state.db, id).await,
        _ => Ok(None),
    }
}

/// The first season's entry, or the one entry that spans every season.
///
/// Season zero is where TheTVDB files films and specials, so it comes last; a
/// television entry beats a special filed in the same season; and where one
/// season is split in two, the half that starts it.
fn primary_of_series(entries: &[Entry]) -> Option<&Entry> {
    entries
        .iter()
        .filter(|e| e.mal_id.is_some() || e.anilist_id.is_some())
        .min_by_key(|e| {
            let season = match e.tvdb_season {
                None | Some(1) => 0,
                Some(s) if s > 1 => s,
                _ => i32::MAX,
            };
            let episodic = matches!(e.kind.as_deref(), Some("TV" | "ONA" | "OVA" | "TV_SHORT"));

            (
                season,
                !episodic,
                e.tvdb_offset.unwrap_or(0),
                e.mal_id.or(e.anilist_id).unwrap_or(i64::MAX),
            )
        })
}

/// The film itself, where the list files something else under the same TMDB
/// id too.
fn primary_of_movie(entries: &[Entry]) -> Option<&Entry> {
    entries
        .iter()
        .filter(|e| e.mal_id.is_some() || e.anilist_id.is_some())
        .min_by_key(|e| {
            (
                e.kind.as_deref() != Some("MOVIE"),
                e.mal_id.or(e.anilist_id).unwrap_or(i64::MAX),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mal: i64, season: Option<i32>, offset: Option<i32>, kind: &str) -> Entry {
        Entry {
            mal_id: Some(mal),
            anilist_id: Some(mal + 1_000_000),
            tvdb_id: Some(267440),
            tvdb_season: season,
            tvdb_offset: offset,
            kind: Some(kind.into()),
            ..Default::default()
        }
    }

    #[test]
    fn the_first_season_stands_for_the_series() {
        // Attack on Titan, as the list files it.
        let entries = vec![
            entry(18397, Some(0), None, "OVA"),
            entry(36702, Some(0), None, "MOVIE"),
            entry(25777, Some(2), None, "TV"),
            entry(38524, Some(3), Some(12), "TV"),
            entry(35760, Some(3), None, "TV"),
            entry(16498, Some(1), None, "TV"),
        ];

        assert_eq!(
            primary_of_series(&entries).and_then(|e| e.mal_id),
            Some(16498)
        );
    }

    #[test]
    fn an_entry_that_spans_the_series_stands_for_it() {
        // One Piece: one entry for everything, and a score of films in season 0.
        let entries = vec![
            entry(460, Some(0), None, "MOVIE"),
            entry(459, Some(0), Some(1), "MOVIE"),
            entry(21, None, None, "TV"),
        ];

        assert_eq!(primary_of_series(&entries).and_then(|e| e.mal_id), Some(21));
    }

    #[test]
    fn the_half_that_starts_a_split_season_comes_first() {
        let entries = vec![
            entry(40540, Some(4), Some(12), "TV"),
            entry(39597, Some(4), None, "TV"),
        ];

        assert_eq!(
            primary_of_series(&entries).and_then(|e| e.mal_id),
            Some(39597)
        );
    }

    #[test]
    fn a_series_known_only_through_its_specials_still_has_an_entry() {
        let entries = vec![entry(20021, Some(0), Some(9), "SPECIAL")];
        assert_eq!(
            primary_of_series(&entries).and_then(|e| e.mal_id),
            Some(20021)
        );
        assert_eq!(primary_of_series(&[]), None);
    }

    #[test]
    fn a_film_is_preferred_to_what_shares_its_tmdb_id() {
        let entries = vec![
            entry(100, None, None, "SPECIAL"),
            entry(32281, None, None, "MOVIE"),
        ];

        assert_eq!(
            primary_of_movie(&entries).and_then(|e| e.mal_id),
            Some(32281)
        );
    }

    /// A person's entry wins, then the list's, then the work's own.
    #[test]
    fn the_entry_asked_about_is_the_one_a_person_set_first() {
        let list = Chosen {
            mal: Some(10),
            anilist: Some(20),
        };
        let own = ExternalIds {
            mal: vec![3, 1],
            anilist: vec![7],
            ..ExternalIds::default()
        };
        let pinned = ExternalIds {
            anilist: vec![169941],
            ..ExternalIds::default()
        };
        assert_eq!(
            choose(Some(list), Some(&pinned), &own),
            Chosen {
                mal: Some(10),
                anilist: Some(169941)
            }
        );
        assert_eq!(choose(Some(list), None, &own), list);
        assert_eq!(
            choose(None, None, &own),
            Chosen {
                mal: Some(1),
                anilist: Some(7)
            }
        );
        assert!(choose(None, None, &ExternalIds::default()).is_empty());
    }
}

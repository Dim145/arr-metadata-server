//! Addressing works that have no TVDB entry.
//!
//! Sonarr addresses every series by TVDB id — there is no other key in the
//! protocol. But TMDB indexes titles TVDB does not: adult content, and a long
//! tail of shows nobody has cross-linked yet. Refusing to serve them would drop
//! them from search results entirely.
//!
//! So a TMDB-only series is handed a **synthetic** TVDB id: its TMDB id plus a
//! fixed offset. Sonarr stores that number like any other and sends it back on
//! the detail request, where it is decoded again.
//!
//! The offset is chosen so the two spaces cannot overlap. Real TVDB series ids
//! are below 500 000 and TMDB TV ids below 1 000 000; 100 000 000 clears both
//! and still fits in the `int` Sonarr stores it in.
//!
//! Fankai's productions take a second range, from 200 000 000: a Fan-Kai is a
//! recut TheTVDB and TMDB list nothing of, so its client id is Fankai's own id
//! plus that offset, and the two ranges are told apart by size alone.

/// Start of the synthetic id range.
pub const SYNTHETIC_OFFSET: i64 = 100_000_000;

/// Start of the range that stands for Fankai's productions.
///
/// Above the TMDB range, which TMDB's own ids — nowhere near a hundred million
/// — never fill, so an id says which it is by its size.
pub const FANKAI_OFFSET: i64 = 200_000_000;

/// Largest id that still fits Sonarr's 32-bit column.
const MAX_SONARR_ID: i64 = i32::MAX as i64;

/// Wrap a TMDB id as a synthetic TVDB id.
///
/// Returns `None` if the result would not fit in the client's integer column,
/// which would otherwise be silently truncated into a collision — or would
/// reach into the range that names Fankai's productions.
pub fn to_synthetic(tmdb_id: i64) -> Option<i64> {
    let encoded = SYNTHETIC_OFFSET.checked_add(tmdb_id)?;
    (tmdb_id > 0 && encoded < FANKAI_OFFSET).then_some(encoded)
}

/// Recover the TMDB id from a synthetic one.
pub fn from_synthetic(id: i64) -> Option<i64> {
    (SYNTHETIC_OFFSET..FANKAI_OFFSET)
        .contains(&id)
        .then(|| id - SYNTHETIC_OFFSET)
}

/// Wrap Fankai's id for a production as a client id.
pub fn to_fankai(fankai_id: i64) -> Option<i64> {
    let encoded = FANKAI_OFFSET.checked_add(fankai_id)?;
    (fankai_id > 0 && encoded <= MAX_SONARR_ID).then_some(encoded)
}

/// Recover Fankai's id from a client id made by [`to_fankai`].
pub fn from_fankai(id: i64) -> Option<i64> {
    (id >= FANKAI_OFFSET).then(|| id - FANKAI_OFFSET)
}

/// A search term of the form `prefix:value`, which Sonarr and Radarr both use
/// to look a title up by a specific provider's id.
#[derive(Debug, PartialEq, Eq)]
pub enum TermLookup<'a> {
    Tvdb(i64),
    Tmdb(i64),
    Imdb(String),
    Mal(i64),
    AniList(i64),
    /// One of Fankai's productions, by its id there.
    Fankai(i64),
    /// An ordinary free-text search.
    Text(&'a str),
}

/// Classify a search term.
///
/// Sonarr sends `imdb:`, `tmdb:`, `mal:` and `anilist:` straight through to the
/// metadata server, and resolves `tvdb:` itself — but accepting `tvdb:` here too
/// costs nothing and makes the endpoint usable by hand. `fankai:` is this
/// server's own, for a production nothing else has an id for.
pub fn classify(term: &str) -> TermLookup<'_> {
    let trimmed = term.trim();

    let Some((prefix, rest)) = trimmed.split_once(':') else {
        return TermLookup::Text(trimmed);
    };

    let rest = rest.trim();
    let numeric = || rest.parse::<i64>().ok();

    match prefix.trim().to_ascii_lowercase().as_str() {
        "tvdb" | "tvdbid" => numeric().map_or(TermLookup::Text(trimmed), TermLookup::Tvdb),
        "tmdb" | "tmdbid" => numeric().map_or(TermLookup::Text(trimmed), TermLookup::Tmdb),
        "mal" | "myanimelist" => numeric().map_or(TermLookup::Text(trimmed), TermLookup::Mal),
        "anilist" => numeric().map_or(TermLookup::Text(trimmed), TermLookup::AniList),
        "fankai" | "fan-kai" | "fankaiid" => {
            numeric().map_or(TermLookup::Text(trimmed), TermLookup::Fankai)
        }
        "imdb" | "imdbid" => crate::domain::ids::normalize_imdb_id(rest)
            .map_or(TermLookup::Text(trimmed), TermLookup::Imdb),
        _ => TermLookup::Text(trimmed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_ids_round_trip() {
        let encoded = to_synthetic(1396).unwrap();
        assert_eq!(encoded, 100_001_396);
        assert_eq!(from_synthetic(encoded), Some(1396));
    }

    #[test]
    fn fankai_ids_take_the_second_range() {
        let encoded = to_fankai(12).unwrap();
        assert_eq!(encoded, 200_000_012);
        assert_eq!(from_fankai(encoded), Some(12));
        // Not a TMDB id in disguise, and a TMDB id never reaches into it.
        assert_eq!(from_synthetic(encoded), None);
        assert_eq!(to_synthetic(100_000_000), None);
        assert_eq!(from_fankai(100_001_396), None);
        assert_eq!(to_fankai(0), None);
        assert_eq!(to_fankai(i64::MAX), None);
    }

    #[test]
    fn real_tvdb_ids_are_never_mistaken_for_synthetic_ones() {
        // The largest TVDB series ids in circulation are around 500 000.
        for id in [1, 81189, 499_999, 1_000_000, 99_999_999] {
            assert_eq!(from_synthetic(id), None, "{id} should not look synthetic");
            assert_eq!(from_fankai(id), None, "{id} should not look like a Fan-Kai");
        }
    }

    #[test]
    fn ids_that_would_overflow_the_client_are_refused() {
        // Sonarr stores this in a 32-bit column; a value past that would be
        // truncated into a collision rather than simply failing.
        assert_eq!(to_synthetic(i64::MAX), None);
        assert_eq!(to_synthetic(2_147_483_647), None);
        assert_eq!(to_synthetic(0), None);
        assert_eq!(to_synthetic(-5), None);
    }

    #[test]
    fn prefixed_terms_are_recognised() {
        assert_eq!(classify("tvdb:81189"), TermLookup::Tvdb(81189));
        assert_eq!(classify("tvdbid: 81189"), TermLookup::Tvdb(81189));
        assert_eq!(classify("tmdb:1396"), TermLookup::Tmdb(1396));
        assert_eq!(classify("mal:5114"), TermLookup::Mal(5114));
        assert_eq!(classify("anilist:9253"), TermLookup::AniList(9253));
        assert_eq!(classify("fankai:12"), TermLookup::Fankai(12));
        assert_eq!(classify("Fan-Kai: 12"), TermLookup::Fankai(12));
        assert_eq!(
            classify("imdb:tt0903747"),
            TermLookup::Imdb("tt0903747".into())
        );
        assert_eq!(
            classify("IMDB:903747"),
            TermLookup::Imdb("tt0903747".into())
        );
    }

    #[test]
    fn a_malformed_prefix_falls_back_to_a_text_search() {
        // "Face/Off: the sequel" must not be read as a provider lookup.
        assert_eq!(
            classify("tvdb:not-a-number"),
            TermLookup::Text("tvdb:not-a-number")
        );
        assert_eq!(classify("imdb:nope"), TermLookup::Text("imdb:nope"));
        assert_eq!(
            classify("Alien: Romulus"),
            TermLookup::Text("Alien: Romulus")
        );
        assert_eq!(classify("  spaced  "), TermLookup::Text("spaced"));
    }
}

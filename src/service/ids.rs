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
//!
//! A work entered by hand that nothing elsewhere names — a series with no
//! TheTVDB, TMDB or Fankai id, a film with no TMDB id — takes a third range,
//! from 1 000 000 000 to the top of the `int` both clients keep an id in: a
//! number made of the catalogue's own id for the work ([`own`]), which nothing
//! ever changes, so that the work is handed the same one at every answer, after
//! a restart, on every instance. Radarr is handed it as the film's TMDB id, and
//! TMDB's film ids are nowhere near a billion either. Neither client takes an
//! id of nought or less for a work it adds, so the range is a positive one. A
//! client asking by such an id is answered from the works entered by hand
//! ([`crate::service::by_own_id`]); no source is asked, since none has it.

use crate::domain::{ExternalIds, MediaKind};

/// Start of the synthetic id range.
pub const SYNTHETIC_OFFSET: i64 = 100_000_000;

/// Start of the range that stands for Fankai's productions.
///
/// Above the TMDB range, which TMDB's own ids — nowhere near a hundred million
/// — never fill, so an id says which it is by its size.
pub const FANKAI_OFFSET: i64 = 200_000_000;

/// Start of the range a work entered by hand is handed an id in, when nothing
/// elsewhere names it. Above Fankai's, which its productions — a few hundred —
/// never fill.
pub const OWN_OFFSET: i64 = 1_000_000_000;

/// Largest id that still fits Sonarr's 32-bit column, and Radarr's.
const MAX_SONARR_ID: i64 = i32::MAX as i64;

/// How many ids the range of works entered by hand holds: a little over a
/// billion.
const OWN_SPAN: u64 = (MAX_SONARR_ID - OWN_OFFSET + 1) as u64;

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
///
/// `None` past the range's end, where a work entered by hand's begin.
pub fn to_fankai(fankai_id: i64) -> Option<i64> {
    let encoded = FANKAI_OFFSET.checked_add(fankai_id)?;
    (fankai_id > 0 && encoded < OWN_OFFSET).then_some(encoded)
}

/// Recover Fankai's id from a client id made by [`to_fankai`].
pub fn from_fankai(id: i64) -> Option<i64> {
    (FANKAI_OFFSET..OWN_OFFSET)
        .contains(&id)
        .then(|| id - FANKAI_OFFSET)
}

/// The id a client is handed for a work entered by hand that nothing
/// elsewhere names: made of the catalogue's own id for it, which no refresh,
/// restart or edit changes.
///
/// A hash, so two catalogue ids may make one number — a chance in a little
/// over a billion for each two works: the one entered first keeps the number
/// ([`crate::service::own_holders`]), and a work entered by hand now is given
/// a catalogue id whose number no other holds.
pub fn own(catalogue_id: &str) -> i64 {
    // FNV-1a, as `own_episode` below, on 64 bits: stable across builds and
    // platforms, and spread over the whole range.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in catalogue_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Below the span, so within an i64.
    OWN_OFFSET + (hash % OWN_SPAN) as i64
}

/// Whether a client's id is one [`own`] makes: one only a work entered by
/// hand is handed.
pub fn is_own(id: i64) -> bool {
    (OWN_OFFSET..=MAX_SONARR_ID).contains(&id)
}

/// The id Sonarr (a series) or Radarr (a film) keeps a work under: for a
/// series TheTVDB's, else one made of TMDB's or of Fankai's; for a film
/// TMDB's; and for a work entered by hand that none of those names, one made
/// of the catalogue's own id for it ([`own`]). `None` for a work the client
/// has no way to address.
pub fn for_client(
    kind: MediaKind,
    catalogue_id: &str,
    is_manual: bool,
    ids: &ExternalIds,
) -> Option<i64> {
    let named = match kind {
        MediaKind::Series => series_named(ids.tvdb, ids.tmdb, ids.fankai),
        MediaKind::Movie => ids.tmdb,
    };
    named.or_else(|| is_manual.then(|| own(catalogue_id)))
}

/// The id Sonarr keeps a series under from what names it elsewhere:
/// TheTVDB's, else one made of TMDB's, else one made of Fankai's.
pub fn series_named(tvdb: Option<i64>, tmdb: Option<i64>, fankai: Option<i64>) -> Option<i64> {
    tvdb.or_else(|| tmdb.and_then(to_synthetic))
        .or_else(|| fankai.and_then(to_fankai))
}

/// The id an episode is served under when an order carries it as a record
/// of its own and TheTVDB gave it none: one a person wrote, one of a TMDB
/// group the work holds no episode for. Sonarr keeps an episode's id in a
/// 32-bit column and matches episodes by their numbers, not by it; the id
/// is negative, so it is never one of TheTVDB's, and made of what names the
/// record — its TMDB id, or its place's own — so it is the same from one
/// answer to the next. Never zero, which Sonarr reads as none.
pub fn own_episode(names: &str) -> i64 {
    // FNV-1a: stable across builds and platforms, where `DefaultHasher`
    // promises neither.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in names.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    -(i64::from(hash % (i32::MAX as u32)) + 1)
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
        // It ends where the works entered by hand begin.
        assert_eq!(to_fankai(OWN_OFFSET - FANKAI_OFFSET), None);
        assert_eq!(from_fankai(OWN_OFFSET), None);
        assert_eq!(from_fankai(OWN_OFFSET - 1), Some(799_999_999));
    }

    #[test]
    fn real_tvdb_ids_are_never_mistaken_for_synthetic_ones() {
        // The largest TVDB series ids in circulation are around 500 000.
        for id in [1, 81189, 499_999, 1_000_000, 99_999_999] {
            assert_eq!(from_synthetic(id), None, "{id} should not look synthetic");
            assert_eq!(from_fankai(id), None, "{id} should not look like a Fan-Kai");
            assert!(!is_own(id), "{id} should not look entered by hand");
        }
    }

    #[test]
    fn a_work_entered_by_hand_takes_the_third_range_under_a_number_of_its_own_id() {
        let id = "01a17789-b167-736d-8873-605960d7e943";
        let handed = own(id);
        // The same from one answer, one restart, one build to the next.
        assert_eq!(handed, own(id));
        assert_eq!(handed, 1_827_537_293);
        assert!(is_own(handed));
        // In no other range, and within the clients' column.
        assert_eq!(from_synthetic(handed), None);
        assert_eq!(from_fankai(handed), None);
        assert!(handed <= i64::from(i32::MAX));
        assert_eq!(own("01a17789-b191-73f2-9253-421af11df012"), 1_564_715_190);
        // The top and the bottom of the range, and nothing past either.
        assert!(is_own(OWN_OFFSET) && is_own(i64::from(i32::MAX)));
        assert!(!is_own(OWN_OFFSET - 1) && !is_own(i64::from(i32::MAX) + 1));
        assert!(!is_own(-5));
    }

    #[test]
    fn a_client_is_handed_what_names_a_work_elsewhere_before_its_own_number() {
        let named = ExternalIds {
            tvdb: Some(81189),
            tmdb: Some(1396),
            ..Default::default()
        };
        let id = "01a17789-b167-736d-8873-605960d7e943";
        assert_eq!(for_client(MediaKind::Series, id, true, &named), Some(81189));
        assert_eq!(for_client(MediaKind::Movie, id, true, &named), Some(1396));

        let nothing = ExternalIds::default();
        // Entered by hand: its own number, for either client.
        assert_eq!(
            for_client(MediaKind::Series, id, true, &nothing),
            Some(own(id))
        );
        assert_eq!(
            for_client(MediaKind::Movie, id, true, &nothing),
            Some(own(id))
        );
        // A work a source gave with none of them is no client's.
        assert_eq!(for_client(MediaKind::Series, id, false, &nothing), None);
        // An IMDb id alone names no series to Sonarr, nor a film to Radarr.
        let imdb = ExternalIds {
            imdb: Some("tt0903747".into()),
            ..Default::default()
        };
        assert_eq!(for_client(MediaKind::Movie, id, true, &imdb), Some(own(id)));
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
    fn an_episode_of_an_order_s_own_is_given_a_negative_id_that_stays() {
        let first = own_episode("tmdb:170135");
        assert!(first < 0 && first >= -i64::from(i32::MAX), "{first}");
        assert_eq!(own_episode("tmdb:170135"), first);
        assert_ne!(own_episode("tmdb:170132"), first);
        assert!(own_episode("") < 0);
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

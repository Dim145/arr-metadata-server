//! Kodi/XBMC `.nfo` documents.
//!
//! For Kodi, and for a Plex library that reads files beside the media — its
//! Personal Media / XBMCnfo agent — rather than taking this server as its
//! metadata provider, which Plex Media Server 1.43 and later can (`/plex`, see
//! `docs/plex.md`).
//!
//! Note that if Sonarr or Radarr manage the library, enabling *their* Kodi
//! metadata writer is the better route: they already write these files next to
//! the media, from the data this server gave them, so your edits arrive without
//! anything here being involved. What this module is for is the rest — manual
//! entries no arr manages, and libraries not run by the arr stack.

use std::fmt::Write as _;

use crate::{
    domain::{
        CoverType, Credit, CreditType, Episode, EpisodeOrder, MediaItem, MediaKind, OrderKind,
        person::PersonRef,
    },
    service::orders,
};

/// Escape text for an XML text node or a double-quoted attribute.
///
/// Both contexts are covered by one function: escaping `"` and `'` inside a text
/// node is harmless, and forgetting them inside an attribute is not.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 forbids most control characters outright; a stray one in
            // provider data would make the whole document unparsable.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            // And the two code points that are no characters: a string may
            // hold them, a document may not. The feeds and the Kometa file
            // leave them out too.
            '\u{FFFE}' | '\u{FFFF}' => {}
            c => out.push(c),
        }
    }

    out
}

/// Write `<tag>value</tag>`, or nothing when the value is absent or blank.
fn tag(out: &mut String, name: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };

    let _ = writeln!(out, "  <{name}>{}</{name}>", escape(value));
}

fn tag_num<T: std::fmt::Display>(out: &mut String, name: &str, value: Option<T>) {
    if let Some(value) = value {
        let _ = writeln!(out, "  <{name}>{value}</{name}>");
    }
}

const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// The collection a film belongs to, as Kodi's `<set>` names it: the same
/// name in every film of the collection is what puts them together.
pub struct Collection<'a> {
    pub name: &'a str,
    pub overview: Option<&'a str>,
}

/// The document for a work that is in no collection, or whose collection is
/// not asked about: what the tests of the document read. The export goes by
/// [`for_item_in`].
#[cfg(test)]
pub fn for_item(item: &MediaItem) -> String {
    for_item_in(item, None, &[])
}

/// The document for a work: `<tvshow>` or `<movie>` — a film's with the
/// collection it belongs to, when its name is known: TMDB gives the catalogue
/// the collection's number, and the name is a second question to ask it, so the
/// caller that has it hands it in.
///
/// A series' with the orders the catalogue keeps for it
/// (`repo::order::list`): one frozen as the aired numbering is named to
/// Jellyfin, which numbers the series by the order its `<displayorder>`
/// names — TheTVDB's type, through its TheTVDB plugin; TMDB's, through its
/// TMDb provider, with the group's id beside it. An order made here is no
/// order Jellyfin knows, and is not named; its episodes' documents carry the
/// numbers it gives them all the same.
pub fn for_item_in(
    item: &MediaItem,
    collection: Option<&Collection<'_>>,
    orders: &[EpisodeOrder],
) -> String {
    let frozen = orders
        .iter()
        .find(|order| order.is_frozen && !order.is_default);
    let root = match item.kind {
        MediaKind::Series => "tvshow",
        MediaKind::Movie => "movie",
    };

    let mut out = String::with_capacity(4096);
    out.push_str(HEADER);
    let _ = writeln!(out, "<{root}>");

    tag(&mut out, "title", Some(&item.title));
    tag(&mut out, "originaltitle", item.original_title.as_deref());
    tag(&mut out, "sorttitle", item.sort_title.as_deref());
    tag(&mut out, "plot", item.overview.as_deref());
    tag(&mut out, "outline", item.overview.as_deref());
    tag(&mut out, "tagline", item.tagline.as_deref());
    tag_num(&mut out, "year", item.year);
    tag_num(&mut out, "runtime", item.runtime);
    tag(&mut out, "mpaa", item.content_rating.as_deref());
    tag(&mut out, "studio", item.studio.as_deref());

    match item.kind {
        MediaKind::Series => {
            tag(&mut out, "premiered", item.first_aired.as_deref());
            tag(&mut out, "status", item.status.as_deref());
            // Kodi expects the broadcaster here, which our `network` is.
            tag(&mut out, "studio", item.network.as_deref());
            tag(
                &mut out,
                "displayorder",
                frozen.and_then(display_order).as_deref(),
            );
        }
        MediaKind::Movie => {
            tag(&mut out, "premiered", item.in_cinemas.as_deref());
            tag(&mut out, "releasedate", item.digital_release.as_deref());
        }
    }

    // Every company that made it beside the studio already written, and
    // every country it was made in, named as Kodi's own scraper names them.
    let written = [item.studio.as_deref(), item.network.as_deref()];
    for company in &item.production_companies {
        let again = written
            .iter()
            .flatten()
            .any(|w| w.trim().eq_ignore_ascii_case(company.trim()));
        if !again {
            tag(&mut out, "studio", Some(company));
        }
    }
    for country in &item.production_countries {
        let name = crate::providers::lang::country_name(country).unwrap_or(country);
        tag(&mut out, "country", Some(name));
    }

    for genre in &item.genres {
        tag(&mut out, "genre", Some(genre));
    }
    for keyword in &item.keywords {
        tag(&mut out, "tag", Some(keyword));
    }

    if item.kind == MediaKind::Movie
        && let Some(collection) = collection
        && !collection.name.trim().is_empty()
    {
        // The form Kodi has read since Jarvis: a name, and what the
        // collection is about. The plain `<set>Name</set>` it reads too is
        // the older one, which says no more.
        out.push_str("  <set>\n");
        let _ = writeln!(out, "    <name>{}</name>", escape(collection.name.trim()));
        if let Some(overview) = collection.overview.map(str::trim).filter(|o| !o.is_empty()) {
            let _ = writeln!(out, "    <overview>{}</overview>", escape(overview));
        }
        out.push_str("  </set>\n");
    }

    if item.kind == MediaKind::Series {
        season_titles(&mut out, item);
    }

    unique_ids(&mut out, item);
    // The group a frozen TMDB order is: what Jellyfin's TMDb provider
    // numbers the series by, whatever its display order says.
    if let Some(group) = frozen
        .filter(|order| order.source == crate::providers::names::TMDB)
        .and_then(|order| order.source_ref.as_deref())
    {
        let _ = writeln!(
            out,
            "  <uniqueid type=\"TmdbEpisodeGroupId\">{}</uniqueid>",
            escape(group)
        );
    }
    ratings(&mut out, item);
    artwork(&mut out, item);

    if let Some(id) = &item.trailer_youtube_id {
        // The form Kodi and Jellyfin both recognise.
        tag(
            &mut out,
            "trailer",
            Some(&format!(
                "plugin://plugin.video.youtube/?action=play_video&videoid={id}"
            )),
        );
    }

    for credit in &item.credits {
        write_credit(&mut out, credit, item.people.of(credit));
    }

    let _ = writeln!(out, "</{root}>");
    out
}

/// The document for one episode: `<episodedetails>`. With the orders the
/// catalogue keeps for the series, a special one of them places among the
/// regular seasons is displayed there by Kodi — `displayseason`, and
/// `displayepisode`, the episode it comes before — where TheTVDB's own hint
/// of where it aired says nothing.
pub fn for_episode(item: &MediaItem, episode: &Episode, orders: &[EpisodeOrder]) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str(HEADER);
    out.push_str("<episodedetails>\n");

    let (display_season, display_episode) = match (
        episode.aired_before_season_number,
        episode.aired_before_episode_number,
    ) {
        (None, None) if episode.season_number == 0 => {
            special_place(item, episode, orders).unwrap_or_default()
        }
        given => given,
    };

    tag(&mut out, "title", Some(&episode.title));
    tag(&mut out, "showtitle", Some(&item.title));
    tag_num(&mut out, "season", Some(episode.season_number));
    tag_num(&mut out, "episode", Some(episode.episode_number));
    tag_num(&mut out, "displayseason", display_season);
    tag_num(&mut out, "displayepisode", display_episode);
    tag(&mut out, "plot", episode.overview.as_deref());
    tag(&mut out, "aired", episode.air_date.as_deref());
    tag_num(&mut out, "runtime", episode.runtime);

    if let Some(rating) = episode.rating {
        let _ = writeln!(
            out,
            "  <rating name=\"tmdb\" max=\"10\" default=\"true\">\n    <value>{:.1}</value>\n    <votes>{}</votes>\n  </rating>",
            rating.value, rating.votes
        );
    }

    if let Some(url) = &episode.image {
        let _ = writeln!(out, "  <thumb>{}</thumb>", escape(url));
    }

    // The episode's own ids, TheTVDB's first: one of them, and only one, is
    // the default — an episode known by TMDB alone is known by it.
    let ids = [("tvdb", episode.tvdb_id), ("tmdb", episode.tmdb_id)];
    for (index, (kind, id)) in ids
        .iter()
        .filter_map(|(kind, id)| Some((*kind, (*id)?)))
        .enumerate()
    {
        let default = if index == 0 { " default=\"true\"" } else { "" };
        let _ = writeln!(out, "  <uniqueid type=\"{kind}\"{default}>{id}</uniqueid>");
    }

    out.push_str("</episodedetails>\n");
    out
}

/// What Jellyfin calls an order in a `<displayorder>`: TheTVDB's own type
/// for one of its orders, as its TheTVDB plugin compares it; for a TMDB
/// group, the order its TMDb provider maps to the group's type. None for an
/// order made here, which Jellyfin cannot ask anybody for.
fn display_order(order: &EpisodeOrder) -> Option<String> {
    match order.source.as_str() {
        crate::providers::names::TVDB => Some(order.key.clone()),
        crate::providers::names::TMDB => Some(
            match order.kind {
                OrderKind::Aired => "originalAirDate",
                OrderKind::Absolute => "absolute",
                OrderKind::Dvd => "dvd",
                OrderKind::Digital => "digital",
                OrderKind::StoryArc => "storyArc",
                OrderKind::Production => "production",
                OrderKind::Tv => "tv",
                OrderKind::TvdbAlt | OrderKind::Custom => return None,
            }
            .to_string(),
        ),
        _ => None,
    }
}

/// Where the first order that places a special among the regular seasons —
/// in the order they are offered, the DVDs' first; the one the episodes are
/// numbered by aside — puts it, in the numbers the library goes by: the
/// season and number of the episode the order has after it, or, at the end
/// of a season, that season alone.
fn special_place(
    item: &MediaItem,
    episode: &Episode,
    kept: &[EpisodeOrder],
) -> Option<(Option<i32>, Option<i32>)> {
    kept.iter()
        .filter(|order| !order.is_default && !order.is_frozen)
        .find_map(|order| {
            let mut placed = orders::placements(item, order);
            placed.sort_by_key(|p| (p.season_number, p.episode_number));
            let at = placed
                .iter()
                .position(|p| !p.own && p.episode.id == episode.id)?;
            let season = placed[at].season_number;
            if season == 0 {
                return None;
            }
            let regular = |p: &&orders::Placed<'_>| {
                p.season_number == season && !p.own && p.episode.season_number > 0
            };
            if let Some(next) = placed[at + 1..].iter().find(regular) {
                return Some((
                    Some(next.episode.season_number),
                    Some(next.episode.episode_number),
                ));
            }
            placed[..at]
                .iter()
                .rev()
                .find(regular)
                .map(|before| (Some(before.episode.season_number), None))
        })
}

fn unique_ids(out: &mut String, item: &MediaItem) {
    let ids = &item.external_ids;

    // In the order they are written.
    let mut known: Vec<(&str, String)> = Vec::new();
    if let Some(v) = ids.tvdb {
        known.push(("tvdb", v.to_string()));
    }
    if let Some(v) = ids.tmdb {
        known.push(("tmdb", v.to_string()));
    }
    if let Some(v) = &ids.imdb {
        known.push(("imdb", v.clone()));
    }
    if let Some(v) = ids.fankai {
        known.push(("fankai", v.to_string()));
    }

    // Exactly one id is marked default — both Kodi, which takes the first
    // that is, and Plex's NFO agent, which builds the work's identity from
    // it, read it so — and it should be the one the client's own agent keys
    // on. Failing that, the first the work has: a series known by IMDb alone
    // is known by it, and one with no default is known by none.
    let preferred: &[&str] = match item.kind {
        MediaKind::Series => &["tvdb", "tmdb", "fankai"],
        MediaKind::Movie => &["tmdb", "imdb"],
    };
    let primary = preferred
        .iter()
        .find(|kind| known.iter().any(|(k, _)| k == *kind))
        .copied()
        .or_else(|| known.first().map(|(kind, _)| *kind));

    for (kind, value) in &known {
        let default = if Some(*kind) == primary {
            " default=\"true\""
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  <uniqueid type=\"{kind}\"{default}>{}</uniqueid>",
            escape(value)
        );
    }
}

/// The word for a season in the languages the catalogue's providers name
/// them in: what they fall back on, as a title, when a season has none.
const SEASON_WORDS: &[&str] = &[
    "season",
    "series",
    "saison",
    "staffel",
    "temporada",
    "stagione",
    "seizoen",
    "säsong",
    "sæson",
    "sesong",
    "kausi",
    "sezon",
    "сезон",
];

/// Whether a season's title is no more than its number said in words —
/// `Season 3`, `Saison 03`, `Specials` — which a media server writes for
/// itself, in the reader's language, and which an English one would
/// overwrite. Only the words providers fall back on: a *Part 1* is a title.
fn is_numbered(title: &str, number: i32) -> bool {
    let lower = title.trim().to_lowercase();
    let mut words = lower.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        // `Season 3`, `Saison 03`, `Staffel 3`.
        (Some(word), Some(given), None) => {
            SEASON_WORDS.contains(&word) && given.parse::<i32>().is_ok_and(|n| n == number)
        }
        // The season of the specials, as it is called when nothing else is
        // said of it.
        (Some("specials"), None, None) => number == 0,
        _ => false,
    }
}

/// The titles of seasons that have one, as `<namedseason>`: what a season is
/// called beside its number — *Book One: Water* — which Kodi shows and the
/// numbered title a provider falls back on is not.
fn season_titles(out: &mut String, item: &MediaItem) {
    for season in &item.seasons {
        let Some(title) = season
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty() && !is_numbered(title, season.season_number))
        else {
            continue;
        };
        let _ = writeln!(
            out,
            "  <namedseason number=\"{}\">{}</namedseason>",
            season.season_number,
            escape(title)
        );
    }
}

fn ratings(out: &mut String, item: &MediaItem) {
    if item.ratings.is_empty() {
        return;
    }

    out.push_str("  <ratings>\n");

    for (i, rating) in item.ratings.iter().enumerate() {
        let default = if i == 0 { " default=\"true\"" } else { "" };
        let _ = writeln!(
            out,
            "    <rating name=\"{}\" max=\"10\"{default}>\n      <value>{:.1}</value>\n      <votes>{}</votes>\n    </rating>",
            escape(&rating.source),
            rating.value.unwrap_or(0.0),
            rating.votes.unwrap_or(0)
        );
    }

    out.push_str("  </ratings>\n");
}

fn artwork(out: &mut String, item: &MediaItem) {
    for image in &item.images {
        match image.cover_type {
            CoverType::Poster => {
                let season = image
                    .season_number
                    .map(|n| format!(" type=\"season\" season=\"{n}\""))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "  <thumb aspect=\"poster\"{season}>{}</thumb>",
                    escape(&image.url)
                );
            }
            CoverType::Banner => {
                let _ = writeln!(
                    out,
                    "  <thumb aspect=\"banner\">{}</thumb>",
                    escape(&image.url)
                );
            }
            CoverType::Clearlogo => {
                let _ = writeln!(
                    out,
                    "  <thumb aspect=\"clearlogo\">{}</thumb>",
                    escape(&image.url)
                );
            }
            CoverType::Fanart => {
                // Kodi nests fanart, unlike every other kind.
                let _ = writeln!(
                    out,
                    "  <fanart>\n    <thumb>{}</thumb>\n  </fanart>",
                    escape(&image.url)
                );
            }
            _ => {}
        }
    }

    // A season's own pictures are kept on the season, not among the work's:
    // the first of its posters — the one chosen, where one was — is what
    // Kodi shows for it.
    for season in &item.seasons {
        if let Some(poster) = season
            .images
            .iter()
            .find(|image| image.cover_type == CoverType::Poster)
        {
            let _ = writeln!(
                out,
                "  <thumb aspect=\"poster\" type=\"season\" season=\"{}\">{}</thumb>",
                season.season_number,
                escape(&poster.url)
            );
        }
    }
}

/// One credit, as the element Kodi reads its kind from. An actor is named
/// with the ids their record is known by — TMDB's, else the credit's own,
/// and IMDb's — which Jellyfin and Emby file the person under.
fn write_credit(out: &mut String, credit: &Credit, person: Option<&PersonRef>) {
    match credit.credit_type {
        CreditType::Actor | CreditType::Guest => {
            out.push_str("  <actor>\n");
            let _ = writeln!(out, "    <name>{}</name>", escape(&credit.person_name));
            if let Some(role) = &credit.character_name {
                let _ = writeln!(out, "    <role>{}</role>", escape(role));
            }
            let _ = writeln!(out, "    <order>{}</order>", credit.sort_order);
            if let Some(image) = &credit.image {
                let _ = writeln!(out, "    <thumb>{}</thumb>", escape(image));
            }
            if let Some(id) = person.and_then(|p| p.tmdb_id).or(credit.tmdb_person_id) {
                let _ = writeln!(out, "    <tmdbid>{id}</tmdbid>");
            }
            if let Some(id) = person
                .and_then(|p| p.imdb_id.as_deref())
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                let _ = writeln!(out, "    <imdbid>{}</imdbid>", escape(id));
            }
            out.push_str("  </actor>\n");
        }
        CreditType::Director => tag(out, "director", Some(&credit.person_name)),
        CreditType::Writer => tag(out, "credits", Some(&credit.person_name)),
        CreditType::Producer => tag(out, "producer", Some(&credit.person_name)),
    }
}

/// Where this work's document belongs, relative to an export root.
///
/// Kodi and Plex both look for `tvshow.nfo` at a series' root and `movie.nfo`
/// beside a film. The slug directory is ours: nothing can know the layout of
/// somebody else's library.
pub fn relative_path(item: &MediaItem) -> String {
    match item.kind {
        MediaKind::Series => format!("series/{}/tvshow.nfo", item.slug),
        MediaKind::Movie => format!("movies/{}/movie.nfo", item.slug),
    }
}

pub fn episode_relative_path(item: &MediaItem, episode: &Episode) -> String {
    format!(
        "series/{}/Season {:02}/S{:02}E{:02}.nfo",
        item.slug, episode.season_number, episode.season_number, episode.episode_number
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{ExternalIds, Image, Rating, RatingValue},
        export::xml,
    };

    fn series() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Breaking Bad".into();
        item.slug = "breaking-bad-2008".into();
        item.overview = Some("A teacher & a student.".into());
        item.year = Some(2008);
        item.runtime = Some(48);
        item.status = Some("ended".into());
        item.network = Some("AMC".into());
        item.content_rating = Some("TV-MA".into());
        item.first_aired = Some("2008-01-20".into());
        item.genres = vec!["Drama".into(), "Crime".into()];
        item.external_ids = ExternalIds {
            tvdb: Some(81189),
            tmdb: Some(1396),
            imdb: Some("tt0903747".into()),
            ..Default::default()
        };
        item.ratings = vec![Rating {
            source: "tmdb".into(),
            value: Some(8.9),
            votes: Some(18661),
            rating_type: Some("user".into()),
        }];
        item.images = vec![
            Image {
                id: "1".into(),
                season_number: None,
                cover_type: CoverType::Poster,
                url: "https://example.invalid/p.jpg".into(),
                language: None,
                sort_order: 0,
                source: None,
                is_manual: false,
            },
            Image {
                id: "2".into(),
                season_number: None,
                cover_type: CoverType::Fanart,
                url: "https://example.invalid/b.jpg?a=1&b=2".into(),
                language: None,
                sort_order: 0,
                source: None,
                is_manual: false,
            },
        ];
        item.credits = vec![
            Credit {
                id: "c1".into(),
                credit_type: CreditType::Actor,
                person_name: "Bryan Cranston".into(),
                character_name: Some("Walter White".into()),
                image: None,
                tmdb_person_id: None,
                credit_tmdb_id: None,
                sort_order: 0,
                is_manual: false,
            },
            Credit {
                id: "c2".into(),
                credit_type: CreditType::Director,
                person_name: "Vince Gilligan".into(),
                character_name: None,
                image: None,
                tmdb_person_id: None,
                credit_tmdb_id: None,
                sort_order: 0,
                is_manual: false,
            },
        ];
        item
    }

    #[test]
    fn text_is_escaped_for_xml() {
        assert_eq!(escape("a & b"), "a &amp; b");
        assert_eq!(escape("<tag>"), "&lt;tag&gt;");
        assert_eq!(escape(r#"say "hi""#), "say &quot;hi&quot;");
        assert_eq!(escape("it's"), "it&apos;s");
        // A control character would make the document unparsable.
        assert_eq!(escape("a\u{0007}b"), "ab");
        assert_eq!(
            escape("keep\ttabs\nand newlines"),
            "keep\ttabs\nand newlines"
        );
    }

    #[test]
    fn the_two_code_points_xml_does_not_admit_are_left_out_of_a_document() {
        // A Rust string may hold U+FFFE and U+FFFF and an XML 1.0 document
        // may not: one in a provider's title was written as it came, and a
        // strict reader refused the whole file.
        assert_eq!(escape("a\u{FFFE}b\u{FFFF}c"), "abc");
        // Their neighbours stay.
        assert_eq!(escape("\u{FFFD}\u{10000}"), "\u{FFFD}\u{10000}");

        let mut item = series();
        item.title = "Break\u{FFFE}ing Bad".into();
        item.overview = Some("A teacher\u{FFFF}.".into());
        let nfo = for_item(&item);
        assert!(nfo.contains("<title>Breaking Bad</title>"), "{nfo}");
        assert!(nfo.contains("<plot>A teacher.</plot>"), "{nfo}");
        assert!(!nfo.contains(['\u{FFFE}', '\u{FFFF}']));
    }

    #[test]
    fn a_fan_kai_is_keyed_on_fankai_s_id() {
        // Nothing else names a recut, so Fankai's id is the one the client's
        // agent can key on — and the only one written.
        let mut item = series();
        item.external_ids = ExternalIds {
            fankai: Some(12),
            ..Default::default()
        };
        let nfo = for_item(&item);

        assert!(nfo.contains("<uniqueid type=\"fankai\" default=\"true\">12</uniqueid>"));
        assert!(!nfo.contains("type=\"tvdb\""));
        assert!(!nfo.contains("type=\"tmdb\""));
    }

    #[test]
    fn fankai_s_id_rides_beside_the_others_without_taking_the_default() {
        let mut item = series();
        item.external_ids.fankai = Some(12);
        let nfo = for_item(&item);

        assert!(nfo.contains("<uniqueid type=\"tvdb\" default=\"true\">81189</uniqueid>"));
        assert!(nfo.contains("<uniqueid type=\"fankai\">12</uniqueid>"));
    }

    #[test]
    fn a_series_document_carries_what_kodi_reads() {
        let nfo = for_item(&series());

        assert!(nfo.starts_with("<?xml version=\"1.0\""));
        assert!(nfo.contains("<tvshow>"));
        assert!(nfo.contains("<title>Breaking Bad</title>"));
        assert!(nfo.contains("<plot>A teacher &amp; a student.</plot>"));
        assert!(nfo.contains("<year>2008</year>"));
        assert!(nfo.contains("<mpaa>TV-MA</mpaa>"));
        assert!(nfo.contains("<premiered>2008-01-20</premiered>"));
        assert!(nfo.contains("<genre>Drama</genre>"));
        assert!(nfo.contains("<genre>Crime</genre>"));
        assert!(nfo.ends_with("</tvshow>\n"));
    }

    #[test]
    fn exactly_one_identifier_is_marked_default() {
        let nfo = for_item(&series());

        assert_eq!(
            nfo.matches("default=\"true\"").count(),
            2,
            "one id, one rating"
        );
        assert!(nfo.contains("<uniqueid type=\"tvdb\" default=\"true\">81189</uniqueid>"));
        assert!(nfo.contains("<uniqueid type=\"tmdb\">1396</uniqueid>"));
        assert!(nfo.contains("<uniqueid type=\"imdb\">tt0903747</uniqueid>"));
    }

    #[test]
    fn a_movie_keys_on_tmdb_rather_than_tvdb() {
        let mut item = series();
        item.kind = MediaKind::Movie;

        let nfo = for_item(&item);
        assert!(nfo.contains("<movie>"));
        assert!(nfo.contains("<uniqueid type=\"tmdb\" default=\"true\">1396</uniqueid>"));
        assert!(nfo.contains("<uniqueid type=\"tvdb\">81189</uniqueid>"));
    }

    #[test]
    fn artwork_uses_the_shape_each_kind_needs() {
        let nfo = for_item(&series());

        assert!(nfo.contains("<thumb aspect=\"poster\">https://example.invalid/p.jpg</thumb>"));
        // Fanart nests, and the query string has to survive escaping.
        assert!(nfo.contains(
            "<fanart>\n    <thumb>https://example.invalid/b.jpg?a=1&amp;b=2</thumb>\n  </fanart>"
        ));
    }

    #[test]
    fn credits_are_split_by_the_element_kodi_expects() {
        let nfo = for_item(&series());

        assert!(
            nfo.contains("<actor>\n    <name>Bryan Cranston</name>\n    <role>Walter White</role>")
        );
        assert!(nfo.contains("<director>Vince Gilligan</director>"));
        assert!(!nfo.contains("<tmdbid>"), "nobody here is known by an id");
    }

    #[test]
    fn an_actor_carries_the_ids_their_record_is_known_by() {
        let mut item = series();
        item.credits[0].tmdb_person_id = Some(17419);
        item.people.by_credit.insert(
            "c1".into(),
            PersonRef {
                person_id: Some("p1".into()),
                tmdb_id: Some(17419),
                imdb_id: Some("nm0186505".into()),
                ..PersonRef::default()
            },
        );
        let nfo = for_item(&item);
        assert!(
            nfo.contains("    <tmdbid>17419</tmdbid>\n    <imdbid>nm0186505</imdbid>\n  </actor>"),
            "{nfo}"
        );

        // A credit by name alone filed under somebody TMDB knows is named by
        // their record's id; one TMDB names, by its own, record or not.
        item.credits[0].tmdb_person_id = None;
        item.people.by_credit.get_mut("c1").expect("held").imdb_id = None;
        let nfo = for_item(&item);
        assert!(nfo.contains("<tmdbid>17419</tmdbid>"));
        assert!(!nfo.contains("<imdbid>"));
        item.people.by_credit.clear();
        item.credits[0].tmdb_person_id = Some(17419);
        assert!(for_item(&item).contains("<tmdbid>17419</tmdbid>"));
    }

    #[test]
    fn absent_values_produce_no_empty_elements() {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "Bare".into();

        let nfo = for_item(&item);
        assert!(!nfo.contains("<plot>"));
        assert!(!nfo.contains("<year>"));
        assert!(!nfo.contains("<mpaa>"));
        assert!(!nfo.contains("<ratings>"));
        assert!(nfo.contains("<title>Bare</title>"));
    }

    #[test]
    fn an_episode_document_names_its_place_in_the_run() {
        let item = series();
        let mut episode = crate::db::repo::child::blank_episode(1, 1);
        episode.title = "Pilot".into();
        episode.overview = Some("It begins.".into());
        episode.air_date = Some("2008-01-20".into());
        episode.runtime = Some(58);
        episode.rating = Some(RatingValue {
            value: 8.3,
            votes: 260,
        });

        let nfo = for_episode(&item, &episode, &[]);

        assert!(nfo.contains("<episodedetails>"));
        assert!(nfo.contains("<title>Pilot</title>"));
        assert!(nfo.contains("<showtitle>Breaking Bad</showtitle>"));
        assert!(nfo.contains("<season>1</season>"));
        assert!(nfo.contains("<episode>1</episode>"));
        assert!(nfo.contains("<aired>2008-01-20</aired>"));
        assert!(nfo.contains("<value>8.3</value>"));
    }

    fn order(key: &str, source: &str, kind: OrderKind, frozen: bool) -> EpisodeOrder {
        EpisodeOrder {
            key: key.into(),
            kind,
            source: source.into(),
            source_ref: (source == "tmdb").then(|| "5acf93e60e0a26346d0000ce".to_string()),
            label: key.into(),
            is_default: false,
            is_frozen: frozen,
            announced: None,
            updated_at: "2026-10-19T00:00:00.000Z".into(),
            episodes: Vec::new(),
        }
    }

    #[test]
    fn a_frozen_order_is_named_to_jellyfin_as_it_knows_it() {
        let item = series();
        let nfo = for_item_in(&item, None, &[order("dvd", "tvdb", OrderKind::Dvd, true)]);
        assert!(nfo.contains("<displayorder>dvd</displayorder>"), "{nfo}");
        assert!(!nfo.contains("TmdbEpisodeGroupId"), "{nfo}");

        // A TMDB group: the order Jellyfin's TMDb provider maps to its type,
        // and the group itself.
        let nfo = for_item_in(
            &item,
            None,
            &[order(
                "tmdb-5acf93e60e0a26346d0000ce",
                "tmdb",
                OrderKind::StoryArc,
                true,
            )],
        );
        assert!(
            nfo.contains("<displayorder>storyArc</displayorder>"),
            "{nfo}"
        );
        assert!(
            nfo.contains(
                "<uniqueid type=\"TmdbEpisodeGroupId\">5acf93e60e0a26346d0000ce</uniqueid>"
            ),
            "{nfo}"
        );
        // Read to the end by a parser: one default id still, the group's
        // beside it.
        let elements = xml::read(&nfo);
        let defaults = xml::at(&elements, "tvshow/uniqueid")
            .into_iter()
            .filter(|id| id.attribute("default") == Some("true"))
            .count();
        assert_eq!(defaults, 1, "{nfo}");

        // One made here is no order Jellyfin knows; one not frozen, none to
        // name.
        for orders in [
            vec![order("ams-mine", "local", OrderKind::Custom, true)],
            vec![order("dvd", "tvdb", OrderKind::Dvd, false)],
        ] {
            let nfo = for_item_in(&item, None, &orders);
            assert!(!nfo.contains("displayorder"), "{nfo}");
        }
    }

    #[test]
    fn a_special_an_order_places_among_the_seasons_is_displayed_there() {
        let mut item = series();
        let mut special = crate::db::repo::child::blank_episode(0, 1);
        special.id = "special".into();
        special.title = "Minisode".into();
        special.tvdb_id = Some(900);
        item.episodes = vec![special.clone()];
        for (number, tvdb_id) in [(1, 101), (2, 102), (3, 103)] {
            let mut episode = crate::db::repo::child::blank_episode(1, number);
            episode.id = format!("e{number}");
            episode.tvdb_id = Some(tvdb_id);
            item.episodes.push(episode);
        }
        // The DVDs put the special between the first two episodes.
        let mut dvd = order("dvd", "tvdb", OrderKind::Dvd, false);
        dvd.episodes = [(101, 1), (900, 2), (102, 3), (103, 4)]
            .into_iter()
            .map(|(id, number)| crate::domain::PlacedEpisode::of_tvdb(id, 1, number))
            .collect();

        let nfo = for_episode(&item, &special, std::slice::from_ref(&dvd));
        assert!(nfo.contains("<displayseason>1</displayseason>"), "{nfo}");
        assert!(nfo.contains("<displayepisode>2</displayepisode>"), "{nfo}");

        // Last of its season in the order: that season, and no episode.
        dvd.episodes = [(101, 1), (102, 2), (103, 3), (900, 4)]
            .into_iter()
            .map(|(id, number)| crate::domain::PlacedEpisode::of_tvdb(id, 1, number))
            .collect();
        let nfo = for_episode(&item, &special, std::slice::from_ref(&dvd));
        assert!(nfo.contains("<displayseason>1</displayseason>"), "{nfo}");
        assert!(!nfo.contains("displayepisode"), "{nfo}");

        // TheTVDB's own word on where it aired comes first; and no order,
        // nothing said.
        let mut hinted = special.clone();
        hinted.aired_before_season_number = Some(2);
        let nfo = for_episode(&item, &hinted, std::slice::from_ref(&dvd));
        assert!(nfo.contains("<displayseason>2</displayseason>"), "{nfo}");
        let nfo = for_episode(&item, &special, &[]);
        assert!(!nfo.contains("displayseason"), "{nfo}");
    }

    #[test]
    fn paths_follow_the_layout_kodi_and_plex_look_in() {
        let item = series();
        assert_eq!(relative_path(&item), "series/breaking-bad-2008/tvshow.nfo");

        let episode = crate::db::repo::child::blank_episode(2, 7);
        assert_eq!(
            episode_relative_path(&item, &episode),
            "series/breaking-bad-2008/Season 02/S02E07.nfo"
        );

        let mut movie = item.clone();
        movie.kind = MediaKind::Movie;
        movie.slug = "arrival-2016".into();
        assert_eq!(relative_path(&movie), "movies/arrival-2016/movie.nfo");
    }

    /// The `type`s of a document's identifiers, and which of them is default.
    fn identifiers(nfo: &str, root: &str) -> Vec<(String, bool)> {
        let elements = xml::read(nfo);
        xml::at(&elements, &format!("{root}/uniqueid"))
            .into_iter()
            .map(|e| {
                (
                    format!("{}={}", e.attribute("type").unwrap_or("?"), e.text),
                    e.attribute("default") == Some("true"),
                )
            })
            .collect()
    }

    #[test]
    fn a_work_is_given_exactly_one_default_identifier_whatever_it_is_known_by() {
        let known = |kind: MediaKind, ids: ExternalIds| {
            let mut item = MediaItem::empty(kind);
            item.title = "W".into();
            item.external_ids = ids;
            let root = if kind == MediaKind::Movie {
                "movie"
            } else {
                "tvshow"
            };
            identifiers(&for_item(&item), root)
        };
        let defaults = |all: &[(String, bool)]| all.iter().filter(|(_, d)| *d).count();

        // A series by TheTVDB, a film by TMDB: what their agents key on.
        let full = ExternalIds {
            tvdb: Some(1),
            tmdb: Some(2),
            imdb: Some("tt3".into()),
            fankai: Some(4),
            ..Default::default()
        };
        let series = known(MediaKind::Series, full.clone());
        assert_eq!(defaults(&series), 1);
        assert_eq!(
            series[0],
            ("tvdb=1".to_string(), true),
            "the series' own, first"
        );
        let film = known(MediaKind::Movie, full);
        assert_eq!(defaults(&film), 1);
        assert!(film.contains(&("tmdb=2".to_string(), true)));

        // Known by nothing a client's agent keys on: by the first it has.
        let imdb_only = ExternalIds {
            imdb: Some("tt3".into()),
            ..Default::default()
        };
        assert_eq!(
            known(MediaKind::Series, imdb_only),
            [("imdb=tt3".to_string(), true)]
        );
        let tvdb_only = ExternalIds {
            tvdb: Some(9),
            ..Default::default()
        };
        assert_eq!(
            known(MediaKind::Movie, tvdb_only),
            [("tvdb=9".to_string(), true)]
        );
        let fankai_only = ExternalIds {
            fankai: Some(12),
            ..Default::default()
        };
        assert_eq!(
            known(MediaKind::Movie, fankai_only),
            [("fankai=12".to_string(), true)]
        );
        // Known by nothing, no identifier at all — and no default.
        assert!(known(MediaKind::Movie, ExternalIds::default()).is_empty());
    }

    #[test]
    fn an_episode_is_given_one_default_identifier_too() {
        let item = series();
        let episode_ids = |tvdb: Option<i64>, tmdb: Option<i64>| {
            let mut episode = crate::db::repo::child::blank_episode(1, 1);
            episode.tvdb_id = tvdb;
            episode.tmdb_id = tmdb;
            identifiers(&for_episode(&item, &episode, &[]), "episodedetails")
        };

        assert_eq!(
            episode_ids(Some(5), Some(6)),
            [("tvdb=5".to_string(), true), ("tmdb=6".to_string(), false)]
        );
        // TMDB alone is the default; it was none.
        assert_eq!(episode_ids(None, Some(6)), [("tmdb=6".to_string(), true)]);
        assert_eq!(episode_ids(Some(5), None), [("tvdb=5".to_string(), true)]);
        assert!(episode_ids(None, None).is_empty());
    }

    #[test]
    fn a_film_of_a_collection_is_put_in_its_set() {
        let mut film = series();
        film.kind = MediaKind::Movie;
        let collection = Collection {
            name: "Alien & Predator <Collection>",
            overview: Some("Xenomorphs,\nmostly."),
        };

        let nfo = for_item_in(&film, Some(&collection), &[]);
        let elements = xml::read(&nfo);
        assert_eq!(
            xml::texts(&elements, "movie/set/name"),
            ["Alien & Predator <Collection>"]
        );
        assert_eq!(
            xml::texts(&elements, "movie/set/overview"),
            ["Xenomorphs,\nmostly."]
        );

        // Without an overview, a name alone; with no name, no set.
        let nameless = for_item_in(
            &film,
            Some(&Collection {
                name: "  ",
                overview: Some("x"),
            }),
            &[],
        );
        assert!(!nameless.contains("<set>"));
        let bare = for_item_in(
            &film,
            Some(&Collection {
                name: "Heat",
                overview: None,
            }),
            &[],
        );
        assert!(bare.contains("<set>\n    <name>Heat</name>\n  </set>"));
        assert!(!bare.contains("<overview>"));
        assert!(!for_item(&film).contains("<set>"));

        // A series belongs to no collection, whatever it is handed.
        let show = for_item_in(&series(), Some(&collection), &[]);
        assert!(!show.contains("<set>"));
    }

    #[test]
    fn a_season_with_a_title_of_its_own_is_named() {
        let mut item = series();
        let season = |number: i32, title: Option<&str>| crate::domain::Season {
            id: format!("s{number}"),
            season_number: number,
            title: title.map(str::to_string),
            overview: None,
            air_date: None,
            tmdb_id: None,
            tvdb_id: None,
            is_manual: false,
            images: Vec::new(),
            primary_images: Default::default(),
        };
        item.seasons = vec![
            season(0, Some("Specials")),
            season(1, Some("Book One: Water & Fire")),
            season(2, Some("Season 2")),
            season(3, Some("Saison 03")),
            // A title that says a number is still a title when it is not
            // the season's own word for it.
            season(4, Some("Part 4")),
            season(5, Some("  ")),
            season(6, None),
            season(7, Some("Season 8")),
        ];

        let elements = xml::read(&for_item(&item));
        let named: Vec<(&str, &str)> = xml::at(&elements, "tvshow/namedseason")
            .into_iter()
            .map(|e| (e.attribute("number").unwrap(), e.text.as_str()))
            .collect();
        assert_eq!(
            named,
            [
                ("1", "Book One: Water & Fire"),
                ("4", "Part 4"),
                ("7", "Season 8")
            ]
        );

        // A film has seasons of no kind.
        let mut film = item.clone();
        film.kind = MediaKind::Movie;
        assert!(!for_item(&film).contains("namedseason"));
    }

    #[test]
    fn a_numbered_title_is_what_a_media_server_writes_for_itself() {
        assert!(is_numbered("Season 3", 3));
        assert!(is_numbered("saison 03", 3));
        assert!(is_numbered("  STAFFEL 12  ", 12));
        assert!(is_numbered("Сезон 2", 2));
        assert!(is_numbered("Specials", 0));
        assert!(!is_numbered("Specials", 1));
        assert!(!is_numbered("Season 3", 4));
        assert!(!is_numbered("Season", 1));
        assert!(!is_numbered("Season Three", 3));
        assert!(!is_numbered("Part 1", 1));
        assert!(!is_numbered("The Final Season 3", 3));
        assert!(!is_numbered("", 1));
    }

    #[test]
    fn a_seasons_poster_is_a_thumb_of_that_season() {
        let mut item = series();
        let poster = |id: &str, url: &str| Image {
            id: id.into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: url.into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: false,
        };
        let mut season = crate::domain::Season {
            id: "s1".into(),
            season_number: 1,
            title: None,
            overview: None,
            air_date: None,
            tmdb_id: None,
            tvdb_id: None,
            is_manual: false,
            images: vec![
                poster("a", "https://example.invalid/s1.jpg?a=1&b=2"),
                poster("b", "https://example.invalid/s1-other.jpg"),
            ],
            primary_images: Default::default(),
        };
        item.seasons = vec![season.clone()];
        season.season_number = 0;
        season.images = vec![poster("c", "https://example.invalid/s0.jpg")];
        item.seasons.insert(0, season);

        let elements = xml::read(&for_item(&item));
        let thumbs: Vec<(&str, &str, &str)> = xml::at(&elements, "tvshow/thumb")
            .into_iter()
            .filter(|e| e.attribute("type") == Some("season"))
            .map(|e| {
                (
                    e.attribute("season").unwrap(),
                    e.attribute("aspect").unwrap(),
                    e.text.as_str(),
                )
            })
            .collect();
        // One a season — the first, which is the one chosen where one was —
        // and the query string survived the escaping.
        assert_eq!(
            thumbs,
            [
                ("0", "poster", "https://example.invalid/s0.jpg"),
                ("1", "poster", "https://example.invalid/s1.jpg?a=1&b=2"),
            ]
        );
    }

    #[test]
    fn what_is_written_is_xml_whatever_the_providers_said() {
        let mut item = series();
        item.title = "Tom & Jerry <the \"movie\"> ]]> 🎬".into();
        item.overview = Some("Line one\u{7}\nLine two & <b>bold</b>".into());
        item.genres = vec!["Kids & Family".into()];
        item.credits[0].person_name = "Zoë \"Z\" <Z>".into();
        item.credits[0].character_name = Some("&amp;".into());

        let elements = xml::read(&for_item(&item));
        assert_eq!(
            xml::texts(&elements, "tvshow/title"),
            ["Tom & Jerry <the \"movie\"> ]]> 🎬"]
        );
        assert_eq!(
            xml::texts(&elements, "tvshow/plot"),
            ["Line one\nLine two & <b>bold</b>"]
        );
        assert_eq!(
            xml::texts(&elements, "tvshow/actor/name"),
            ["Zoë \"Z\" <Z>"]
        );
        assert_eq!(xml::texts(&elements, "tvshow/actor/role"), ["&amp;"]);
        assert_eq!(xml::texts(&elements, "tvshow/genre"), ["Kids & Family"]);

        let mut episode = crate::db::repo::child::blank_episode(1, 2);
        episode.title = "<Pilot> & \"more\"".into();
        let elements = xml::read(&for_episode(&item, &episode, &[]));
        assert_eq!(
            xml::texts(&elements, "episodedetails/title"),
            ["<Pilot> & \"more\""]
        );
    }
}

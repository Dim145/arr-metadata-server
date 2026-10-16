//! Kometa metadata files.
//!
//! [Kometa](https://kometa.wiki) — what Plex Meta Manager became — edits what
//! Plex shows of a library: a metadata file names a work, by TMDB or TheTVDB
//! id or by title and year, and gives the fields to put over what Plex's own
//! agent found. A lock is the same thing seen from here: a correction a
//! person made, meant to outlast every refresh. So the file carries *only
//! what has been locked* — never what a provider said, which Plex has already
//! — and a library that applies it shows the catalogue's corrections.
//!
//! Which fields, and under which names, is the format's:
//!
//! | locked | written |
//! |---|---|
//! | title, sort title, original title | `title`, `sort_title`, `original_title` |
//! | overview, tagline | `summary`, `tagline` |
//! | first aired (a series), in cinemas (a film) | `originally_available` |
//! | content rating | `content_rating` |
//! | studio (a film), network or studio (a series) | `studio` |
//! | genres | `genre.sync`: the whole list, not an addition to Plex's |
//! | primary poster, primary background | `url_poster`, `url_background` |
//! | a season's title, overview, poster | `seasons: N:` `title`, `summary`, `url_poster` |
//! | an episode's title, overview, air date | `episodes: N:` `title`, `summary`, `originally_available` |
//!
//! A field that was locked *cleared* writes nothing: Kometa has no way to say
//! "no value" that does not also fail the entry.
//!
//! The YAML is written here rather than by a library. The shapes are few and
//! fixed, and the one part that goes wrong is a string — a title with a colon,
//! a summary with quotes and line breaks, an emoji — so every string is
//! written double-quoted and escaped, which every YAML reader, 1.1 or 1.2,
//! reads back as it was.

use std::{
    collections::{BTreeSet, HashSet},
    fmt::Write as _,
};

use crate::domain::{Image, MediaItem, MediaKind, ids::normalize_imdb_id};

/// A metadata file, and how many works it speaks of.
pub struct Document {
    pub text: String,
    pub entries: usize,
}

/// The metadata file of the works of one kind, as a library of that kind
/// reads it: an entry for each that has something locked, in the order given.
///
/// `works` are as the catalogue serves them to a client elsewhere: the locks
/// applied — which is where `locked_fields` and the chosen pictures come
/// from — and the addresses of the pictures ones a Kometa server can fetch.
/// `day` is the date the header says it was written.
pub fn document(kind: MediaKind, works: &[MediaItem], day: &str) -> Document {
    let mut used: HashSet<String> = HashSet::new();
    let entries: Vec<String> = works
        .iter()
        .filter(|work| work.kind == kind)
        .filter_map(|work| entry(work, &mut used))
        .collect();

    let (what, library) = match kind {
        MediaKind::Movie => ("films", "movie"),
        MediaKind::Series => ("series", "show"),
    };
    let mut text = format!(
        "# Kometa metadata file, written by Cinémathèque on {day}.\n\
         # What has been locked here on {what}, and nothing else: corrections made by hand, put over what Plex's agent found.\n\
         # Name it under metadata_files of a {library} library, as a file or by this address.\n"
    );
    if entries.is_empty() {
        let _ = writeln!(text, "# Nothing is locked on any of them yet.");
        text.push_str("metadata: {}\n");
    } else {
        text.push_str("metadata:\n");
        text.push_str(&entries.join("\n"));
    }

    Document {
        text,
        entries: entries.len(),
    }
}

/// The identifier Kometa matches a work by: TMDB's for a film, TheTVDB's for a
/// series, or IMDb's — which both kinds of library read — where the first is
/// not known. Nothing for a work none of them names: Kometa then goes by the
/// title and the year.
enum Mapping {
    Number(i64),
    Imdb(String),
}

fn mapping(work: &MediaItem) -> Option<Mapping> {
    let ids = &work.external_ids;
    let first = match work.kind {
        MediaKind::Movie => ids.tmdb,
        MediaKind::Series => ids.tvdb,
    };
    first.map(Mapping::Number).or_else(|| {
        ids.imdb
            .as_deref()
            .and_then(normalize_imdb_id)
            .map(Mapping::Imdb)
    })
}

/// One work's entry under `metadata:`, or nothing where nothing of it is
/// locked in a form Kometa can be told.
///
/// `used` is the names given so far: the name is the entry's key, and two
/// works of one title and year — remakes of a year — must not share it, or
/// the second would replace the first.
fn entry(work: &MediaItem, used: &mut HashSet<String>) -> Option<String> {
    let locked: HashSet<&str> = work.locked_fields.iter().map(String::as_str).collect();
    let on_work = |field: &str| locked.contains(format!("item/{field}").as_str());

    let mut edits = Edits::default();
    let text = |field: &str, value: Option<&str>| -> Option<String> {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty() && on_work(field))
            .map(str::to_string)
    };

    edits.text("title", text("title", Some(work.title.as_str())));
    edits.text("sort_title", text("sortTitle", work.sort_title.as_deref()));
    edits.text(
        "original_title",
        text("originalTitle", work.original_title.as_deref()),
    );
    edits.text("summary", text("overview", work.overview.as_deref()));
    edits.text("tagline", text("tagline", work.tagline.as_deref()));

    // A film's "originally available" is when it first came out, which is
    // the day it was in cinemas; a series', the day it first aired.
    let (field, day) = match work.kind {
        MediaKind::Movie => ("inCinemas", work.in_cinemas.as_deref()),
        MediaKind::Series => ("firstAired", work.first_aired.as_deref()),
    };
    if on_work(field) {
        edits.date("originally_available", day);
    }

    edits.text(
        "content_rating",
        text("contentRating", work.content_rating.as_deref()),
    );
    // Plex's studio of a series is its network.
    let studio = match work.kind {
        MediaKind::Movie => text("studio", work.studio.as_deref()),
        MediaKind::Series => text("network", work.network.as_deref())
            .or_else(|| text("studio", work.studio.as_deref())),
    };
    edits.text("studio", studio);

    if on_work("genres") {
        edits.list("genre.sync", &work.genres);
    }
    if on_work("primaryPoster") {
        edits.text(
            "url_poster",
            chosen(&work.images, work.primary_images.poster.as_deref()),
        );
    }
    if on_work("primaryFanart") {
        edits.text(
            "url_background",
            chosen(&work.images, work.primary_images.fanart.as_deref()),
        );
    }

    let seasons = if work.kind == MediaKind::Series {
        seasons(work, &locked)
    } else {
        Vec::new()
    };
    if edits.is_empty() && seasons.is_empty() {
        return None;
    }

    let mapped = mapping(work);
    let year = work.year.filter(|year| *year > 0);
    let name = unique_name(work, year, mapped.is_some(), used);

    let mut out = String::new();
    let _ = writeln!(out, "  {}:", quoted(&name));
    out.push_str("    match:\n");
    match &mapped {
        Some(Mapping::Number(id)) => {
            let _ = writeln!(out, "      mapping_id: {id}");
        }
        Some(Mapping::Imdb(id)) => {
            let _ = writeln!(out, "      mapping_id: {}", quoted(id));
        }
        None => {}
    }
    let _ = writeln!(out, "      title: {}", quoted(work.title.trim()));
    if let Some(year) = year {
        let _ = writeln!(out, "      year: {year}");
    }
    edits.write(&mut out, 4);

    if !seasons.is_empty() {
        out.push_str("    seasons:\n");
        for season in &seasons {
            let _ = writeln!(out, "      {}:", season.number);
            season.edits.write(&mut out, 8);
            if !season.episodes.is_empty() {
                out.push_str("        episodes:\n");
                for (number, edits) in &season.episodes {
                    let _ = writeln!(out, "          {number}:");
                    edits.write(&mut out, 12);
                }
            }
        }
    }

    Some(out)
}

/// What an entry is called: the title and the year, which is what Kometa
/// itself would name it — made different from every name used already, and
/// from the shape Kometa takes for an IMDb id where an entry is given no
/// `mapping_id`: a name beginning `tt`. (A name that is a number is one too,
/// but only unquoted, and every name is written quoted.)
fn unique_name(
    work: &MediaItem,
    year: Option<i32>,
    mapped: bool,
    used: &mut HashSet<String>,
) -> String {
    let title = work.title.trim();
    let mut name = match year {
        Some(year) => format!("{title} ({year})"),
        None => title.to_string(),
    };
    if !mapped && name.starts_with("tt") {
        name = match work.kind {
            MediaKind::Movie => format!("film: {name}"),
            MediaKind::Series => format!("series: {name}"),
        };
    }
    if used.contains(&name) {
        let by = match mapping(work) {
            Some(Mapping::Number(id)) => id.to_string(),
            Some(Mapping::Imdb(id)) => id,
            None => work.id.clone(),
        };
        name = format!("{name} [{by}]");
        // Two works with nothing to tell them apart but an id that is not one.
        let mut n = 2;
        while used.contains(&name) {
            name = format!("{name} {n}");
            n += 1;
        }
    }
    used.insert(name.clone());
    name
}

/// A season's edits, and its episodes'.
struct SeasonEdits {
    number: i32,
    edits: Edits,
    episodes: Vec<(i32, Edits)>,
}

/// The seasons of a series with something locked on them or on one of their
/// episodes, in order.
fn seasons(work: &MediaItem, locked: &HashSet<&str>) -> Vec<SeasonEdits> {
    // A season is named by the work's seasons and by the episodes that say
    // they are in one: a lock on an episode of a season nothing else lists
    // is no less a lock.
    let numbers: BTreeSet<i32> = work
        .seasons
        .iter()
        .map(|season| season.season_number)
        .chain(work.episodes.iter().map(|episode| episode.season_number))
        .collect();

    numbers
        .into_iter()
        .filter_map(|number| {
            let on_season =
                |field: &str| locked.contains(format!("season:{number}/{field}").as_str());
            let mut edits = Edits::default();

            if let Some(season) = work.seasons.iter().find(|s| s.season_number == number) {
                let text = |field: &str, value: Option<&str>| -> Option<String> {
                    value
                        .map(str::trim)
                        .filter(|value| !value.is_empty() && on_season(field))
                        .map(str::to_string)
                };
                edits.text("title", text("title", season.title.as_deref()));
                edits.text("summary", text("overview", season.overview.as_deref()));
                if on_season("primaryPoster") {
                    edits.text(
                        "url_poster",
                        chosen(&season.images, season.primary_images.poster.as_deref()),
                    );
                }
            }

            let episodes: Vec<(i32, Edits)> = work
                .episodes
                .iter()
                .filter(|episode| episode.season_number == number)
                .filter_map(|episode| {
                    let scope = format!("episode:{number}x{}", episode.episode_number);
                    let on_episode =
                        |field: &str| locked.contains(format!("{scope}/{field}").as_str());
                    let text = |field: &str, value: Option<&str>| -> Option<String> {
                        value
                            .map(str::trim)
                            .filter(|value| !value.is_empty() && on_episode(field))
                            .map(str::to_string)
                    };

                    let mut edits = Edits::default();
                    edits.text("title", text("title", Some(episode.title.as_str())));
                    edits.text("summary", text("overview", episode.overview.as_deref()));
                    if on_episode("airDate") {
                        edits.date("originally_available", episode.air_date.as_deref());
                    }
                    (!edits.is_empty()).then_some((episode.episode_number, edits))
                })
                .collect();

            (!edits.is_empty() || !episodes.is_empty()).then_some(SeasonEdits {
                number,
                edits,
                episodes,
            })
        })
        .collect()
}

/// The address of the picture a person chose, among the work's own, where a
/// client elsewhere can follow it.
fn chosen(images: &[Image], id: Option<&str>) -> Option<String> {
    let id = id?;
    images
        .iter()
        .find(|image| image.id == id)
        .map(|image| image.url.as_str())
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        .map(str::to_string)
}

/// A day as Kometa takes one, `YYYY-MM-DD`: the first ten characters of a
/// date, or of a date-time a provider gave in its place, when they are a day
/// of the calendar.
fn day(raw: &str) -> Option<String> {
    let head = raw.trim().get(..10)?;
    chrono::NaiveDate::parse_from_str(head, "%Y-%m-%d")
        .ok()
        .map(|day| day.format("%Y-%m-%d").to_string())
}

/// The attributes of one entry, a season or an episode, in the order they
/// are written.
#[derive(Default)]
struct Edits(Vec<(&'static str, Value)>);

enum Value {
    Text(String),
    Day(String),
    List(Vec<String>),
}

impl Edits {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn text(&mut self, key: &'static str, value: Option<String>) {
        if let Some(value) = value {
            self.0.push((key, Value::Text(value)));
        }
    }

    fn date(&mut self, key: &'static str, value: Option<&str>) {
        if let Some(value) = value.and_then(day) {
            self.0.push((key, Value::Day(value)));
        }
    }

    fn list(&mut self, key: &'static str, values: &[String]) {
        let values: Vec<String> = values
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
        if !values.is_empty() {
            self.0.push((key, Value::List(values)));
        }
    }

    /// Written at `indent` spaces.
    fn write(&self, out: &mut String, indent: usize) {
        let pad = " ".repeat(indent);
        for (key, value) in &self.0 {
            match value {
                Value::Text(text) => {
                    let _ = writeln!(out, "{pad}{key}: {}", quoted(text));
                }
                // Bare, as the format's own examples write one.
                Value::Day(day) => {
                    let _ = writeln!(out, "{pad}{key}: {day}");
                }
                Value::List(items) => {
                    let _ = writeln!(out, "{pad}{key}:");
                    for item in items {
                        let _ = writeln!(out, "{pad}  - {}", quoted(item));
                    }
                }
            }
        }
    }
}

/// A string as a double-quoted YAML scalar.
///
/// Always quoted, never plain: a plain scalar is read as whatever it looks
/// like — `yes`, `null`, `1e3`, `2008-01-20`, `0x1F` — by one reader or
/// another, and a title is whatever it is. Within the quotes, a backslash
/// and a quote are escaped; so is every control character, the next-line and
/// the two line separators a YAML 1.1 reader breaks lines on, and the byte
/// order mark; and the two code points that are no characters are left out.
/// Everything else — accents, scripts, emoji — is written as it is.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\L"),
            '\u{2029}' => out.push_str("\\P"),
            '\u{FEFF}' => out.push_str("\\uFEFF"),
            '\u{FFFE}' | '\u{FFFF}' => {}
            c if (c as u32) < 0x20 || (0x7F..=0x9F).contains(&(c as u32)) => {
                let _ = write!(out, "\\x{:02X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CoverType, Season};

    /// A scalar as a YAML reader reads it back: the escapes of a
    /// double-quoted string undone. A reader of the same grammar, as far as
    /// this module writes it — enough to say that what was written is what
    /// was meant.
    fn unquoted(scalar: &str) -> String {
        let inner = scalar
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .expect("a double-quoted scalar");
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                assert!(c != '"', "a quote left bare inside the scalar");
                out.push(c);
                continue;
            }
            match chars.next().expect("an escape") {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'L' => out.push('\u{2028}'),
                'P' => out.push('\u{2029}'),
                'x' => {
                    let hex: String = chars.by_ref().take(2).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
                }
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
                }
                other => panic!("an escape this module does not write: \\{other}"),
            }
        }
        out
    }

    fn image(id: &str, kind: CoverType, url: &str) -> Image {
        Image {
            id: id.into(),
            season_number: None,
            cover_type: kind,
            url: url.into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: false,
        }
    }

    fn film() -> MediaItem {
        let mut work = MediaItem::empty(MediaKind::Movie);
        work.id = "w-heat".into();
        work.title = "Heat".into();
        work.year = Some(1995);
        work.overview = Some("A crew. A cop.".into());
        work.original_title = Some("Heat".into());
        work.studio = Some("Regency".into());
        work.content_rating = Some("R".into());
        work.in_cinemas = Some("1995-12-15T00:00:00Z".into());
        work.genres = vec!["Crime".into(), "Drama".into()];
        work.external_ids.tmdb = Some(949);
        work.external_ids.imdb = Some("tt0113277".into());
        work.images = vec![
            image("p1", CoverType::Poster, "https://img.example/p1.jpg"),
            image("p2", CoverType::Poster, "https://img.example/p2.jpg"),
            image("f1", CoverType::Fanart, "https://img.example/f1.jpg"),
        ];
        work
    }

    fn locked(mut work: MediaItem, fields: &[&str]) -> MediaItem {
        work.locked_fields = fields.iter().map(|f| f.to_string()).collect();
        work
    }

    fn written(work: &MediaItem) -> String {
        entry(work, &mut HashSet::new()).expect("an entry")
    }

    #[test]
    fn a_work_with_nothing_locked_has_no_entry() {
        assert!(entry(&film(), &mut HashSet::new()).is_none());

        // Locked fields Kometa has no attribute for are not an entry either.
        let work = locked(film(), &["item/runtime", "item/year", "item/keywords"]);
        assert!(entry(&work, &mut HashSet::new()).is_none());

        let doc = document(MediaKind::Movie, &[work], "2026-10-11");
        assert_eq!(doc.entries, 0);
        assert!(doc.text.contains("metadata: {}\n"));
        assert!(doc.text.contains("# Nothing is locked"));
    }

    #[test]
    fn only_what_is_locked_is_written_and_under_kometas_names() {
        let mut work = locked(
            film(),
            &[
                "item/title",
                "item/overview",
                "item/tagline",
                "item/inCinemas",
                "item/genres",
                "item/primaryPoster",
                "item/primaryFanart",
                "item/studio",
            ],
        );
        work.primary_images.poster = Some("p2".into());
        work.primary_images.fanart = Some("f1".into());
        work.tagline = Some("A crew. A cop. A city.".into());

        assert_eq!(
            written(&work),
            "  \"Heat (1995)\":\n\
             \x20   match:\n\
             \x20     mapping_id: 949\n\
             \x20     title: \"Heat\"\n\
             \x20     year: 1995\n\
             \x20   title: \"Heat\"\n\
             \x20   summary: \"A crew. A cop.\"\n\
             \x20   tagline: \"A crew. A cop. A city.\"\n\
             \x20   originally_available: 1995-12-15\n\
             \x20   studio: \"Regency\"\n\
             \x20   genre.sync:\n\
             \x20     - \"Crime\"\n\
             \x20     - \"Drama\"\n\
             \x20   url_poster: \"https://img.example/p2.jpg\"\n\
             \x20   url_background: \"https://img.example/f1.jpg\"\n"
        );

        // What is not locked is not said, whatever the work holds.
        let only_title = locked(film(), &["item/title"]);
        let text = written(&only_title);
        for absent in [
            "summary",
            "studio",
            "content_rating",
            "genre.sync",
            "url_poster",
            "originally_available",
            "original_title",
        ] {
            assert!(!text.contains(absent), "{absent} in\n{text}");
        }
    }

    #[test]
    fn a_work_is_matched_by_the_id_its_library_reads() {
        // A film by TMDB, a series by TheTVDB; the other kind's number is
        // not used.
        let mut work = locked(film(), &["item/title"]);
        work.external_ids.tvdb = Some(99);
        assert!(written(&work).contains("mapping_id: 949\n"));

        let mut show = MediaItem::empty(MediaKind::Series);
        show.title = "Dark".into();
        show.year = Some(2017);
        show.external_ids.tmdb = Some(70523);
        show.external_ids.tvdb = Some(334824);
        let show = locked(show, &["item/title"]);
        assert!(written(&show).contains("mapping_id: 334824\n"));

        // Failing that IMDb's, quoted, which both kinds' libraries read.
        let mut no_tmdb = work.clone();
        no_tmdb.external_ids.tmdb = None;
        assert!(written(&no_tmdb).contains("mapping_id: \"tt0113277\"\n"));

        // And with none, the title and the year alone.
        let mut bare = work.clone();
        bare.external_ids = Default::default();
        let text = written(&bare);
        assert!(!text.contains("mapping_id"));
        assert!(text.contains("      title: \"Heat\"\n      year: 1995\n"));
        let mut yearless = bare.clone();
        yearless.year = None;
        let text = written(&yearless);
        assert!(text.starts_with("  \"Heat\":\n"));
        assert!(!text.contains("year:"));
    }

    #[test]
    fn a_series_carries_the_titles_summaries_and_days_of_its_seasons_and_episodes() {
        let mut show = MediaItem::empty(MediaKind::Series);
        show.title = "Avatar".into();
        show.year = Some(2005);
        show.network = Some("Nickelodeon".into());
        show.studio = Some("Not this one".into());
        show.first_aired = Some("2005-02-21".into());
        show.external_ids.tvdb = Some(74852);
        show.seasons = ["Book One: Water", "Book Two: Earth"]
            .iter()
            .enumerate()
            .map(|(index, title)| Season {
                id: format!("s{index}"),
                season_number: index as i32 + 1,
                title: Some((*title).into()),
                overview: Some(format!("About {title}.")),
                air_date: Some("2005-02-21".into()),
                tmdb_id: None,
                tvdb_id: None,
                is_manual: false,
                images: vec![image(
                    &format!("sp{index}"),
                    CoverType::Poster,
                    &format!("https://img.example/s{index}.jpg"),
                )],
                primary_images: Default::default(),
            })
            .collect();
        show.seasons[0].primary_images.poster = Some("sp0".into());
        let mut episode = crate::db::repo::child::blank_episode(2, 5);
        episode.title = "Zuko Alone".into();
        episode.overview = Some("A flashback.".into());
        episode.air_date = Some("2006-09-22T00:00:00Z".into());
        let mut other = crate::db::repo::child::blank_episode(2, 6);
        other.title = "Not locked".into();
        show.episodes = vec![episode, other];
        let show = locked(
            show,
            &[
                "item/network",
                "item/firstAired",
                "season:1/title",
                "season:1/primaryPoster",
                "season:2/overview",
                "episode:2x5/title",
                "episode:2x5/overview",
                "episode:2x5/airDate",
                // A lock on a season nothing lists is not an entry of its own.
                "season:9/title",
            ],
        );

        assert_eq!(
            written(&show),
            "  \"Avatar (2005)\":\n\
             \x20   match:\n\
             \x20     mapping_id: 74852\n\
             \x20     title: \"Avatar\"\n\
             \x20     year: 2005\n\
             \x20   originally_available: 2005-02-21\n\
             \x20   studio: \"Nickelodeon\"\n\
             \x20   seasons:\n\
             \x20     1:\n\
             \x20       title: \"Book One: Water\"\n\
             \x20       url_poster: \"https://img.example/s0.jpg\"\n\
             \x20     2:\n\
             \x20       summary: \"About Book Two: Earth.\"\n\
             \x20       episodes:\n\
             \x20         5:\n\
             \x20           title: \"Zuko Alone\"\n\
             \x20           summary: \"A flashback.\"\n\
             \x20           originally_available: 2006-09-22\n"
        );

        // A lock on an episode alone makes the entry, with no season edits.
        let mut lone = MediaItem::empty(MediaKind::Series);
        lone.title = "Lone".into();
        let mut ep = crate::db::repo::child::blank_episode(1, 1);
        ep.title = "Pilot".into();
        lone.episodes = vec![ep];
        let lone = locked(lone, &["episode:1x1/title"]);
        let text = written(&lone);
        assert!(text.contains("    seasons:\n      1:\n        episodes:\n          1:\n            title: \"Pilot\"\n"));
        // And a film has no seasons to speak of, whatever is locked on it.
        let mut film_with_season = locked(film(), &["season:1/title"]);
        film_with_season.seasons = show.seasons.clone();
        assert!(entry(&film_with_season, &mut HashSet::new()).is_none());
    }

    #[test]
    fn what_was_cleared_or_cannot_be_followed_is_not_written() {
        // Locked cleared: no value to say.
        let mut work = locked(film(), &["item/overview", "item/title", "item/genres"]);
        work.overview = None;
        work.genres = Vec::new();
        let text = written(&work);
        assert!(
            !text.contains("summary") && !text.contains("genre.sync"),
            "{text}"
        );

        // A chosen picture that is no address — an upload not kept — is
        // not an address to give.
        let mut upload = locked(film(), &["item/primaryPoster", "item/title"]);
        upload.images = vec![image("u", CoverType::Poster, "/media/ab.jpg")];
        upload.primary_images.poster = Some("u".into());
        assert!(!written(&upload).contains("url_poster"));

        // A date that is not one of the calendar is not a date.
        let mut dated = locked(film(), &["item/inCinemas", "item/title"]);
        dated.in_cinemas = Some("1995-13-40".into());
        assert!(!written(&dated).contains("originally_available"));
        dated.in_cinemas = Some("1995".into());
        assert!(!written(&dated).contains("originally_available"));
    }

    #[test]
    fn two_works_of_one_name_get_two_entries() {
        let mut first = locked(film(), &["item/title"]);
        first.id = "a".into();
        let mut second = first.clone();
        second.id = "b".into();
        second.external_ids.tmdb = Some(1234);
        let mut third = second.clone();
        third.id = "c".into();
        third.external_ids = Default::default();
        let mut fourth = third.clone();
        fourth.id = "d".into();

        let doc = document(
            MediaKind::Movie,
            &[first, second, third, fourth],
            "2026-10-11",
        );
        assert_eq!(doc.entries, 4);
        let names: Vec<&str> = doc
            .text
            .lines()
            .filter(|line| line.starts_with("  \""))
            .collect();
        assert_eq!(
            names,
            [
                "  \"Heat (1995)\":",
                "  \"Heat (1995) [1234]\":",
                "  \"Heat (1995) [c]\":",
                "  \"Heat (1995) [d]\":",
            ]
        );
    }

    #[test]
    fn an_entry_without_an_id_is_never_named_like_one() {
        // Kometa reads a name that begins `tt` as an IMDb id where no
        // `mapping_id` is given; a number is only one when it is not quoted.
        let mut odd = locked(film(), &["item/title"]);
        odd.external_ids = Default::default();
        odd.title = "ttyl".into();
        odd.year = None;
        assert!(written(&odd).starts_with("  \"film: ttyl\":\n"));
        odd.title = "9".into();
        assert!(written(&odd).starts_with("  \"9\":\n"));
        // With an id, the name is the title's.
        odd.external_ids.tmdb = Some(5);
        odd.title = "ttyl".into();
        assert!(written(&odd).starts_with("  \"ttyl\":\n"));
    }

    #[test]
    fn a_document_holds_the_works_of_its_kind_only() {
        let mut show = MediaItem::empty(MediaKind::Series);
        show.title = "Dark".into();
        let show = locked(show, &["item/title"]);
        let movie = locked(film(), &["item/title"]);

        let films = document(
            MediaKind::Movie,
            &[show.clone(), movie.clone()],
            "2026-10-11",
        );
        assert_eq!(films.entries, 1);
        assert!(
            films
                .text
                .starts_with("# Kometa metadata file, written by Cinémathèque on 2026-10-11.\n")
        );
        assert!(films.text.contains("a movie library"));
        assert!(films.text.contains("\nmetadata:\n  \"Heat (1995)\":\n"));
        assert!(!films.text.contains("Dark"));

        let shows = document(MediaKind::Series, &[show, movie], "2026-10-11");
        assert_eq!(shows.entries, 1);
        assert!(shows.text.contains("a show library"));
        assert!(shows.text.contains("  \"Dark\":\n"));
    }

    #[test]
    fn a_string_is_written_as_it_was() {
        let awkward = [
            "Plain",
            "Star Wars: Episode IV - A New Hope",
            "She said \"no\", then 'yes'",
            "two\nlines\r\nand a tab\there",
            "back\\slash and \\n that is not a newline",
            "# not a comment",
            "- not a list",
            "yes",
            "null",
            "123",
            "1e3",
            "2008-01-20",
            "0x1F",
            "~",
            "{flow}: [seq]",
            "ends with colon:",
            " leading and trailing ",
            "",
            "emoji 🎬 and accents éàü, 日本語",
            "bell \u{7} nul \u{0} escape \u{1b} del \u{7f} next-line \u{85}",
            "separators \u{2028} \u{2029} and a mark \u{feff}",
        ];
        for text in awkward {
            let scalar = quoted(text);
            // On one line, whatever the text held.
            assert!(
                !scalar.contains('\n') && !scalar.contains('\r'),
                "{scalar:?}"
            );
            assert_eq!(unquoted(&scalar), text, "{scalar}");
        }
        // The two code points that are no characters cannot be written.
        assert_eq!(unquoted(&quoted("a\u{fffe}b\u{ffff}c")), "abc");

        // And the same inside an entry: a title like that is one value.
        let mut work = locked(film(), &["item/title", "item/overview"]);
        work.title = "Pt. 1: \"Fire\" & Ice 🔥".into();
        work.overview = Some("First line.\n\nSecond: with a colon and a # hash.".into());
        let text = written(&work);
        assert!(text.contains("  \"Pt. 1: \\\"Fire\\\" & Ice 🔥 (1995)\":\n"));
        assert!(
            text.contains("    summary: \"First line.\\n\\nSecond: with a colon and a # hash.\"\n")
        );
        // Every line of the document is a key, a list item or a scalar of
        // one line: nothing the text said can start another.
        for line in text.lines() {
            assert!(
                line.trim_start()
                    .starts_with(|c: char| c == '"' || c == '-' || c.is_ascii_alphanumeric()),
                "{line:?}"
            );
        }
    }
}

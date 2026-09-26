//! Kodi/XBMC `.nfo` documents.
//!
//! This is the only route to Plex. Plex has no configurable metadata source —
//! since the legacy agents were removed it talks to `metadata.provider.plex.tv`
//! over a pinned connection tied to a Plex account — so the supported way to get
//! your own data in front of it is the Personal Media / XBMCnfo agent reading
//! files beside the media.
//!
//! Note that if Sonarr or Radarr manage the library, enabling *their* Kodi
//! metadata writer is the better route: they already write these files next to
//! the media, from the data this server gave them, so your edits arrive without
//! anything here being involved. What this module is for is the rest — manual
//! entries no arr manages, and libraries not run by the arr stack.

use std::fmt::Write as _;

use crate::domain::{CoverType, Credit, CreditType, Episode, MediaItem, MediaKind};

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

/// The document for a work: `<tvshow>` or `<movie>`.
pub fn for_item(item: &MediaItem) -> String {
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
        }
        MediaKind::Movie => {
            tag(&mut out, "premiered", item.in_cinemas.as_deref());
            tag(&mut out, "releasedate", item.digital_release.as_deref());
        }
    }

    for genre in &item.genres {
        tag(&mut out, "genre", Some(genre));
    }
    for keyword in &item.keywords {
        tag(&mut out, "tag", Some(keyword));
    }

    unique_ids(&mut out, item);
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
        write_credit(&mut out, credit);
    }

    let _ = writeln!(out, "</{root}>");
    out
}

/// The document for one episode: `<episodedetails>`.
pub fn for_episode(item: &MediaItem, episode: &Episode) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str(HEADER);
    out.push_str("<episodedetails>\n");

    tag(&mut out, "title", Some(&episode.title));
    tag(&mut out, "showtitle", Some(&item.title));
    tag_num(&mut out, "season", Some(episode.season_number));
    tag_num(&mut out, "episode", Some(episode.episode_number));
    tag_num(
        &mut out,
        "displayseason",
        episode.aired_before_season_number,
    );
    tag_num(
        &mut out,
        "displayepisode",
        episode.aired_before_episode_number,
    );
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

    if let Some(id) = episode.tvdb_id {
        let _ = writeln!(
            out,
            "  <uniqueid type=\"tvdb\" default=\"true\">{id}</uniqueid>"
        );
    }
    if let Some(id) = episode.tmdb_id {
        let _ = writeln!(out, "  <uniqueid type=\"tmdb\">{id}</uniqueid>");
    }

    out.push_str("</episodedetails>\n");
    out
}

fn unique_ids(out: &mut String, item: &MediaItem) {
    let ids = &item.external_ids;

    // Exactly one id must be marked default, and it should be the one the
    // client's own agent keys on.
    let primary = match item.kind {
        MediaKind::Series => ids
            .tvdb
            .map(|_| "tvdb")
            .or(ids.tmdb.map(|_| "tmdb"))
            .or(ids.fankai.map(|_| "fankai")),
        MediaKind::Movie => ids
            .tmdb
            .map(|_| "tmdb")
            .or(ids.imdb.as_ref().map(|_| "imdb")),
    };

    let mut emit = |kind: &str, value: String| {
        let default = if Some(kind) == primary {
            " default=\"true\""
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  <uniqueid type=\"{kind}\"{default}>{}</uniqueid>",
            escape(&value)
        );
    };

    if let Some(v) = ids.tvdb {
        emit("tvdb", v.to_string());
    }
    if let Some(v) = ids.tmdb {
        emit("tmdb", v.to_string());
    }
    if let Some(v) = &ids.imdb {
        emit("imdb", v.clone());
    }
    if let Some(v) = ids.fankai {
        emit("fankai", v.to_string());
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
}

fn write_credit(out: &mut String, credit: &Credit) {
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
    use crate::domain::{ExternalIds, Image, Rating, RatingValue};

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

        let nfo = for_episode(&item, &episode);

        assert!(nfo.contains("<episodedetails>"));
        assert!(nfo.contains("<title>Pilot</title>"));
        assert!(nfo.contains("<showtitle>Breaking Bad</showtitle>"));
        assert!(nfo.contains("<season>1</season>"));
        assert!(nfo.contains("<episode>1</episode>"));
        assert!(nfo.contains("<aired>2008-01-20</aired>"));
        assert!(nfo.contains("<value>8.3</value>"));
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
}

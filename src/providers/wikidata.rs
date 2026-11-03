//! Wikidata's query service, read for the links between the sites' ids
//! (wave P, P5).
//!
//! Wikidata files each site's id for a work under a property of its own, on
//! the work's item: TMDB's films under P4947, IMDb's titles under P345. One
//! query a site reads every item that carries an id of that site's, with the
//! item's IMDb ids beside it, and the eight together give each item every id
//! it carries of the nine sites: what [`crate::jobs::wikidata`] keeps. IMDb is
//! never asked alone — five to six million items carry an IMDb id, people for
//! the most part.
//!
//! Asked as Wikimedia asks to be asked: one query at a time, with a pause
//! between two (the job's), a User-Agent that says who asks and where they are
//! found, and a 429 waited out as its `Retry-After` says. The answers are
//! tables, tab-separated. The largest, every TMDB film, was 287 000 rows and
//! 18 MB in November 2026 — 4 MB on the wire. Asked whole, the join with the
//! items' IMDb ids came to the sixty seconds the service allows a query by
//! that month's end — TMDB's films and TheTVDB's were cut partway, and
//! refused — so each site is asked in eleven parts ([`Part`]), by the last
//! digit of its ids, each a few seconds.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{StatusCode, header};

use crate::{
    db::repo::wikidata::Link,
    domain::{ExternalSource, MediaKind},
    providers::{describe_request_error, error_text, read_body},
};

/// A site whose ids Wikidata files on its items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Site {
    TmdbMovie,
    TmdbTv,
    TvdbSeries,
    TvdbMovie,
    Imdb,
    AniList,
    Mal,
    AniDb,
    TvMaze,
}

impl Site {
    /// Every site, in the order an item's ids are given.
    pub const ALL: [Self; 9] = [
        Self::TmdbMovie,
        Self::TmdbTv,
        Self::TvdbSeries,
        Self::TvdbMovie,
        Self::Imdb,
        Self::AniList,
        Self::Mal,
        Self::AniDb,
        Self::TvMaze,
    ];

    /// The sites asked a query each, in the order they are asked: every one
    /// but IMDb, whose ids are read beside theirs.
    pub const ANCHORS: [Self; 8] = [
        Self::TmdbMovie,
        Self::TmdbTv,
        Self::TvdbSeries,
        Self::TvdbMovie,
        Self::AniList,
        Self::Mal,
        Self::AniDb,
        Self::TvMaze,
    ];

    /// The property Wikidata files the site's ids under.
    pub const fn property(self) -> u16 {
        match self {
            Self::TmdbMovie => 4947,
            Self::TmdbTv => 4983,
            Self::TvdbSeries => 4835,
            Self::TvdbMovie => 12196,
            Self::Imdb => 345,
            Self::AniList => 8729,
            Self::Mal => 4086,
            Self::AniDb => 5646,
            Self::TvMaze => 8600,
        }
    }

    pub fn of_property(property: u16) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|site| site.property() == property)
    }

    /// The source a work held here carries the site's id under; none for
    /// AniDB's, which the catalogue does not keep.
    pub const fn source(self) -> Option<ExternalSource> {
        match self {
            Self::TmdbMovie => Some(ExternalSource::TmdbMovie),
            Self::TmdbTv => Some(ExternalSource::TmdbTv),
            Self::TvdbSeries => Some(ExternalSource::TvdbSeries),
            Self::TvdbMovie => Some(ExternalSource::TvdbMovie),
            Self::Imdb => Some(ExternalSource::Imdb),
            Self::AniList => Some(ExternalSource::AniList),
            Self::Mal => Some(ExternalSource::Mal),
            Self::AniDb => None,
            Self::TvMaze => Some(ExternalSource::TvMaze),
        }
    }

    pub fn of_source(source: ExternalSource) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|site| site.source() == Some(source))
    }

    /// The kind of work the site's ids name, where it numbers films and
    /// series apart; none where one numbering holds both.
    pub const fn kind(self) -> Option<MediaKind> {
        match self {
            Self::TmdbMovie | Self::TvdbMovie => Some(MediaKind::Movie),
            Self::TmdbTv | Self::TvdbSeries | Self::TvMaze => Some(MediaKind::Series),
            Self::Imdb | Self::AniList | Self::Mal | Self::AniDb => None,
        }
    }

    /// What the site's ids are called in a message.
    pub const fn label(self) -> &'static str {
        match self {
            Self::TmdbMovie => "TMDB's films",
            Self::TmdbTv => "TMDB's series",
            Self::TvdbSeries => "TheTVDB's series",
            Self::TvdbMovie => "TheTVDB's films",
            Self::Imdb => "IMDb's titles",
            Self::AniList => "AniList's entries",
            Self::Mal => "MyAnimeList's entries",
            Self::AniDb => "AniDB's anime",
            Self::TvMaze => "TVmaze's shows",
        }
    }

    /// An id of the site's, written as the site writes it, as the number
    /// kept: none for anything else. IMDb's are `tt` and seven digits or
    /// more, with no more zeros in front than make seven; the others are
    /// numbers. Every one fits PostgreSQL's `INTEGER`, which keeps them.
    pub fn number(self, id: &str) -> Option<u32> {
        let digits = match self {
            Self::Imdb => {
                let digits = id.strip_prefix("tt")?;
                if !(7..=10).contains(&digits.len())
                    || (digits.len() > 7 && digits.starts_with('0'))
                {
                    return None;
                }
                digits
            }
            _ => id,
        };
        if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits
            .parse::<u32>()
            .ok()
            .filter(|&number| number > 0 && i32::try_from(number).is_ok())
    }

    /// The id a number kept is, as the site writes it.
    pub fn id(self, number: u32) -> String {
        match self {
            Self::Imdb => format!("tt{number:07}"),
            _ => number.to_string(),
        }
    }
}

/// What one site's query brought.
#[derive(Debug, Default)]
pub struct Read {
    /// Each id of the site's on an item, and each IMDb id of those items.
    pub links: Vec<Link>,
    /// The values that were not ids of their site's — a link, a slug, IMDb's
    /// id of a person or an event, Wikidata's "unknown value" — skipped.
    pub skipped: usize,
}

/// Who asks, and where they are found, as Wikimedia's policy wants every
/// client to say.
const AGENT: &str = concat!(
    "arr-metadata-server/",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

/// How long a query may take, answer read: half as long again as the
/// service gives itself to run one.
const TIMEOUT: Duration = Duration::from_secs(90);

/// The most of an answer read: three and a half times the largest, every
/// TMDB film, as it stood in November 2026.
const MAX_ANSWER: u64 = 64 * 1024 * 1024;

/// How many times a query is asked, the first time and after each 429.
const TRIES: usize = 3;

/// How long a 429 that does not say for how long is taken to ask.
const UNSAID_WAIT: Duration = Duration::from_secs(60);

/// The longest a 429 is waited out within a reading. Past it the reading
/// fails, the links kept stay, and the service is not asked again before
/// the time it said.
const LONGEST_WAIT: Duration = Duration::from_secs(5 * 60);

/// Until when each address of the service asked to be left alone, by its
/// address: what a reading that comes sooner is refused by, unasked.
static LEFT_ALONE: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// One of the parts a site's ids are asked in: those ending in a digit, or
/// the values ending in none — no id, but read so that they are counted as
/// skipped. A tenth of a large site's join with IMDb's ids takes a few
/// seconds where the whole came to the service's sixty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Ending(u8),
    Rest,
}

impl Part {
    /// Every part, once: together, every value of the site's.
    pub const ALL: [Self; 11] = [
        Self::Ending(0),
        Self::Ending(1),
        Self::Ending(2),
        Self::Ending(3),
        Self::Ending(4),
        Self::Ending(5),
        Self::Ending(6),
        Self::Ending(7),
        Self::Ending(8),
        Self::Ending(9),
        Self::Rest,
    ];

    /// The query's filter that keeps the part's values alone.
    fn filter(self) -> String {
        match self {
            Self::Ending(digit) => format!("FILTER(STRENDS(STR(?value), \"{digit}\"))"),
            Self::Rest => "FILTER(!REGEX(STR(?value), \"[0-9]$\"))".to_string(),
        }
    }

    /// What the part holds, in a message.
    fn said(self) -> String {
        match self {
            Self::Ending(digit) => format!("the ids ending in {digit}"),
            Self::Rest => "the values ending in no digit".to_string(),
        }
    }

    /// The part a value of the site's falls in, as the filters place it.
    #[cfg(test)]
    pub fn of(value: &str) -> Self {
        match value.bytes().last() {
            Some(last) if last.is_ascii_digit() => Self::Ending(last - b'0'),
            _ => Self::Rest,
        }
    }
}

/// The pause between two parts of a site's reading: one query at a time,
/// as Wikimedia asks, and a moment between them.
#[cfg(not(test))]
pub const BETWEEN_PARTS: Duration = Duration::from_secs(1);
#[cfg(test)]
pub const BETWEEN_PARTS: Duration = Duration::from_millis(25);

/// The query for one part of a site's ids: every item carrying one of them,
/// with its IMDb ids beside.
pub fn query(site: Site, part: Part) -> String {
    format!(
        "PREFIX wdt: <http://www.wikidata.org/prop/direct/>\n\
         SELECT ?item ?value ?imdb WHERE {{\n  \
         ?item wdt:P{} ?value .\n  \
         {}\n  \
         OPTIONAL {{ ?item wdt:P345 ?imdb . }}\n\
         }}",
        site.property(),
        part.filter()
    )
}

/// The header of the table the query asks for.
pub(crate) const HEADER: &str = "?item\t?value\t?imdb";

/// Ask Wikidata's query service at `endpoint`, through `http`, for one
/// site's ids and the IMDb ids beside them.
pub async fn read(http: &reqwest::Client, endpoint: &str, site: Site) -> Result<Read> {
    let mut whole = Read::default();
    for (n, part) in Part::ALL.into_iter().enumerate() {
        if n > 0 {
            tokio::time::sleep(BETWEEN_PARTS).await;
        }
        let body = ask(http, endpoint, &query(site, part))
            .await
            .with_context(|| part.said())?;
        let read = tokio::task::spawn_blocking(move || parse(&body, site))
            .await
            .context("the reading of Wikidata's answer ended abruptly")?
            .with_context(|| part.said())?;
        whole.skipped += read.skipped;
        whole.links.extend(read.links);
    }
    Ok(whole)
}

/// One query's answer, as the service wrote it.
async fn ask(http: &reqwest::Client, endpoint: &str, query: &str) -> Result<Vec<u8>> {
    for attempt in 1..=TRIES {
        if let Some(left) = left_alone(endpoint) {
            bail!(
                "Wikidata's query service asked to be left alone for another {} s",
                left.as_secs().max(1)
            );
        }

        let response = http
            .get(endpoint)
            .query(&[("query", query)])
            .header(header::USER_AGENT, AGENT)
            .header(header::ACCEPT, "text/tab-separated-values")
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|e| {
                anyhow!(
                    "Wikidata's query service could not be asked: {}",
                    describe_request_error(&e)
                )
            })?;

        let status = response.status();
        let refused = status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::SERVICE_UNAVAILABLE
                && response.headers().contains_key(header::RETRY_AFTER));
        if refused {
            let wait = retry_after(&response).unwrap_or(UNSAID_WAIT);
            leave_alone(endpoint, wait);
            if wait > LONGEST_WAIT || attempt == TRIES {
                bail!(
                    "Wikidata's query service answered {status} and asked to be left alone for {} s",
                    wait.as_secs()
                );
            }
            tracing::info!(
                %status,
                wait = wait.as_secs(),
                "Wikidata's query service asked to wait; waiting"
            );
            tokio::time::sleep(wait).await;
            continue;
        }
        if !status.is_success() {
            let said = error_text(response).await;
            bail!("Wikidata's query service answered {status}: {said}");
        }

        return read_body(response, MAX_ANSWER)
            .await
            .map_err(|e| anyhow!("Wikidata's answer could not be read: {e:#}"));
    }
    unreachable!("the last try returns or fails")
}

/// How long a `Retry-After` asks for: seconds, or a date.
fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    let value = response
        .headers()
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    let wait = match value.parse::<u64>() {
        Ok(seconds) => Duration::from_secs(seconds),
        Err(_) => {
            let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
            (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
                .to_std()
                .unwrap_or_default()
        }
    };
    Some(wait.max(Duration::from_secs(1)))
}

fn left_alone(endpoint: &str) -> Option<Duration> {
    let mut until = LEFT_ALONE.lock().unwrap_or_else(|e| e.into_inner());
    let left = until
        .get(endpoint)
        .and_then(|at| at.checked_duration_since(Instant::now()))
        .filter(|left| !left.is_zero());
    if left.is_none() {
        until.remove(endpoint);
    }
    left
}

fn leave_alone(endpoint: &str, wait: Duration) {
    LEFT_ALONE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(endpoint.to_string(), Instant::now() + wait);
}

/// The links a site's table gives.
///
/// Strict where a table cut short shows: the service may fail partway
/// through an answer it has begun with a success, and what it then writes —
/// a row half written, an error's trace — is no row of the table. Such an
/// answer is refused whole rather than read for less than it is. Lenient
/// where Wikidata's own values are: one that is not an id of its site's is
/// skipped, and counted.
fn parse(body: &[u8], site: Site) -> Result<Read> {
    let text = std::str::from_utf8(body).context("Wikidata's answer is not text")?;
    let Some(rows) = text.strip_suffix('\n') else {
        bail!("Wikidata's answer for {} was cut short", site.label());
    };
    let mut lines = rows.split('\n');
    let header = lines.next().unwrap_or_default();
    if header.trim_end_matches('\r') != HEADER {
        bail!(
            "Wikidata's answer for {} is not the table asked for: it begins {:?}",
            site.label(),
            crate::providers::clip(header, 80)
        );
    }

    let mut read = Read::default();
    for (n, line) in lines.enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let mut columns = line.split('\t');
        let row = (
            columns.next(),
            columns.next(),
            columns.next(),
            columns.next(),
        );
        let (Some(item), Some(value), Some(imdb), None) = row else {
            bail!(
                "line {} of Wikidata's answer for {} is no row of the table: the answer was cut \
                 short, or the service failed partway",
                n + 2,
                site.label()
            );
        };
        let Some(address) = item
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
        else {
            bail!(
                "line {} of Wikidata's answer for {} names no item",
                n + 2,
                site.label()
            );
        };
        // Another entity than an item — a lexeme, a property — is no work.
        let Some(item) = entity(address) else {
            read.skipped += 1;
            continue;
        };

        match literal(value).and_then(|value| site.number(value)) {
            Some(value) => read.links.push(Link {
                property: site.property(),
                value,
                item,
            }),
            None => read.skipped += 1,
        }
        if !imdb.is_empty() {
            match literal(imdb).and_then(|imdb| Site::Imdb.number(imdb)) {
                Some(value) => read.links.push(Link {
                    property: Site::Imdb.property(),
                    value,
                    item,
                }),
                None => read.skipped += 1,
            }
        }
    }
    Ok(read)
}

/// The number of the item an address names: `…/entity/Q1079` is 1079.
fn entity(address: &str) -> Option<u32> {
    let id = address.rsplit('/').next()?.strip_prefix('Q')?;
    if id.is_empty() || id.len() > 10 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    id.parse::<u32>()
        .ok()
        .filter(|&number| number > 0 && i32::try_from(number).is_ok())
}

/// What a literal of the table says, without its quotes — and without a
/// type or a language after them, should a service write one: none for
/// anything else, an address among them, or one with an escape in it, which
/// no id has.
fn literal(text: &str) -> Option<&str> {
    let inner = text.strip_prefix('"')?;
    let end = inner.rfind('"')?;
    let value = &inner[..end];
    (!value.contains('\\')).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_site_has_its_property_and_its_ids() {
        for site in Site::ALL {
            assert_eq!(Site::of_property(site.property()), Some(site));
            if let Some(source) = site.source() {
                assert_eq!(Site::of_source(source), Some(site));
            }
        }
        assert_eq!(Site::of_source(ExternalSource::Fankai), None);
        assert!(!Site::ANCHORS.contains(&Site::Imdb));

        assert_eq!(Site::Imdb.number("tt0903747"), Some(903_747));
        assert_eq!(Site::Imdb.id(903_747), "tt0903747");
        assert_eq!(Site::Imdb.number("tt10872600"), Some(10_872_600));
        assert_eq!(Site::Imdb.id(10_872_600), "tt10872600");
        for not_one in [
            "nm0311986",
            "ev0000101",
            "tt090374",
            "tt00903747",
            "903747",
            "tt",
        ] {
            assert_eq!(Site::Imdb.number(not_one), None, "{not_one}");
        }
        assert_eq!(Site::TmdbTv.number("1396"), Some(1396));
        assert_eq!(Site::TmdbTv.id(1396), "1396");
        for not_one in [
            "",
            "0",
            "+5",
            "-5",
            "lists/breaking-bad",
            "430493-series",
            "/205918",
            "99999999999",
            "3000000000",
        ] {
            assert_eq!(Site::TvdbSeries.number(not_one), None, "{not_one}");
        }
    }

    #[test]
    fn the_query_asks_for_the_site_with_imdb_beside() {
        let asked = query(Site::TvdbMovie, Part::Ending(3));
        assert!(asked.contains("?item wdt:P12196 ?value ."), "{asked}");
        assert!(
            asked.contains("FILTER(STRENDS(STR(?value), \"3\"))"),
            "{asked}"
        );
        let rest = query(Site::TvdbMovie, Part::Rest);
        assert!(
            rest.contains("FILTER(!REGEX(STR(?value), \"[0-9]$\"))"),
            "{rest}"
        );
        assert!(
            asked.contains("OPTIONAL { ?item wdt:P345 ?imdb . }"),
            "{asked}"
        );
    }

    /// The eleven parts hold every value once: its last digit's, or the rest.
    #[test]
    fn every_value_falls_in_one_part() {
        for (value, part) in [
            ("27205", Part::Ending(5)),
            ("113", Part::Ending(3)),
            ("tt1375666", Part::Ending(6)),
            ("https://thetvdb.com/movies/inception", Part::Rest),
            ("", Part::Rest),
        ] {
            assert_eq!(Part::of(value), part, "{value}");
            assert_eq!(Part::ALL.iter().filter(|&&p| p == part).count(), 1);
        }
    }

    const ITEM: &str = "<http://www.wikidata.org/entity/Q";

    #[test]
    fn a_table_is_read_into_links_and_what_is_no_id_is_skipped() {
        // As the service wrote them in November 2026: a film with its IMDb
        // id, one without, Wikidata's "unknown value" where an id goes, a
        // link where an id goes, IMDb's id of a person; and a lexeme, which
        // is no work.
        let body = format!(
            "{HEADER}\n\
             {ITEM}27204>\t\"253\"\t\"tt0070328\"\n\
             {ITEM}141514705>\t\"1151\"\t\n\
             <http://www.wikidata.org/entity/L7>\t\"7\"\t\n\
             {ITEM}5>\t<http://www.wikidata.org/.well-known/genid/094e>\t\"tt0000005\"\n\
             {ITEM}6>\t\"https://www.themoviedb.org/movie/1656332-stars\"\t\"nm2118717\"\n"
        );
        let read = parse(body.as_bytes(), Site::TmdbMovie).expect("read");
        assert_eq!(
            read.links,
            vec![
                Link {
                    property: 4947,
                    value: 253,
                    item: 27204
                },
                Link {
                    property: 345,
                    value: 70328,
                    item: 27204
                },
                Link {
                    property: 4947,
                    value: 1151,
                    item: 141_514_705
                },
                Link {
                    property: 345,
                    value: 5,
                    item: 5
                },
            ]
        );
        assert_eq!(read.skipped, 4);

        // What names no entity at all is no row of the table asked.
        let body = format!("{HEADER}\n\"Q7\"\t\"7\"\t\n");
        assert!(parse(body.as_bytes(), Site::TmdbMovie).is_err());
    }

    #[test]
    fn a_table_cut_short_is_refused_whole() {
        let whole = format!("{HEADER}\n{ITEM}1079>\t\"1396\"\t\"tt0903747\"\n");
        assert_eq!(
            parse(whole.as_bytes(), Site::TmdbTv)
                .expect("read")
                .links
                .len(),
            2
        );
        // No answer at all, or nothing but the header: a table of nothing.
        assert!(parse(b"", Site::TmdbTv).is_err());
        assert!(
            parse(format!("{HEADER}\n").as_bytes(), Site::TmdbTv)
                .expect("read")
                .links
                .is_empty()
        );

        // A row half written, at the end.
        let cut = &whole[..whole.len() - 8];
        assert!(parse(cut.as_bytes(), Site::TmdbTv).is_err());
        // A trace written after the rows the service had sent.
        let traced = format!(
            "{whole}SPARQL-QUERY: queryStr=SELECT\njava.util.concurrent.TimeoutException\n\tat \
             com.bigdata.rdf.sail.webapp.BigdataRDFContext\n"
        );
        assert!(parse(traced.as_bytes(), Site::TmdbTv).is_err());
        // Another table, or none.
        assert!(parse(b"{\"head\": {}}\n", Site::TmdbTv).is_err());
    }
}

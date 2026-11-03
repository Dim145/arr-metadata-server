//! AniList's shape: a work held here written as AniList's clients read an
//! entry — its `Media` — for the AniList relay to answer from the catalogue
//! (`anilist.relayMode = local-first`).
//!
//! AniList keeps one entry for each season or cour of a series, where the
//! catalogue keeps one work for the whole of it. An entry is written from
//! where the anime identifier list places it on the work, and from what the
//! work's episodes say of that part as they are served, corrections included
//! ([`crate::service::cour`]): its days, its status, its count, the season it
//! premiered in, its next episode and its schedule. Then, field by field:
//!
//! 1. **the catalogue**: what the work holds — its locks, its English text,
//!    its pictures, its genres, its characters and who voices them — for the
//!    entry that is the whole work; and, for any entry, what a person
//!    corrected here, as the relay writes it over AniList's own;
//! 2. **AniList's own document of the entry**, as the work's last refresh
//!    kept it, where it was about this very entry: its romaji and native
//!    titles, its other names, its tags and its studios with AniList's ids
//!    for them, its score, its count of members, the entries it is related
//!    to;
//! 3. **MyAnimeList's**, for an entry it described: its titles, synopsis,
//!    planned count, type, source and studios;
//! 4. **else null, or empty**, as AniList's schema writes what an entry has
//!    none of. Nothing is made up: an identifier only AniList gives — a tag's,
//!    a studio's — cannot be written here where the kept document has none,
//!    and a document that asks for one is AniList's to answer
//!    ([`FieldError::Unanswerable`]), as is anything about an account. What
//!    an entry is made of and AniList's own ids do not name here — a
//!    character only TheTVDB lists, a voice, an episode on the schedule —
//!    takes a number of this server's ([`ids::anilist_character`],
//!    [`ids::anilist_voice`], [`ids::anilist_airing`]).
//!
//! A page of entries — AniList's `Page` — is answered from entries named by
//! their ids alone, narrowed and sorted as AniList would ([`Listing`]), and
//! a page of their schedules likewise ([`Scheduling`]): such a page rests on
//! entries held here and nothing else.
//!
//! No I/O: the relay reads what an entry is written from ([`Entry`]); this
//! lays it out ([`Written`], [`media`], [`page`]) for the projection of
//! [`crate::graphql`].

use serde_json::{Value, json};

use crate::{
    domain::{
        CoverType, Image, MediaItem, MediaKind, Relation, lead_images,
        person::{Character as Held, CharacterCredit, CharacterRole, CharacterVoice},
    },
    graphql::{Arguments, FieldError, Object, Resolved},
    providers::mal,
    service::{
        cour::{Airing, AnilistSeason, Cour, Next, Status},
        ids,
    },
};

/// AniList's genres: the nineteen there are, in its order — what its
/// `GenreCollection` answers, the same on 1 and 3 November 2026.
pub const GENRES: [&str; 19] = [
    "Action",
    "Adventure",
    "Comedy",
    "Drama",
    "Ecchi",
    "Fantasy",
    "Hentai",
    "Horror",
    "Mahou Shoujo",
    "Mecha",
    "Music",
    "Mystery",
    "Psychological",
    "Romance",
    "Sci-Fi",
    "Slice of Life",
    "Sports",
    "Supernatural",
    "Thriller",
];

/// How many characters the AniList source asks for, a page of AniList's: a
/// list it kept before it asked whether more followed may have been cut
/// there.
const KEPT_CHARACTERS: usize = 25;

/// The order the AniList source asks an entry's characters in: a list cut
/// at a page is that order's first page.
const KEPT_ORDER: [&str; 3] = ["ROLE", "RELEVANCE", "ID"];

/// How many entries a page of a list within an entry holds when the client
/// does not say, and at the most: AniList's own figures.
const PER_PAGE: i64 = 25;
const MOST_PER_PAGE: i64 = 50;

/// What an entry is written from: the work held here, where the anime
/// identifier list places the entry on it, and what AniList and MyAnimeList
/// last said of it.
pub struct Entry<'a> {
    /// AniList's id for the entry: what it is answered under.
    pub id: i64,
    /// The work, as it is served: its locks laid, its pictures addressed as
    /// clients elsewhere follow them.
    pub item: &'a MediaItem,
    /// Where the list places the entry on the work, described from the
    /// work's episodes: a season, a cour, or the whole work.
    pub cour: &'a Cour,
    /// When each of the entry's episodes airs.
    pub schedule: &'a [Airing],
    /// AniList's own document of the work's entry — `data.Media` of what it
    /// answered at the work's last refresh — whichever entry is asked.
    pub anilist: Option<&'a Value>,
    /// MyAnimeList's description of the entry, from what was kept or asked
    /// lately.
    pub mal: Option<&'a mal::Described>,
    /// What the work's sources called the entry, naming it beside the work.
    pub named: Option<&'a str>,
    /// The work's title and overview in English, AniList's language, where
    /// held ([`english`]).
    pub english: English,
    /// Now, in seconds since the epoch.
    pub now: i64,
    /// Whether the caller may be shown what is for adults: its policy, as
    /// Sonarr's and Radarr's is read.
    pub adult: bool,
}

/// A work's title and overview in English, where held here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct English {
    pub title: Option<String>,
    pub overview: Option<String>,
}

/// What the work is called and told of in English, the language AniList
/// writes in, as it is held here: a text locked in English; else one locked
/// in the catalogue's language or in every one — what the relay writes over
/// AniList's own; else the catalogue's own text, where the catalogue is kept
/// in English (`stored`, as translations are filed: `eng`); else the
/// translation held in English. None where none of them says: another
/// language's text is never written for it.
pub fn english(item: &MediaItem, stored: &str) -> English {
    let held = |field: &str, own: &Option<String>, translated: Option<&Option<String>>| {
        if let Some(lock) = item
            .locked_texts
            .iter()
            .find(|t| t.scope == "item" && t.field == field && t.language == "eng")
        {
            return lock.value.clone().and_then(|text| said(&text));
        }
        if stored == "eng" || item.locked_fields.contains(&format!("item/{field}")) {
            return own.as_deref().and_then(said);
        }
        translated.and_then(|text| text.as_deref()).and_then(said)
    };
    let translation = item.translations.iter().find(|t| t.language == "eng");
    English {
        title: held(
            "title",
            &Some(item.title.clone()),
            translation.map(|t| &t.title),
        ),
        overview: held("overview", &item.overview, translation.map(|t| &t.overview)),
    }
}

/// A text that says something, its blanks around it aside.
fn said(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A string AniList's document holds at `pointer`, when it says something.
fn their_text(document: Option<&Value>, pointer: &str) -> Option<String> {
    document?.pointer(pointer)?.as_str().and_then(said)
}

/// What the work's own locks say of when an entry airs, as the relay writes
/// them into AniList's: its status — ended is every entry finished, upcoming
/// every one not yet released, continuing something only of an entry that
/// is the whole work, a film's released or in cinemas finished; its first
/// day, the work's, for the entry that opens the work; its last, the work's,
/// for the entry that closes a work that ended; a film's day in cinemas its
/// start and its end. Each none where nothing is locked, or the lock is empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Locked {
    pub status: Option<Status>,
    pub start: Option<String>,
    pub end: Option<String>,
}

pub fn locked_airing(item: &MediaItem, cour: &Cour) -> Locked {
    let locked = |field: &str| item.locked_fields.contains(&format!("item/{field}"));
    let film = item.kind == MediaKind::Movie;
    let mut said = Locked::default();
    if locked("status") {
        said.status = match (item.status.as_deref(), film) {
            (Some("ended"), false) | (Some("released" | "inCinemas"), true) => {
                Some(Status::Finished)
            }
            (Some("upcoming"), false) | (Some("announced" | "tba"), true) => {
                Some(Status::NotYetReleased)
            }
            (Some("continuing"), false) if cour.whole => Some(Status::Releasing),
            _ => None,
        };
    }
    if film {
        if locked("inCinemas")
            && let Some(day) = day_of(item.in_cinemas.as_deref())
        {
            said.start = Some(day.clone());
            said.end = Some(day);
        }
    } else {
        if locked("firstAired")
            && cour.opens_work
            && let Some(day) = day_of(item.first_aired.as_deref())
        {
            said.start = Some(day);
        }
        if locked("lastAired")
            && cour.ends_work
            && item.status.as_deref() == Some("ended")
            && let Some(day) = day_of(item.last_aired.as_deref())
        {
            said.end = Some(day);
        }
    }
    said
}

/// The day a date or an instant falls on, `YYYY-MM-DD`.
fn day_of(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .and_then(|text| text.get(..10))
        .filter(|day| chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").is_ok())
        .map(String::from)
}

/// How an entry runs: its first day and its last — `YYYY-MM-DD`, or less
/// where less is known — its status in AniList's words, and its next
/// episode.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Run {
    start: Option<String>,
    end: Option<String>,
    status: Option<&'static str>,
    next: Option<Next>,
}

/// An entry's titles, as AniList names them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Titles {
    romaji: Option<String>,
    english: Option<String>,
    native: Option<String>,
    preferred: Option<String>,
}

/// An entry laid out for writing: what it is written from, and what is
/// worked out of it once — how it runs, what it is called.
pub struct Written<'a> {
    entry: &'a Entry<'a>,
    /// Whether AniList's document is of this very entry.
    own: bool,
    run: Run,
    titles: Titles,
}

impl<'a> Written<'a> {
    pub fn of(entry: &'a Entry<'a>) -> Self {
        let own = entry
            .anilist
            .and_then(|theirs| theirs.get("id"))
            .and_then(Value::as_i64)
            == Some(entry.id);
        let mut written = Self {
            entry,
            own,
            run: Run::default(),
            titles: Titles::default(),
        };
        written.run = written.run();
        written.titles = written.titles();
        written
    }

    fn item(&self) -> &'a MediaItem {
        self.entry.item
    }

    fn whole(&self) -> bool {
        self.entry.cour.whole
    }

    fn film(&self) -> bool {
        self.entry.item.kind == MediaKind::Movie
    }

    fn locked(&self, field: &str) -> bool {
        self.item().locked_fields.contains(&format!("item/{field}"))
    }

    /// AniList's document, where it is of this entry.
    fn theirs(&self) -> Option<&'a Value> {
        self.entry.anilist.filter(|_| self.own)
    }

    fn run(&self) -> Run {
        let (item, cour) = (self.item(), self.entry.cour);
        let mut run = if self.film() {
            // A film is its day in cinemas, or its year alone.
            let day = day_of(item.in_cinemas.as_deref());
            let start = day
                .clone()
                .or_else(|| item.year.map(|year| format!("{year:04}")));
            let today = chrono::DateTime::from_timestamp(self.entry.now, 0)
                .map(|now| now.format("%Y-%m-%d").to_string());
            let status = match item.status.as_deref() {
                Some("released" | "inCinemas") => Some(Status::Finished),
                Some("announced" | "tba") => Some(Status::NotYetReleased),
                _ => match (&day, &today) {
                    (Some(day), Some(today)) if day <= today => Some(Status::Finished),
                    (Some(_), Some(_)) => Some(Status::NotYetReleased),
                    _ => None,
                },
            };
            Run {
                end: start.clone().filter(|_| status == Some(Status::Finished)),
                start,
                status: status.map(Status::anilist),
                next: None,
            }
        } else {
            Run {
                start: cour.starts.clone(),
                end: cour.ends.clone(),
                status: cour.status.map(Status::anilist),
                next: cour.next.clone(),
            }
        };

        // Nothing of it held: what MyAnimeList says of it.
        if run.status.is_none()
            && let Some(described) = self.entry.mal
        {
            run.status = described.status.as_deref().and_then(mal_status);
            if run.start.is_none() {
                run.start = described.starts.clone();
            }
            if run.end.is_none() && run.status == Some("FINISHED") {
                run.end = described.ends.clone();
            }
        }

        // What the work's locks say, as the relay writes them.
        let locked = locked_airing(item, cour);
        if let Some(status) = locked.status {
            run.status = Some(status.anilist());
        }
        if locked.start.is_some() {
            run.start = locked.start;
        }
        if locked.end.is_some() {
            run.end = locked.end;
        }
        if run.status == Some("FINISHED") {
            run.next = None;
        }
        run
    }

    fn titles(&self) -> Titles {
        let item = self.item();
        let theirs = |key: &str| their_text(self.theirs(), &format!("/title/{key}"));
        let described = self.entry.mal;
        let alternative = |kind: &str| {
            item.alternative_titles
                .iter()
                .find(|t| t.title_type.as_deref() == Some(kind))
                .and_then(|t| said(&t.title))
        };
        let whole = self.whole();
        // The title locked here is the work's: written over the entry that
        // is the whole of it, as the relay writes it.
        let locked = (whole && self.locked("title")).then(|| item.title.clone());

        let english = locked
            .clone()
            .or_else(|| whole.then(|| self.entry.english.title.clone()).flatten())
            .or_else(|| theirs("english"))
            .or_else(|| described.and_then(|d| d.english.as_deref()).and_then(said));
        let romaji = theirs("romaji")
            .or_else(|| whole.then(|| alternative("Romaji")).flatten())
            .or_else(|| described.and_then(|d| d.title.as_deref()).and_then(said))
            .or_else(|| (!whole).then(|| self.entry.named.and_then(said)).flatten());
        let native = theirs("native")
            .or_else(|| {
                whole
                    .then(|| {
                        alternative("Native")
                            .or_else(|| item.original_title.as_deref().and_then(said))
                    })
                    .flatten()
            })
            .or_else(|| described.and_then(|d| d.japanese.as_deref()).and_then(said));
        let preferred = locked
            .or_else(|| romaji.clone())
            .or_else(|| english.clone())
            .or_else(|| native.clone())
            .or_else(|| self.entry.named.and_then(said))
            .or_else(|| whole.then(|| said(&item.title)).flatten());
        Titles {
            romaji,
            english,
            native,
            preferred,
        }
    }

    fn id_mal(&self) -> Option<i64> {
        self.entry
            .cour
            .mal
            .or_else(|| {
                self.theirs()
                    .and_then(|theirs| theirs.get("idMal"))
                    .and_then(Value::as_i64)
            })
            .or_else(|| {
                self.whole()
                    .then(|| self.item().external_ids.mal.iter().copied().min())
                    .flatten()
            })
    }

    fn format(&self) -> Option<String> {
        let item = self.item();
        let known = |format: &str| {
            let format = format.trim().to_ascii_uppercase();
            ["TV", "TV_SHORT", "MOVIE", "SPECIAL", "OVA", "ONA", "MUSIC"]
                .contains(&format.as_str())
                .then_some(format)
        };
        self.whole()
            .then(|| item.format.as_deref().and_then(known))
            .flatten()
            .or_else(|| their_text(self.theirs(), "/format").and_then(|f| known(&f)))
            .or_else(|| self.entry.cour.format.as_deref().and_then(known))
            .or_else(|| {
                self.entry
                    .mal
                    .and_then(|d| d.media_type.as_deref())
                    .and_then(mal_format)
                    .map(String::from)
            })
            .or_else(|| self.film().then(|| "MOVIE".to_string()))
    }

    /// AniList's season and year of the entry's first day.
    fn season(&self) -> Option<(AnilistSeason, i32)> {
        self.run.start.as_deref().and_then(AnilistSeason::of)
    }

    /// How many episodes it is to have: a film's one; a finished entry's,
    /// as held; else as MyAnimeList plans them, or as held where the entry
    /// is closed by the next; none while nothing says.
    fn episodes(&self) -> Option<i64> {
        if self.film() {
            return Some(1);
        }
        let cour = self.entry.cour;
        let held = (cour.episodes > 0).then_some(cour.episodes);
        let planned = self.entry.mal.and_then(|d| d.episodes).filter(|n| *n > 0);
        if self.run.status == Some("FINISHED") {
            return held.or(planned);
        }
        planned.or_else(|| held.filter(|_| cour.closed))
    }

    /// What the entry is about, and whether it is AniList's own text — which
    /// is written with its markup — rather than plain text held here.
    fn description(&self) -> Option<(String, bool)> {
        self.whole()
            .then(|| self.entry.english.overview.clone())
            .flatten()
            .map(|text| (text, false))
            .or_else(|| their_text(self.theirs(), "/description").map(|text| (text, true)))
            .or_else(|| {
                self.entry
                    .mal
                    .and_then(|d| d.synopsis.as_deref())
                    .and_then(said)
                    .map(|text| (text, false))
            })
    }

    /// The entry's other names, less those a person hid here among the
    /// work's other titles: AniList's own, for the entry its document is
    /// of; the work's other titles, for the whole work; else MyAnimeList's.
    fn synonyms(&self) -> Vec<String> {
        let item = self.item();
        let titles = &self.titles;
        let given: Vec<String> = [&titles.romaji, &titles.english, &titles.native]
            .into_iter()
            .flatten()
            .map(|t| folded(t))
            .collect();
        let names: Vec<String> = match self.theirs().and_then(|theirs| theirs.get("synonyms")) {
            Some(Value::Array(theirs)) => theirs
                .iter()
                .filter_map(|name| name.as_str().and_then(said))
                .collect(),
            _ if self.whole() => item
                .alternative_titles
                .iter()
                .filter_map(|t| said(&t.title))
                .filter(|name| !given.contains(&folded(name)))
                .collect(),
            _ => self
                .entry
                .mal
                .map(|d| d.synonyms.iter().filter_map(|s| said(s)).collect())
                .unwrap_or_default(),
        };
        let hidden: Vec<String> = item
            .hidden
            .alternative_titles
            .iter()
            .map(|t| folded(&t.title))
            .collect();
        let mut kept: Vec<String> = Vec::new();
        for name in names {
            if !hidden.contains(&folded(&name)) && !kept.iter().any(|k| folded(k) == folded(&name))
            {
                kept.push(name);
            }
        }
        kept
    }

    /// The genres: those locked here, as the relay writes them into every
    /// entry; else AniList's for the work; else the catalogue's that are
    /// among AniList's nineteen.
    fn genres(&self) -> Vec<String> {
        let item = self.item();
        if self.locked("genres") {
            return item.genres.clone();
        }
        if let Some(Value::Array(theirs)) = self.entry.anilist.and_then(|a| a.get("genres")) {
            return theirs
                .iter()
                .filter_map(|genre| genre.as_str().and_then(said))
                .collect();
        }
        anilist_genres(&item.genres)
    }

    /// AniList's score of the entry, out of a hundred: the work's, from
    /// AniList, for the entry its document is of.
    fn score(&self) -> Option<i64> {
        if !self.own {
            return None;
        }
        self.item()
            .ratings
            .iter()
            .find(|rating| rating.source == "anilist")
            .and_then(|rating| rating.value)
            .map(|value| (value * 10.0).round() as i64)
            .or_else(|| {
                let theirs = self.theirs()?;
                ["weightedScore", "averageScore"]
                    .iter()
                    .find_map(|key| theirs.get(*key).and_then(Value::as_f64))
                    .filter(|score| *score > 0.0)
                    .map(|score| score.round() as i64)
            })
    }

    /// The cover: the poster chosen here, which the relay writes into every
    /// entry; else the season's own, for an entry that is part of a work;
    /// else the work's.
    fn cover(&self) -> Option<String> {
        let item = self.item();
        let work = || {
            lead(
                item.images.iter().filter(|i| i.season_number.is_none()),
                item,
                CoverType::Poster,
            )
        };
        if self.locked("primaryPoster") {
            return work();
        }
        let season = match self.entry.cour.span {
            Some(crate::service::cour::Span::Season { number, .. }) if !self.whole() => item
                .seasons
                .iter()
                .find(|season| season.season_number == number)
                .and_then(|season| {
                    lead_images(&season.images, &season.primary_images)
                        .into_iter()
                        .find(|image| image.cover_type == CoverType::Poster)
                        .map(|image| image.url.clone())
                }),
            _ => None,
        };
        season.or_else(work)
    }

    /// The banner: the work's background, the one chosen here first, as the
    /// relay writes it into every entry.
    fn banner(&self) -> Option<String> {
        let item = self.item();
        lead(
            item.images.iter().filter(|i| i.season_number.is_none()),
            item,
            CoverType::Fanart,
        )
    }

    /// The entry's main studios, by name: the one locked here, as the relay
    /// writes it into every entry; AniList's, for the entry its document is
    /// of; the work's, for the whole work; else MyAnimeList's.
    fn studios(&self) -> Vec<StudioRef> {
        let item = self.item();
        let theirs = self.theirs().map(their_studios).unwrap_or_default();
        let by_name = |name: String| StudioRef {
            id: None,
            name,
            main: true,
            animation: None,
        };
        if self.locked("studio") {
            let Some(name) = item.studio.as_deref().and_then(said) else {
                return Vec::new();
            };
            // As the relay writes it: AniList's studio of that name first,
            // the one main studio; else the name alone, which no id of
            // AniList's goes with.
            let wanted = folded(&name);
            return match theirs.iter().position(|s| folded(&s.name) == wanted) {
                Some(at) => {
                    let mut studios = theirs;
                    let mut chosen = studios.remove(at);
                    chosen.main = true;
                    for other in &mut studios {
                        other.main = false;
                    }
                    studios.insert(0, chosen);
                    studios
                }
                None => vec![by_name(name)],
            };
        }
        if !theirs.is_empty() {
            return theirs;
        }
        if self.whole() {
            return item
                .studio
                .as_deref()
                .and_then(said)
                .map(by_name)
                .into_iter()
                .collect();
        }
        self.entry
            .mal
            .map(|d| {
                d.studios
                    .iter()
                    .filter_map(|s| said(s))
                    .map(by_name)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// AniList's count of the members who list the entry, as its kept
    /// document of this very entry says: AniList's figure, never this
    /// server's.
    fn their_popularity(&self) -> Option<i64> {
        self.theirs()?
            .get("popularity")
            .and_then(Value::as_f64)
            .map(|figure| figure.round() as i64)
    }

    /// The characters of the entry, as the catalogue keeps the work's: those
    /// of an entry that is the whole work, or that AniList's kept document is
    /// of — whose characters the work's are. None where the catalogue does
    /// not know them, which AniList does: a work last refreshed before its
    /// characters were kept, another season or cour. A work entered by hand,
    /// under a number of this server's, has what it holds, none perhaps:
    /// nobody else has it.
    /// How many of AniList's characters its document holds, where they are
    /// a page cut from a longer list: none where they are all AniList lists
    /// of the entry the document is of — its page of them said no other
    /// followed, or, kept before it said so, held fewer than a page. A work
    /// no AniList document is kept for has what it holds.
    fn characters_cut(&self) -> Option<usize> {
        let theirs = self.entry.anilist?;
        let kept = theirs
            .pointer("/characters/edges")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let more = theirs
            .pointer("/characters/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(kept >= KEPT_CHARACTERS);
        more.then_some(kept)
    }

    fn characters(&self) -> Option<&'a [CharacterCredit]> {
        let item = self.item();
        if ids::is_own(self.entry.id) {
            return Some(&item.characters);
        }
        let known = !item.characters.is_empty()
            || self
                .entry
                .anilist
                .is_some_and(|theirs| theirs.get("characters").is_some());
        (known && (self.whole() || self.own)).then_some(item.characters.as_slice())
    }

    /// The entries this one is related to: AniList's own list, for the entry
    /// its document is of, then those of AniList's the work keeps besides;
    /// for the whole work without it, those the work keeps.
    fn related(&self) -> Vec<Related> {
        let mut related: Vec<Related> = Vec::new();
        if let Some(edges) = self
            .theirs()
            .and_then(|theirs| theirs.pointer("/relations/edges"))
            .and_then(Value::as_array)
        {
            related.extend(edges.iter().filter_map(Related::from_theirs));
        }
        if self.own || self.whole() {
            for kept in self
                .item()
                .relations
                .iter()
                .filter(|relation| relation.source == "anilist")
            {
                if !related.iter().any(|r| r.id == kept.external_id) {
                    related.push(Related::from_kept(kept));
                }
            }
        }
        // Kept from a caller who may not see works for adults, by AniList's
        // flag, as the catalogue's lists keep them (`hide_adult_relations`).
        if !self.entry.adult {
            related.retain(|r| !r.is_adult);
        }
        related
    }

    fn trailer(&self) -> Option<String> {
        self.whole()
            .then(|| self.item().trailer_youtube_id.as_deref().and_then(said))
            .flatten()
    }

    /// The country it was made in, AniList's two letters.
    fn country(&self) -> Option<String> {
        let country = self.item().original_country.as_deref()?.trim();
        match country.len() {
            2 => Some(country.to_ascii_uppercase()),
            3 => crate::providers::lang::iso_3166_3_to_2(country).map(str::to_ascii_uppercase),
            _ => None,
        }
    }

    fn updated_at(&self) -> Option<i64> {
        crate::db::parse_rfc3339(&self.item().updated_at).map(|at| at.timestamp())
    }

    fn site_url(&self) -> String {
        format!("https://anilist.co/anime/{}", self.entry.id)
    }
}

/// A name or a title as it is compared: its case and the blanks around it
/// aside.
fn folded(text: &str) -> String {
    text.trim().to_lowercase()
}

/// A studio of an entry: AniList's, with its id, where its kept document
/// names it so; else by its name alone.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StudioRef {
    id: Option<i64>,
    name: String,
    /// Whether it is one of the entry's main studios.
    main: bool,
    /// Whether it animates, as AniList says. A studio named here by its
    /// name alone is the one that animated the entry.
    animation: Option<bool>,
}

/// The studios of AniList's kept document: every studio of the entry, main
/// or not, each with its id, as the AniList source keeps them now
/// (`studios { edges { isMain node { id name isAnimationStudio } } }`); or,
/// in a document kept before, the names of its main ones.
fn their_studios(theirs: &Value) -> Vec<StudioRef> {
    if let Some(edges) = theirs.pointer("/studios/edges").and_then(Value::as_array) {
        return edges
            .iter()
            .filter_map(|edge| {
                let node = edge.get("node")?;
                Some(StudioRef {
                    id: node.get("id").and_then(Value::as_i64),
                    name: node.get("name").and_then(Value::as_str).and_then(said)?,
                    main: edge.get("isMain").and_then(Value::as_bool).unwrap_or(false),
                    animation: node.get("isAnimationStudio").and_then(Value::as_bool),
                })
            })
            .collect();
    }
    theirs
        .pointer("/studios/nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| {
                    Some(StudioRef {
                        id: node.get("id").and_then(Value::as_i64),
                        name: node.get("name").and_then(Value::as_str).and_then(said)?,
                        main: true,
                        animation: node.get("isAnimationStudio").and_then(Value::as_bool),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The address of the lead image of a kind among `images`: the one a person
/// chose, else one added by hand, else the first.
fn lead<'i>(
    images: impl IntoIterator<Item = &'i Image>,
    item: &MediaItem,
    kind: CoverType,
) -> Option<String> {
    let of_kind: Vec<&Image> = images
        .into_iter()
        .filter(|image| image.cover_type == kind)
        .collect();
    lead_images(of_kind, &item.primary_images)
        .first()
        .map(|image| image.url.clone())
}

/// The catalogue's genres that are among AniList's nineteen, by their
/// English names — TMDB's combined ones taken apart, its science fiction
/// AniList's sci-fi. A genre of another language, or one AniList does not
/// have, is left out.
fn anilist_genres(genres: &[String]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for part in genres
        .iter()
        .flat_map(|genre| crate::domain::genre_parts(genre))
    {
        let part = part.trim();
        let named = if part.eq_ignore_ascii_case("Science Fiction")
            || part.eq_ignore_ascii_case("Science-Fiction")
        {
            Some("Sci-Fi")
        } else {
            GENRES
                .iter()
                .copied()
                .find(|genre| genre.eq_ignore_ascii_case(part))
        };
        if let Some(genre) = named
            && !found.iter().any(|f| f == genre)
        {
            found.push(genre.to_string());
        }
    }
    found
}

/// MyAnimeList's status of an entry, in AniList's words.
fn mal_status(status: &str) -> Option<&'static str> {
    match status {
        "finished_airing" => Some("FINISHED"),
        "currently_airing" => Some("RELEASING"),
        "not_yet_aired" => Some("NOT_YET_RELEASED"),
        _ => None,
    }
}

/// MyAnimeList's type of an entry, as AniList's format.
fn mal_format(media_type: &str) -> Option<&'static str> {
    match media_type {
        "tv" => Some("TV"),
        "ova" => Some("OVA"),
        "movie" => Some("MOVIE"),
        "special" | "tv_special" => Some("SPECIAL"),
        "ona" => Some("ONA"),
        "music" => Some("MUSIC"),
        _ => None,
    }
}

/// What MyAnimeList says an entry was adapted from, in AniList's
/// `MediaSource` of `version`. Each version names a few more than the one
/// before — the second `NOVEL`, `DOUJINSHI` and `ANIME`, the third
/// `WEB_NOVEL`, `GAME`, `COMIC`, `LIVE_ACTION`, `MULTIMEDIA_PROJECT` and
/// `PICTURE_BOOK` — and one a version does not name is its `OTHER`, a web
/// novel the second's `NOVEL`.
fn source(mal: &str, version: i64) -> Option<&'static str> {
    let latest = match mal {
        "original" => "ORIGINAL",
        "manga" | "4_koma_manga" | "web_manga" | "digital_manga" => "MANGA",
        "light_novel" => "LIGHT_NOVEL",
        "visual_novel" => "VISUAL_NOVEL",
        "game" => "VIDEO_GAME",
        "card_game" => "GAME",
        "novel" => "NOVEL",
        "web_novel" => "WEB_NOVEL",
        "picture_book" => "PICTURE_BOOK",
        "mixed_media" => "MULTIMEDIA_PROJECT",
        "book" | "music" | "radio" | "other" => "OTHER",
        _ => return None,
    };
    Some(match (latest, version) {
        ("NOVEL" | "DOUJINSHI" | "ANIME", ..=1) => "OTHER",
        ("WEB_NOVEL", 2) => "NOVEL",
        (
            "WEB_NOVEL" | "GAME" | "COMIC" | "LIVE_ACTION" | "MULTIMEDIA_PROJECT" | "PICTURE_BOOK",
            ..=2,
        ) => "OTHER",
        (latest, _) => latest,
    })
}

/// A relation's kind in AniList's `MediaRelation` of `version`: the second
/// adds `SOURCE`, `COMPILATION` and `CONTAINS`, the third `SAME_UNIVERSE`.
/// The first calls a source an adaptation, as AniList answered on
/// 2 November 2026; a kind a version does not name is its `OTHER`.
fn relation_type(kind: &str, version: i64) -> &str {
    match (kind, version) {
        ("SOURCE", ..=1) => "ADAPTATION",
        ("COMPILATION" | "CONTAINS", ..=1) | ("SAME_UNIVERSE", ..=2) => "OTHER",
        (kind, _) => kind,
    }
}

/// HTML of a description, as AniList writes it when asked for HTML: one
/// paragraph, its lines broken. AniList's own text keeps its markup; text
/// held here is escaped first.
fn as_html(text: &str, theirs: bool) -> String {
    let body = if theirs {
        text.to_string()
    } else {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    format!(
        "<p>{}</p>",
        body.replace("\r\n", "\n").replace('\n', "<br />\n")
    )
}

// ─── what AniList's `Media` holds that is not written here ───────────────────

/// What an entry written here answers for a field of AniList's `Media` it
/// holds nothing for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Absent {
    /// Null: a value AniList's schema lets be null.
    Null,
    /// An empty list.
    Empty,
    /// An empty page of the connection of that type.
    Page(&'static str),
    /// Nothing that may stand in: a value AniList's schema will not have
    /// null, what an account holds, what its moderators note.
    Unanswerable,
}

/// Every field of AniList's `Media`, as introspected on 1 November 2026
/// (`scripts/watch/snapshots/anilist.json`), with the arguments it takes,
/// and what an entry written here answers for it where it holds nothing.
const MEDIA: &[(&str, &[&str], Absent)] = &[
    ("id", &[], Absent::Unanswerable),
    ("idMal", &[], Absent::Null),
    ("title", &[], Absent::Null),
    ("type", &[], Absent::Null),
    ("format", &[], Absent::Null),
    ("status", &["version"], Absent::Null),
    ("description", &["asHtml"], Absent::Null),
    ("startDate", &[], Absent::Null),
    ("endDate", &[], Absent::Null),
    ("season", &[], Absent::Null),
    ("seasonYear", &[], Absent::Null),
    ("seasonInt", &[], Absent::Null),
    ("episodes", &[], Absent::Null),
    ("duration", &[], Absent::Null),
    ("chapters", &[], Absent::Null),
    ("volumes", &[], Absent::Null),
    ("countryOfOrigin", &[], Absent::Null),
    ("isLicensed", &[], Absent::Null),
    ("source", &["version"], Absent::Null),
    ("hashtag", &[], Absent::Null),
    ("trailer", &[], Absent::Null),
    ("updatedAt", &[], Absent::Null),
    ("coverImage", &[], Absent::Null),
    ("bannerImage", &[], Absent::Null),
    ("genres", &[], Absent::Empty),
    ("synonyms", &[], Absent::Empty),
    ("averageScore", &[], Absent::Null),
    ("weightedScore", &[], Absent::Null),
    ("meanScore", &[], Absent::Null),
    ("popularity", &[], Absent::Null),
    ("isLocked", &[], Absent::Null),
    ("trending", &[], Absent::Null),
    ("favourites", &[], Absent::Null),
    ("tags", &[], Absent::Empty),
    ("relations", &[], Absent::Page("MediaConnection")),
    (
        "characters",
        &["sort", "role", "page", "perPage"],
        Absent::Page("CharacterConnection"),
    ),
    (
        "staff",
        &["sort", "page", "perPage"],
        Absent::Page("StaffConnection"),
    ),
    (
        "studios",
        &["sort", "isMain"],
        Absent::Page("StudioConnection"),
    ),
    ("isFavourite", &[], Absent::Unanswerable),
    ("isFavouriteBlocked", &[], Absent::Unanswerable),
    ("isAdult", &[], Absent::Null),
    ("nextAiringEpisode", &[], Absent::Null),
    (
        "airingSchedule",
        &["notYetAired", "page", "perPage"],
        Absent::Page("AiringScheduleConnection"),
    ),
    (
        "trends",
        &["sort", "releasing", "page", "perPage"],
        Absent::Page("MediaTrendConnection"),
    ),
    ("externalLinks", &[], Absent::Empty),
    ("streamingEpisodes", &[], Absent::Empty),
    ("rankings", &[], Absent::Empty),
    ("mediaListEntry", &[], Absent::Unanswerable),
    (
        "reviews",
        &["limit", "sort", "page", "perPage"],
        Absent::Unanswerable,
    ),
    (
        "recommendations",
        &["sort", "page", "perPage"],
        Absent::Page("RecommendationConnection"),
    ),
    ("stats", &[], Absent::Null),
    ("siteUrl", &[], Absent::Null),
    ("autoCreateForumThread", &[], Absent::Unanswerable),
    ("isRecommendationBlocked", &[], Absent::Unanswerable),
    ("isReviewBlocked", &[], Absent::Unanswerable),
    ("modNotes", &[], Absent::Unanswerable),
];

/// A field of AniList's `Media` and what stands in for it, its arguments
/// checked against those it takes.
fn media_field(name: &str, arguments: &Arguments) -> Result<Absent, FieldError> {
    let (_, taken, absent) = MEDIA
        .iter()
        .find(|(field, _, _)| *field == name)
        .ok_or(FieldError::Unknown)?;
    arguments.only(taken)?;
    Ok(*absent)
}

fn absent(absent: Absent, arguments: &Arguments) -> Result<Resolved<'static>, FieldError> {
    Ok(match absent {
        Absent::Null => Resolved::Null,
        Absent::Empty => Resolved::List(Vec::new()),
        Absent::Page(typename) => {
            let (page, per_page) = page_asked(arguments)?;
            Resolved::object(EmptyPage {
                typename,
                page,
                per_page,
            })
        }
        Absent::Unanswerable => return Err(FieldError::Unanswerable),
    })
}

/// The page asked of a list within an entry, and how many a page holds:
/// AniList's first and its own count when the client does not say.
fn page_asked(arguments: &Arguments) -> Result<(i64, i64), FieldError> {
    let page = arguments.int("page")?.unwrap_or(1).max(1);
    let per_page = arguments
        .int("perPage")?
        .unwrap_or(PER_PAGE)
        .clamp(1, MOST_PER_PAGE);
    Ok((page, per_page))
}

// ─── the objects ─────────────────────────────────────────────────────────────

/// The entry, as AniList's `Media`.
pub fn media<'w>(written: &'w Written<'w>) -> impl Object + 'w {
    Media { written }
}

struct Media<'w> {
    written: &'w Written<'w>,
}

impl Object for Media<'_> {
    fn typename(&self) -> &'static str {
        "Media"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        let fallback = media_field(name, arguments)?;
        let written = self.written;
        let entry = written.entry;
        let item = entry.item;
        Ok(match name {
            "id" => Resolved::Value(json!(entry.id)),
            "idMal" => Resolved::some(written.id_mal()),
            "title" => Resolved::object(Title(&written.titles)),
            "type" => Resolved::Value(json!("ANIME")),
            "format" => Resolved::some(written.format()),
            "status" => {
                arguments.int("version")?;
                Resolved::some(written.run.status)
            }
            "description" => {
                let html = arguments.boolean("asHtml")?.unwrap_or(false);
                Resolved::some(written.description().map(|(text, theirs)| {
                    if html { as_html(&text, theirs) } else { text }
                }))
            }
            "startDate" => Resolved::object(Fuzzy::of(written.run.start.as_deref())),
            "endDate" => Resolved::object(Fuzzy::of(written.run.end.as_deref())),
            "season" => Resolved::some(written.season().map(|(season, _)| season.anilist())),
            "seasonYear" => Resolved::some(written.season().map(|(_, year)| year)),
            "seasonInt" => Resolved::some(written.season().map(|(season, year)| season.int(year))),
            "episodes" => Resolved::some(written.episodes()),
            "duration" => Resolved::some(item.runtime.filter(|minutes| *minutes > 0)),
            "countryOfOrigin" => Resolved::some(written.country()),
            "source" => {
                let version = arguments.int("version")?.unwrap_or(1);
                Resolved::some(
                    entry
                        .mal
                        .and_then(|d| d.source.as_deref())
                        .and_then(|said| source(said, version)),
                )
            }
            "trailer" => match written.trailer() {
                Some(id) => Resolved::object(Trailer(id)),
                None => Resolved::Null,
            },
            "updatedAt" => Resolved::some(written.updated_at()),
            "coverImage" => Resolved::object(Cover(written.cover())),
            "bannerImage" => Resolved::some(written.banner()),
            "genres" => Resolved::Value(json!(written.genres())),
            "synonyms" => Resolved::Value(json!(written.synonyms())),
            "averageScore" | "weightedScore" => Resolved::some(written.score()),
            // AniList's own count, where its kept document of the entry has
            // it: never this server's figure, which is another.
            "popularity" => Resolved::some(written.their_popularity()),
            "characters" => Resolved::object(Characters::of(written, arguments)?),
            "tags" => match written
                .theirs()
                .and_then(|theirs| theirs.get("tags"))
                .and_then(Value::as_array)
            {
                Some(tags) => Resolved::objects(tags.iter().map(Tag)),
                None => Resolved::List(Vec::new()),
            },
            "relations" => Resolved::object(Relations {
                related: written.related(),
            }),
            "studios" => {
                let sort = arguments.names("sort")?.unwrap_or_default();
                let main = arguments.boolean("isMain")?;
                let mut studios: Vec<StudioRef> = written
                    .studios()
                    .into_iter()
                    .filter(|studio| main.is_none_or(|main| studio.main == main))
                    .collect();
                for key in sort.iter().rev() {
                    match *key {
                        "NAME" => studios.sort_by_key(|s| folded(&s.name)),
                        "NAME_DESC" => studios.sort_by_key(|s| std::cmp::Reverse(folded(&s.name))),
                        "ID" => studios.sort_by_key(|s| s.id),
                        "ID_DESC" => studios.sort_by_key(|s| std::cmp::Reverse(s.id)),
                        // AniList's own measures and matches: its order stands.
                        _ => {}
                    }
                }
                Resolved::object(Studios { studios })
            }
            "isAdult" => Resolved::Value(json!(item.is_adult)),
            // Two episodes of the entry under one id would be one to a
            // client, whichever of them it is shown.
            "nextAiringEpisode" | "airingSchedule" if !distinct(entry.schedule) => {
                return Err(FieldError::Unanswerable);
            }
            "nextAiringEpisode" => match &written.run.next {
                Some(next) => Resolved::object(Scheduled {
                    written,
                    airing: entry
                        .schedule
                        .iter()
                        .find(|a| a.episode == next.episode && a.airing_at == next.airing_at)
                        .copied()
                        .ok_or(FieldError::Unanswerable)?,
                }),
                None => Resolved::Null,
            },
            "airingSchedule" => {
                let (page, per_page) = page_asked(arguments)?;
                let now = entry.now;
                let airings: Vec<Airing> = match arguments.boolean("notYetAired")? {
                    Some(true) => entry
                        .schedule
                        .iter()
                        .filter(|a| a.airing_at > now)
                        .copied()
                        .collect(),
                    Some(false) => entry
                        .schedule
                        .iter()
                        .filter(|a| a.airing_at <= now)
                        .copied()
                        .collect(),
                    None => entry.schedule.to_vec(),
                };
                Resolved::object(Schedule {
                    written,
                    airings,
                    page,
                    per_page,
                })
            }
            "stats" => Resolved::object(Stats(written.theirs())),
            // A number of this server's names no page of AniList's.
            "siteUrl" => Resolved::some((!ids::is_own(entry.id)).then(|| written.site_url())),
            _ => absent(fallback, arguments)?,
        })
    }
}

/// An entry's titles, AniList's `MediaTitle`. A title is written as it is
/// held, whether a client asks for it styled or not.
struct Title<'t>(&'t Titles);

impl Object for Title<'_> {
    fn typename(&self) -> &'static str {
        "MediaTitle"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        let titles = self.0;
        let styled = |title: &Option<String>| {
            arguments.only(&["stylised"])?;
            arguments.boolean("stylised")?;
            Ok(Resolved::some(title.clone()))
        };
        match name {
            "romaji" => styled(&titles.romaji),
            "english" => styled(&titles.english),
            "native" => styled(&titles.native),
            "userPreferred" => {
                arguments.only(&[])?;
                Ok(Resolved::some(titles.preferred.clone()))
            }
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A day, as much of it as is known: AniList's `FuzzyDate`.
struct Fuzzy {
    year: Option<i64>,
    month: Option<i64>,
    day: Option<i64>,
}

impl Fuzzy {
    /// From `YYYY-MM-DD` — an instant's day too — `YYYY-MM` or `YYYY`.
    fn of(text: Option<&str>) -> Self {
        // Each part's leading digits: an instant's day is followed by its time.
        let number = |part: &str| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<i64>().ok()
        };
        let mut parts = text.map(str::trim).unwrap_or_default().split('-');
        let year = parts.next().and_then(number).filter(|y| *y > 0);
        let month = year
            .and(parts.next().and_then(number))
            .filter(|m| (1..=12).contains(m));
        let day = month
            .and(parts.next().and_then(number))
            .filter(|d| (1..=31).contains(d));
        Self { year, month, day }
    }
}

impl Object for Fuzzy {
    fn typename(&self) -> &'static str {
        "FuzzyDate"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "year" => Ok(Resolved::some(self.year)),
            "month" => Ok(Resolved::some(self.month)),
            "day" => Ok(Resolved::some(self.day)),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A cover, AniList's `MediaCoverImage`: the one picture held for it, at
/// every size — as the relay writes a poster chosen here into all three.
struct Cover(Option<String>);

impl Object for Cover {
    fn typename(&self) -> &'static str {
        "MediaCoverImage"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "extraLarge" | "large" | "medium" => Ok(Resolved::some(self.0.clone())),
            "color" => Ok(Resolved::Null),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A trailer on YouTube, AniList's `MediaTrailer`.
struct Trailer(String);

impl Object for Trailer {
    fn typename(&self) -> &'static str {
        "MediaTrailer"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "id" => Ok(Resolved::Value(json!(self.0))),
            "site" => Ok(Resolved::Value(json!("youtube"))),
            "thumbnail" => Ok(Resolved::Value(json!(format!(
                "https://i.ytimg.com/vi/{}/hqdefault.jpg",
                self.0
            )))),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A tag of AniList's document, AniList's `MediaTag`: its name, its rank
/// and whether it spoils. Its id is AniList's, which the document kept here
/// does not hold.
struct Tag<'t>(&'t Value);

impl Object for Tag<'_> {
    fn typename(&self) -> &'static str {
        "MediaTag"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        let kept = self.0.get(name).filter(|value| !value.is_null()).cloned();
        match name {
            // Never null in AniList's schema: its id is AniList's own.
            "id" | "name" => kept.map(Resolved::Value).ok_or(FieldError::Unanswerable),
            "rank" | "isGeneralSpoiler" | "isMediaSpoiler" | "category" | "description"
            | "isAdult" => Ok(Resolved::some(kept)),
            // Who added it, which is somebody.
            "userId" => Ok(Resolved::Null),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// How AniList's members scored the entry, AniList's `MediaStats`: the
/// distribution kept with AniList's document of it.
struct Stats<'d>(Option<&'d Value>);

impl Object for Stats<'_> {
    fn typename(&self) -> &'static str {
        "MediaStats"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "scoreDistribution" => Ok(Resolved::objects(
                self.0
                    .and_then(|theirs| theirs.pointer("/stats/scoreDistribution"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(Scored),
            )),
            "statusDistribution" | "airingProgression" => Ok(Resolved::List(Vec::new())),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// One step of a score distribution, AniList's `ScoreDistribution`.
struct Scored<'d>(&'d Value);

impl Object for Scored<'_> {
    fn typename(&self) -> &'static str {
        "ScoreDistribution"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "score" | "amount" => Ok(Resolved::some(self.0.get(name).and_then(Value::as_i64))),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A page's description, AniList's `PageInfo`.
#[derive(Clone, Copy, Debug, Default)]
struct PageInfo {
    total: Option<i64>,
    per_page: Option<i64>,
    current_page: Option<i64>,
    last_page: Option<i64>,
    has_next_page: bool,
}

impl PageInfo {
    /// A page of a list of `total`: the last page as Laravel counts it,
    /// which AniList runs on — one at least.
    fn of(total: usize, page: i64, per_page: i64) -> Self {
        let total = i64::try_from(total).unwrap_or(i64::MAX);
        let last_page = ((total + per_page - 1) / per_page).max(1);
        Self {
            total: Some(total),
            per_page: Some(per_page),
            current_page: Some(page),
            last_page: Some(last_page),
            has_next_page: page < last_page,
        }
    }
}

impl Object for PageInfo {
    fn typename(&self) -> &'static str {
        "PageInfo"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "total" => Ok(Resolved::some(self.total)),
            "perPage" => Ok(Resolved::some(self.per_page)),
            "currentPage" => Ok(Resolved::some(self.current_page)),
            "lastPage" => Ok(Resolved::some(self.last_page)),
            "hasNextPage" => Ok(Resolved::Value(json!(self.has_next_page))),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// Whether `airings` are each under an id of their own: two episodes made one
/// number would be one episode to a client.
pub fn distinct<'i>(airings: impl IntoIterator<Item = &'i Airing>) -> bool {
    let mut seen = std::collections::HashSet::new();
    airings.into_iter().all(|airing| seen.insert(airing.id))
}

/// The part of `items` on `page`, `per_page` a page.
fn page_of<T: Clone>(items: &[T], page: i64, per_page: i64) -> Vec<T> {
    let start = usize::try_from((page - 1).saturating_mul(per_page)).unwrap_or(usize::MAX);
    let per_page = usize::try_from(per_page).unwrap_or(0);
    items.iter().skip(start).take(per_page).cloned().collect()
}

/// An empty page of one of AniList's connections: `edges`, `nodes`,
/// `pageInfo`, and nothing in them.
struct EmptyPage {
    typename: &'static str,
    page: i64,
    per_page: i64,
}

impl Object for EmptyPage {
    fn typename(&self) -> &'static str {
        self.typename
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "edges" | "nodes" => Ok(Resolved::List(Vec::new())),
            "pageInfo" => Ok(Resolved::object(PageInfo::of(0, self.page, self.per_page))),
            _ => Err(FieldError::Unknown),
        }
    }
}

// ─── related entries ─────────────────────────────────────────────────────────

/// An entry this one is related to, as AniList's document or the work's
/// relations name it.
#[derive(Clone, Debug, PartialEq)]
struct Related {
    id: i64,
    id_mal: Option<i64>,
    /// `ANIME` or `MANGA`.
    medium: String,
    format: Option<String>,
    is_adult: bool,
    titles: Titles,
    year: Option<i64>,
    cover: Option<String>,
    /// Its kind, in the third version of AniList's list.
    relation: String,
}

impl Related {
    /// One of the edges of AniList's document.
    fn from_theirs(edge: &Value) -> Option<Self> {
        let node = edge.get("node")?;
        let text = |pointer: &str| node.pointer(pointer).and_then(Value::as_str).and_then(said);
        let titles = Titles {
            romaji: text("/title/romaji"),
            english: text("/title/english"),
            native: text("/title/native"),
            preferred: text("/title/romaji")
                .or_else(|| text("/title/english"))
                .or_else(|| text("/title/native")),
        };
        Some(Self {
            id: node.get("id")?.as_i64()?,
            id_mal: node.get("idMal").and_then(Value::as_i64),
            medium: text("/type").unwrap_or_else(|| "ANIME".to_string()),
            format: text("/format"),
            is_adult: node
                .get("isAdult")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            titles,
            year: node.pointer("/startDate/year").and_then(Value::as_i64),
            cover: text("/coverImage/extraLarge").or_else(|| text("/coverImage/large")),
            relation: edge
                .get("relationType")
                .and_then(Value::as_str)
                .unwrap_or("OTHER")
                .to_string(),
        })
    }

    /// One of the work's relations, as kept: one title, whose language it
    /// does not say — written as the preferred one and the romaji one,
    /// which AniList always has.
    fn from_kept(kept: &Relation) -> Self {
        let title = said(&kept.title);
        Self {
            id: kept.external_id,
            id_mal: kept.mal_id,
            medium: kept.medium.to_ascii_uppercase(),
            format: kept.format.clone(),
            is_adult: kept.is_adult,
            titles: Titles {
                romaji: title.clone(),
                english: None,
                native: None,
                preferred: title,
            },
            year: kept.year.map(i64::from),
            cover: kept.image.clone(),
            relation: kept.relation_type.clone(),
        }
    }
}

/// The entries one is related to, AniList's `MediaConnection`: all of them
/// on one page, as AniList gives them.
struct Relations {
    related: Vec<Related>,
}

impl Object for Relations {
    fn typename(&self) -> &'static str {
        "MediaConnection"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "edges" => Ok(Resolved::objects(self.related.iter().map(RelatedEdge))),
            "nodes" => Ok(Resolved::objects(self.related.iter().map(RelatedMedia))),
            // Not counted, as AniList does not count them.
            "pageInfo" => Ok(Resolved::object(PageInfo::default())),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A relation, AniList's `MediaEdge`.
struct RelatedEdge<'r>(&'r Related);

impl Object for RelatedEdge<'_> {
    fn typename(&self) -> &'static str {
        "MediaEdge"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        match name {
            "relationType" => {
                arguments.only(&["version"])?;
                let version = arguments.int("version")?.unwrap_or(1);
                Ok(Resolved::Value(json!(relation_type(
                    &self.0.relation,
                    version
                ))))
            }
            "voiceActors" | "voiceActorRoles" => {
                arguments.only(&["language", "sort"])?;
                Ok(Resolved::List(Vec::new()))
            }
            _ => {
                arguments.only(&[])?;
                match name {
                    "node" => Ok(Resolved::object(RelatedMedia(self.0))),
                    // An entry's relation is no studio's.
                    "isMainStudio" => Ok(Resolved::Value(json!(false))),
                    "characters" => Ok(Resolved::List(Vec::new())),
                    "id" | "characterRole" | "characterName" | "roleNotes" | "dubGroup"
                    | "staffRole" | "favouriteOrder" => Ok(Resolved::Null),
                    _ => Err(FieldError::Unknown),
                }
            }
        }
    }
}

/// An entry another is related to, as AniList's `Media`: what the relation
/// says of it, and nothing more.
struct RelatedMedia<'r>(&'r Related);

impl Object for RelatedMedia<'_> {
    fn typename(&self) -> &'static str {
        "Media"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        let fallback = media_field(name, arguments)?;
        let related = self.0;
        Ok(match name {
            "id" => Resolved::Value(json!(related.id)),
            "idMal" => Resolved::some(related.id_mal),
            "type" => Resolved::Value(json!(related.medium)),
            "format" => Resolved::some(related.format.clone()),
            "isAdult" => Resolved::Value(json!(related.is_adult)),
            "title" => Resolved::object(Title(&related.titles)),
            "startDate" => Resolved::object(Fuzzy {
                year: related.year,
                month: None,
                day: None,
            }),
            "endDate" => Resolved::object(Fuzzy::of(None)),
            "coverImage" => Resolved::object(Cover(related.cover.clone())),
            "siteUrl" => Resolved::Value(json!(format!(
                "https://anilist.co/{}/{}",
                related.medium.to_ascii_lowercase(),
                related.id
            ))),
            "relations" => Resolved::object(Relations {
                related: Vec::new(),
            }),
            "stats" => Resolved::object(Stats(None)),
            _ => absent(fallback, arguments)?,
        })
    }
}

// ─── characters ──────────────────────────────────────────────────────────────

/// AniList's `StaffLanguage`, the languages a client may ask voices in, by
/// the model's ISO 639-3.
const STAFF_LANGUAGES: [(&str, &str); 10] = [
    ("JAPANESE", "jpn"),
    ("ENGLISH", "eng"),
    ("KOREAN", "kor"),
    ("ITALIAN", "ita"),
    ("SPANISH", "spa"),
    ("PORTUGUESE", "por"),
    ("FRENCH", "fra"),
    ("GERMAN", "deu"),
    ("HEBREW", "heb"),
    ("HUNGARIAN", "hun"),
];

/// A dub's language as AniList names it — its `languageV2` — from the
/// model's ISO 639-3: the names the AniList source reads them from, the
/// other way round (`providers::anilist`).
fn dub_name(language: &str) -> Option<&'static str> {
    Some(match language {
        "jpn" => "Japanese",
        "eng" => "English",
        "kor" => "Korean",
        "zho" => "Chinese",
        "cmn" => "Mandarin",
        "yue" => "Cantonese",
        "nan" => "Taiwanese",
        "fra" => "French",
        "deu" => "German",
        "ita" => "Italian",
        "spa" => "Spanish",
        "por" => "Portuguese",
        "heb" => "Hebrew",
        "hun" => "Hungarian",
        "ara" => "Arabic",
        "fil" => "Filipino",
        "tgl" => "Tagalog",
        "cat" => "Catalan",
        "fin" => "Finnish",
        "tur" => "Turkish",
        "nld" => "Dutch",
        "swe" => "Swedish",
        "nor" => "Norwegian",
        "dan" => "Danish",
        "pol" => "Polish",
        "ces" => "Czech",
        "slk" => "Slovak",
        "rus" => "Russian",
        "ukr" => "Ukrainian",
        "ell" => "Greek",
        "ron" => "Romanian",
        "tha" => "Thai",
        "vie" => "Vietnamese",
        "ind" => "Indonesian",
        "msa" => "Malaysian",
        "hin" => "Hindi",
        "urd" => "Urdu",
        "ben" => "Bengali",
        "tam" => "Tamil",
        "tel" => "Telugu",
        "nep" => "Nepali",
        "fas" => "Persian",
        _ => return None,
    })
}

/// A role's place when characters are sorted by it, AniList's order: the
/// leads, those beside them, those in the background, then those whose
/// source says nothing.
fn role_rank(role: Option<CharacterRole>) -> u8 {
    match role {
        Some(CharacterRole::Main) => 0,
        Some(CharacterRole::Supporting) => 1,
        Some(CharacterRole::Background) => 2,
        None => 3,
    }
}

/// A role in AniList's words.
fn role_name(role: CharacterRole) -> &'static str {
    match role {
        CharacterRole::Main => "MAIN",
        CharacterRole::Supporting => "SUPPORTING",
        CharacterRole::Background => "BACKGROUND",
    }
}

/// A page of the entry's characters, AniList's `CharacterConnection`: the
/// work's as the catalogue keeps them, in their order — the leads first, as
/// AniList's relevance put them where it gave them — narrowed by `role`,
/// sorted by `sort`.
///
/// `ROLE` and `ID` sort as AniList does; its members' favourites
/// (`FAVOURITES`) are not kept here, so a sort by them keeps the order kept,
/// AniList's relevance — the same characters, in an order near AniList's.
struct Characters<'w> {
    written: &'w Written<'w>,
    /// The page's characters.
    credits: Vec<&'w CharacterCredit>,
    total: usize,
    page: i64,
    per_page: i64,
    /// Whether the list kept is all AniList lists: how many there are, and
    /// whether more follow, is not known otherwise.
    whole: bool,
}

impl<'w> Characters<'w> {
    fn of(written: &'w Written<'w>, arguments: &Arguments) -> Result<Self, FieldError> {
        let (page, per_page) = page_asked(arguments)?;
        let role = match arguments.name("role")? {
            None => None,
            Some("MAIN") => Some(CharacterRole::Main),
            Some("SUPPORTING") => Some(CharacterRole::Supporting),
            Some("BACKGROUND") => Some(CharacterRole::Background),
            Some(other) => {
                return Err(FieldError::Invalid(format!(
                    "Argument \"role\" has invalid value {other}."
                )));
            }
        };
        let held = written.characters().ok_or(FieldError::Unanswerable)?;
        // A list cut at a page of AniList's: what is known of it is that
        // page alone — no other page, no longer one, nor one of other
        // characters drawn by role. Asked by role first, as it was kept, it
        // leads with the same characters, the work's main ones all among
        // those kept; within a role, the order it was kept in stands for the
        // one asked (Jellyfin's `[ROLE, FAVOURITES_DESC]`). Asked in no
        // order, or by anything else first, it would be other characters.
        let cut = written.characters_cut();
        let sorted = arguments.names("sort")?.unwrap_or_default();
        if let Some(kept) = cut
            && (page > 1
                || usize::try_from(per_page).unwrap_or(usize::MAX) > kept.min(held.len())
                || role.is_some()
                || sorted.first().copied() != Some(KEPT_ORDER[0]))
        {
            return Err(FieldError::Unanswerable);
        }
        let mut credits: Vec<&CharacterCredit> = held
            .iter()
            .filter(|credit| role.is_none() || credit.role == role)
            .collect();
        credits.sort_by_key(|credit| credit.sort_order);
        for key in sorted.iter().rev() {
            match *key {
                "ROLE" => credits.sort_by_key(|c| role_rank(c.role)),
                "ROLE_DESC" => credits.sort_by_key(|c| std::cmp::Reverse(role_rank(c.role))),
                "ID" => credits.sort_by_key(|c| ids::anilist_character(&c.character)),
                "ID_DESC" => {
                    credits.sort_by_key(|c| std::cmp::Reverse(ids::anilist_character(&c.character)))
                }
                "RELEVANCE" | "SEARCH_MATCH" | "FAVOURITES" | "FAVOURITES_DESC" => {}
                other => {
                    return Err(FieldError::Invalid(format!(
                        "Argument \"sort\" has invalid value {other}."
                    )));
                }
            }
        }
        let total = credits.len();
        Ok(Self {
            written,
            credits: page_of(&credits, page, per_page),
            total,
            page,
            per_page,
            whole: cut.is_none(),
        })
    }
}

impl Object for Characters<'_> {
    fn typename(&self) -> &'static str {
        "CharacterConnection"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "edges" => Ok(Resolved::objects(self.credits.iter().map(|credit| {
                CharacterEdge {
                    written: self.written,
                    credit,
                }
            }))),
            "nodes" => Ok(Resolved::objects(
                self.credits
                    .iter()
                    .map(|credit| CharacterNode(&credit.character)),
            )),
            "pageInfo" if self.whole => Ok(Resolved::object(PageInfo::of(
                self.total,
                self.page,
                self.per_page,
            ))),
            "pageInfo" => Err(FieldError::Unanswerable),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A character on the entry, AniList's `CharacterEdge`: its role, and who
/// voices it.
struct CharacterEdge<'w> {
    written: &'w Written<'w>,
    credit: &'w CharacterCredit,
}

impl CharacterEdge<'_> {
    /// Its voices, in `language` (AniList's `StaffLanguage`) or in every
    /// one: the original's first — Japanese, as AniList lists them — then
    /// the dubs, each in its own order.
    fn voices(&self, arguments: &Arguments) -> Result<Vec<&CharacterVoice>, FieldError> {
        arguments.only(&["language", "sort"])?;
        arguments.names("sort")?;
        let language = match arguments.name("language")? {
            None => None,
            Some(asked) => Some(
                STAFF_LANGUAGES
                    .iter()
                    .find(|(name, _)| *name == asked)
                    .map(|(_, code)| *code)
                    .ok_or_else(|| {
                        FieldError::Invalid(format!(
                            "Argument \"language\" has invalid value {asked}."
                        ))
                    })?,
            ),
        };
        let mut voices: Vec<&CharacterVoice> = self
            .credit
            .voices
            .iter()
            .filter(|voice| language.is_none_or(|language| voice.language == language))
            .collect();
        voices.sort_by(|a, b| {
            (a.language != "jpn", &a.language, a.sort_order).cmp(&(
                b.language != "jpn",
                &b.language,
                b.sort_order,
            ))
        });
        Ok(voices)
    }
}

impl Object for CharacterEdge<'_> {
    fn typename(&self) -> &'static str {
        "CharacterEdge"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        let character = &self.credit.character;
        match name {
            "voiceActors" => Ok(Resolved::objects(
                self.voices(arguments)?
                    .into_iter()
                    .map(|voice| Voice { voice, character }),
            )),
            "voiceActorRoles" => Ok(Resolved::objects(
                self.voices(arguments)?
                    .into_iter()
                    .map(|voice| VoiceRole(Voice { voice, character })),
            )),
            _ => {
                arguments.only(&[])?;
                match name {
                    "node" => Ok(Resolved::object(CharacterNode(character))),
                    "role" => Ok(Resolved::some(self.credit.role.map(role_name))),
                    "media" => Ok(Resolved::List(vec![Resolved::object(Media {
                        written: self.written,
                    })])),
                    "id" | "name" | "favouriteOrder" => Ok(Resolved::Null),
                    _ => Err(FieldError::Unknown),
                }
            }
        }
    }
}

/// A character, AniList's `Character`: its names and its picture, under
/// AniList's id where kept, else a number of this server's
/// ([`ids::anilist_character`]). What only AniList's editors and members
/// write of one — its description, age, birthday, favourites — is null.
struct CharacterNode<'c>(&'c Held);

impl Object for CharacterNode<'_> {
    fn typename(&self) -> &'static str {
        "Character"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        let character = self.0;
        match name {
            "description" => {
                arguments.only(&["asHtml"])?;
                arguments.boolean("asHtml")?;
                return Ok(Resolved::Null);
            }
            "media" => {
                arguments.only(&["sort", "type", "onList", "page", "perPage"])?;
                return Err(FieldError::Unanswerable);
            }
            _ => arguments.only(&[])?,
        }
        Ok(match name {
            "id" => Resolved::Value(json!(ids::anilist_character(character))),
            "name" => Resolved::object(Named {
                typename: "CharacterName",
                name: &character.name,
                native: character.native_name.as_deref(),
            }),
            "image" => Resolved::object(Picture {
                typename: "CharacterImage",
                url: character.image.as_deref(),
            }),
            "dateOfBirth" => Resolved::object(Fuzzy::of(None)),
            "siteUrl" => Resolved::some(
                character
                    .anilist_id
                    .map(|id| format!("https://anilist.co/character/{id}")),
            ),
            "gender" | "age" | "bloodType" | "updatedAt" | "favourites" => Resolved::Null,
            "isFavourite" | "isFavouriteBlocked" | "modNotes" => {
                return Err(FieldError::Unanswerable);
            }
            _ => return Err(FieldError::Unknown),
        })
    }
}

/// Somebody who voices a character, as one of AniList's `Staff`: their
/// names, their portrait and the language they voice it in, under AniList's
/// id where kept, else a number of this server's ([`ids::anilist_voice`]).
struct Voice<'c> {
    voice: &'c CharacterVoice,
    character: &'c Held,
}

impl Object for Voice<'_> {
    fn typename(&self) -> &'static str {
        "Staff"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        let voice = self.voice;
        match name {
            "description" => {
                arguments.only(&["asHtml"])?;
                arguments.boolean("asHtml")?;
                return Ok(Resolved::Null);
            }
            "staffMedia" | "characterMedia" => {
                arguments.only(&["sort", "type", "onList", "page", "perPage"])?;
                return Err(FieldError::Unanswerable);
            }
            "characters" => {
                arguments.only(&["sort", "page", "perPage"])?;
                return Err(FieldError::Unanswerable);
            }
            _ => arguments.only(&[])?,
        }
        Ok(match name {
            "id" => Resolved::Value(json!(ids::anilist_voice(voice, self.character))),
            "name" => Resolved::object(Named {
                typename: "StaffName",
                name: &voice.name,
                native: voice.native_name.as_deref(),
            }),
            "languageV2" => Resolved::some(dub_name(&voice.language)),
            "language" => Resolved::some(
                STAFF_LANGUAGES
                    .iter()
                    .find(|(_, code)| *code == voice.language)
                    .map(|(name, _)| *name),
            ),
            "image" => Resolved::object(Picture {
                typename: "StaffImage",
                url: voice.image.as_deref(),
            }),
            "primaryOccupations" | "yearsActive" => Resolved::List(Vec::new()),
            "dateOfBirth" | "dateOfDeath" => Resolved::object(Fuzzy::of(None)),
            "siteUrl" => Resolved::some(
                voice
                    .anilist_id
                    .map(|id| format!("https://anilist.co/staff/{id}")),
            ),
            "gender" | "age" | "homeTown" | "bloodType" | "updatedAt" | "favourites" => {
                Resolved::Null
            }
            "isFavourite" | "isFavouriteBlocked" | "staff" | "submitter" | "submissionStatus"
            | "submissionNotes" | "modNotes" => return Err(FieldError::Unanswerable),
            _ => return Err(FieldError::Unknown),
        })
    }
}

/// A voice in a role, AniList's `StaffRoleType`.
struct VoiceRole<'c>(Voice<'c>);

impl Object for VoiceRole<'_> {
    fn typename(&self) -> &'static str {
        "StaffRoleType"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "voiceActor" => Ok(Resolved::object(Voice {
                voice: self.0.voice,
                character: self.0.character,
            })),
            "roleNotes" | "dubGroup" => Ok(Resolved::Null),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A character's or a person's names, AniList's `CharacterName` and
/// `StaffName`: the full one as kept, the one in its own script; how AniList
/// splits it into first and last is not kept.
struct Named<'n> {
    typename: &'static str,
    name: &'n str,
    native: Option<&'n str>,
}

impl Object for Named<'_> {
    fn typename(&self) -> &'static str {
        self.typename
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "full" | "userPreferred" => Ok(Resolved::Value(json!(self.name))),
            "native" => Ok(Resolved::some(self.native)),
            "first" | "middle" | "last" => Ok(Resolved::Null),
            "alternative" => Ok(Resolved::List(Vec::new())),
            "alternativeSpoiler" if self.typename == "CharacterName" => {
                Ok(Resolved::List(Vec::new()))
            }
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A character's picture or a person's portrait, AniList's
/// `CharacterImage` and `StaffImage`: the one picture kept, at both sizes.
struct Picture<'u> {
    typename: &'static str,
    url: Option<&'u str>,
}

impl Object for Picture<'_> {
    fn typename(&self) -> &'static str {
        self.typename
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "large" | "medium" => Ok(Resolved::some(self.url)),
            _ => Err(FieldError::Unknown),
        }
    }
}

// ─── studios ─────────────────────────────────────────────────────────────────

/// The entry's studios, AniList's `StudioConnection`: all of them on one
/// page, in the order AniList's document lists them, as AniList gives them.
struct Studios {
    studios: Vec<StudioRef>,
}

impl Object for Studios {
    fn typename(&self) -> &'static str {
        "StudioConnection"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "edges" => Ok(Resolved::objects(self.studios.iter().map(StudioEdge))),
            "nodes" => Ok(Resolved::objects(self.studios.iter().map(Studio))),
            // Not counted, as AniList does not count them.
            "pageInfo" => Ok(Resolved::object(PageInfo::default())),
            _ => Err(FieldError::Unknown),
        }
    }
}

struct StudioEdge<'s>(&'s StudioRef);

impl Object for StudioEdge<'_> {
    fn typename(&self) -> &'static str {
        "StudioEdge"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "node" => Ok(Resolved::object(Studio(self.0))),
            "isMain" => Ok(Resolved::Value(json!(self.0.main))),
            "id" | "favouriteOrder" => Ok(Resolved::Null),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A studio, AniList's `Studio`: by AniList's id and its own address where
/// AniList's kept document gives them; a studio known here by its name alone
/// has neither, and is the one that animated the entry.
struct Studio<'s>(&'s StudioRef);

impl Object for Studio<'_> {
    fn typename(&self) -> &'static str {
        "Studio"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        let studio = self.0;
        match name {
            "media" => {
                arguments.only(&["sort", "isMain", "onList", "page", "perPage"])?;
                return Err(FieldError::Unanswerable);
            }
            _ => arguments.only(&[])?,
        }
        match name {
            "id" => studio
                .id
                .map(|id| Resolved::Value(json!(id)))
                .ok_or(FieldError::Unanswerable),
            "siteUrl" => studio
                .id
                .map(|id| Resolved::Value(json!(format!("https://anilist.co/studio/{id}"))))
                .ok_or(FieldError::Unanswerable),
            "name" => Ok(Resolved::Value(json!(studio.name))),
            "isAnimationStudio" => match (studio.animation, studio.main) {
                (Some(animates), _) => Ok(Resolved::Value(json!(animates))),
                (None, true) => Ok(Resolved::Value(json!(true))),
                (None, false) => Err(FieldError::Unanswerable),
            },
            "favourites" => Ok(Resolved::Null),
            "isFavourite" => Err(FieldError::Unanswerable),
            _ => Err(FieldError::Unknown),
        }
    }
}

// ─── the schedule ────────────────────────────────────────────────────────────

/// One episode of the entry and when it airs, AniList's `AiringSchedule`,
/// under a number of this server's ([`ids::anilist_airing`]): AniList's own
/// is never kept.
struct Scheduled<'w> {
    written: &'w Written<'w>,
    airing: Airing,
}

impl Object for Scheduled<'_> {
    fn typename(&self) -> &'static str {
        "AiringSchedule"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        let now = self.written.entry.now;
        match name {
            "airingAt" => Ok(Resolved::Value(json!(self.airing.airing_at))),
            "timeUntilAiring" => Ok(Resolved::Value(json!(self.airing.airing_at - now))),
            "episode" => Ok(Resolved::Value(json!(self.airing.episode))),
            "mediaId" => Ok(Resolved::Value(json!(self.written.entry.id))),
            "media" => Ok(Resolved::object(Media {
                written: self.written,
            })),
            "id" => Ok(Resolved::Value(json!(self.airing.id))),
            _ => Err(FieldError::Unknown),
        }
    }
}

/// A page of the entry's schedule, AniList's `AiringScheduleConnection`.
struct Schedule<'w> {
    written: &'w Written<'w>,
    airings: Vec<Airing>,
    page: i64,
    per_page: i64,
}

impl Object for Schedule<'_> {
    fn typename(&self) -> &'static str {
        "AiringScheduleConnection"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        let on_page = || page_of(&self.airings, self.page, self.per_page);
        let scheduled = |airing| Scheduled {
            written: self.written,
            airing,
        };
        match name {
            "nodes" => Ok(Resolved::objects(on_page().into_iter().map(scheduled))),
            "edges" => Ok(Resolved::objects(
                on_page()
                    .into_iter()
                    .map(|airing| ScheduleEdge(scheduled(airing))),
            )),
            "pageInfo" => Ok(Resolved::object(PageInfo::of(
                self.airings.len(),
                self.page,
                self.per_page,
            ))),
            _ => Err(FieldError::Unknown),
        }
    }
}

struct ScheduleEdge<'w>(Scheduled<'w>);

impl Object for ScheduleEdge<'_> {
    fn typename(&self) -> &'static str {
        "AiringScheduleEdge"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        arguments.only(&[])?;
        match name {
            "node" => Ok(Resolved::object(Scheduled {
                written: self.0.written,
                airing: self.0.airing,
            })),
            "id" => Ok(Resolved::Null),
            _ => Err(FieldError::Unknown),
        }
    }
}

// ─── pages ───────────────────────────────────────────────────────────────────

/// How many entries a page lists when the client does not say, and at the
/// most: AniList's own figures for its `Page`, measured on 3 November 2026.
pub const PAGE_SIZE: i64 = 50;

/// The page asked of AniList's `Page`, and how many it holds.
pub fn page_of_page(arguments: &Arguments) -> Result<(i64, i64), FieldError> {
    arguments.only(&["page", "perPage"])?;
    let page = arguments.int("page")?.unwrap_or(1).max(1);
    let per_page = arguments
        .int("perPage")?
        .unwrap_or(PAGE_SIZE)
        .clamp(1, PAGE_SIZE);
    Ok((page, per_page))
}

/// What a page lists: entries, or the episodes on their schedules.
pub enum Listed<'w> {
    Media(Vec<&'w Written<'w>>),
    Schedules(Vec<(&'w Written<'w>, Airing)>),
}

/// The fields of AniList's `Page`: the one listing entries, the one listing
/// a schedule, and those about anything else — people, lists, users — which
/// are AniList's to answer.
const PAGE_FIELDS: &[&str] = &[
    "pageInfo",
    "media",
    "airingSchedules",
    "users",
    "characters",
    "staff",
    "studios",
    "mediaList",
    "mediaTrends",
    "notifications",
    "followers",
    "following",
    "mutualFollowers",
    "activities",
    "activityReplies",
    "threads",
    "threadComments",
    "reviews",
    "recommendations",
    "likes",
];

/// A page, AniList's `Page`: its description, and the part of what it lists
/// on it. `total` counts all of it, every page.
pub fn page<'w>(total: usize, page: i64, per_page: i64, listed: Listed<'w>) -> impl Object + 'w {
    let listed = match listed {
        Listed::Media(entries) => Listed::Media(page_of(&entries, page, per_page)),
        Listed::Schedules(airings) => Listed::Schedules(page_of(&airings, page, per_page)),
    };
    Page {
        info: PageInfo::of(total, page, per_page),
        listed,
    }
}

struct Page<'w> {
    info: PageInfo,
    listed: Listed<'w>,
}

impl Object for Page<'_> {
    fn typename(&self) -> &'static str {
        "Page"
    }

    fn field(&self, name: &str, arguments: &Arguments) -> Result<Resolved<'_>, FieldError> {
        if !PAGE_FIELDS.contains(&name) {
            return Err(FieldError::Unknown);
        }
        // What the page lists was read off these very arguments.
        match (name, &self.listed) {
            ("pageInfo", _) => {
                arguments.only(&[])?;
                Ok(Resolved::object(self.info))
            }
            ("media", Listed::Media(entries)) => Ok(Resolved::objects(
                entries.iter().map(|written| Media { written }),
            )),
            ("airingSchedules", Listed::Schedules(airings)) => Ok(Resolved::objects(
                airings.iter().map(|(written, airing)| Scheduled {
                    written,
                    airing: *airing,
                }),
            )),
            _ => Err(FieldError::Unanswerable),
        }
    }
}

/// A comparison a filter makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Compared {
    Equal,
    Greater,
    Lesser,
}

impl Compared {
    /// Whether `held` stands so to `asked`: never when nothing is held, as
    /// a database compares a null.
    fn holds(self, held: Option<i64>, asked: i64) -> bool {
        held.is_some_and(|held| match self {
            Self::Equal => held == asked,
            Self::Greater => held > asked,
            Self::Lesser => held < asked,
        })
    }
}

/// An order AniList's `MediaSort` names, as answered here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    Id,
    TitleRomaji,
    TitleEnglish,
    TitleNative,
    Kind,
    Format,
    StartDate,
    EndDate,
    Score,
    Popularity,
    Episodes,
    Duration,
    Status,
    /// What a manga has: nothing of an anime's, all of them alike.
    MangaCount,
    UpdatedAt,
    /// How well a text searched for matches: no text is searched here.
    SearchMatch,
}

/// AniList's formats and statuses, in the order its schema declares them:
/// how an order by them goes.
const FORMATS: [&str; 10] = [
    "TV", "TV_SHORT", "MOVIE", "SPECIAL", "OVA", "ONA", "MUSIC", "MANGA", "NOVEL", "ONE_SHOT",
];
const STATUSES: [&str; 5] = [
    "FINISHED",
    "RELEASING",
    "NOT_YET_RELEASED",
    "CANCELLED",
    "HIATUS",
];

/// What a page of AniList's `Page.media` lists, as answered here: the
/// entries its ids name — `id`, `id_in`, or, beside `type: ANIME`, `idMal`
/// and `idMal_in` — narrowed by the filters AniList's clients send, in the
/// order AniList would put them in. The answer then rests on entries held
/// here alone. Anything else — a text searched for, a page of AniList's whole
/// catalogue, a filter or an order on what only AniList keeps (tags,
/// sources, its members' scores, trends and favourites) — is AniList's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    /// AniList's ids the page is drawn from; else MyAnimeList's.
    pub anilist: Vec<i64>,
    pub mal: Vec<i64>,
    /// MyAnimeList's ids, asked beside AniList's: a filter then.
    mal_filter: Vec<i64>,
    not_anilist: Vec<i64>,
    not_mal: Vec<i64>,
    kind: Option<String>,
    format_is: Vec<String>,
    format_not: Vec<String>,
    status_is: Vec<String>,
    status_not: Vec<String>,
    season: Option<String>,
    season_year: Option<i64>,
    /// Each a genre the entry must have; one of them; none of them.
    genre_all: Vec<String>,
    genre_any: Vec<String>,
    genre_none: Vec<String>,
    adult: Option<bool>,
    country: Option<String>,
    /// A day's comparison, `YYYYMMDD` as AniList writes a fuzzy date — the
    /// first day's, else the last's.
    dates: Vec<(bool, Compared, i64)>,
    /// A count's comparison: of episodes, else of minutes.
    counts: Vec<(bool, Compared, i64)>,
    order: Vec<(Order, bool)>,
}

impl Listing {
    /// Read off `Page.media`'s arguments; none where they ask in a way not
    /// answered here — see [`Listing`].
    pub fn of(arguments: &Arguments) -> Option<Self> {
        let mut listing = Self::default();
        let ints = |name: &str| arguments.ints(name).ok().flatten().unwrap_or_default();
        let int = |name: &str| arguments.int(name).ok().flatten();
        let names = |name: &str| -> Option<Vec<String>> {
            Some(
                arguments
                    .names(name)
                    .ok()?
                    .unwrap_or_default()
                    .into_iter()
                    .map(String::from)
                    .collect(),
            )
        };
        let one = |name: &str| -> Option<Option<String>> {
            Some(arguments.name(name).ok()?.map(String::from))
        };
        for given in arguments.names_given() {
            let taken = matches!(
                given,
                "id" | "id_in"
                    | "idMal"
                    | "idMal_in"
                    | "id_not"
                    | "id_not_in"
                    | "idMal_not"
                    | "idMal_not_in"
                    | "type"
                    | "format"
                    | "format_in"
                    | "format_not"
                    | "format_not_in"
                    | "status"
                    | "status_in"
                    | "status_not"
                    | "status_not_in"
                    | "season"
                    | "seasonYear"
                    | "genre"
                    | "genre_in"
                    | "genre_not_in"
                    | "isAdult"
                    | "countryOfOrigin"
                    | "startDate"
                    | "startDate_greater"
                    | "startDate_lesser"
                    | "endDate"
                    | "endDate_greater"
                    | "endDate_lesser"
                    | "episodes"
                    | "episodes_greater"
                    | "episodes_lesser"
                    | "duration"
                    | "duration_greater"
                    | "duration_lesser"
                    | "sort"
            );
            if !taken {
                return None;
            }
        }

        let mut anilist = ints("id_in");
        anilist.extend(int("id"));
        let mut mal = ints("idMal_in");
        mal.extend(int("idMal"));
        listing.kind = one("type")?;
        // MyAnimeList numbers its anime and its manga apart.
        if !mal.is_empty() && listing.kind.as_deref() != Some("ANIME") {
            return None;
        }
        if anilist.is_empty() {
            listing.mal = mal;
        } else {
            listing.anilist = anilist;
            listing.mal_filter = mal;
        }
        if listing.anilist.is_empty() && listing.mal.is_empty() {
            return None;
        }
        listing.not_anilist = ints("id_not_in");
        listing.not_anilist.extend(int("id_not"));
        listing.not_mal = ints("idMal_not_in");
        listing.not_mal.extend(int("idMal_not"));

        listing.format_is = names("format_in")?;
        listing.format_is.extend(one("format")?);
        listing.format_not = names("format_not_in")?;
        listing.format_not.extend(one("format_not")?);
        listing.status_is = names("status_in")?;
        listing.status_is.extend(one("status")?);
        listing.status_not = names("status_not_in")?;
        listing.status_not.extend(one("status_not")?);
        listing.season = one("season")?;
        listing.season_year = int("seasonYear");
        listing.genre_all = one("genre")?.into_iter().collect();
        listing.genre_any = names("genre_in")?;
        listing.genre_none = names("genre_not_in")?;
        listing.adult = arguments.boolean("isAdult").ok()?;
        listing.country = one("countryOfOrigin")?;
        for (name, end, compared) in [
            ("startDate", false, Compared::Equal),
            ("startDate_greater", false, Compared::Greater),
            ("startDate_lesser", false, Compared::Lesser),
            ("endDate", true, Compared::Equal),
            ("endDate_greater", true, Compared::Greater),
            ("endDate_lesser", true, Compared::Lesser),
        ] {
            if let Some(day) = int(name) {
                listing.dates.push((end, compared, day));
            }
        }
        for (name, minutes, compared) in [
            ("episodes", false, Compared::Equal),
            ("episodes_greater", false, Compared::Greater),
            ("episodes_lesser", false, Compared::Lesser),
            ("duration", true, Compared::Equal),
            ("duration_greater", true, Compared::Greater),
            ("duration_lesser", true, Compared::Lesser),
        ] {
            if let Some(count) = int(name) {
                listing.counts.push((minutes, compared, count));
            }
        }
        // An argument given a value of the wrong type is AniList's to refuse.
        for name in ["id", "idMal", "id_not", "idMal_not", "seasonYear"] {
            arguments.int(name).ok()?;
        }
        for name in ["id_in", "idMal_in", "id_not_in", "idMal_not_in"] {
            arguments.ints(name).ok()?;
        }

        for sort in names("sort")? {
            let (key, descending) = match sort.strip_suffix("_DESC") {
                Some(key) => (key, true),
                None => (sort.as_str(), false),
            };
            let order = match key {
                "ID" => Order::Id,
                "TITLE_ROMAJI" => Order::TitleRomaji,
                "TITLE_ENGLISH" => Order::TitleEnglish,
                "TITLE_NATIVE" => Order::TitleNative,
                "TYPE" => Order::Kind,
                "FORMAT" => Order::Format,
                "START_DATE" => Order::StartDate,
                "END_DATE" => Order::EndDate,
                "SCORE" => Order::Score,
                "POPULARITY" => Order::Popularity,
                "EPISODES" => Order::Episodes,
                "DURATION" => Order::Duration,
                "STATUS" => Order::Status,
                "CHAPTERS" | "VOLUMES" => Order::MangaCount,
                "UPDATED_AT" => Order::UpdatedAt,
                "SEARCH_MATCH" => Order::SearchMatch,
                // Trends and favourites are AniList's members' alone.
                _ => return None,
            };
            listing.order.push((order, descending));
        }
        Some(listing)
    }

    /// Whether the page lists `written`: none where a filter asks of it what
    /// is not known here, which AniList knows — the page is AniList's then.
    pub fn keeps(&self, written: &Written<'_>) -> Option<bool> {
        let entry = written.entry;
        let id_mal = written.id_mal();
        if self.not_anilist.contains(&entry.id)
            || id_mal.is_some_and(|mal| self.not_mal.contains(&mal))
        {
            return Some(false);
        }
        if !self.mal_filter.is_empty() && !id_mal.is_some_and(|mal| self.mal_filter.contains(&mal))
        {
            return Some(false);
        }
        if self.kind.as_deref().is_some_and(|kind| kind != "ANIME") {
            return Some(false);
        }
        if !self.format_is.is_empty() || !self.format_not.is_empty() {
            let format = written.format()?;
            if (!self.format_is.is_empty() && !self.format_is.contains(&format))
                || self.format_not.contains(&format)
            {
                return Some(false);
            }
        }
        if !self.status_is.is_empty() || !self.status_not.is_empty() {
            let status = written.run.status?.to_string();
            if (!self.status_is.is_empty() && !self.status_is.contains(&status))
                || self.status_not.contains(&status)
            {
                return Some(false);
            }
        }
        if self.season.is_some() || self.season_year.is_some() {
            let (season, year) = written.season()?;
            if self
                .season
                .as_deref()
                .is_some_and(|s| s != season.anilist())
                || self.season_year.is_some_and(|y| y != i64::from(year))
            {
                return Some(false);
            }
        }
        if !self.genre_all.is_empty() || !self.genre_any.is_empty() || !self.genre_none.is_empty() {
            let genres = written.genres();
            let has = |wanted: &String| genres.iter().any(|g| g.eq_ignore_ascii_case(wanted));
            if !self.genre_all.iter().all(has)
                || (!self.genre_any.is_empty() && !self.genre_any.iter().any(has))
                || self.genre_none.iter().any(has)
            {
                return Some(false);
            }
        }
        if self.adult.is_some_and(|adult| adult != entry.item.is_adult) {
            return Some(false);
        }
        if let Some(country) = &self.country
            && !written
                .country()
                .is_some_and(|held| held.eq_ignore_ascii_case(country))
        {
            return Some(false);
        }
        for (end, compared, day) in &self.dates {
            let held = if *end {
                &written.run.end
            } else {
                &written.run.start
            };
            if !compared.holds(fuzzy_int(held.as_deref()), *day) {
                return Some(false);
            }
        }
        for (minutes, compared, count) in &self.counts {
            let held = if *minutes {
                entry.item.runtime.filter(|m| *m > 0).map(i64::from)
            } else {
                written.episodes()
            };
            if !compared.holds(held, *count) {
                return Some(false);
            }
        }
        Some(true)
    }

    /// Put `entries` in the order asked, then by AniList's id, as AniList
    /// lists a page of ids it is not asked to sort. Where a value is not
    /// known, it comes first going up and last going down, as a database
    /// puts its nulls.
    ///
    /// By popularity: AniList's count where its kept document has one, then
    /// this server's figure (`MediaItem.popularity`, TMDB's or its own) for
    /// the others — two figures of different kinds, never compared with each
    /// other, so those AniList counted come first going down.
    pub fn order(&self, entries: &mut [&Written<'_>]) {
        entries.sort_by(|a, b| {
            for (order, descending) in &self.order {
                let ordering = compare(*order, a, b);
                let ordering = if *descending {
                    ordering.reverse()
                } else {
                    ordering
                };
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            a.entry.id.cmp(&b.entry.id)
        });
    }
}

/// A fuzzy date as AniList compares one: `YYYYMMDD`, a part not known
/// nought.
fn fuzzy_int(day: Option<&str>) -> Option<i64> {
    let day = Fuzzy::of(day);
    let year = day.year?;
    Some(year * 10_000 + day.month.unwrap_or(0) * 100 + day.day.unwrap_or(0))
}

/// Two entries going up by `order`, a value not known before any.
fn compare(order: Order, a: &Written<'_>, b: &Written<'_>) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    fn by<T: PartialOrd>(a: Option<T>, b: Option<T>) -> Ordering {
        match (a, b) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
        }
    }
    let place = |list: &[&str], value: Option<&str>| {
        value.and_then(|value| list.iter().position(|known| *known == value))
    };
    let title = |written: &Written<'_>, which: fn(&Titles) -> &Option<String>| {
        which(&written.titles).as_deref().map(folded)
    };
    let popularity = |written: &Written<'_>| -> Option<(u8, f64)> {
        written
            .their_popularity()
            .map(|count| (2, count as f64))
            .or_else(|| written.item().popularity.map(|figure| (1, figure)))
    };
    match order {
        Order::Id => a.entry.id.cmp(&b.entry.id),
        Order::TitleRomaji => by(title(a, |t| &t.romaji), title(b, |t| &t.romaji)),
        Order::TitleEnglish => by(title(a, |t| &t.english), title(b, |t| &t.english)),
        Order::TitleNative => by(title(a, |t| &t.native), title(b, |t| &t.native)),
        Order::Kind | Order::MangaCount | Order::SearchMatch => Ordering::Equal,
        Order::Format => by(
            place(&FORMATS, a.format().as_deref()),
            place(&FORMATS, b.format().as_deref()),
        ),
        Order::StartDate => by(
            fuzzy_int(a.run.start.as_deref()),
            fuzzy_int(b.run.start.as_deref()),
        ),
        Order::EndDate => by(
            fuzzy_int(a.run.end.as_deref()),
            fuzzy_int(b.run.end.as_deref()),
        ),
        Order::Score => by(a.score(), b.score()),
        Order::Popularity => by(popularity(a), popularity(b)),
        Order::Episodes => by(a.episodes(), b.episodes()),
        Order::Duration => by(a.item().runtime, b.item().runtime),
        Order::Status => by(
            place(&STATUSES, a.run.status),
            place(&STATUSES, b.run.status),
        ),
        Order::UpdatedAt => by(a.updated_at(), b.updated_at()),
    }
}

/// An order AniList's `AiringSort` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiringOrder {
    Id,
    MediaId,
    Time,
    Episode,
}

/// What a page of AniList's `Page.airingSchedules` lists, as answered here:
/// the episodes on the schedules of the entries its `mediaId` or
/// `mediaId_in` names, narrowed and sorted as AniList would. A schedule of
/// every entry airing between two days is AniList's whole catalogue's: its
/// to answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scheduling {
    /// The entries whose schedules are listed, by AniList's ids.
    pub media: Vec<i64>,
    not_media: Vec<i64>,
    not_yet_aired: Option<bool>,
    ids_in: Vec<i64>,
    ids_not: Vec<i64>,
    episodes_in: Vec<i64>,
    episodes_not: Vec<i64>,
    episodes: Vec<(Compared, i64)>,
    airing_at: Vec<(Compared, i64)>,
    order: Vec<(AiringOrder, bool)>,
}

impl Scheduling {
    /// Read off `Page.airingSchedules`' arguments; none where they ask in a
    /// way not answered here.
    pub fn of(arguments: &Arguments) -> Option<Self> {
        arguments
            .only(&[
                "id",
                "id_in",
                "id_not",
                "id_not_in",
                "mediaId",
                "mediaId_in",
                "mediaId_not",
                "mediaId_not_in",
                "episode",
                "episode_in",
                "episode_not",
                "episode_not_in",
                "episode_greater",
                "episode_lesser",
                "airingAt",
                "airingAt_greater",
                "airingAt_lesser",
                "notYetAired",
                "sort",
            ])
            .ok()?;
        let ints = |name: &str| arguments.ints(name).ok().map(Option::unwrap_or_default);
        let int = |name: &str| arguments.int(name).ok();
        let mut scheduling = Self {
            media: ints("mediaId_in")?,
            not_media: ints("mediaId_not_in")?,
            not_yet_aired: arguments.boolean("notYetAired").ok()?,
            ids_in: ints("id_in")?,
            ids_not: ints("id_not_in")?,
            episodes_in: ints("episode_in")?,
            episodes_not: ints("episode_not_in")?,
            ..Self::default()
        };
        scheduling.media.extend(int("mediaId")?);
        scheduling.not_media.extend(int("mediaId_not")?);
        scheduling.ids_in.extend(int("id")?);
        scheduling.ids_not.extend(int("id_not")?);
        scheduling.episodes_not.extend(int("episode_not")?);
        if scheduling.media.is_empty() {
            return None;
        }
        for (name, compared) in [
            ("episode", Compared::Equal),
            ("episode_greater", Compared::Greater),
            ("episode_lesser", Compared::Lesser),
        ] {
            if let Some(episode) = int(name)? {
                scheduling.episodes.push((compared, episode));
            }
        }
        for (name, compared) in [
            ("airingAt", Compared::Equal),
            ("airingAt_greater", Compared::Greater),
            ("airingAt_lesser", Compared::Lesser),
        ] {
            if let Some(at) = int(name)? {
                scheduling.airing_at.push((compared, at));
            }
        }
        for sort in arguments.names("sort").ok()?.unwrap_or_default() {
            let (key, descending) = match sort.strip_suffix("_DESC") {
                Some(key) => (key, true),
                None => (sort, false),
            };
            let order = match key {
                "ID" => AiringOrder::Id,
                "MEDIA_ID" => AiringOrder::MediaId,
                "TIME" => AiringOrder::Time,
                "EPISODE" => AiringOrder::Episode,
                _ => return None,
            };
            scheduling.order.push((order, descending));
        }
        Some(scheduling)
    }

    /// The episodes listed of `written`'s schedule, as of `now`.
    pub fn of_entry<'w>(&self, written: &'w Written<'w>) -> Vec<(&'w Written<'w>, Airing)> {
        let entry = written.entry;
        if self.not_media.contains(&entry.id) {
            return Vec::new();
        }
        entry
            .schedule
            .iter()
            .filter(|airing| {
                let id = airing.id;
                let episode = i64::from(airing.episode);
                self.not_yet_aired
                    .is_none_or(|not_yet| (airing.airing_at > entry.now) == not_yet)
                    && (self.ids_in.is_empty() || self.ids_in.contains(&id))
                    && !self.ids_not.contains(&id)
                    && (self.episodes_in.is_empty() || self.episodes_in.contains(&episode))
                    && !self.episodes_not.contains(&episode)
                    && self
                        .episodes
                        .iter()
                        .all(|(compared, asked)| compared.holds(Some(episode), *asked))
                    && self
                        .airing_at
                        .iter()
                        .all(|(compared, asked)| compared.holds(Some(airing.airing_at), *asked))
            })
            .map(|airing| (written, *airing))
            .collect()
    }

    /// Put `airings` in the order asked, then in the order they air — as
    /// AniList's own ids for them go, which follow its schedule.
    pub fn order(&self, airings: &mut [(&Written<'_>, Airing)]) {
        airings.sort_by(|(a, x), (b, y)| {
            for (order, descending) in &self.order {
                let ordering = match order {
                    AiringOrder::Id => x.id.cmp(&y.id),
                    AiringOrder::MediaId => a.entry.id.cmp(&b.entry.id),
                    AiringOrder::Time => x.airing_at.cmp(&y.airing_at),
                    AiringOrder::Episode => x.episode.cmp(&y.episode),
                };
                let ordering = if *descending {
                    ordering.reverse()
                } else {
                    ordering
                };
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            (x.airing_at, a.entry.id, x.episode).cmp(&(y.airing_at, b.entry.id, y.episode))
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{Map, json};

    use super::*;
    use crate::{
        db::repo::{anime, child::blank_episode},
        domain::person::{Character as Person, CharacterCredit, CharacterRole, CharacterVoice},
        domain::{AlternativeTitle, Episode, ExternalIds, Rating, Relation, Season, Translation},
        graphql::{Failure, Prepared, Projection},
        service::cour::{self, Span},
        wire::contract::{GapKind, assert_same_shape, gaps},
    };

    /// 1 November 2026, noon UTC.
    const NOW: i64 = 1_793_534_400;

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(NOW, 0).unwrap()
    }

    /// AniList's answers on 2 November 2026 (`fixtures/anilist`), trimmed:
    /// to `all-fields.graphql` for *Frieren* (finished), *ONE PIECE*
    /// (airing) and *Your Name.* (a film); to `clients.graphql`, each
    /// client's document under its name, for the last two.
    fn fixture(name: &str) -> Value {
        let text = match name {
            "media-finished" => include_str!("fixtures/anilist/media-finished.json"),
            "media-airing" => include_str!("fixtures/anilist/media-airing.json"),
            "media-film" => include_str!("fixtures/anilist/media-film.json"),
            "clients-airing" => include_str!("fixtures/anilist/clients-airing.json"),
            "clients-film" => include_str!("fixtures/anilist/clients-film.json"),
            other => panic!("no fixture {other}"),
        };
        serde_json::from_str(text).unwrap()
    }

    const ALL_FIELDS: &str = include_str!("fixtures/anilist/all-fields.graphql");
    const CLIENTS: &str = include_str!("fixtures/anilist/clients.graphql");

    /// Where AniList writes a value and an entry written here, holding
    /// nothing for it, writes null: what only AniList's members and editors
    /// make — its hashtag, its licence, its mean score, popularity, trend
    /// and favourites, whether it is locked, its cover's colour, a tag's
    /// category and description — the ids of AniList's own edges, and what
    /// the related entries' kept documents do not say.
    const ANILIST_ONLY: &[&str] = &[
        "hashtag",
        "isLicensed",
        "meanScore",
        "popularity",
        "trending",
        "favourites",
        "isLocked",
        "coverImage.color",
        "tags[].description",
        "tags[].category",
        "tags[].isAdult",
        "relations.edges[].id",
        "relations.edges[].node.status",
        "relations.edges[].node.startDate.month",
        "relations.edges[].node.startDate.day",
        "relations.edges[].node.title.english",
        "relations.edges[].node.title.native",
        "studios.edges[].id",
        "tags[].userId",
    ];

    /// What only MyAnimeList says of an entry, null for one it did not
    /// describe: what it was adapted from.
    const NOT_DESCRIBED: &[&str] = &["source", "sourceV3"];

    /// All the fields, less those holding an identifier of AniList's own,
    /// which no entry written here can give: a tag's, a studio's, a
    /// scheduled episode's.
    fn answerable(document: &str) -> String {
        let mut answerable = document.to_string();
        for (theirs, ours) in [
            ("tags { id name", "tags { name"),
            (
                "node { id name isAnimationStudio siteUrl }",
                "node { name isAnimationStudio }",
            ),
            ("nodes { id name } pageInfo", "nodes { name } pageInfo"),
            (
                "{ nodes { id name isAnimationStudio } }",
                "{ nodes { name isAnimationStudio } }",
            ),
            (
                "nextAiringEpisode { id airingAt",
                "nextAiringEpisode { airingAt",
            ),
            ("nodes { id airingAt", "nodes { airingAt"),
        ] {
            assert!(answerable.contains(theirs), "{theirs}");
            answerable = answerable.replace(theirs, ours);
        }
        answerable
    }

    /// AniList's answer, less the same identifiers.
    fn answerable_theirs(mut theirs: Value) -> Value {
        for path in [
            "tags[].id",
            "studios.edges[].node.id",
            "studios.edges[].node.siteUrl",
            "studios.nodes[].id",
            "mainStudios.nodes[].id",
            "nextAiringEpisode.id",
            "airingSchedule.nodes[].id",
        ] {
            let parts: Vec<&str> = path.split('.').collect();
            without(&mut theirs, &parts);
        }
        theirs
    }

    fn without(value: &mut Value, path: &[&str]) {
        let Some((first, rest)) = path.split_first() else {
            return;
        };
        let (key, each) = match first.strip_suffix("[]") {
            Some(key) => (key, true),
            None => (*first, false),
        };
        let Some(object) = value.as_object_mut() else {
            return;
        };
        if rest.is_empty() && !each {
            object.remove(key);
            return;
        }
        match object.get_mut(key) {
            Some(Value::Array(items)) if each => {
                for item in items {
                    without(item, rest);
                }
            }
            Some(inner) => without(inner, rest),
            None => {}
        }
    }

    /// What the AniList source keeps of an entry — the fields its own query
    /// asks for, in `providers::anilist` — out of AniList's answer to all of
    /// them.
    fn kept(all: &Value) -> Value {
        let media = &all["data"]["Media"];
        let titles = |of: &Value| json!({ "romaji": of["title"]["romaji"], "english": of["title"]["english"], "native": of["title"]["native"] });
        let each = |list: &Value, map: &dyn Fn(&Value) -> Value| {
            Value::Array(list.as_array().unwrap().iter().map(map).collect())
        };
        json!({
            "id": media["id"], "idMal": media["idMal"], "format": media["format"],
            "title": titles(media),
            "synonyms": media["synonyms"],
            "description": media["description"],
            "weightedScore": media["weightedScore"], "averageScore": media["averageScore"],
            "stats": { "scoreDistribution": media["stats"]["scoreDistribution"] },
            "genres": media["genres"],
            "tags": each(&media["tags"], &|tag| json!({
                "name": tag["name"], "rank": tag["rank"],
                "isMediaSpoiler": tag["isMediaSpoiler"], "isGeneralSpoiler": tag["isGeneralSpoiler"]
            })),
            "studios": { "nodes": each(&media["mainStudios"]["nodes"], &|studio| json!({ "name": studio["name"] })) },
            "isAdult": media["isAdult"],
            "coverImage": { "extraLarge": media["coverImage"]["extraLarge"] },
            "startDate": { "year": media["startDate"]["year"] },
            "characters": { "edges": [] },
            "relations": { "edges": each(&media["relations"]["edges"], &|edge| json!({
                "relationType": edge["relationTypeV3"],
                "node": {
                    "id": edge["node"]["id"], "idMal": edge["node"]["idMal"], "type": edge["node"]["type"],
                    "format": edge["node"]["format"], "isAdult": edge["node"]["isAdult"],
                    "title": titles(&edge["node"]),
                    "startDate": { "year": edge["node"]["startDate"]["year"] },
                    "coverImage": { "extraLarge": edge["node"]["coverImage"]["large"], "large": edge["node"]["coverImage"]["large"] }
                }
            })) }
        })
    }

    fn picture(kind: CoverType, url: &str, season: Option<i32>) -> Image {
        Image {
            id: url.into(),
            season_number: season,
            cover_type: kind,
            url: url.into(),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        }
    }

    fn named(title: &str, kind: &str) -> AlternativeTitle {
        AlternativeTitle {
            id: title.into(),
            title: title.into(),
            title_type: Some(kind.into()),
            language: None,
            is_manual: false,
        }
    }

    /// `count` episodes of a season, a week apart from `first`, at 15:00
    /// UTC.
    fn weekly(season: i32, first: &str, count: i32, absolute: Option<i32>) -> Vec<Episode> {
        let start = chrono::NaiveDate::parse_from_str(first, "%Y-%m-%d").unwrap();
        (0..count)
            .map(|n| {
                let day = start + chrono::TimeDelta::weeks(i64::from(n));
                let mut episode = blank_episode(season, n + 1);
                episode.is_manual = false;
                episode.air_date = Some(day.format("%Y-%m-%d").to_string());
                episode.air_date_utc = Some(format!("{}T15:00:00Z", day.format("%Y-%m-%d")));
                episode.absolute_episode_number = absolute.map(|from| from + n);
                episode
            })
            .collect()
    }

    /// *Frieren* as a catalogue kept in English holds it: one season of
    /// twenty-eight, ended, AniList's one entry for it.
    fn frieren() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.id = "frieren".into();
        item.title = "Frieren: Beyond Journey's End".into();
        item.original_title = Some("葬送のフリーレン".into());
        item.overview = Some("An elf mage outlives her party.\nShe sets out again.".into());
        item.status = Some("ended".into());
        item.original_country = Some("jpn".into());
        item.runtime = Some(24);
        item.studio = Some("Madhouse".into());
        item.format = Some("TV".into());
        item.trailer_youtube_id = Some("tR8YH0G67Rk".into());
        item.genres = vec![
            "Animation".into(),
            "Action & Adventure".into(),
            "Drama".into(),
        ];
        item.external_ids = ExternalIds {
            anilist: vec![154_587],
            mal: vec![52_991],
            tvdb: Some(424_536),
            ..ExternalIds::default()
        };
        item.ratings = vec![Rating {
            source: "anilist".into(),
            value: Some(8.9),
            votes: Some(206_823),
            rating_type: Some("user".into()),
        }];
        item.images = vec![
            picture(
                CoverType::Poster,
                "https://image.tmdb.org/t/p/original/frieren.jpg",
                None,
            ),
            picture(
                CoverType::Fanart,
                "https://image.tmdb.org/t/p/original/frieren-wide.jpg",
                None,
            ),
        ];
        item.alternative_titles = vec![
            named("Sousou no Frieren", "Romaji"),
            named("Frieren at the Funeral", "Synonym"),
        ];
        item.episodes = weekly(1, "2023-09-29", 28, None);
        item
    }

    /// *ONE PIECE*, airing: two episodes of the last fortnight, two to
    /// come, numbered along the series as TheTVDB counts it.
    fn one_piece() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.id = "one-piece".into();
        item.title = "One Piece".into();
        item.status = Some("continuing".into());
        item.original_country = Some("JP".into());
        item.runtime = Some(24);
        item.studio = Some("Toei Animation".into());
        item.external_ids = ExternalIds {
            anilist: vec![21],
            mal: vec![21],
            tvdb: Some(81_797),
            ..ExternalIds::default()
        };
        item.images = vec![
            picture(CoverType::Poster, "https://example.org/op.jpg", None),
            picture(CoverType::Fanart, "https://example.org/op-wide.jpg", None),
        ];
        item.episodes = weekly(23, "2026-10-18", 4, Some(1178));
        item
    }

    /// *Your Name.*, a film.
    fn your_name() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.id = "your-name".into();
        item.title = "Your Name.".into();
        item.overview = Some("Two teenagers share a profound connection.".into());
        item.status = Some("released".into());
        item.in_cinemas = Some("2016-08-26".into());
        item.year = Some(2016);
        item.runtime = Some(107);
        item.original_country = Some("jpn".into());
        item.studio = Some("CoMix Wave".into());
        item.trailer_youtube_id = Some("k4xGqY5IDBE".into());
        item.genres = vec!["Romance".into(), "Animation".into(), "Drama".into()];
        item.external_ids = ExternalIds {
            anilist: vec![21_519],
            mal: vec![32_281],
            tmdb: Some(372_058),
            ..ExternalIds::default()
        };
        item.images = vec![
            picture(CoverType::Poster, "https://example.org/kimi.jpg", None),
            picture(CoverType::Fanart, "https://example.org/kimi-wide.jpg", None),
        ];
        item
    }

    /// What an entry is written from, for `item` as one entry of its own,
    /// with AniList's kept document `anilist`.
    struct Held {
        item: MediaItem,
        cour: Cour,
        schedule: Vec<Airing>,
        anilist: Option<Value>,
        mal: Option<mal::Described>,
        named: Option<String>,
        /// Whether the client may be shown what is for adults.
        adult: bool,
    }

    impl Held {
        fn whole(item: MediaItem, anilist: Option<Value>) -> Self {
            let cour = cour::of_unlisted(
                &item,
                item.external_ids
                    .anilist
                    .first()
                    .copied()
                    .or(Some(1_900_000_001)),
                item.external_ids.mal.first().copied(),
                now(),
            );
            Self::of(item, cour, anilist)
        }

        fn of(item: MediaItem, cour: Cour, anilist: Option<Value>) -> Self {
            let schedule = cour::schedule(&item, &cour);
            Self {
                item,
                cour,
                schedule,
                anilist,
                mal: None,
                named: None,
                adult: false,
            }
        }

        fn entry(&self) -> Entry<'_> {
            Entry {
                id: self.cour.anilist.unwrap(),
                item: &self.item,
                cour: &self.cour,
                schedule: &self.schedule,
                anilist: self.anilist.as_ref(),
                mal: self.mal.as_ref(),
                named: self.named.as_deref(),
                english: english(&self.item, "eng"),
                now: NOW,
                adult: self.adult,
            }
        }

        /// What this server answers `query`: the root fields named `keys`,
        /// or every one when none is named, each this entry.
        fn answer(&self, query: &str, keys: &[&str]) -> Result<Value, Failure> {
            let variables: Map<String, Value> = json!({ "id": self.cour.anilist })
                .as_object()
                .unwrap()
                .clone();
            let prepared = Prepared::new(query, &variables, None).map_err(Failure::Invalid)?;
            let projection = Projection::new(&prepared);
            let entry = self.entry();
            let written = Written::of(&entry);
            let mut data = Map::new();
            for field in prepared.root_fields().map_err(Failure::Invalid)? {
                if !keys.is_empty() && !keys.contains(&field.key) {
                    continue;
                }
                let value = projection.complete(&field, Resolved::object(media(&written)))?;
                data.insert(field.key.to_string(), value);
            }
            Ok(Value::Object(data))
        }
    }

    #[test]
    fn a_finished_series_is_written_as_anilist_writes_its_entry() {
        let theirs = fixture("media-finished");
        let held = Held::whole(frieren(), Some(kept(&theirs)));
        let ours = held.answer(&answerable(ALL_FIELDS), &[]).unwrap();
        let allowed: Vec<&str> = ANILIST_ONLY.iter().chain(NOT_DESCRIBED).copied().collect();
        assert_same_shape(
            &answerable_theirs(theirs["data"]["Media"].clone()),
            &ours["Media"],
            &allowed,
        );

        let media = &ours["Media"];
        // The catalogue's first: its English title, its text, its pictures.
        assert_eq!(
            media["title"],
            json!({
                "romaji": "Sousou no Frieren",
                "english": "Frieren: Beyond Journey's End",
                "native": "葬送のフリーレン",
                "userPreferred": "Sousou no Frieren"
            })
        );
        assert_eq!(
            media["description"],
            json!("An elf mage outlives her party.\nShe sets out again.")
        );
        assert_eq!(
            media["descriptionHtml"],
            json!("<p>An elf mage outlives her party.<br />\nShe sets out again.</p>")
        );
        assert_eq!(
            media["coverImage"]["large"],
            json!("https://image.tmdb.org/t/p/original/frieren.jpg")
        );
        assert_eq!(
            media["bannerImage"],
            json!("https://image.tmdb.org/t/p/original/frieren-wide.jpg")
        );
        // From its episodes: twenty-eight, aired from the fall of 2023.
        assert_eq!(media["status"], json!("FINISHED"));
        assert_eq!(
            media["startDate"],
            json!({ "year": 2023, "month": 9, "day": 29 })
        );
        assert_eq!(
            media["endDate"],
            json!({ "year": 2024, "month": 4, "day": 5 })
        );
        assert_eq!(
            (&media["season"], &media["seasonYear"], &media["seasonInt"]),
            (&json!("FALL"), &json!(2023), &json!(234))
        );
        assert_eq!(media["episodes"], json!(28));
        assert_eq!(media["nextAiringEpisode"], Value::Null);
        assert_eq!(media["upcoming"]["nodes"], json!([]));
        assert_eq!(media["airingSchedule"]["pageInfo"]["total"], json!(28));
        assert_eq!(media["airingSchedule"]["pageInfo"]["lastPage"], json!(10));
        assert_eq!(media["airingSchedule"]["nodes"][0]["episode"], json!(1));
        // AniList's own, kept with the work: its genres, score, studios,
        // relations — a source called an adaptation in the first version.
        assert_eq!(media["genres"], json!(["Adventure", "Drama", "Fantasy"]));
        assert_eq!(media["averageScore"], json!(89));
        // AniList's own spelling of the work's studio.
        assert_eq!(media["studios"]["nodes"], json!([{ "name": "MADHOUSE" }]));
        let edges = media["relations"]["edges"].as_array().unwrap();
        let source = edges
            .iter()
            .find(|edge| edge["relationTypeV3"] == json!("SOURCE"))
            .expect("its source");
        assert_eq!(source["relationType"], json!("ADAPTATION"));
        assert_eq!(source["relationTypeV2"], json!("SOURCE"));
        assert_eq!(media["countryOfOrigin"], json!("JP"));
        assert_eq!(media["siteUrl"], json!("https://anilist.co/anime/154587"));
        assert_eq!(
            media["trailer"]["thumbnail"],
            json!("https://i.ytimg.com/vi/tR8YH0G67Rk/hqdefault.jpg")
        );
        assert_eq!(media["characters"]["edges"], json!([]));
        assert_eq!(media["__typename"], json!("Media"));
    }

    #[test]
    fn a_series_airing_is_written_with_its_next_episode_and_schedule() {
        let theirs = fixture("media-airing");
        let held = Held::whole(one_piece(), Some(kept(&theirs)));
        let ours = held.answer(&answerable(ALL_FIELDS), &[]).unwrap();
        let allowed: Vec<&str> = ANILIST_ONLY.iter().chain(NOT_DESCRIBED).copied().collect();
        assert_same_shape(
            &answerable_theirs(theirs["data"]["Media"].clone()),
            &ours["Media"],
            &allowed,
        );

        let media = &ours["Media"];
        assert_eq!(media["status"], json!("RELEASING"));
        assert_eq!(media["episodes"], Value::Null, "still growing");
        assert_eq!(
            media["endDate"],
            json!({ "year": null, "month": null, "day": null })
        );
        let next = &media["nextAiringEpisode"];
        assert_eq!(next["episode"], json!(1180), "TheTVDB's count along it");
        assert_eq!(next["mediaId"], json!(21));
        let airing_at = next["airingAt"].as_i64().unwrap();
        assert_eq!(next["timeUntilAiring"], json!(airing_at - NOW));
        assert_eq!(
            media["upcoming"]["nodes"],
            json!([
                { "airingAt": airing_at, "episode": 1180 },
                { "airingAt": airing_at + 7 * 86_400, "episode": 1181 }
            ])
        );
        assert_eq!(media["upcoming"]["pageInfo"]["hasNextPage"], json!(false));
        // The catalogue's English title over AniList's; AniList's romaji.
        assert_eq!(media["title"]["english"], json!("One Piece"));
        assert_eq!(media["title"]["romaji"], json!("ONE PIECE"));
    }

    #[test]
    fn a_film_is_written_as_its_one_entry() {
        let theirs = fixture("media-film");
        let held = Held::whole(your_name(), Some(kept(&theirs)));
        let ours = held.answer(&answerable(ALL_FIELDS), &[]).unwrap();
        let allowed: Vec<&str> = ANILIST_ONLY.iter().chain(NOT_DESCRIBED).copied().collect();
        assert_same_shape(
            &answerable_theirs(theirs["data"]["Media"].clone()),
            &ours["Media"],
            &allowed,
        );

        let media = &ours["Media"];
        assert_eq!(media["format"], json!("MOVIE"));
        assert_eq!(media["status"], json!("FINISHED"));
        assert_eq!(media["episodes"], json!(1));
        assert_eq!(media["duration"], json!(107));
        let day = json!({ "year": 2016, "month": 8, "day": 26 });
        assert_eq!((&media["startDate"], &media["endDate"]), (&day, &day));
        assert_eq!(media["seasonInt"], json!(163));
    }

    #[test]
    fn each_clients_document_is_answered_in_anilists_shape() {
        let allowed: Vec<&str> = ANILIST_ONLY
            .iter()
            .chain(NOT_DESCRIBED)
            .copied()
            .chain(["tags[].category"])
            .collect();
        for (fixture_name, item, all) in [
            ("clients-film", your_name(), "media-film"),
            ("clients-airing", one_piece(), "media-airing"),
        ] {
            let theirs = fixture(fixture_name);
            let held = Held::whole(item, Some(kept(&fixture(all))));
            for client in ["yamtrack", "taiga", "kometa", "kometaRelations"] {
                let ours = held.answer(CLIENTS, &[client]).unwrap();
                assert_same_shape(&theirs["data"][client], &ours[client], &allowed);
            }
            // Jellyfin asks for AniList's own ids of the tags and studios:
            // AniList's to answer.
            assert_eq!(
                held.answer(CLIENTS, &["jellyfin"]),
                Err(Failure::Unanswerable),
                "{fixture_name}"
            );
        }
    }

    #[test]
    fn a_work_entered_by_hand_is_written_with_every_key_and_nothing_made_up() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.id = "by-hand".into();
        item.is_manual = true;
        item.title = "Wombelune".into();
        item.overview = Some("Typed in.".into());
        item.genres = vec!["Drama".into(), "Sci-Fi & Fantasy".into()];
        item.status = Some("continuing".into());
        item.episodes = weekly(1, "2026-10-04", 6, None);
        let held = Held::whole(item, None);
        assert_eq!(held.cour.anilist, Some(1_900_000_001));

        // Jellyfin's document too: no tag and no studio to name.
        for (theirs, document) in [
            (fixture("media-airing"), answerable(ALL_FIELDS)),
            (fixture("clients-airing"), CLIENTS.to_string()),
        ] {
            let ours = held.answer(&document, &[]).unwrap();
            for (key, value) in ours.as_object().unwrap() {
                let found = gaps(&answerable_theirs(theirs["data"][key].clone()), value, &[]);
                let wrong: Vec<_> = found
                    .iter()
                    .filter(|gap| !matches!(gap.what, GapKind::Nulled { .. }))
                    .collect();
                assert!(wrong.is_empty(), "{key}: {wrong:#?}");
            }
        }
        let ours = held.answer(&answerable(ALL_FIELDS), &[]).unwrap();
        let media = &ours["Media"];
        assert_eq!(media["id"], json!(1_900_000_001));
        assert_eq!(media["title"]["userPreferred"], json!("Wombelune"));
        assert_eq!(media["title"]["english"], json!("Wombelune"));
        assert_eq!(media["title"]["romaji"], Value::Null);
        assert_eq!(media["genres"], json!(["Drama", "Sci-Fi", "Fantasy"]));
        assert_eq!(media["status"], json!("RELEASING"));
        assert_eq!(media["idMal"], Value::Null);
        assert_eq!(media["averageScore"], Value::Null);
        assert_eq!(media["tags"], json!([]));
        assert_eq!(media["relations"]["edges"], json!([]));
    }

    #[test]
    fn what_a_person_corrected_here_is_written_as_the_relay_writes_it() {
        let theirs = fixture("media-finished");
        let mut item = frieren();
        item.title = "Frieren, mon titre".into();
        item.studio = Some("Studio corrigé".into());
        item.status = Some("ended".into());
        item.locked_fields = vec![
            "item/title".into(),
            "item/studio".into(),
            "item/genres".into(),
            "item/status".into(),
        ];
        item.genres = vec!["Drame".into()];
        item.hidden.alternative_titles = vec![named("Frieren at the Funeral", "Synonym")];
        let held = Held::whole(item, Some(kept(&theirs)));
        let ours = held
            .answer(
                "query ($id: Int) { Media(id: $id) { title { english userPreferred romaji } \
                 synonyms genres studios { nodes { name } } mainStudios: studios(isMain: true) \
                 { edges { isMain node { name isAnimationStudio } } } \
                 others: studios(isMain: false) { nodes { name } } } }",
                &[],
            )
            .unwrap();
        let media = &ours["Media"];
        assert_eq!(
            media["title"],
            json!({ "english": "Frieren, mon titre", "userPreferred": "Frieren, mon titre", "romaji": "Sousou no Frieren" })
        );
        assert!(
            !media["synonyms"]
                .as_array()
                .unwrap()
                .contains(&json!("Frieren at the Funeral")),
            "{}",
            media["synonyms"]
        );
        assert_eq!(media["genres"], json!(["Drame"]));
        assert_eq!(
            media["studios"]["nodes"],
            json!([{ "name": "Studio corrigé" }])
        );
        assert_eq!(
            media["mainStudios"]["edges"],
            json!([{ "isMain": true, "node": { "name": "Studio corrigé", "isAnimationStudio": true } }])
        );
        assert_eq!(media["others"]["nodes"], json!([]));
    }

    #[test]
    fn the_cour_of_a_split_series_is_written_as_the_list_places_it() {
        // Two entries in one season: the second starts six episodes in.
        let mut item = frieren();
        item.status = Some("continuing".into());
        item.external_ids.anilist = vec![154_587];
        item.episodes = weekly(1, "2026-08-30", 12, None);
        item.seasons = vec![Season {
            images: vec![picture(
                CoverType::Poster,
                "https://example.org/season-1.jpg",
                Some(1),
            )],
            ..crate::db::repo::child::blank_season(1)
        }];
        let entries = [154_587, 182_255].map(|id| anime::Entry {
            anilist_id: Some(id),
            mal_id: Some(id + 1),
            tvdb_id: Some(424_536),
            tvdb_season: Some(1),
            tvdb_offset: (id == 182_255).then_some(6),
            kind: Some("TV".into()),
            ..anime::Entry::default()
        });
        let cours = cour::of_series(&item, &entries, &HashMap::new(), now());
        let second = cours[1].clone();
        assert_eq!(
            second.span,
            Some(Span::Season {
                number: 1,
                first: 7,
                last: None
            })
        );
        let mut held = Held::of(item, second, Some(kept(&fixture("media-finished"))));
        held.named = Some("Frieren Part 2".into());
        held.mal = Some(mal::Described {
            mal_id: 182_256,
            title: Some("Sousou no Frieren 2nd Cour".into()),
            english: Some("Frieren: Part Two".into()),
            synopsis: Some("The journey goes on.".into()),
            episodes: Some(6),
            source: Some("manga".into()),
            studios: vec!["Madhouse".into()],
            ..mal::Described::default()
        });
        let ours = held
            .answer(
                "query ($id: Int) { Media(id: $id) { id idMal title { romaji english userPreferred } \
                 description status startDate { year month day } episodes season seasonYear \
                 source v3: source(version: 3) genres coverImage { large } studios { nodes { name } } \
                 relations { edges { relationType } } nextAiringEpisode { episode } averageScore } }",
                &[],
            )
            .unwrap();
        let media = &ours["Media"];
        assert_eq!(
            (&media["id"], &media["idMal"]),
            (&json!(182_255), &json!(182_256))
        );
        // Its own names and words, MyAnimeList's: not the work's.
        assert_eq!(
            media["title"],
            json!({ "romaji": "Sousou no Frieren 2nd Cour", "english": "Frieren: Part Two", "userPreferred": "Sousou no Frieren 2nd Cour" })
        );
        assert_eq!(media["description"], json!("The journey goes on."));
        // Its own episodes: from the seventh, the next its fourth.
        assert_eq!(media["status"], json!("RELEASING"));
        assert_eq!(
            media["startDate"],
            json!({ "year": 2026, "month": 10, "day": 11 })
        );
        assert_eq!(media["nextAiringEpisode"]["episode"], json!(4));
        assert_eq!(media["episodes"], json!(6), "as MyAnimeList plans it");
        assert_eq!(
            (&media["season"], &media["seasonYear"]),
            (&json!("FALL"), &json!(2026))
        );
        assert_eq!(
            (&media["source"], &media["v3"]),
            (&json!("MANGA"), &json!("MANGA"))
        );
        // The series' genres; its season's poster; no score of its own.
        assert_eq!(media["genres"], json!(["Adventure", "Drama", "Fantasy"]));
        assert_eq!(
            media["coverImage"]["large"],
            json!("https://example.org/season-1.jpg")
        );
        assert_eq!(media["studios"]["nodes"], json!([{ "name": "Madhouse" }]));
        assert_eq!(media["relations"]["edges"], json!([]));
        assert_eq!(media["averageScore"], Value::Null);
    }

    #[test]
    fn what_only_anilist_can_answer_leaves_the_document_to_it() {
        let held = Held::whole(frieren(), Some(kept(&fixture("media-finished"))));
        for unanswerable in [
            "{ Media { tags { id } } }",
            "{ Media { studios { nodes { id } } } }",
            "{ Media { studios { edges { node { siteUrl } } } } }",
            "{ Media { studios { nodes { media { nodes { id } } } } } }",
            "{ Media { isFavourite } }",
            "{ Media { mediaListEntry { id } } }",
            "{ Media { reviews { nodes { id } } } }",
            "{ Media { relations { nodes { isFavourite } } } }",
        ] {
            assert_eq!(
                held.answer(unanswerable, &[]),
                Err(Failure::Unanswerable),
                "{unanswerable}"
            );
        }
        for refused in [
            "{ Media { idd } }",
            "{ Media { title { romaji(stylised: 5) } } }",
            "{ Media { description(asHtml: true, x: 1) } }",
            "{ Media { status(version: \"2\") } }",
            "{ Media { coverImage } }",
            "{ Media { id { x } } }",
        ] {
            assert!(
                matches!(held.answer(refused, &[]), Err(Failure::Invalid(_))),
                "{refused}"
            );
        }
        // Lists AniList's members make: empty here, and fine to ask.
        let ours = held
            .answer(
                "{ Media { recommendations(page: 2) { nodes { id } pageInfo { currentPage total hasNextPage } } \
                 characters(sort: [ROLE], perPage: 5) { edges { role } } rankings { rank } } }",
                &[],
            )
            .unwrap();
        assert_eq!(
            ours["Media"]["recommendations"],
            json!({ "nodes": [], "pageInfo": { "currentPage": 2, "total": 0, "hasNextPage": false } })
        );
        assert_eq!(ours["Media"]["rankings"], json!([]));
    }

    #[test]
    fn the_english_text_is_the_one_held_in_english_and_never_another_languages() {
        let mut item = frieren();
        item.title = "Frieren : Après la fin du voyage".into();
        item.overview = Some("Une elfe survit à ses compagnons.".into());
        // A catalogue kept in French, no English held: none.
        assert_eq!(english(&item, "fra"), English::default());
        // Kept in English: its own.
        assert_eq!(
            english(&item, "eng").title.as_deref(),
            Some("Frieren : Après la fin du voyage")
        );
        // The translation held in English.
        item.translations = vec![Translation {
            language: "eng".into(),
            title: Some("Frieren: Beyond Journey's End".into()),
            overview: Some("An elf outlives her party.".into()),
            tagline: None,
            is_manual: false,
        }];
        assert_eq!(
            english(&item, "fra"),
            English {
                title: Some("Frieren: Beyond Journey's End".into()),
                overview: Some("An elf outlives her party.".into()),
            }
        );
        // What is locked in the catalogue's language, as the relay writes it.
        item.locked_fields.push("item/overview".into());
        assert_eq!(
            english(&item, "fra").overview.as_deref(),
            Some("Une elfe survit à ses compagnons.")
        );
        // What is locked in English, first.
        item.locked_texts.push(crate::domain::fields::LockedText {
            scope: "item".into(),
            field: "title".into(),
            language: "eng".into(),
            value: Some("Frieren".into()),
        });
        assert_eq!(english(&item, "fra").title.as_deref(), Some("Frieren"));
    }

    #[test]
    fn a_description_held_here_is_escaped_as_html_and_anilists_keeps_its_markup() {
        assert_eq!(
            as_html("Tom & Jerry <3\nThe end.", false),
            "<p>Tom &amp; Jerry &lt;3<br />\nThe end.</p>"
        );
        // AniList's own, as it writes it asked for HTML (2 November 2026).
        assert_eq!(
            as_html("…a new adventure…\n<br><br>\n(Source: Crunchyroll)", true),
            "<p>…a new adventure…<br />\n<br><br><br />\n(Source: Crunchyroll)</p>"
        );
    }

    #[test]
    fn a_kind_or_a_source_is_named_as_each_version_of_anilists_lists_names_it() {
        assert_eq!(relation_type("SOURCE", 1), "ADAPTATION");
        assert_eq!(relation_type("SOURCE", 2), "SOURCE");
        assert_eq!(relation_type("SAME_UNIVERSE", 2), "OTHER");
        assert_eq!(relation_type("SAME_UNIVERSE", 3), "SAME_UNIVERSE");
        assert_eq!(relation_type("COMPILATION", 1), "OTHER");
        assert_eq!(relation_type("SEQUEL", 1), "SEQUEL");

        assert_eq!(source("manga", 1), Some("MANGA"));
        assert_eq!(source("web_manga", 3), Some("MANGA"));
        assert_eq!(source("novel", 1), Some("OTHER"));
        assert_eq!(source("novel", 2), Some("NOVEL"));
        assert_eq!(source("web_novel", 1), Some("OTHER"));
        assert_eq!(source("web_novel", 2), Some("NOVEL"));
        assert_eq!(source("web_novel", 3), Some("WEB_NOVEL"));
        assert_eq!(source("card_game", 2), Some("OTHER"));
        assert_eq!(source("card_game", 3), Some("GAME"));
        assert_eq!(source("mixed_media", 3), Some("MULTIMEDIA_PROJECT"));
        assert_eq!(source("unheard_of", 3), None);

        assert_eq!(
            anilist_genres(&[
                "Animation".into(),
                "Action & Adventure".into(),
                "Science Fiction".into(),
                "slice of life".into(),
                "Drame".into(),
                "Action".into(),
            ]),
            ["Action", "Adventure", "Sci-Fi", "Slice of Life"]
        );
    }

    #[test]
    fn the_works_locks_say_what_they_can_of_each_entry() {
        let mut item = frieren();
        item.first_aired = Some("2023-09-29".into());
        item.last_aired = Some("2024-03-22".into());
        item.locked_fields = vec![
            "item/status".into(),
            "item/firstAired".into(),
            "item/lastAired".into(),
        ];
        let whole = cour::of_unlisted(&item, Some(1), None, now());
        assert_eq!(
            locked_airing(&item, &whole),
            Locked {
                status: Some(Status::Finished),
                start: Some("2023-09-29".into()),
                end: Some("2024-03-22".into()),
            }
        );
        // Written over what the episodes say.
        let held = Held::of(item, whole, None);
        let ours = held
            .answer("{ Media { endDate { year month day } } }", &[])
            .unwrap();
        assert_eq!(
            ours["Media"]["endDate"],
            json!({ "year": 2024, "month": 3, "day": 22 })
        );
    }

    // ─── phase 2: characters, AniList's ids kept, pages ──────────────────────

    /// AniList's answers on 3 November 2026: `pages.graphql` (pages of
    /// entries named by their ids, schedules, the genres) and
    /// `characters.graphql` (*Your Name.*'s first two characters).
    fn fixture_now(name: &str) -> Value {
        let text = match name {
            "pages" => include_str!("fixtures/anilist/pages.json"),
            "characters" => include_str!("fixtures/anilist/characters.json"),
            other => panic!("no fixture {other}"),
        };
        serde_json::from_str(text).unwrap()
    }

    const PAGES: &str = include_str!("fixtures/anilist/pages.graphql");
    const CHARACTERS: &str = include_str!("fixtures/anilist/characters.graphql");

    /// AniList's document of an entry as the AniList source keeps it in this
    /// version: [`kept`], with every tag's id, category and description,
    /// every studio with its id, main or not, and its count of members.
    fn kept_now(all: &Value) -> Value {
        let media = &all["data"]["Media"];
        let mut kept = kept(all);
        kept["tags"] = media["tags"].clone();
        kept["studios"] = json!({ "edges": media["studios"]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|edge| json!({ "isMain": edge["isMain"], "node": {
                "id": edge["node"]["id"], "name": edge["node"]["name"],
                "isAnimationStudio": edge["node"]["isAnimationStudio"]
            } }))
            .collect::<Vec<_>>() });
        kept["popularity"] = media["popularity"].clone();
        kept
    }

    /// *Your Name.*'s document as kept in this version, its studios those
    /// Jellyfin's query was answered with: asked beside `studios(isMain:
    /// true)` in one query, AniList answered the other with the main one
    /// alone.
    fn film_kept_now() -> Value {
        let mut kept = kept_now(&fixture("media-film"));
        kept["studios"] =
            json!({ "edges": fixture("clients-film")["data"]["jellyfin"]["studios"]["edges"] });
        kept
    }

    fn voice(language: &str, name: &str, anilist: Option<i64>) -> CharacterVoice {
        CharacterVoice {
            language: language.into(),
            name: name.into(),
            native_name: None,
            image: Some(format!("https://s4.anilist.co/staff/{name}.jpg")),
            anilist_id: anilist,
            mal_id: None,
            sort_order: 0,
        }
    }

    /// *Your Name.*'s characters as the catalogue keeps them: two leads of
    /// AniList's, with their voices in three languages, and one of
    /// TheTVDB's alone.
    fn your_name_cast() -> Vec<CharacterCredit> {
        let character = |id: &str, name: &str, anilist: Option<i64>| Person {
            id: id.into(),
            name: name.into(),
            native_name: None,
            image: Some(format!("https://s4.anilist.co/character/{id}.png")),
            tvdb_id: None,
            anilist_id: anilist,
            mal_id: None,
        };
        vec![
            CharacterCredit {
                character: Person {
                    native_name: Some("宮水三葉".into()),
                    ..character("c-mitsuha", "Mitsuha Miyamizu", Some(121_514))
                },
                role: Some(CharacterRole::Main),
                sort_order: 0,
                credit_id: None,
                voices: vec![
                    voice("eng", "Stephanie Sheh", Some(95_396)),
                    voice("jpn", "Mone Kamishiraishi", Some(121_515)),
                    voice("fra", "Une voix française", None),
                ],
                source: Some("anilist".into()),
            },
            CharacterCredit {
                character: Person {
                    tvdb_id: Some(70_003),
                    ..character("c-teshi", "Katsuhiko Teshigawara", None)
                },
                role: Some(CharacterRole::Supporting),
                sort_order: 2,
                credit_id: None,
                voices: Vec::new(),
                source: Some("tvdb".into()),
            },
            CharacterCredit {
                character: Person {
                    native_name: Some("立花瀧".into()),
                    ..character("c-taki", "Taki Tachibana", Some(121_516))
                },
                role: Some(CharacterRole::Main),
                sort_order: 1,
                credit_id: None,
                voices: vec![voice("jpn", "Ryuunosuke Kamiki", Some(95_947))],
                source: Some("anilist".into()),
            },
        ]
    }

    #[test]
    fn the_characters_are_the_works_with_anilists_ids_or_numbers_made_here() {
        let mut item = your_name();
        item.characters = your_name_cast();
        let held = Held::whole(item, Some(kept(&fixture("media-film"))));
        let ours = held.answer(CHARACTERS, &[]).unwrap();
        let theirs = fixture_now("characters");
        assert_same_shape(
            &theirs["data"]["Media"],
            &ours["Media"],
            &[
                // AniList's edges' ids; how it splits a name; what its
                // editors and members write of a character or a person.
                "characters.edges[].id",
                "characters.edges[].node.name.first",
                "characters.edges[].node.name.last",
                "characters.edges[].node.gender",
                "characters.edges[].node.age",
                "characters.edges[].node.description",
                "characters.edges[].node.dateOfBirth.month",
                "characters.edges[].node.dateOfBirth.day",
                "characters.edges[].node.favourites",
                "characters.edges[].voiceActors[].name.first",
                "characters.edges[].voiceActors[].name.last",
                "characters.edges[].voiceActors[].name.native",
                "characters.edges[].voiceActors[].gender",
            ],
        );

        let characters = &ours["Media"]["characters"];
        // The leads first, in their order; three in all, two a page.
        assert_eq!(
            characters["pageInfo"],
            json!({ "total": 3, "perPage": 2, "currentPage": 1, "lastPage": 2, "hasNextPage": true })
        );
        assert_eq!(
            characters["nodes"],
            json!([{ "id": 121_514 }, { "id": 121_516 }])
        );
        let mitsuha = &characters["edges"][0];
        assert_eq!(mitsuha["role"], json!("MAIN"));
        assert_eq!(
            mitsuha["node"]["name"],
            json!({
                "first": null, "middle": null, "last": null,
                "full": "Mitsuha Miyamizu", "native": "宮水三葉",
                "alternative": [], "alternativeSpoiler": [],
                "userPreferred": "Mitsuha Miyamizu"
            })
        );
        assert_eq!(
            mitsuha["node"]["siteUrl"],
            json!("https://anilist.co/character/121514")
        );
        // Asked in Japanese: the Japanese voice alone; in every language,
        // the Japanese one first, then the dubs.
        assert_eq!(mitsuha["voiceActors"][0]["id"], json!(121_515));
        assert_eq!(mitsuha["voiceActors"][0]["languageV2"], json!("Japanese"));
        assert_eq!(mitsuha["voiceActors"].as_array().unwrap().len(), 1);
        assert_eq!(
            mitsuha["voiceActorRoles"],
            json!([{ "roleNotes": null, "dubGroup": null, "voiceActor": { "id": 121_515 } }])
        );
        let all = mitsuha["all"].as_array().unwrap();
        assert_eq!(all[0], json!({ "id": 121_515, "languageV2": "Japanese" }));
        assert_eq!(all[1], json!({ "id": 95_396, "languageV2": "English" }));
        assert_eq!(all[2]["languageV2"], json!("French"));
        let made = all[2]["id"].as_i64().unwrap();
        assert!(crate::service::ids::is_own(made), "{made}");

        // TheTVDB's alone: a number of this server's, no address of AniList's.
        let supporting = held
            .answer(
                "{ Media { characters(role: SUPPORTING) { edges { role node { id siteUrl \
                 name { full } } } } } }",
                &[],
            )
            .unwrap();
        let edge = &supporting["Media"]["characters"]["edges"][0];
        assert_eq!(edge["role"], json!("SUPPORTING"));
        assert!(crate::service::ids::is_own(
            edge["node"]["id"].as_i64().unwrap()
        ));
        assert_eq!(edge["node"]["siteUrl"], Value::Null);
        // A page further.
        let second = held
            .answer(
                "{ Media { characters(sort: [ROLE, FAVOURITES_DESC], page: 2, perPage: 2) \
                 { edges { node { name { full } } } pageInfo { hasNextPage } } } }",
                &[],
            )
            .unwrap();
        assert_eq!(
            second["Media"]["characters"],
            json!({ "edges": [{ "node": { "name": { "full": "Katsuhiko Teshigawara" } } }],
                    "pageInfo": { "hasNextPage": false } })
        );
        // What the character is in, by AniList's graph: AniList's to say.
        assert_eq!(
            held.answer(
                "{ Media { characters { nodes { media { nodes { id } } } } } }",
                &[]
            ),
            Err(Failure::Unanswerable)
        );
    }

    #[test]
    fn the_characters_of_a_cour_or_of_a_work_not_refreshed_since_are_anilists() {
        let mut item = frieren();
        item.characters = your_name_cast();
        item.episodes = weekly(1, "2026-08-30", 12, None);
        let entries = [154_587, 182_255].map(|id| anime::Entry {
            anilist_id: Some(id),
            tvdb_id: Some(424_536),
            tvdb_season: Some(1),
            tvdb_offset: (id == 182_255).then_some(6),
            kind: Some("TV".into()),
            ..anime::Entry::default()
        });
        let cours = cour::of_series(&item, &entries, &HashMap::new(), now());
        let first = Held::of(
            item.clone(),
            cours[0].clone(),
            Some(kept(&fixture("media-finished"))),
        );
        let second = Held::of(
            item,
            cours[1].clone(),
            Some(kept(&fixture("media-finished"))),
        );
        let asked = "{ Media { characters { edges { role } } } }";
        // The entry AniList's document is of: the work's characters.
        assert_eq!(
            first.answer(asked, &[]).unwrap()["Media"]["characters"]["edges"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(second.answer(asked, &[]), Err(Failure::Unanswerable));
        // A work whose AniList document was kept before its characters were.
        let mut before = kept(&fixture("media-film"));
        before.as_object_mut().unwrap().remove("characters");
        let old = Held::whole(your_name(), Some(before));
        assert_eq!(old.answer(asked, &[]), Err(Failure::Unanswerable));
    }

    #[test]
    fn the_tags_and_studios_anilist_keeps_with_their_ids_answer_jellyfin() {
        let mut item = your_name();
        item.characters = your_name_cast();
        let held = Held::whole(item, Some(film_kept_now()));
        let theirs = fixture("clients-film");
        let ours = held.answer(CLIENTS, &["jellyfin"]).unwrap();
        assert_same_shape(
            &theirs["data"]["jellyfin"],
            &ours["jellyfin"],
            &[
                "meanScore",
                "characters.edges[].node.name.first",
                "characters.edges[].node.name.last",
                "characters.edges[].voiceActors[].name.first",
                "characters.edges[].voiceActors[].name.last",
                "characters.edges[].voiceActors[].name.native",
            ],
        );
        let jellyfin = &ours["jellyfin"];
        assert_eq!(jellyfin["tags"][0]["id"], json!(154));
        assert_eq!(jellyfin["tags"][0]["category"], json!("Theme-Fantasy"));
        // Every studio, the main one first as AniList lists them.
        let studios = jellyfin["studios"]["edges"].as_array().unwrap();
        assert_eq!(
            studios[0],
            json!({ "node": { "id": 291, "name": "CoMix Wave", "isAnimationStudio": true }, "isMain": true })
        );
        assert_eq!(studios[1]["isMain"], json!(false));

        let more = held
            .answer(
                "{ Media { main: studios(isMain: true) { nodes { id siteUrl } } \
                 others: studios(isMain: false) { nodes { name } } popularity } }",
                &[],
            )
            .unwrap();
        assert_eq!(
            more["Media"]["main"]["nodes"],
            json!([{ "id": 291, "siteUrl": "https://anilist.co/studio/291" }])
        );
        assert_eq!(
            more["Media"]["others"]["nodes"].as_array().unwrap().len(),
            studios.len() - 1
        );
        // AniList's count of its members, as its document of the entry has it.
        assert_eq!(more["Media"]["popularity"], json!(726_372));

        // A studio locked here, among AniList's: it, by its id, the one main.
        let mut locked = your_name();
        locked.studio = Some("Kadokawa Pictures Japan".into());
        locked.locked_fields.push("item/studio".into());
        let locked = Held::whole(locked, Some(film_kept_now()));
        let answer = locked
            .answer(
                "{ Media { studios { edges { isMain node { id name } } } } }",
                &[],
            )
            .unwrap();
        let edges = answer["Media"]["studios"]["edges"].as_array().unwrap();
        assert_eq!(
            edges[0],
            json!({ "isMain": true, "node": { "id": 352, "name": "Kadokawa Pictures Japan" } })
        );
        assert!(edges[1..].iter().all(|edge| edge["isMain"] == json!(false)));
    }

    #[test]
    fn this_servers_popularity_is_never_written_as_anilists() {
        let mut item = one_piece();
        item.popularity = Some(55.5);
        // A document kept before this version has no count: none.
        let held = Held::whole(item, Some(kept(&fixture("media-airing"))));
        let ours = held.answer("{ Media { popularity } }", &[]).unwrap();
        assert_eq!(ours["Media"]["popularity"], Value::Null);
    }

    /// What this server lays out for `document` — its `Page`s and its
    /// `GenreCollection` — drawn from `held`, as the relay reads them.
    fn pages_answer(document: &str, held: &[&Held]) -> Result<Value, Failure> {
        let prepared = Prepared::new(document, &Map::new(), None).map_err(Failure::Invalid)?;
        let projection = Projection::new(&prepared);
        let entries: Vec<Entry<'_>> = held.iter().map(|held| held.entry()).collect();
        let written: Vec<Written<'_>> = entries.iter().map(Written::of).collect();
        let mut data = Map::new();
        for field in prepared.root_fields().map_err(Failure::Invalid)? {
            let resolved = match field.name {
                "GenreCollection" => Resolved::Value(json!(GENRES)),
                "Page" => {
                    let (number, per_page) = page_of_page(&field.arguments).unwrap();
                    let mut listed = None;
                    let sub = prepared
                        .collect("Page", &field.selections)
                        .map_err(Failure::Invalid)?;
                    for asked in sub {
                        if asked.name == "media" {
                            let listing =
                                Listing::of(&asked.arguments).ok_or(Failure::Unanswerable)?;
                            let mut chosen: Vec<&Written<'_>> = written
                                .iter()
                                .filter(|w| {
                                    listing.anilist.contains(&w.entry.id)
                                        || w.id_mal().is_some_and(|mal| listing.mal.contains(&mal))
                                })
                                .filter(|w| listing.keeps(w) == Some(true))
                                .collect();
                            listing.order(&mut chosen);
                            listed =
                                Some(page(chosen.len(), number, per_page, Listed::Media(chosen)));
                        } else if asked.name == "airingSchedules" {
                            let scheduling =
                                Scheduling::of(&asked.arguments).ok_or(Failure::Unanswerable)?;
                            let mut airings: Vec<(&Written<'_>, Airing)> = written
                                .iter()
                                .filter(|w| scheduling.media.contains(&w.entry.id))
                                .flat_map(|w| scheduling.of_entry(w))
                                .collect();
                            scheduling.order(&mut airings);
                            listed = Some(page(
                                airings.len(),
                                number,
                                per_page,
                                Listed::Schedules(airings),
                            ));
                        }
                    }
                    Resolved::object(listed.expect("a list"))
                }
                other => panic!("{other}"),
            };
            data.insert(
                field.key.to_string(),
                projection.complete(&field, resolved)?,
            );
        }
        Ok(Value::Object(data))
    }

    #[test]
    fn a_page_of_entries_named_by_their_ids_is_listed_as_anilist_lists_it() {
        let finished = Held::whole(frieren(), Some(kept(&fixture("media-finished"))));
        let airing = Held::whole(one_piece(), Some(kept(&fixture("media-airing"))));
        let film = Held::whole(your_name(), Some(kept(&fixture("media-film"))));
        let held = [&finished, &airing, &film];
        let ours = pages_answer(PAGES, &held).unwrap();
        let theirs = fixture_now("pages");
        assert_same_shape(&theirs["data"], &ours, &[]);

        // As AniList answered, to the value: by AniList's id where no order
        // is asked, by MyAnimeList's beside `type: ANIME`, narrowed by
        // format and status, an empty page, its genres.
        for same in ["byIds", "mal", "filtered", "empty", "GenreCollection"] {
            assert_eq!(ours[same], theirs["data"][same], "{same}");
        }
        assert_eq!(
            ours["secondPage"]["pageInfo"],
            theirs["data"]["secondPage"]["pageInfo"]
        );
        // By the first day, latest first: the catalogue's days.
        assert_eq!(
            ours["sorted"]["media"],
            json!([{ "id": 21 }, { "id": 154_587 }])
        );
        assert_eq!(
            ours["sorted"]["pageInfo"],
            json!({ "total": 3, "perPage": 2, "currentPage": 1, "lastPage": 2, "hasNextPage": true })
        );
        // ONE PIECE's two to come, in the order they air.
        let schedule = ours["schedule"]["airingSchedules"].as_array().unwrap();
        assert_eq!(schedule.len(), 2);
        assert_eq!(schedule[0]["episode"], json!(1180));
        assert_eq!(schedule[0]["mediaId"], json!(21));
        assert_eq!(schedule[0]["media"], json!({ "id": 21 }));
        assert!(crate::service::ids::is_own(
            schedule[0]["id"].as_i64().unwrap()
        ));
        assert_eq!(
            ours["scheduleDefault"]["airingSchedules"]
                .as_array()
                .unwrap()
                .iter()
                .map(|airing| airing["episode"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(ours["scheduleDefault"]["pageInfo"]["total"], json!(28));
    }

    #[test]
    fn a_page_is_filtered_and_sorted_by_what_the_catalogue_knows_of_each_entry() {
        let finished = Held::whole(frieren(), Some(kept(&fixture("media-finished"))));
        let mut airing_item = one_piece();
        airing_item.popularity = Some(80.0);
        let airing = Held::whole(airing_item, Some(kept(&fixture("media-airing"))));
        let mut film_item = your_name();
        film_item.popularity = Some(99.0);
        let film = Held::whole(film_item, Some(kept_now(&fixture("media-film"))));
        let held = [&finished, &airing, &film];
        let ids = |document: &str| -> Vec<i64> {
            let ours = pages_answer(document, &held).unwrap();
            ours["Page"]["media"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["id"].as_i64().unwrap())
                .collect()
        };
        let all = "id_in: [154587, 21, 21519]";
        assert_eq!(
            ids(&format!(
                "{{ Page {{ media({all}, sort: [ID_DESC]) {{ id }} }} }}"
            )),
            [154_587, 21_519, 21]
        );
        assert_eq!(
            ids(&format!(
                "{{ Page {{ media({all}, format_in: [TV], sort: [EPISODES_DESC]) {{ id }} }} }}"
            )),
            // Twenty-eight; ONE PIECE's count still growing, unknown: last.
            [154_587, 21]
        );
        assert_eq!(
            ids(&format!(
                "{{ Page {{ media({all}, season: FALL, seasonYear: 2023) {{ id }} }} }}"
            )),
            [154_587]
        );
        assert_eq!(
            ids(&format!(
                "{{ Page {{ media({all}, genre_in: [\"Romance\"]) {{ id }} }} }}"
            )),
            [21_519]
        );
        assert_eq!(
            ids(&format!(
                "{{ Page {{ media({all}, genre_not_in: [\"Romance\"], status_not: RELEASING) {{ id }} }} }}"
            )),
            [154_587]
        );
        assert_eq!(
            ids(&format!(
                "{{ Page {{ media({all}, startDate_greater: 20200000, startDate_lesser: 20240000) {{ id }} }} }}"
            )),
            [154_587]
        );
        assert_eq!(
            ids(&format!(
                "{{ Page {{ media({all}, id_not_in: [21], isAdult: false, countryOfOrigin: \"JP\") {{ id }} }} }}"
            )),
            [21_519, 154_587]
        );
        assert_eq!(
            ids(&format!(
                "{{ Page {{ media({all}, type: MANGA) {{ id }} }} }}"
            )),
            Vec::<i64>::new()
        );
        // By popularity: AniList's count where its document has one, then
        // this server's figure — never one weighed against the other.
        assert_eq!(
            ids(&format!(
                "{{ Page {{ media({all}, sort: [POPULARITY_DESC]) {{ id }} }} }}"
            )),
            [21_519, 21, 154_587]
        );
        assert_eq!(
            ids(&format!(
                "{{ Page {{ media({all}, sort: [TITLE_ROMAJI]) {{ id }} }} }}"
            )),
            [21_519, 21, 154_587]
        );
        // What only AniList can apply: AniList's page.
        for theirs in [
            "{ Page { media(search: \"frieren\") { id } } }",
            "{ Page { media(season: FALL, seasonYear: 2026) { id } } }",
            "{ Page { media(id_in: [21], sort: [TRENDING_DESC]) { id } } }",
            "{ Page { media(id_in: [21], tag_in: [\"Pirates\"]) { id } } }",
            "{ Page { media(idMal_in: [21]) { id } } }",
            "{ Page { airingSchedules(airingAt_greater: 0) { id } } }",
        ] {
            assert_eq!(
                pages_answer(theirs, &held),
                Err(Failure::Unanswerable),
                "{theirs}"
            );
        }
    }

    #[test]
    fn a_page_of_schedules_is_narrowed_and_sorted_as_anilist_would() {
        let finished = Held::whole(frieren(), Some(kept(&fixture("media-finished"))));
        let airing = Held::whole(one_piece(), Some(kept(&fixture("media-airing"))));
        let held = [&finished, &airing];
        let episodes = |document: &str| -> Vec<(i64, i64)> {
            pages_answer(document, &held).unwrap()["Page"]["airingSchedules"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| {
                    (
                        a["mediaId"].as_i64().unwrap(),
                        a["episode"].as_i64().unwrap(),
                    )
                })
                .collect()
        };
        assert_eq!(
            episodes(
                "{ Page { airingSchedules(mediaId_in: [21, 154587], notYetAired: false, episode_greater: 26, sort: [EPISODE_DESC]) { mediaId episode } } }"
            ),
            [(21, 1179), (21, 1178), (154_587, 28), (154_587, 27)]
        );
        assert_eq!(
            episodes(
                "{ Page(perPage: 2, page: 2) { airingSchedules(mediaId: 154587, sort: [TIME_DESC]) { mediaId episode } } }"
            ),
            [(154_587, 26), (154_587, 25)]
        );
        // By the ids this server gives them: each episode's own.
        let id_of = |episode: i32| {
            airing
                .schedule
                .iter()
                .find(|a| a.episode == episode)
                .unwrap()
                .id
        };
        let id = id_of(1181);
        assert_eq!(
            episodes(&format!(
                "{{ Page {{ airingSchedules(mediaId: 21, id_in: [{id}]) {{ mediaId episode }} }} }}"
            )),
            [(21, 1181)]
        );
        let first = pages_answer(
            "{ Page { airingSchedules(mediaId: 21, notYetAired: true) { id airingAt timeUntilAiring } } }",
            &held,
        )
        .unwrap();
        let next = &first["Page"]["airingSchedules"][0];
        assert_eq!(
            next["timeUntilAiring"],
            json!(next["airingAt"].as_i64().unwrap() - NOW)
        );
        assert_eq!(next["id"], json!(id_of(1180)));
    }

    #[test]
    fn a_list_of_characters_cut_at_a_page_answers_that_page_alone() {
        let mut item = your_name();
        item.characters = your_name_cast();
        let as_kept = |characters: Value| {
            let mut theirs = kept(&fixture("media-film"));
            theirs["characters"] = characters;
            Held::whole(item.clone(), Some(theirs))
        };
        let edges = |count: usize| vec![json!({ "role": "MAIN" }); count];
        let nodes = |held: &Held, query: &str| -> Result<Value, Failure> {
            held.answer(query, &[])
                .map(|answer| answer["Media"]["characters"].clone())
        };

        // AniList said more follow its first page of three: that page, in
        // the order it was asked in, and nothing beyond it.
        let cut = as_kept(json!({
            "pageInfo": { "total": 120, "hasNextPage": true }, "edges": edges(3)
        }));
        assert_eq!(
            nodes(
                &cut,
                "{ Media { characters(sort: [ROLE, RELEVANCE, ID], perPage: 2) { nodes { id } } } }"
            )
            .unwrap(),
            json!({ "nodes": [{ "id": 121_514 }, { "id": 121_516 }] })
        );
        // By role first, in another order within a role, as Jellyfin asks:
        // the same characters, in the order they were kept.
        for by_role in [
            "{ Media { characters(sort: [ROLE, FAVOURITES_DESC], perPage: 2) { nodes { id } } } }",
            "{ Media { characters(sort: [ROLE], perPage: 2) { nodes { id } } } }",
        ] {
            assert_eq!(
                nodes(&cut, by_role).unwrap(),
                json!({ "nodes": [{ "id": 121_514 }, { "id": 121_516 }] }),
                "{by_role}"
            );
        }
        for beyond in [
            // How many there are, and whether more follow.
            "{ Media { characters(sort: [ROLE, RELEVANCE, ID], perPage: 2) { pageInfo { total } } } }",
            // Another page, a longer one, other characters, another order.
            "{ Media { characters(sort: [ROLE, RELEVANCE, ID], page: 2, perPage: 2) { nodes { id } } } }",
            "{ Media { characters(sort: [ROLE, RELEVANCE, ID], perPage: 4) { nodes { id } } } }",
            "{ Media { characters(sort: [ROLE, RELEVANCE, ID], perPage: 2, role: MAIN) { nodes { id } } } }",
            "{ Media { characters(sort: [FAVOURITES_DESC], perPage: 2) { nodes { id } } } }",
            "{ Media { characters(sort: [ID], perPage: 2) { nodes { id } } } }",
            "{ Media { characters(perPage: 2) { nodes { id } } } }",
        ] {
            assert_eq!(nodes(&cut, beyond), Err(Failure::Unanswerable), "{beyond}");
        }

        // AniList said none follow: the whole list, any page of it.
        let whole = as_kept(json!({
            "pageInfo": { "total": 3, "hasNextPage": false }, "edges": edges(3)
        }));
        assert_eq!(
            nodes(
                &whole,
                "{ Media { characters(page: 2, perPage: 2) { pageInfo { total hasNextPage } } } }"
            )
            .unwrap(),
            json!({ "pageInfo": { "total": 3, "hasNextPage": false } })
        );
        // Kept before AniList was asked whether more followed: a full page
        // may have been cut there, a shorter one was not.
        let counted = "{ Media { characters { pageInfo { total } } } }";
        assert_eq!(
            nodes(&as_kept(json!({ "edges": edges(25) })), counted),
            Err(Failure::Unanswerable)
        );
        assert_eq!(
            nodes(&as_kept(json!({ "edges": edges(24) })), counted).unwrap(),
            json!({ "pageInfo": { "total": 3 } })
        );
    }

    #[test]
    fn relations_for_adults_are_kept_from_a_client_not_shown_them() {
        let mut theirs = kept(&fixture("media-finished"));
        theirs["relations"]["edges"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "relationType": "SPIN_OFF",
                "node": {
                    "id": 9_990_001, "idMal": null, "type": "ANIME", "format": "OVA",
                    "isAdult": true,
                    "title": { "romaji": "For adults", "english": null, "native": null },
                    "startDate": { "year": 2026 },
                    "coverImage": { "extraLarge": null, "large": null }
                }
            }));
        let mut item = frieren();
        item.relations.push(Relation {
            id: "kept-for-adults".into(),
            relation_type: "SIDE_STORY".into(),
            source: "anilist".into(),
            external_id: 9_990_002,
            mal_id: None,
            title: "Kept for adults".into(),
            medium: "anime".into(),
            format: Some("OVA".into()),
            year: Some(2026),
            image: None,
            is_adult: true,
            work_id: None,
            sort_order: 99,
        });
        let mut held = Held::whole(item, Some(theirs));
        let related = |held: &Held| -> Vec<i64> {
            held.answer("{ Media { relations { edges { node { id } } } } }", &[])
                .unwrap()["Media"]["relations"]["edges"]
                .as_array()
                .unwrap()
                .iter()
                .map(|edge| edge["node"]["id"].as_i64().unwrap())
                .collect()
        };
        // AniList's own edges and the catalogue's alike.
        let hidden = related(&held);
        assert!(!hidden.contains(&9_990_001), "{hidden:?}");
        assert!(!hidden.contains(&9_990_002), "{hidden:?}");
        assert!(!hidden.is_empty());
        held.adult = true;
        let shown = related(&held);
        assert!(shown.contains(&9_990_001) && shown.contains(&9_990_002));
        assert_eq!(shown.len(), hidden.len() + 2);
    }

    #[test]
    fn a_number_of_this_servers_names_no_page_of_anilists() {
        let mut item = your_name();
        item.external_ids = ExternalIds::default();
        item.is_manual = true;
        let held = Held::whole(item, None);
        let number = held.cour.anilist.unwrap();
        assert!(crate::service::ids::is_own(number), "{number}");
        assert_eq!(
            held.answer("{ Media { id siteUrl } }", &[]).unwrap()["Media"],
            json!({ "id": number, "siteUrl": null })
        );
        // AniList's own entries keep their page.
        let theirs = Held::whole(your_name(), Some(kept(&fixture("media-film"))));
        assert_eq!(
            theirs.answer("{ Media { siteUrl } }", &[]).unwrap()["Media"]["siteUrl"],
            json!("https://anilist.co/anime/21519")
        );
    }

    #[test]
    fn two_episodes_under_one_id_are_anilists_to_schedule() {
        let mut held = Held::whole(one_piece(), Some(kept(&fixture("media-airing"))));
        let asked =
            "{ Media { airingSchedule(notYetAired: true, perPage: 2) { nodes { id episode } } } }";
        let first = held.answer(asked, &[]).unwrap();
        let listed = first["Media"]["airingSchedule"]["nodes"]
            .as_array()
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert_ne!(listed[0]["id"], listed[1]["id"]);
        assert!(distinct(&held.schedule));

        // Two episodes made one number: a client would take them for one.
        let last = held.schedule.len() - 1;
        held.schedule[last].id = held.schedule[0].id;
        assert!(!distinct(&held.schedule));
        assert_eq!(held.answer(asked, &[]), Err(Failure::Unanswerable));
    }
}

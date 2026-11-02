//! TMDB's shape, written from the catalogue: what the TMDB relay answers
//! itself for a work held here, where an administrator chose so
//! (`tmdb.relayMode = local-first`), rather than asking TMDB.
//!
//! A document is written from the catalogue first — the merged values, the
//! locks, the translations held, the texts locked in a language, without
//! what a person hid — then from TMDB's own document of the work, as it was
//! last fetched, for what the catalogue does not keep: a budget, the full
//! crew, the networks' and the companies' ids and logos, the pictures'
//! languages and votes. What neither has is written empty or null, as TMDB
//! writes it when it knows nothing: an empty text, a nought, an empty list.
//! Nothing is made up, and where TMDB files a thing by an id the catalogue
//! does not know — a company, a keyword, a person added by hand — the id is
//! TMDB's never-used nought.
//!
//! A document follows the language a client asked TMDB in (`language=`,
//! TMDB's `en-US` when none was), as TMDB does: in the catalogue's own, the
//! work as it is held; in another, what was locked in that language, else
//! the translation held in it, else what TMDB answers a language it holds
//! nothing in — the original title, an empty overview, `Season 1`,
//! `Episode 1`. Never another language's text in its place.
//!
//! A picture is named by TMDB's path when it is one of TMDB's; another —
//! a source's, or one uploaded — only when TMDB's clients are sent to this
//! server for pictures (`tmdb.images = server`), by the name its gate serves
//! the copy kept here under ([`Pictures`]). A work's poster is otherwise its
//! best of TMDB's, or none.
//!
//! A series' seasons and episodes are written as the catalogue numbers them
//! only where that is TMDB's numbering too ([`Numbering`]): a work numbered
//! by hand, or by TMDB itself. Otherwise its own document lists TMDB's
//! seasons, from TMDB's document, and a season asked for is TMDB's to
//! answer.

use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

use chrono::NaiveDate;
use serde_json::{Map, Value, json};

use crate::{
    domain::{
        CompanyCredit, CompanyRole, CoverType, Credit, CreditType, Episode, EpisodeOrder, Image,
        ImageDetails, MediaItem, MediaKind, ReleaseKind, Season, Translation, Video,
        fields::{self, LockedText},
    },
    providers::lang::{base_language, country_name, iso_3166_2_to_3, iso_3166_3_to_2},
    service::{language, orders},
};

/// The most a client may append to one document: TMDB refuses more
/// (`status_code` 27), and a request that asks for more is TMDB's to refuse.
pub const MOST_APPENDED: usize = 20;

/// Where TMDB serves its pictures, a size and a path after it.
const TMDB_PICTURES: &str = "https://image.tmdb.org/t/p/";

// ─── the language asked ──────────────────────────────────────────────────────

/// The language a document is written in, as TMDB reads `language=`, beside
/// the one the catalogue keeps its works in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Language {
    /// As the catalogue files a language: `eng`.
    pub asked: String,
    /// As TMDB files it: `en`.
    pub code: String,
    /// The catalogue's own, as filed: `fra`.
    pub stored: String,
}

impl Language {
    /// `asked` as a client wrote it — `fr-FR`, `en` — and `stored`, the
    /// catalogue's, as filed. What is no language TMDB answers in English.
    pub fn new(asked: &str, stored: &str) -> Self {
        let asked = match language::normalize(asked) {
            filed if filed.is_empty() => "eng".to_string(),
            filed => filed,
        };
        Self {
            code: language::two_letter(&asked),
            asked,
            stored: stored.to_string(),
        }
    }

    /// Whether the document is in the language the catalogue holds its
    /// works in: their own texts, as they are held.
    pub fn is_stored(&self) -> bool {
        self.asked == self.stored
    }
}

/// Which pictures and which videos a document lists, by their language, as
/// TMDB chooses them: `include_image_language` (`en,null`), else the
/// language asked alone — and every picture when no language was asked;
/// `include_video_language`, else the language asked, English when none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filters {
    /// `None` for every picture; a language `None` for one with no text.
    images: Option<Vec<Option<String>>>,
    videos: Vec<Option<String>>,
}

impl Filters {
    /// The filters a request's query asks for, `language` being the one it
    /// asked, if it asked any.
    pub fn from_query(query: Option<&str>, language: Option<&Language>) -> Self {
        let asked = |name: &str| -> Option<Vec<Option<String>>> {
            let value = url::form_urlencoded::parse(query?.as_bytes())
                .filter(|(key, _)| key == name)
                .last()
                .map(|(_, value)| value.into_owned())?;
            let codes: Vec<Option<String>> = value
                .split(',')
                .map(str::trim)
                .filter(|code| !code.is_empty())
                .map(|code| {
                    (!code.eq_ignore_ascii_case("null"))
                        .then(|| base_language(code).to_ascii_lowercase())
                })
                .collect();
            (!codes.is_empty()).then_some(codes)
        };
        let own = language.map(|language| vec![Some(language.code.clone())]);
        Self {
            images: asked("include_image_language").or_else(|| own.clone()),
            videos: asked("include_video_language")
                .or(own)
                .unwrap_or_else(|| vec![Some("en".to_string())]),
        }
    }

    /// Every picture, and the videos in English: a request that asked for
    /// no language.
    pub fn all() -> Self {
        Self::from_query(None, None)
    }

    fn takes_image(&self, language: Option<&str>) -> bool {
        self.images.as_ref().is_none_or(|codes| {
            codes
                .iter()
                .any(|code| code.as_deref() == language.map(str::to_ascii_lowercase).as_deref())
        })
    }

    fn takes_video(&self, language: Option<&str>) -> bool {
        self.videos
            .iter()
            .any(|code| code.as_deref() == language.map(str::to_ascii_lowercase).as_deref())
    }
}

// ─── pictures ────────────────────────────────────────────────────────────────

/// How the catalogue's pictures are named to TMDB's clients: by the path a
/// client puts after TMDB's configured base and a size.
pub trait Pictures: Sync {
    /// The address a provider gave a picture this server keeps a copy of,
    /// for the copy's address; any other address as it is.
    fn origin(&self, address: &str) -> String {
        address.to_string()
    }

    /// The name this server's gate serves a picture it keeps under
    /// (`/tmdb-images/{size}/{name}`), when TMDB's clients are sent there
    /// for pictures: `/{hash}.jpg`. None by default.
    fn kept(&self, _address: &str) -> Option<String> {
        None
    }

    /// The picture's width and height in pixels, where this server knows
    /// them.
    fn size(&self, _address: &str) -> Option<(u32, u32)> {
        None
    }

    /// The path a TMDB client fetches a picture by: TMDB's own for one of
    /// TMDB's, else the gate's name for a copy kept here ([`Self::kept`]).
    fn path(&self, address: &str) -> Option<String> {
        tmdb_path(&self.origin(address)).or_else(|| self.kept(address))
    }
}

/// TMDB's own pictures alone, named by their paths: what can be named when
/// no other picture can be fetched by a TMDB client.
#[cfg(test)]
pub struct TmdbOnly;

#[cfg(test)]
impl Pictures for TmdbOnly {}

/// The path TMDB files a picture under, from its address: `/abc.jpg` from
/// `https://image.tmdb.org/t/p/original/abc.jpg`, whatever the size asked.
pub fn tmdb_path(address: &str) -> Option<String> {
    let rest = address.strip_prefix(TMDB_PICTURES)?;
    let (_size, file) = rest.split_once('/')?;
    (!file.is_empty() && !file.contains('/')).then(|| format!("/{file}"))
}

// ─── what a person hid ───────────────────────────────────────────────────────

/// What the sources gave a work and a person hid here, as TMDB names each:
/// a credit by the person's TMDB id and its kind, a picture by the path TMDB
/// files it under, an alternative title by its text and the country TMDB
/// files it for. A credit a source named by the person's name alone — one of
/// Skyhook's, Fan-Kai's — is none TMDB's lists can be told by: TMDB names
/// every person by an id, and a name could be somebody else's.
pub(crate) struct Withheld {
    pub(crate) credits: Vec<(CreditType, i64)>,
    pub(crate) pictures: HashSet<String>,
    pub(crate) titles: Vec<(String, Option<String>)>,
}

impl Withheld {
    /// What `item` hid, its pictures named by the address a provider gave
    /// each (`origin`): a TMDB client names one by TMDB's path alone.
    pub(crate) fn of(item: &MediaItem, origin: &dyn Fn(&str) -> String) -> Self {
        Self {
            credits: item
                .hidden
                .credits
                .iter()
                .filter_map(|credit| Some((credit.credit_type, credit.tmdb_person_id?)))
                .collect(),
            pictures: item
                .hidden
                .images
                .iter()
                .filter_map(|image| tmdb_path(&origin(&image.url)))
                .collect(),
            titles: item
                .hidden
                .alternative_titles
                .iter()
                .map(|title| {
                    (
                        folded(&title.title),
                        title.language.clone().filter(|code| !code.is_empty()),
                    )
                })
                .collect(),
        }
    }

    pub(crate) fn hides(&self, kind: CreditType, person: i64) -> bool {
        self.credits.contains(&(kind, person))
    }

    /// Whether one of TMDB's crew is hidden here: by the kind of credit
    /// their job makes ([`crew_kind`]) and their id.
    fn hides_crew(&self, member: &Value) -> bool {
        let (Some(id), Some(job)) = (
            member.get("id").and_then(Value::as_i64),
            member.get("job").and_then(Value::as_str),
        ) else {
            return false;
        };
        let department = member.get("department").and_then(Value::as_str);
        crew_kind(job, department).is_some_and(|kind| self.hides(kind, id))
    }

    /// Whether TMDB's alternative title is one hidden here: its text, its
    /// case and the blanks around it aside, and its country, as the
    /// catalogue files it ([`iso_3166_2_to_3`]).
    pub(crate) fn hides_title(&self, entry: &Value) -> bool {
        let Some(title) = entry.get("title").and_then(Value::as_str) else {
            return false;
        };
        let country = entry
            .get("iso_3166_1")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|code| !code.is_empty())
            .map(iso_3166_2_to_3);
        let title = folded(title);
        self.titles
            .iter()
            .any(|(text, filed)| *text == title && *filed == country)
    }
}

/// A title as it is compared: its case and the blanks around it aside.
pub(crate) fn folded(text: &str) -> String {
    text.trim().to_lowercase()
}

/// The kind of credit a job of TMDB's crew is, as the catalogue reads it
/// ([`crate::providers::tmdb::map`]): a director by the job, a producer by
/// the job — the producers proper, not the executive ones — a writer by the
/// department, whatever the job, and the rest of the crew. A person's crew
/// credits are one credit to hide: hidden, every job of theirs of that kind
/// goes.
pub(crate) fn crew_kind(job: &str, department: Option<&str>) -> Option<CreditType> {
    match job {
        "Director" => Some(CreditType::Director),
        "Producer" => Some(CreditType::Producer),
        _ if department == Some("Writing") => Some(CreditType::Writer),
        _ => Some(CreditType::Crew),
    }
}

// ─── the work ────────────────────────────────────────────────────────────────

/// Whose numbering a series' seasons and episodes are written in, beside
/// TMDB's.
#[derive(Clone, Copy, Debug)]
pub enum Numbering<'a> {
    /// The catalogue's seasons and episodes are TMDB's, or the only ones
    /// there are: a work numbered by hand, one no TMDB id names, one TMDB
    /// numbers. Written from the catalogue.
    Own,
    /// TMDB numbers the series otherwise than the catalogue, which follows
    /// TheTVDB, and the catalogue keeps TMDB's numbering as an order — its
    /// own, `tmdb-aired`, or the one frozen as the work's: each place the
    /// episode held for it, or the order's record of one it does not hold.
    Order(&'a EpisodeOrder),
    /// TMDB numbers the series otherwise, and nothing here keeps how: its
    /// seasons are listed from TMDB's own document, and a season or an
    /// episode asked for is TMDB's to answer.
    Theirs,
}

/// An episode where TMDB's numbering puts it: the episode held, or the
/// record an order keeps of one the work does not hold (`own`), under TMDB's
/// id for it.
#[derive(Clone, Debug)]
struct Spot {
    season: i32,
    number: i32,
    tmdb_id: Option<i64>,
    episode: Episode,
    own: bool,
}

/// An episode's text held in the language asked, by the catalogue's numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EpisodeText {
    pub title: Option<String>,
    pub overview: Option<String>,
}

/// A work as a document of TMDB's shape is written from it.
pub struct Work<'a> {
    /// As served in the language asked: what was locked in it laid over
    /// the catalogue's own text ([`fields::lay_only_in`]).
    item: MediaItem,
    /// The texts locked on it, in every language they were locked in.
    locked: &'a [LockedText],
    /// Its id in TMDB's dialect.
    id: i64,
    /// What Sonarr keeps a series under: TheTVDB's id, else one of this
    /// server's ([`crate::service::ids::for_client`]).
    tvdb_id: Option<i64>,
    /// TMDB's own document of it, in the catalogue's language.
    snapshot: Option<&'a Value>,
    language: &'a Language,
    pictures: &'a dyn Pictures,
    numbering: Numbering<'a>,
    /// Its episodes as TMDB numbers them, worked out once ([`Self::spots`]).
    spots: OnceLock<Vec<Spot>>,
    episode_texts: HashMap<(i32, i32), EpisodeText>,
    /// The credits of its episodes read for the seasons asked, by the
    /// catalogue's numbers ([`Self::with_episode_credits`]).
    episode_credits: HashMap<(i32, i32), Vec<Credit>>,
    today: NaiveDate,
    withheld: Withheld,
}

impl<'a> Work<'a> {
    /// `read`, the work as the catalogue reads it — its texts as locked for
    /// its own language, its pictures' copies at this server's addresses —
    /// known to TMDB's clients as `id`, to be written in `language`.
    pub fn new(
        read: &'a MediaItem,
        id: i64,
        language: &'a Language,
        pictures: &'a dyn Pictures,
    ) -> Self {
        let mut item = read.clone();
        if !language.is_stored() {
            fields::lay_only_in(&mut item, &language.asked);
        }
        let withheld = Withheld::of(read, &|address| pictures.origin(address));
        Self {
            item,
            locked: &read.locked_texts,
            id,
            tvdb_id: None,
            snapshot: None,
            language,
            pictures,
            numbering: Numbering::Own,
            spots: OnceLock::new(),
            episode_texts: HashMap::new(),
            episode_credits: HashMap::new(),
            today: chrono::Utc::now().date_naive(),
            withheld,
        }
    }

    /// TMDB's own document of the work, where the catalogue keeps one of
    /// the same TMDB id.
    pub fn with_snapshot(mut self, snapshot: Option<&'a Value>) -> Self {
        self.snapshot = snapshot.filter(|document| {
            document.get("id").and_then(Value::as_i64).is_some()
                && document.get("id").and_then(Value::as_i64) == self.item.external_ids.tmdb
        });
        self
    }

    pub fn with_tvdb_id(mut self, tvdb_id: Option<i64>) -> Self {
        self.tvdb_id = tvdb_id;
        self
    }

    pub fn numbered(mut self, numbering: Numbering<'a>) -> Self {
        self.numbering = numbering;
        self.spots = OnceLock::new();
        self
    }

    pub fn with_episode_texts(mut self, texts: HashMap<(i32, i32), EpisodeText>) -> Self {
        self.episode_texts = texts;
        self
    }

    /// The credits of its episodes — their guests, their directors and
    /// writers — each by its episode's numbers in the catalogue's own
    /// numbering, as [`crate::db::repo::item::episode_credits`] reads them
    /// for the seasons [`Self::held_seasons`] names.
    pub fn with_episode_credits(mut self, credits: Vec<(i32, i32, Credit)>) -> Self {
        let mut by_episode: HashMap<(i32, i32), Vec<Credit>> = HashMap::new();
        for (season, number, credit) in credits {
            by_episode.entry((season, number)).or_default().push(credit);
        }
        self.episode_credits = by_episode;
        self
    }

    /// The catalogue's seasons that the episodes of TMDB's seasons `asked`
    /// are of, as the work numbers them: whose episodes' credits a season's
    /// document lists.
    pub fn held_seasons(&self, asked: &[i32]) -> Vec<i32> {
        let mut seasons: Vec<i32> = asked
            .iter()
            .flat_map(|season| self.episodes_of(*season))
            .filter(|spot| !spot.own)
            .map(|spot| spot.episode.season_number)
            .collect();
        seasons.sort_unstable();
        seasons.dedup();
        seasons
    }

    #[cfg(test)]
    pub fn kind(&self) -> MediaKind {
        self.item.kind
    }

    /// TMDB's own id of the work, where TMDB knows it.
    pub fn tmdb_id(&self) -> Option<i64> {
        self.item.external_ids.tmdb
    }

    /// Whether the catalogue holds the season, numbered as TMDB numbers it.
    pub fn has_season(&self, number: i32) -> bool {
        self.season_numbers().contains(&number)
    }

    /// Whether the catalogue holds the episode, numbered as TMDB numbers it.
    pub fn has_episode(&self, season: i32, number: i32) -> bool {
        self.episode_at(season, number).is_some()
    }

    /// Whether the catalogue keeps the series' seasons as TMDB numbers them:
    /// its own, or an order of TMDB's numbering.
    fn numbers_seasons(&self) -> bool {
        !matches!(self.numbering, Numbering::Theirs)
    }

    // ─── reading ─────────────────────────────────────────────────────────

    /// A value of TMDB's own document, by its JSON pointer: `/budget`.
    fn theirs(&self, pointer: &str) -> Option<&'a Value> {
        self.snapshot?
            .pointer(pointer)
            .filter(|value| !value.is_null())
    }

    /// A list of TMDB's own document, or none.
    fn their_list(&self, pointer: &str) -> &'a [Value] {
        self.theirs(pointer)
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    fn stored(&self) -> bool {
        self.language.is_stored()
    }

    fn is_movie(&self) -> bool {
        self.item.kind == MediaKind::Movie
    }

    /// The path a picture of the catalogue is named by, when it has one.
    fn path(&self, address: Option<&str>) -> Value {
        address
            .and_then(|address| self.pictures.path(address))
            .map_or(Value::Null, Value::String)
    }

    // ─── the work's texts ────────────────────────────────────────────────

    /// The translation held in the language asked.
    fn translation(&self) -> Option<&Translation> {
        self.item
            .translations
            .iter()
            .find(|t| t.language == self.language.asked)
    }

    /// The work's title: as held, in the catalogue's language; in another,
    /// as locked in it, else as translated, else — as TMDB answers a
    /// language it has no title in — the original title.
    fn title(&self) -> String {
        if self.stored() || self.item.is_locked("title") {
            return self.item.title.clone();
        }
        self.translation()
            .and_then(|t| some_text(t.title.as_deref()))
            .or_else(|| some_text(self.item.original_title.as_deref()))
            .unwrap_or_else(|| self.item.title.clone())
    }

    fn original_title(&self) -> String {
        some_text(self.item.original_title.as_deref()).unwrap_or_else(|| self.item.title.clone())
    }

    /// The overview or the tagline, as the title is, and empty where none
    /// is held in the language asked.
    fn text(&self, field: &str) -> String {
        let held = |item: &MediaItem| match field {
            "overview" => item.overview.clone(),
            _ => item.tagline.clone(),
        };
        if self.stored() || self.item.is_locked(field) {
            return held(&self.item).unwrap_or_default();
        }
        self.translation()
            .and_then(|t| match field {
                "overview" => t.overview.clone(),
                _ => t.tagline.clone(),
            })
            .unwrap_or_default()
    }

    /// A film's homepage in the language asked — TMDB's translation's, as
    /// TMDB gives one; a series' is the one it has whatever the language.
    fn homepage(&self) -> String {
        if self.stored() || !self.is_movie() || self.item.is_locked("homepage") {
            return self.item.homepage.clone().unwrap_or_default();
        }
        self.their_translation()
            .and_then(|t| t.pointer("/data/homepage"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default()
    }

    /// TMDB's own translation in the language asked, the region it is at
    /// home in first.
    fn their_translation(&self) -> Option<&'a Value> {
        let entries = self.their_list("/translations/translations");
        let of_language = |entry: &&Value| {
            entry.get("iso_639_1").and_then(Value::as_str) == Some(self.language.code.as_str())
        };
        entries.iter().find(of_language)
    }

    // ─── genres ──────────────────────────────────────────────────────────

    /// The work's genres, as locked, each with TMDB's id — found among the
    /// genres of TMDB's document by its name, else in TMDB's lists — and
    /// named in the language asked: as held in the catalogue's own, else as
    /// TMDB names it in English or in French, else in English, as TMDB's
    /// documents name a genre it has no name for in a language. A genre
    /// TMDB has no id for is left out: an id cannot be made up.
    fn genres(&self) -> Vec<(i64, String)> {
        let table = genre_table(self.item.kind);
        let mut out: Vec<(i64, String)> = Vec::new();
        for name in &self.item.genres {
            let wanted = folded(name);
            let id = self
                .their_list("/genres")
                .iter()
                .find(|genre| {
                    genre
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|theirs| folded(theirs) == wanted)
                })
                .and_then(|genre| genre.get("id").and_then(Value::as_i64))
                .or_else(|| {
                    table
                        .iter()
                        .find(|(_, en, fr)| folded(en) == wanted || folded(fr) == wanted)
                        .map(|(id, _, _)| *id)
                });
            let Some(id) = id else {
                continue;
            };
            if out.iter().any(|(kept, _)| *kept == id) {
                continue;
            }
            let named = if self.stored() {
                name.clone()
            } else {
                let known = table.iter().find(|(known, _, _)| *known == id);
                match (known, self.language.code.as_str()) {
                    (Some((_, _, fr)), "fr") => (*fr).to_string(),
                    (Some((_, en, _)), _) => (*en).to_string(),
                    (None, _) => name.clone(),
                }
            };
            out.push((id, named));
        }
        out
    }

    fn genre_objects(&self) -> Value {
        Value::Array(
            self.genres()
                .into_iter()
                .map(|(id, name)| json!({ "id": id, "name": name }))
                .collect(),
        )
    }

    fn genre_ids(&self) -> Value {
        Value::Array(self.genres().into_iter().map(|(id, _)| json!(id)).collect())
    }

    // ─── what TMDB's document says, else the catalogue ───────────────────

    fn original_language(&self) -> String {
        self.item
            .original_language
            .as_deref()
            .filter(|code| !code.trim().is_empty())
            .map(language::two_letter)
            .or_else(|| {
                self.theirs("/original_language")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }

    /// Where the work comes from: TMDB's list, else the catalogue's country.
    fn origin_country(&self) -> Value {
        if let Some(theirs) = self.theirs("/origin_country").filter(|v| v.is_array()) {
            return theirs.clone();
        }
        let country = self
            .item
            .original_country
            .as_deref()
            .and_then(iso_3166_3_to_2)
            .map(str::to_string)
            .or_else(|| self.item.production_countries.first().cloned());
        Value::Array(country.into_iter().map(Value::String).collect())
    }

    fn popularity(&self) -> Value {
        let popularity = self
            .item
            .popularity
            .or_else(|| self.theirs("/popularity").and_then(Value::as_f64))
            .unwrap_or(0.0);
        json!(popularity)
    }

    /// TMDB's vote: the catalogue's rating of TMDB's, else TMDB's document.
    fn votes(&self) -> (Value, Value) {
        if let Some(rating) = self.item.rating("tmdb")
            && let Some(value) = rating.value
        {
            return (json!(value), json!(rating.votes.unwrap_or(0)));
        }
        (
            json!(
                self.theirs("/vote_average")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
            ),
            json!(
                self.theirs("/vote_count")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            ),
        )
    }

    fn flag(&self, key: &str) -> bool {
        self.theirs(&format!("/{key}"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// The work's companies in a role, as the catalogue keeps them — TMDB's
    /// list alone where TMDB gave one — in their order.
    fn companies(&self, role: CompanyRole) -> Vec<&CompanyCredit> {
        let mut credited: Vec<&CompanyCredit> = self
            .item
            .companies
            .iter()
            .filter(|credit| credit.role == role)
            .collect();
        if credited
            .iter()
            .any(|credit| credit.source.as_deref() == Some("tmdb"))
        {
            credited.retain(|credit| credit.source.as_deref() == Some("tmdb"));
        }
        credited.sort_by_key(|credit| credit.sort_order);
        credited
    }

    /// A company as TMDB lists one: its id there, its logo, its country.
    fn company_entry(&self, credit: &CompanyCredit) -> Value {
        let company = &credit.company;
        json!({
            "id": company.tmdb_id.unwrap_or(0),
            "logo_path": self.path(company.logo.as_deref()),
            "name": company.name,
            "origin_country": company.country.clone().unwrap_or_default(),
        })
    }

    /// The companies in a role named as a person locked them, or as the
    /// catalogue's texts name them: each as the catalogue keeps it, else as
    /// TMDB's document lists it, else with nought for an id.
    fn named_companies<'n>(
        &self,
        names: impl Iterator<Item = &'n String>,
        ours: &[&CompanyCredit],
        theirs: &[Value],
    ) -> Value {
        Value::Array(
            names
                .filter(|name| !name.trim().is_empty())
                .map(|name| {
                    ours.iter()
                        .find(|credit| folded(&credit.company.name) == folded(name))
                        .map(|credit| self.company_entry(credit))
                        .unwrap_or_else(|| company(theirs, name))
                })
                .collect(),
        )
    }

    /// The companies that made the work: as the catalogue keeps them, with
    /// TMDB's ids, logos and countries; as locked, where a person locked
    /// their names; else — a work not refreshed since the catalogue kept
    /// them — TMDB's document's list.
    fn production_companies(&self) -> Value {
        let theirs = self.their_list("/production_companies");
        let ours = self.companies(CompanyRole::Production);
        if !self.item.is_locked("productionCompanies") {
            if !ours.is_empty() {
                return Value::Array(ours.iter().map(|c| self.company_entry(c)).collect());
            }
            if !theirs.is_empty() {
                return Value::Array(theirs.to_vec());
            }
        }
        self.named_companies(self.item.production_companies.iter(), &ours, theirs)
    }

    /// The networks a series aired on, as its companies are: the one locked
    /// where a person locked it.
    fn networks(&self) -> Value {
        let theirs = self.their_list("/networks");
        let ours = self.companies(CompanyRole::Network);
        if !self.item.is_locked("network") {
            if !ours.is_empty() {
                return Value::Array(ours.iter().map(|c| self.company_entry(c)).collect());
            }
            if !theirs.is_empty() {
                return Value::Array(theirs.to_vec());
            }
        }
        self.named_companies(self.item.network.iter(), &ours, theirs)
    }

    /// The countries the work was made in, each named as TMDB names it.
    fn production_countries(&self) -> Value {
        let theirs = self.their_list("/production_countries");
        Value::Array(
            self.item
                .production_countries
                .iter()
                .map(|code| {
                    let name = theirs
                        .iter()
                        .find(|c| {
                            c.get("iso_3166_1")
                                .and_then(Value::as_str)
                                .is_some_and(|theirs| theirs.eq_ignore_ascii_case(code))
                        })
                        .and_then(|c| c.get("name").and_then(Value::as_str))
                        .map(str::to_string)
                        .or_else(|| country_name(code).map(str::to_string))
                        .unwrap_or_default();
                    json!({ "iso_3166_1": code.to_ascii_uppercase(), "name": name })
                })
                .collect(),
        )
    }

    fn spoken_languages(&self) -> Value {
        Value::Array(self.their_list("/spoken_languages").to_vec())
    }

    // ─── pictures ────────────────────────────────────────────────────────

    /// The path of the picture the work leads with, of a kind: the one a
    /// person chose, else one added by hand, else the first the sources
    /// gave — where it has a path; else its first of TMDB's; else none.
    fn lead(&self, kind: CoverType) -> Value {
        let own: Vec<&Image> = self
            .item
            .images
            .iter()
            .filter(|image| image.season_number.is_none() && image.cover_type == kind)
            .collect();
        let lead = crate::domain::lead_images(own.iter().copied(), &self.item.primary_images)
            .into_iter()
            .find(|image| image.cover_type == kind)
            .and_then(|image| self.pictures.path(&image.url));
        lead.or_else(|| {
            own.iter()
                .find_map(|image| tmdb_path(&self.pictures.origin(&image.url)))
        })
        .map_or(Value::Null, Value::String)
    }

    /// The chosen picture of a kind, by path, which a list leads with.
    fn chosen_path(&self, kind: CoverType) -> Option<String> {
        let chosen = self.item.primary_images.of(kind)?;
        self.item
            .images
            .iter()
            .find(|image| image.id == chosen)
            .and_then(|image| self.pictures.path(&image.url))
    }

    /// One of the work's lists of pictures: TMDB's own entries for it —
    /// their languages, sizes and votes — less those hidden, then the
    /// catalogue's own pictures of those kinds that have a path, the chosen
    /// one first, each kept where its language is one the request takes.
    fn picture_list(
        &self,
        key: &str,
        kinds: &[CoverType],
        theirs: &[Value],
        own: &[&Image],
        filters: &Filters,
    ) -> Vec<Value> {
        let mut listed: Vec<Value> = Vec::new();
        let mut paths: HashSet<String> = HashSet::new();
        for entry in theirs {
            let Some(path) = entry.get("file_path").and_then(Value::as_str) else {
                continue;
            };
            if self.withheld.pictures.contains(path) || !paths.insert(path.to_string()) {
                continue;
            }
            let language = entry.get("iso_639_1").and_then(Value::as_str);
            if filters.takes_image(language) {
                listed.push(picture_entry(entry));
            }
        }
        for image in own.iter().filter(|image| kinds.contains(&image.cover_type)) {
            let Some(path) = self.pictures.path(&image.url) else {
                continue;
            };
            if !paths.insert(path.clone()) {
                continue;
            }
            let language = image
                .language
                .as_deref()
                .map(language::normalize)
                .filter(|code| !code.is_empty())
                .map(|code| language::two_letter(&code));
            if !filters.takes_image(language.as_deref()) {
                continue;
            }
            let details = self.details(image);
            let side = |side: Option<i32>| side.and_then(|n| u32::try_from(n).ok());
            let (width, height) = details
                .and_then(|d| Some((side(d.width)?, side(d.height)?)))
                .or_else(|| self.pictures.size(&image.url))
                .unwrap_or((0, 0));
            listed.push(json!({
                "aspect_ratio": aspect(width, height),
                "height": height,
                "iso_3166_1": self.picture_country(&path),
                "iso_639_1": language,
                "file_path": path,
                "vote_average": details.and_then(|d| d.vote_average).unwrap_or(0.0),
                "vote_count": details.and_then(|d| d.vote_count).unwrap_or(0),
                "width": width,
            }));
        }
        let chosen = match key {
            "posters" => self.chosen_path(CoverType::Poster),
            "backdrops" => self.chosen_path(CoverType::Fanart),
            _ => None,
        };
        if let Some(chosen) = chosen
            && let Some(at) = listed
                .iter()
                .position(|entry| entry["file_path"].as_str() == Some(chosen.as_str()))
        {
            let entry = listed.remove(at);
            listed.insert(0, entry);
        }
        listed
    }

    /// What TMDB says of one of the work's pictures — its size, its votes —
    /// as the catalogue keeps it, by the address a provider gave it.
    fn details(&self, image: &Image) -> Option<&ImageDetails> {
        let details = &self.item.image_details;
        details
            .get(&image.url)
            .or_else(|| details.get(&self.pictures.origin(&image.url)))
    }

    /// The country TMDB files one of its pictures for, where its document
    /// lists the picture: what the catalogue does not keep.
    fn picture_country(&self, path: &str) -> Value {
        ["/images/posters", "/images/backdrops", "/images/logos"]
            .iter()
            .flat_map(|pointer| self.their_list(pointer))
            .find(|entry| entry.get("file_path").and_then(Value::as_str) == Some(path))
            .and_then(|entry| entry.get("iso_3166_1"))
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// The work's pictures: `{backdrops, logos, posters}`, in every language
    /// the catalogue keeps one in that the request takes. TMDB's document's
    /// lists stand in for what the catalogue keeps of TMDB's pictures until
    /// it keeps their sizes and votes: a work not refreshed since.
    fn images(&self, filters: &Filters) -> Value {
        let own: Vec<&Image> = self
            .item
            .images
            .iter()
            .filter(|image| image.season_number.is_none())
            .collect();
        let theirs = |pointer: &str| {
            if self.item.image_details.is_empty() {
                self.their_list(pointer)
            } else {
                &[]
            }
        };
        json!({
            "backdrops": self.picture_list("backdrops", &[CoverType::Fanart],
                theirs("/images/backdrops"), &own, filters),
            "logos": self.picture_list("logos", &[CoverType::Clearlogo],
                theirs("/images/logos"), &own, filters),
            "posters": self.picture_list("posters", &[CoverType::Poster, CoverType::Keyart],
                theirs("/images/posters"), &own, filters),
        })
    }

    // ─── people ──────────────────────────────────────────────────────────

    /// The credits of the work of the kinds given, in their order.
    fn credits_of(&self, kinds: &[CreditType]) -> Vec<&Credit> {
        let mut credits: Vec<&Credit> = self
            .item
            .credits
            .iter()
            .filter(|credit| kinds.contains(&credit.credit_type))
            .collect();
        credits.sort_by_key(|credit| credit.sort_order);
        credits
    }

    /// TMDB's entry for a credit of the catalogue: by TMDB's id of the
    /// credit, among a list of TMDB's document.
    fn their_entry(&self, list: &[&'a Value], credit: &Credit) -> Option<&'a Value> {
        let id = credit.credit_tmdb_id.as_deref()?;
        list.iter()
            .find(|entry| entry.get("credit_id").and_then(Value::as_str) == Some(id))
            .copied()
    }

    /// What every person of TMDB's lists carries, for a credit of the
    /// catalogue: what the catalogue knows of them, else TMDB's entry.
    fn person(
        &self,
        credit: &Credit,
        theirs: Option<&Value>,
        department: &str,
    ) -> Map<String, Value> {
        let known = self.item.people.of(credit);
        let their = |key: &str| {
            theirs
                .and_then(|entry| entry.get(key))
                .filter(|v| !v.is_null())
        };
        let mut person = Map::new();
        person.insert(
            "adult".into(),
            their("adult").cloned().unwrap_or(Value::Bool(false)),
        );
        person.insert(
            "gender".into(),
            known
                .and_then(|k| k.gender)
                .map(|g| json!(g))
                .or_else(|| their("gender").cloned())
                .unwrap_or(json!(0)),
        );
        person.insert(
            "id".into(),
            json!(
                credit
                    .tmdb_person_id
                    .or_else(|| their("id").and_then(Value::as_i64))
                    .unwrap_or(0)
            ),
        );
        person.insert(
            "known_for_department".into(),
            their("known_for_department")
                .cloned()
                .unwrap_or_else(|| json!(department)),
        );
        person.insert("name".into(), json!(credit.person_name));
        person.insert(
            "original_name".into(),
            known
                .and_then(|k| k.original_name.clone())
                .map(Value::String)
                .or_else(|| their("original_name").cloned())
                .unwrap_or_else(|| json!(credit.person_name)),
        );
        person.insert(
            "popularity".into(),
            json!(
                known
                    .and_then(|k| k.popularity)
                    .or_else(|| their("popularity").and_then(Value::as_f64))
                    .unwrap_or(0.0)
            ),
        );
        let portrait = match self.path(credit.image.as_deref()) {
            Value::Null => their("profile_path").cloned().unwrap_or(Value::Null),
            path => path,
        };
        person.insert("profile_path".into(), portrait);
        person
    }

    /// The cast: the catalogue's, each with what TMDB says of them, then
    /// those of TMDB's the catalogue does not keep and nobody hid.
    fn cast(&self) -> Vec<Value> {
        let theirs: Vec<&Value> = self.their_list("/credits/cast").iter().collect();
        let mut listed: HashSet<String> = HashSet::new();
        let mut cast: Vec<Value> = Vec::new();
        for credit in self.credits_of(&[CreditType::Actor]) {
            let entry = self.their_entry(&theirs, credit);
            let mut person = self.person(credit, entry, "Acting");
            if self.is_movie() {
                person.insert(
                    "cast_id".into(),
                    entry
                        .and_then(|e| e.get("cast_id"))
                        .cloned()
                        .unwrap_or(json!(0)),
                );
            }
            person.insert(
                "character".into(),
                json!(credit.character_name.clone().unwrap_or_default()),
            );
            let credit_id = credit.credit_tmdb_id.clone().unwrap_or_default();
            if !credit_id.is_empty() {
                listed.insert(credit_id.clone());
            }
            person.insert("credit_id".into(), json!(credit_id));
            person.insert("order".into(), json!(credit.sort_order));
            cast.push(Value::Object(person));
        }
        for entry in theirs {
            let credit_id = entry.get("credit_id").and_then(Value::as_str);
            if credit_id.is_some_and(|id| listed.contains(id)) {
                continue;
            }
            let hidden = entry
                .get("id")
                .and_then(Value::as_i64)
                .is_some_and(|id| self.withheld.hides(CreditType::Actor, id));
            if !hidden {
                cast.push(entry.clone());
            }
        }
        cast
    }

    /// The crew behind the camera, a job an entry: the catalogue's —
    /// directors, writers, producers, and the rest of the crew it keeps —
    /// each in its department and its job, then those of TMDB's the
    /// catalogue does not keep and nobody hid. A series' creators are not
    /// its crew: TMDB lists them apart ([`Self::created_by`]).
    fn crew(&self) -> Vec<Value> {
        let theirs: Vec<&Value> = self.their_list("/credits/crew").iter().collect();
        let mut listed: HashSet<String> = HashSet::new();
        let mut crew: Vec<Value> = Vec::new();
        for credit in self.credits_of(&[
            CreditType::Director,
            CreditType::Writer,
            CreditType::Producer,
            CreditType::Crew,
        ]) {
            if is_creator(credit) {
                continue;
            }
            let (department, job) = department_and_job(credit);
            let entry = self.their_entry(&theirs, credit);
            let mut person = self.person(credit, entry, &department);
            let credit_id = credit.credit_tmdb_id.clone().unwrap_or_default();
            if !credit_id.is_empty() {
                listed.insert(credit_id.clone());
            }
            person.insert("credit_id".into(), json!(credit_id));
            person.insert("department".into(), json!(department));
            person.insert("job".into(), json!(job));
            crew.push(Value::Object(person));
        }
        // TMDB's document's crew stands in for the rest until the catalogue
        // keeps the whole crew itself — each in its department — which it
        // keeps up to `tmdb.crewLimit`: a work not refreshed since.
        let kept_whole =
            self.item.credits.iter().any(|credit| {
                credit.credit_type == CreditType::Crew || credit.department.is_some()
            });
        if kept_whole {
            return crew;
        }
        for entry in theirs {
            let credit_id = entry.get("credit_id").and_then(Value::as_str);
            if credit_id.is_some_and(|id| listed.contains(id)) || self.withheld.hides_crew(entry) {
                continue;
            }
            crew.push(entry.clone());
        }
        crew
    }

    /// An episode's guests and its crew, as TMDB lists them: the catalogue's
    /// credits of that episode — its guests, its director and its writers —
    /// each with what is known of the person. Not the work's credits, which
    /// are all a person hides: they stay. None for an order's own record of
    /// an episode the work does not hold.
    fn episode_people(&self, spot: &Spot) -> (Vec<Value>, Vec<Value>) {
        let mut guests = Vec::new();
        let mut crew = Vec::new();
        if spot.own {
            return (guests, crew);
        }
        let numbers = (spot.episode.season_number, spot.episode.episode_number);
        for credit in self.episode_credits.get(&numbers).into_iter().flatten() {
            let credit_id = credit.credit_tmdb_id.clone().unwrap_or_default();
            if matches!(credit.credit_type, CreditType::Guest | CreditType::Actor) {
                let mut person = self.person(credit, None, "Acting");
                person.insert(
                    "character".into(),
                    json!(credit.character_name.clone().unwrap_or_default()),
                );
                person.insert("credit_id".into(), json!(credit_id));
                person.insert("order".into(), json!(credit.sort_order));
                guests.push(Value::Object(person));
            } else {
                let (department, job) = department_and_job(credit);
                let mut person = self.person(credit, None, &department);
                person.insert("credit_id".into(), json!(credit_id));
                person.insert("department".into(), json!(department));
                person.insert("job".into(), json!(job));
                crew.push(Value::Object(person));
            }
        }
        (guests, crew)
    }

    /// Who created a series: the catalogue's creators, then TMDB's it does
    /// not keep and nobody hid.
    fn created_by(&self) -> Vec<Value> {
        let theirs: Vec<&Value> = self.their_list("/created_by").iter().collect();
        let mut listed: HashSet<String> = HashSet::new();
        let mut creators: Vec<Value> = Vec::new();
        for credit in self
            .credits_of(&[CreditType::Writer])
            .into_iter()
            .filter(|credit| is_creator(credit))
        {
            let entry = self.their_entry(&theirs, credit);
            let person = self.person(credit, entry, "Writing");
            let credit_id = credit.credit_tmdb_id.clone().unwrap_or_default();
            if !credit_id.is_empty() {
                listed.insert(credit_id.clone());
            }
            creators.push(json!({
                "id": person["id"],
                "credit_id": credit_id,
                "name": person["name"],
                "original_name": person["original_name"],
                "gender": person["gender"],
                "profile_path": person["profile_path"],
            }));
        }
        for entry in theirs {
            let credit_id = entry.get("credit_id").and_then(Value::as_str);
            let hidden = entry
                .get("id")
                .and_then(Value::as_i64)
                .is_some_and(|id| self.withheld.hides(CreditType::Writer, id));
            if !hidden && !credit_id.is_some_and(|id| listed.contains(id)) {
                creators.push(entry.clone());
            }
        }
        creators
    }

    /// `{cast, crew}`.
    fn credits(&self) -> Value {
        json!({ "cast": self.cast(), "crew": self.crew() })
    }

    /// A series' people over all its episodes, as TMDB adds them up: each
    /// person once, with every part they played or every job they had in a
    /// department. Written from the work's credits, the episodes each was
    /// in being what the catalogue does not count: nought.
    fn aggregate_credits(&self) -> Value {
        let mut cast: Vec<Value> = Vec::new();
        for entry in self.cast() {
            let role = json!({
                "credit_id": entry["credit_id"],
                "character": entry["character"],
                "episode_count": 0,
            });
            let id = entry["id"].as_i64().unwrap_or(0);
            if id != 0
                && let Some(Value::Object(kept)) =
                    cast.iter_mut().find(|kept| kept["id"].as_i64() == Some(id))
                && let Some(Value::Array(roles)) = kept.get_mut("roles")
            {
                roles.push(role);
                continue;
            }
            let mut person = person_of(&entry);
            person.insert("roles".into(), json!([role]));
            person.insert("total_episode_count".into(), json!(0));
            person.insert("order".into(), entry["order"].clone());
            cast.push(Value::Object(person));
        }
        let mut crew: Vec<Value> = Vec::new();
        for entry in self.crew() {
            let job = json!({
                "credit_id": entry["credit_id"],
                "job": entry["job"],
                "episode_count": 0,
            });
            let id = entry["id"].as_i64().unwrap_or(0);
            if id != 0
                && let Some(Value::Object(kept)) = crew.iter_mut().find(|kept| {
                    kept["id"].as_i64() == Some(id) && kept["department"] == entry["department"]
                })
                && let Some(Value::Array(jobs)) = kept.get_mut("jobs")
            {
                jobs.push(job);
                continue;
            }
            let mut person = person_of(&entry);
            person.insert("jobs".into(), json!([job]));
            person.insert("department".into(), entry["department"].clone());
            person.insert("total_episode_count".into(), json!(0));
            crew.push(Value::Object(person));
        }
        json!({ "cast": cast, "crew": crew })
    }

    // ─── the work's other lists ──────────────────────────────────────────

    /// The work's videos in the languages the request takes: TMDB's list,
    /// as TMDB orders it. The trailer chosen here — locked, or the one of a
    /// work TMDB has no document of — leads it in every language, as a
    /// picture would: TMDB's own entry for it where TMDB lists it, else one
    /// with nothing but where it is.
    fn videos(&self, filters: &Filters) -> Value {
        // The catalogue's videos, else — a work not refreshed since it
        // keeps them — TMDB's document's.
        let mut kept: Vec<&Video> = self.item.videos.iter().collect();
        kept.sort_by_key(|video| video.sort_order);
        let kept: Vec<Value> = kept.into_iter().map(video_entry).collect();
        let theirs: &[Value] = if kept.is_empty() {
            self.their_list("/videos/results")
        } else {
            &kept
        };
        let mut results: Vec<Value> = theirs
            .iter()
            .filter(|video| filters.takes_video(video.get("iso_639_1").and_then(Value::as_str)))
            .cloned()
            .collect();
        let chosen = (self.item.is_locked("trailerYoutubeId") || self.snapshot.is_none())
            .then_some(self.item.trailer_youtube_id.as_deref())
            .flatten()
            .filter(|key| !key.trim().is_empty());
        if let Some(key) = chosen {
            let listed = |video: &Value| video.get("key").and_then(Value::as_str) == Some(key);
            results.retain(|video| !listed(video));
            let entry = theirs.iter().find(|video| listed(video)).cloned();
            results.insert(
                0,
                entry.unwrap_or_else(|| {
                    json!({
                        "iso_639_1": "",
                        "iso_3166_1": "",
                        "name": "",
                        "key": key,
                        "site": "YouTube",
                        "size": 0,
                        "type": "Trailer",
                        "official": false,
                        "published_at": "",
                        "id": "",
                    })
                }),
            );
        }
        json!({ "results": results })
    }

    /// The work's keywords, as locked, each with TMDB's id where TMDB lists
    /// it — and nought where it does not; else TMDB's list. A film's under
    /// `keywords`, a series' under `results`.
    fn keywords(&self) -> Value {
        let theirs = self.their_list(if self.is_movie() {
            "/keywords/keywords"
        } else {
            "/keywords/results"
        });
        let kept = &self.item.keyword_entities;
        let entry = |id: Option<i64>, name: &str| json!({ "id": id.unwrap_or(0), "name": name });
        let list: Vec<Value> = if !self.item.is_locked("keywords") && !kept.is_empty() {
            kept.iter()
                .map(|keyword| entry(keyword.tmdb_id, &keyword.name))
                .collect()
        } else if !self.item.is_locked("keywords") && !theirs.is_empty() {
            theirs.to_vec()
        } else {
            self.item
                .keywords
                .iter()
                .map(|name| {
                    let id = kept
                        .iter()
                        .find(|keyword| folded(&keyword.name) == folded(name))
                        .and_then(|keyword| keyword.tmdb_id)
                        .or_else(|| {
                            theirs
                                .iter()
                                .find(|k| {
                                    k.get("name")
                                        .and_then(Value::as_str)
                                        .is_some_and(|theirs| folded(theirs) == folded(name))
                                })
                                .and_then(|k| k.get("id").and_then(Value::as_i64))
                        });
                    entry(id, name)
                })
                .collect()
        };
        let key = if self.is_movie() {
            "keywords"
        } else {
            "results"
        };
        json!({ key: list })
    }

    /// The ids other sites file the work under: the catalogue's — IMDb's,
    /// Wikidata's, TVRage's, and the id Sonarr keeps a series under — then
    /// TMDB's document's for the rest.
    fn external_ids(&self) -> Value {
        let their = |key: &str| self.theirs(&format!("/external_ids/{key}")).cloned();
        let text = |value: Option<String>| value.map_or(Value::Null, Value::String);
        let imdb = text(self.item.external_ids.imdb.clone());
        let wikidata = text(self.item.external_ids.wikidata.clone())
            .as_str()
            .map(|q| json!(q))
            .or_else(|| their("wikidata_id"))
            .unwrap_or(Value::Null);
        let or_null = |value: Option<Value>| value.unwrap_or(Value::Null);
        if self.is_movie() {
            return json!({
                "imdb_id": imdb,
                "wikidata_id": wikidata,
                "facebook_id": or_null(their("facebook_id")),
                "instagram_id": or_null(their("instagram_id")),
                "twitter_id": or_null(their("twitter_id")),
            });
        }
        json!({
            "imdb_id": imdb,
            "freebase_mid": or_null(their("freebase_mid")),
            "freebase_id": or_null(their("freebase_id")),
            "tvdb_id": self.tvdb_id,
            "tvrage_id": self.item.external_ids.tvrage,
            "wikidata_id": wikidata,
            "facebook_id": or_null(their("facebook_id")),
            "instagram_id": or_null(their("instagram_id")),
            "twitter_id": or_null(their("twitter_id")),
        })
    }

    /// A film's releases, by country — each with its day, its kind, its
    /// rating and its note — as the catalogue keeps them; what TMDB adds to
    /// one, its rating's descriptors and its language, from TMDB's own.
    fn releases(&self) -> Vec<(String, Vec<Map<String, Value>>)> {
        let theirs = self.their_list("/release_dates/results");
        let rating = self.locked_rating();
        let mut countries: Vec<(String, Vec<Map<String, Value>>)> = Vec::new();
        for release in &self.item.releases {
            let (Some(kind), Some(day)) = (
                release.release_type,
                release.date.as_deref().and_then(|d| d.get(..10)),
            ) else {
                continue;
            };
            let country = release.country.to_ascii_uppercase();
            let code = release_code(kind);
            let their = theirs
                .iter()
                .find(|c| c.get("iso_3166_1").and_then(Value::as_str) == Some(country.as_str()))
                .and_then(|c| c.get("release_dates").and_then(Value::as_array))
                .and_then(|dates| {
                    dates.iter().find(|d| {
                        d.get("type").and_then(Value::as_i64) == Some(code)
                            && d.get("release_date")
                                .and_then(Value::as_str)
                                .is_some_and(|theirs| theirs.starts_with(day))
                    })
                });
            let certification = match &rating {
                Some((of, locked)) if *of == country => locked.clone(),
                _ => release.certification.clone().unwrap_or_default(),
            };
            let mut entry = Map::new();
            entry.insert("certification".into(), json!(certification));
            entry.insert(
                "descriptors".into(),
                their
                    .and_then(|d| d.get("descriptors"))
                    .cloned()
                    .unwrap_or(json!([])),
            );
            entry.insert(
                "iso_639_1".into(),
                their
                    .and_then(|d| d.get("iso_639_1"))
                    .cloned()
                    .unwrap_or(json!("")),
            );
            entry.insert(
                "note".into(),
                json!(release.note.clone().unwrap_or_default()),
            );
            entry.insert("release_date".into(), json!(format!("{day}T00:00:00.000Z")));
            entry.insert("type".into(), json!(code));
            match countries.iter_mut().find(|(of, _)| *of == country) {
                Some((_, dates)) => dates.push(entry),
                None => countries.push((country, vec![entry])),
            }
        }
        countries
    }

    /// The rating a person locked, and the country it is for.
    fn locked_rating(&self) -> Option<(String, String)> {
        if !self.item.is_locked("contentRating") {
            return None;
        }
        Some((
            self.item
                .content_rating_country
                .clone()?
                .to_ascii_uppercase(),
            self.item.content_rating.clone().unwrap_or_default(),
        ))
    }

    /// `{results: [{iso_3166_1, release_dates}]}`.
    fn release_dates(&self) -> Value {
        let results: Vec<Value> = self
            .releases()
            .into_iter()
            .map(|(country, dates)| json!({ "iso_3166_1": country, "release_dates": dates }))
            .collect();
        json!({ "results": results })
    }

    /// TMDB's older form of a film's releases, which Jellyfin, Kodi and Emby
    /// still ask for: one entry a release, its day without the time.
    fn legacy_releases(&self) -> Value {
        let mut countries: Vec<Value> = Vec::new();
        for (country, dates) in self.releases() {
            for date in dates {
                let day = date["release_date"]
                    .as_str()
                    .and_then(|d| d.get(..10))
                    .unwrap_or_default()
                    .to_string();
                countries.push(json!({
                    "certification": date["certification"],
                    "descriptors": date["descriptors"],
                    "iso_3166_1": country,
                    "primary": false,
                    "release_date": day,
                }));
            }
        }
        json!({ "countries": countries })
    }

    /// TMDB's older form of a film's videos, which Kodi and Emby still ask
    /// for: its YouTube videos in the language asked, the trailers first.
    fn trailers(&self, filters: &Filters) -> Value {
        let videos = self.videos(filters);
        let mut youtube: Vec<&Value> = videos["results"]
            .as_array()
            .map(|all| {
                all.iter()
                    .filter(|video| video["site"].as_str() == Some("YouTube"))
                    .collect()
            })
            .unwrap_or_default();
        youtube.sort_by_key(|video| video["type"].as_str() != Some("Trailer"));
        let youtube: Vec<Value> = youtube
            .into_iter()
            .map(|video| {
                json!({
                    "name": video["name"],
                    "size": legacy_size(video["size"].as_i64()),
                    "source": video["key"],
                    "type": video["type"],
                })
            })
            .collect();
        json!({ "quicktime": [], "youtube": youtube })
    }

    /// A series' ratings, by country: the catalogue's, and the descriptors
    /// TMDB adds to one from TMDB's own.
    fn content_ratings(&self) -> Value {
        let theirs = self.their_list("/content_ratings/results");
        let rating = self.locked_rating();
        let mut results: Vec<Value> = Vec::new();
        for release in self
            .item
            .releases
            .iter()
            .filter(|release| release.release_type.is_none())
        {
            let country = release.country.to_ascii_uppercase();
            let certification = match &rating {
                Some((of, locked)) if *of == country => Some(locked.clone()),
                _ => release.certification.clone(),
            };
            let Some(certification) = certification.filter(|c| !c.trim().is_empty()) else {
                continue;
            };
            if results
                .iter()
                .any(|kept| kept["iso_3166_1"].as_str() == Some(country.as_str()))
            {
                continue;
            }
            let descriptors = theirs
                .iter()
                .find(|c| c.get("iso_3166_1").and_then(Value::as_str) == Some(country.as_str()))
                .and_then(|c| c.get("descriptors"))
                .cloned()
                .unwrap_or(json!([]));
            results.push(json!({
                "descriptors": descriptors,
                "iso_3166_1": country,
                "rating": certification,
            }));
        }
        json!({ "results": results })
    }

    /// The work's other titles: the catalogue's, then TMDB's it does not
    /// keep and nobody hid. A film's under `titles`, a series' `results`.
    fn alternative_titles(&self) -> Value {
        let theirs = self.their_list(if self.is_movie() {
            "/alternative_titles/titles"
        } else {
            "/alternative_titles/results"
        });
        let mut listed: HashSet<(String, String)> = HashSet::new();
        let mut titles: Vec<Value> = Vec::new();
        for title in &self.item.alternative_titles {
            let country = title
                .language
                .as_deref()
                .and_then(iso_3166_3_to_2)
                .unwrap_or_default()
                .to_string();
            listed.insert((folded(&title.title), country.clone()));
            titles.push(json!({
                "iso_3166_1": country,
                "title": title.title,
                "type": title.title_type.clone().unwrap_or_default(),
            }));
        }
        for entry in theirs {
            let key = (
                folded(
                    entry
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ),
                entry
                    .get("iso_3166_1")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
            if !self.withheld.hides_title(entry) && listed.insert(key) {
                titles.push(entry.clone());
            }
        }
        let key = if self.is_movie() { "titles" } else { "results" };
        json!({ key: titles })
    }

    /// The work's translations: TMDB's list, each with what a person locked
    /// in its language written over TMDB's text, then the languages the
    /// catalogue holds a text in that TMDB's list has none for.
    fn translations(&self) -> Value {
        let title_key = if self.is_movie() { "title" } else { "name" };
        let locked = |language: &str, field: &str| -> Option<String> {
            self.locked
                .iter()
                .find(|lock| {
                    lock.scope == "item" && lock.field == field && lock.language == language
                })
                .and_then(|lock| lock.value.clone())
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut translations: Vec<Value> = Vec::new();
        for entry in self.their_list("/translations/translations") {
            let mut entry = entry.clone();
            let language = entry
                .get("iso_639_1")
                .and_then(Value::as_str)
                .map(language::normalize)
                .unwrap_or_default();
            seen.insert(language.clone());
            if let Some(Value::Object(data)) = entry.get_mut("data") {
                for (field, key) in [
                    ("title", title_key),
                    ("overview", "overview"),
                    ("tagline", "tagline"),
                ] {
                    if let Some(text) = locked(&language, field) {
                        data.insert(key.into(), json!(text));
                    }
                }
            }
            translations.push(entry);
        }
        for translation in &self.item.translations {
            if !seen.insert(translation.language.clone()) {
                continue;
            }
            let pick = |field: &str, held: &Option<String>| {
                locked(&translation.language, field)
                    .or_else(|| held.clone())
                    .unwrap_or_default()
            };
            let mut data = Map::new();
            if !self.is_movie() {
                data.insert("name".into(), json!(pick("title", &translation.title)));
            }
            data.insert(
                "overview".into(),
                json!(pick("overview", &translation.overview)),
            );
            data.insert("homepage".into(), json!(""));
            data.insert(
                "tagline".into(),
                json!(pick("tagline", &translation.tagline)),
            );
            if self.is_movie() {
                data.insert("runtime".into(), json!(0));
                data.insert("title".into(), json!(pick("title", &translation.title)));
            }
            translations.push(json!({
                "iso_3166_1": "",
                "iso_639_1": language::two_letter(&translation.language),
                "name": "",
                "english_name": "",
                "data": data,
            }));
        }
        json!({ "translations": translations })
    }

    /// The episode groups TMDB lists with a series.
    fn episode_groups(&self) -> Value {
        json!({ "results": self.their_list("/episode_groups/results") })
    }

    // ─── seasons and episodes ────────────────────────────────────────────

    /// The series' episodes as TMDB numbers them, each once, in order: the
    /// catalogue's own where its numbering is TMDB's, the places of the
    /// order that keeps TMDB's numbering ([`orders::served`]), none where
    /// neither is kept here.
    fn spots(&self) -> &[Spot] {
        self.spots.get_or_init(|| {
            let mut spots: Vec<Spot> = match self.numbering {
                Numbering::Own => self
                    .item
                    .episodes
                    .iter()
                    .map(|episode| Spot {
                        season: episode.season_number,
                        number: episode.episode_number,
                        tmdb_id: episode.tmdb_id,
                        episode: episode.clone(),
                        own: false,
                    })
                    .collect(),
                Numbering::Order(order) => orders::served(&self.item, order)
                    .into_iter()
                    .map(|placed| Spot {
                        season: placed.season_number,
                        number: placed.episode_number,
                        tmdb_id: placed.tmdb_id,
                        own: placed.own,
                        episode: placed.episode.into_owned(),
                    })
                    .collect(),
                Numbering::Theirs => Vec::new(),
            };
            spots.sort_by_key(|spot| (spot.season, spot.number));
            spots
        })
    }

    /// Every season number TMDB's numbering of the series holds here, in
    /// order: the catalogue's seasons too, where the numbering is its own.
    fn season_numbers(&self) -> Vec<i32> {
        let own = matches!(self.numbering, Numbering::Own);
        let mut numbers: Vec<i32> = self
            .item
            .seasons
            .iter()
            .filter(|_| own)
            .map(|s| s.season_number)
            .chain(self.spots().iter().map(|spot| spot.season))
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        numbers
    }

    /// The catalogue's season of a number, where its numbering is TMDB's:
    /// in TMDB's numbering kept apart, its seasons are TheTVDB's, and none
    /// is TMDB's of that number.
    fn season_at(&self, number: i32) -> Option<&Season> {
        if !matches!(self.numbering, Numbering::Own) {
            return None;
        }
        self.item
            .seasons
            .iter()
            .find(|season| season.season_number == number)
    }

    /// The episodes of a season, numbered as TMDB numbers them.
    fn episodes_of(&self, season: i32) -> Vec<&Spot> {
        self.spots()
            .iter()
            .filter(|spot| spot.season == season)
            .collect()
    }

    fn episode_at(&self, season: i32, number: i32) -> Option<&Spot> {
        self.spots()
            .iter()
            .find(|spot| spot.season == season && spot.number == number)
    }

    fn season_locked(&self, number: i32, field: &str) -> bool {
        let key = format!("season:{number}/{field}");
        self.item.locked_fields.contains(&key)
    }

    fn episode_locked(&self, episode: &Episode, field: &str) -> bool {
        let key = format!(
            "episode:{}x{}/{field}",
            episode.season_number, episode.episode_number
        );
        self.item.locked_fields.contains(&key)
    }

    /// A season's name in the language asked — `season` the catalogue's of
    /// it, `number` the season's as the document numbers it: as held, in
    /// the catalogue's language, else TMDB's name for it there, else
    /// `Season N`; in another, as locked in it, else as translated, else
    /// what TMDB names a season in a language it holds no name in.
    fn season_name(&self, season: Option<&Season>, number: i32) -> String {
        let fallback = || {
            if number == 0 {
                "Specials".to_string()
            } else {
                format!("Season {number}")
            }
        };
        let locked = season.is_some_and(|s| self.season_locked(s.season_number, "title"));
        if self.stored() || locked {
            let theirs = || {
                self.their_season(number, season)
                    .and_then(|s| s.get("name").and_then(Value::as_str))
                    .and_then(|name| some_text(Some(name)))
            };
            return season
                .and_then(|s| some_text(s.title.as_deref()))
                .or_else(|| self.stored().then(theirs).flatten())
                .unwrap_or_else(fallback);
        }
        season
            .and_then(|s| {
                s.translations
                    .iter()
                    .find(|t| t.language == self.language.asked)
            })
            .and_then(|t| some_text(t.title.as_deref()))
            .unwrap_or_else(fallback)
    }

    /// A season's overview, as its name is, and empty where none is held in
    /// the language asked.
    fn season_overview(&self, season: Option<&Season>) -> String {
        let locked = season.is_some_and(|s| self.season_locked(s.season_number, "overview"));
        if self.stored() || locked {
            return season.and_then(|s| s.overview.clone()).unwrap_or_default();
        }
        season
            .and_then(|s| {
                s.translations
                    .iter()
                    .find(|t| t.language == self.language.asked)
            })
            .and_then(|t| t.overview.clone())
            .unwrap_or_default()
    }

    /// TMDB's entry for a season in its series' document: by the season's
    /// TMDB id, else by its number.
    fn their_season(&self, number: i32, season: Option<&Season>) -> Option<&'a Value> {
        let seasons = self.their_list("/seasons");
        season
            .and_then(|s| s.tmdb_id)
            .and_then(|id| {
                seasons
                    .iter()
                    .find(|entry| entry.get("id").and_then(Value::as_i64) == Some(id))
            })
            .or_else(|| {
                seasons.iter().find(|entry| {
                    entry.get("season_number").and_then(Value::as_i64) == Some(i64::from(number))
                })
            })
    }

    /// A season's id: TMDB's, else a negative one made of the catalogue's
    /// own id for it, which no other season is given and TMDB never uses.
    fn season_id(&self, number: i32) -> i64 {
        let season = self.season_at(number);
        let theirs = || {
            self.their_season(number, None)
                .filter(|_| matches!(self.numbering, Numbering::Order(_)))
                .and_then(|entry| entry.get("id").and_then(Value::as_i64))
        };
        season
            .and_then(|s| s.tmdb_id)
            .or_else(theirs)
            .unwrap_or_else(|| {
                let names = season.map_or_else(
                    || format!("{}:season:{number}", self.item.id),
                    |s| format!("season:{}", s.id),
                );
                crate::service::ids::own_episode(&names)
            })
    }

    fn season_poster(&self, number: i32) -> Value {
        let Some(season) = self.season_at(number) else {
            // TMDB's own, of the season as it numbers it.
            return self
                .their_season(number, None)
                .filter(|_| matches!(self.numbering, Numbering::Order(_)))
                .and_then(|entry| entry.get("poster_path"))
                .cloned()
                .unwrap_or(Value::Null);
        };
        let chosen = season.primary_images.poster.as_deref().and_then(|chosen| {
            season
                .images
                .iter()
                .find(|image| image.id == chosen)
                .and_then(|image| self.pictures.path(&image.url))
        });
        chosen
            .or_else(|| {
                season
                    .images
                    .iter()
                    .find_map(|image| self.pictures.path(&image.url))
            })
            .map_or(Value::Null, Value::String)
    }

    /// The day a season began: as held, else its first episode's.
    fn season_air_date(&self, number: i32) -> Value {
        self.season_at(number)
            .and_then(|s| some_text(s.air_date.as_deref()))
            .or_else(|| {
                self.episodes_of(number)
                    .iter()
                    .filter_map(|spot| some_text(spot.episode.air_date.as_deref()))
                    .min()
            })
            .map_or(Value::Null, Value::String)
    }

    /// An episode's name and overview in the language asked, the episode
    /// numbered `shown` in the document — TMDB's number, where TMDB numbers
    /// the series otherwise: as held, in the catalogue's language; in
    /// another, as locked in it, else as held in it, else `Episode N` and
    /// nothing, as TMDB answers a language it holds nothing in.
    fn episode_texts_as(&self, episode: &Episode, shown: i32) -> (String, String) {
        let placeholder = || format!("Episode {shown}");
        if self.stored() {
            return (
                some_text(Some(&episode.title)).unwrap_or_else(placeholder),
                episode.overview.clone().unwrap_or_default(),
            );
        }
        let held = self
            .episode_texts
            .get(&(episode.season_number, episode.episode_number));
        let name = if self.episode_locked(episode, "title") {
            episode.title.clone()
        } else {
            held.and_then(|t| some_text(t.title.as_deref()))
                .unwrap_or_else(placeholder)
        };
        let overview = if self.episode_locked(episode, "overview") {
            episode.overview.clone().unwrap_or_default()
        } else {
            held.and_then(|t| t.overview.clone()).unwrap_or_default()
        };
        (name, overview)
    }

    /// An episode's id: TMDB's, else a negative one made of the catalogue's.
    fn episode_id(&self, spot: &Spot) -> i64 {
        spot.tmdb_id.unwrap_or_else(|| {
            crate::service::ids::own_episode(&format!("episode:{}", spot.episode.id))
        })
    }

    /// A spot's name and overview in the language asked: the episode held,
    /// as [`Self::episode_texts_as`] gives them; an order's record of one the
    /// work does not hold, as it records them in the catalogue's language,
    /// and as TMDB names an episode in any other.
    fn spot_texts(&self, spot: &Spot) -> (String, String) {
        if !spot.own {
            return self.episode_texts_as(&spot.episode, spot.number);
        }
        let placeholder = format!("Episode {}", spot.number);
        if !self.stored() {
            return (placeholder, String::new());
        }
        (
            some_text(Some(&spot.episode.title)).unwrap_or(placeholder),
            spot.episode.overview.clone().unwrap_or_default(),
        )
    }

    /// An episode as TMDB writes one, without its people.
    fn episode_fields(&self, spot: &Spot) -> Map<String, Value> {
        let episode = &spot.episode;
        let (name, overview) = self.spot_texts(spot);
        let (average, count) = episode
            .rating
            .map_or((0.0, 0), |rating| (rating.value, rating.votes));
        let mut fields = Map::new();
        fields.insert(
            "air_date".into(),
            some_text(episode.air_date.as_deref()).map_or(Value::Null, Value::String),
        );
        fields.insert("episode_number".into(), json!(spot.number));
        fields.insert(
            "episode_type".into(),
            json!(episode_type(episode.finale_type.as_deref())),
        );
        fields.insert("id".into(), json!(self.episode_id(spot)));
        fields.insert("name".into(), json!(name));
        fields.insert("overview".into(), json!(overview));
        fields.insert("production_code".into(), json!(""));
        fields.insert("runtime".into(), json!(episode.runtime));
        fields.insert("season_number".into(), json!(spot.season));
        fields.insert("still_path".into(), self.path(episode.image.as_deref()));
        fields.insert("vote_average".into(), json!(average));
        fields.insert("vote_count".into(), json!(count));
        fields
    }

    /// The last episode aired and the next to air, as the catalogue numbers
    /// them — or, numbered otherwise, as TMDB's own document has them, with
    /// what the catalogue's episode of the same TMDB id says laid over.
    fn aired(&self) -> (Value, Value) {
        let with_show = |mut fields: Map<String, Value>| {
            fields.insert("show_id".into(), json!(self.id));
            Value::Object(fields)
        };
        if self.numbers_seasons() {
            let today = self.today.format("%Y-%m-%d").to_string();
            let dated = || {
                self.spots().iter().filter_map(|spot| {
                    some_text(spot.episode.air_date.as_deref())
                        .map(|day| (day, spot.season, spot.number, spot))
                })
            };
            let last = dated()
                .filter(|(day, ..)| *day <= today)
                .max_by(|a, b| (&a.0, a.1, a.2).cmp(&(&b.0, b.1, b.2)))
                .map(|(.., e)| with_show(self.episode_fields(e)));
            let next = dated()
                .filter(|(day, ..)| *day > today)
                .min_by(|a, b| (&a.0, a.1, a.2).cmp(&(&b.0, b.1, b.2)))
                .map(|(.., e)| with_show(self.episode_fields(e)));
            return (last.unwrap_or(Value::Null), next.unwrap_or(Value::Null));
        }
        let theirs = |key: &str| {
            let entry = self.theirs(&format!("/{key}"))?.as_object()?;
            let mut entry = entry.clone();
            let id = entry.get("id").and_then(Value::as_i64);
            let ours = id.and_then(|id| self.item.episodes.iter().find(|e| e.tmdb_id == Some(id)));
            let shown = entry
                .get("episode_number")
                .and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok());
            match ours {
                Some(episode) => {
                    let (name, overview) =
                        self.episode_texts_as(episode, shown.unwrap_or(episode.episode_number));
                    entry.insert("name".into(), json!(name));
                    entry.insert("overview".into(), json!(overview));
                    for (field, key) in [("airDate", "air_date"), ("runtime", "runtime")] {
                        if self.episode_locked(episode, field) {
                            let value = match key {
                                "air_date" => json!(episode.air_date),
                                _ => json!(episode.runtime),
                            };
                            entry.insert(key.into(), value);
                        }
                    }
                    if self.episode_locked(episode, "image") {
                        entry.insert("still_path".into(), self.path(episode.image.as_deref()));
                    }
                }
                None if !self.stored() => {
                    let number = shown.unwrap_or_default();
                    entry.insert("name".into(), json!(format!("Episode {number}")));
                    entry.insert("overview".into(), json!(""));
                }
                None => {}
            }
            Some(Value::Object(entry))
        };
        (
            theirs("last_episode_to_air").unwrap_or(Value::Null),
            theirs("next_episode_to_air").unwrap_or(Value::Null),
        )
    }

    /// A series' seasons as its own document lists them.
    fn season_list(&self) -> Vec<Value> {
        if !self.numbers_seasons() {
            return self
                .their_list("/seasons")
                .iter()
                .map(|entry| self.their_season_entry(entry))
                .collect();
        }
        self.season_numbers()
            .into_iter()
            .map(|number| {
                let vote = self
                    .their_season(number, self.season_at(number))
                    .and_then(|entry| entry.get("vote_average"))
                    .cloned()
                    .unwrap_or(json!(0.0));
                json!({
                    "air_date": self.season_air_date(number),
                    "episode_count": self.episodes_of(number).len(),
                    "id": self.season_id(number),
                    "name": self.season_name(self.season_at(number), number),
                    "overview": self.season_overview(self.season_at(number)),
                    "poster_path": self.season_poster(number),
                    "season_number": number,
                    "vote_average": vote,
                })
            })
            .collect()
    }

    /// One of TMDB's seasons in its series' document, numbered as TMDB
    /// numbers it: with the locks of the catalogue's season of the same
    /// TMDB id — or of the same number where it knows none — and its texts
    /// in the language asked.
    fn their_season_entry(&self, entry: &Value) -> Value {
        let mut entry = entry.clone();
        let Some(number) = entry
            .get("season_number")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
        else {
            return entry;
        };
        let id = entry.get("id").and_then(Value::as_i64);
        let ours = self
            .item
            .seasons
            .iter()
            .find(|s| s.tmdb_id.is_some() && s.tmdb_id == id)
            .or_else(|| {
                self.item
                    .seasons
                    .iter()
                    .find(|s| s.season_number == number && s.tmdb_id.is_none())
            });
        let Value::Object(map) = &mut entry else {
            return entry;
        };
        if let Some(season) = ours {
            let locked = |field: &str| self.season_locked(season.season_number, field);
            if locked("title") {
                map.insert("name".into(), json!(season.title));
            }
            if locked("overview") {
                map.insert(
                    "overview".into(),
                    json!(season.overview.clone().unwrap_or_default()),
                );
            }
            if locked("airDate") {
                map.insert("air_date".into(), json!(season.air_date));
            }
            if locked("primaryPoster") {
                map.insert(
                    "poster_path".into(),
                    self.season_poster(season.season_number),
                );
            }
        }
        if !self.stored() {
            map.insert("name".into(), json!(self.season_name(ours, number)));
            map.insert("overview".into(), json!(self.season_overview(ours)));
        }
        entry
    }
}

// ─── the documents ───────────────────────────────────────────────────────────

/// A film's own document, `/3/movie/{id}`, without what is appended to it.
pub fn movie(work: &Work) -> Value {
    let (vote_average, vote_count) = work.votes();
    let collection = work
        .theirs("/belongs_to_collection")
        .filter(|collection| {
            collection.get("id").and_then(Value::as_i64) == work.item.collection_tmdb_id
        })
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "adult": work.item.is_adult,
        "backdrop_path": work.lead(CoverType::Fanart),
        "belongs_to_collection": collection,
        "budget": work.theirs("/budget").cloned().unwrap_or(json!(0)),
        "genres": work.genre_objects(),
        "homepage": work.homepage(),
        "id": work.id,
        "imdb_id": work.item.external_ids.imdb,
        "origin_country": work.origin_country(),
        "original_language": work.original_language(),
        "original_title": work.original_title(),
        "overview": work.text("overview"),
        "popularity": work.popularity(),
        "poster_path": work.lead(CoverType::Poster),
        "production_companies": work.production_companies(),
        "production_countries": work.production_countries(),
        "release_date": some_text(work.item.in_cinemas.as_deref())
            .and_then(|day| day.get(..10).map(str::to_string))
            .unwrap_or_default(),
        "revenue": work.theirs("/revenue").cloned().unwrap_or(json!(0)),
        "runtime": work.item.runtime
            .or_else(|| work.theirs("/runtime").and_then(Value::as_i64).and_then(|r| i32::try_from(r).ok()))
            .unwrap_or(0),
        "softcore": work.flag("softcore"),
        "spoken_languages": work.spoken_languages(),
        "status": movie_status(work),
        "tagline": work.text("tagline"),
        "title": work.title(),
        "video": work.flag("video"),
        "vote_average": vote_average,
        "vote_count": vote_count,
    })
}

/// A series' own document, `/3/tv/{id}`, without what is appended to it.
pub fn tv(work: &Work) -> Value {
    let (vote_average, vote_count) = work.votes();
    let (last, next) = work.aired();
    let seasons = work.season_list();
    let own = work.numbers_seasons();
    let counted = |key: &str, own_count: usize| -> Value {
        if own {
            json!(own_count)
        } else {
            work.theirs(&format!("/{key}"))
                .cloned()
                .unwrap_or(json!(own_count))
        }
    };
    let episode_count = work.spots().iter().filter(|spot| spot.season > 0).count();
    let season_count = work.season_numbers().into_iter().filter(|n| *n > 0).count();
    let run_time = if work.item.is_locked("runtime") || work.snapshot.is_none() {
        Value::Array(work.item.runtime.into_iter().map(|r| json!(r)).collect())
    } else {
        work.theirs("/episode_run_time")
            .cloned()
            .unwrap_or(json!([]))
    };
    let in_production = work
        .theirs("/in_production")
        .and_then(Value::as_bool)
        .unwrap_or(matches!(
            work.item.status.as_deref(),
            Some("continuing" | "upcoming")
        ));
    let languages = work.theirs("/languages").cloned().unwrap_or_else(|| {
        let own = work.original_language();
        Value::Array((!own.is_empty()).then(|| json!(own)).into_iter().collect())
    });
    json!({
        "adult": work.item.is_adult,
        "backdrop_path": work.lead(CoverType::Fanart),
        "created_by": work.created_by(),
        "episode_run_time": run_time,
        "first_air_date": some_text(work.item.first_aired.as_deref()).unwrap_or_default(),
        "genres": work.genre_objects(),
        "homepage": work.homepage(),
        "id": work.id,
        "in_production": in_production,
        "languages": languages,
        "last_air_date": some_text(work.item.last_aired.as_deref()),
        "last_episode_to_air": last,
        "name": work.title(),
        "next_episode_to_air": next,
        "networks": work.networks(),
        "number_of_episodes": counted("number_of_episodes", episode_count),
        "number_of_seasons": counted("number_of_seasons", season_count),
        "origin_country": work.origin_country(),
        "original_language": work.original_language(),
        "original_name": work.original_title(),
        "overview": work.text("overview"),
        "popularity": work.popularity(),
        "poster_path": work.lead(CoverType::Poster),
        "production_companies": work.production_companies(),
        "production_countries": work.production_countries(),
        "seasons": seasons,
        "softcore": work.flag("softcore"),
        "spoken_languages": work.spoken_languages(),
        "status": tv_status(work),
        "tagline": work.text("tagline"),
        "type": work.theirs("/type").cloned().unwrap_or(json!("")),
        "vote_average": vote_average,
        "vote_count": vote_count,
    })
}

/// What a document of a work is asked to carry, by its name in
/// `append_to_response`, or the path below the work's own.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// Written here, as TMDB writes the document at that path on its own —
    /// its `id` among it where TMDB puts one ([`appended`] takes it out).
    Doc(Value),
    /// Another resource of TMDB's — where to watch, reviews, lists, the
    /// works like this one — relayed where TMDB knows the work; else its
    /// empty shape, where TMDB has one for it.
    Elsewhere { empty: Option<Value> },
    /// Nothing TMDB has at that path: dropped from a document it is asked
    /// with, as TMDB drops it.
    Unknown,
}

/// A page of results with nothing on it.
pub fn empty_page() -> Value {
    json!({ "page": 1, "results": [], "total_pages": 0, "total_results": 0 })
}

/// The empty shape of another resource's document, where TMDB has one.
fn elsewhere(name: &str, id: i64) -> Part {
    let empty = match name {
        "watch/providers" => Some(json!({ "id": id, "results": {} })),
        "reviews" | "lists" => {
            let mut page = empty_page();
            page["id"] = json!(id);
            Some(page)
        }
        "similar" | "recommendations" => Some(empty_page()),
        "changes" => Some(json!({ "changes": [] })),
        "account_states" | "rating" | "screened_theatrically" => None,
        _ => return Part::Unknown,
    };
    Part::Elsewhere { empty }
}

/// What a work's document carries under `name`, appended or on its own.
pub fn title_part(work: &Work, name: &str, filters: &Filters) -> Part {
    let id = work.id;
    let with_id = |mut value: Value| {
        if let Value::Object(map) = &mut value {
            map.shift_insert(0, "id".into(), json!(id));
        }
        Part::Doc(value)
    };
    let movie = work.is_movie();
    match name {
        "credits" => with_id(work.credits()),
        "casts" if movie => with_id(work.credits()),
        "aggregate_credits" if !movie => with_id(work.aggregate_credits()),
        "images" => with_id(work.images(filters)),
        "videos" => with_id(work.videos(filters)),
        "keywords" => with_id(work.keywords()),
        "external_ids" => with_id(work.external_ids()),
        "release_dates" if movie => with_id(work.release_dates()),
        "releases" if movie => with_id(work.legacy_releases()),
        "trailers" if movie => with_id(work.trailers(filters)),
        "content_ratings" if !movie => with_id(work.content_ratings()),
        "alternative_titles" => with_id(work.alternative_titles()),
        "translations" => with_id(work.translations()),
        "episode_groups" if !movie => with_id(work.episode_groups()),
        // The works TMDB recommends beside it are other works: asked of
        // TMDB, where TMDB knows the work, else what TMDB's document of it
        // kept, else none.
        "recommendations" => Part::Elsewhere {
            empty: Some(
                work.theirs("/recommendations")
                    .filter(|_| work.stored())
                    .cloned()
                    .unwrap_or_else(empty_page),
            ),
        },
        "watch/providers" | "reviews" | "similar" | "lists" | "changes" | "account_states" => {
            elsewhere(name, id)
        }
        "rating" | "screened_theatrically" if !movie => elsewhere(name, id),
        _ => Part::Unknown,
    }
}

/// A season's own document, `/3/tv/{id}/season/{n}`, as the catalogue
/// numbers it: none where it holds no such season.
pub fn season(work: &Work, number: i32) -> Option<Value> {
    if !work.has_season(number) {
        return None;
    }
    let episodes: Vec<Value> = work
        .episodes_of(number)
        .into_iter()
        .map(|episode| {
            let mut fields = work.episode_fields(episode);
            let (guests, crew) = work.episode_people(episode);
            fields.insert("show_id".into(), json!(work.id));
            fields.insert("crew".into(), json!(crew));
            fields.insert("guest_stars".into(), json!(guests));
            Value::Object(fields)
        })
        .collect();
    let vote = work
        .their_season(number, work.season_at(number))
        .and_then(|entry| entry.get("vote_average"))
        .cloned()
        .unwrap_or(json!(0.0));
    Some(json!({
        "_id": "",
        "air_date": work.season_air_date(number),
        "episodes": episodes,
        "id": work.season_id(number),
        "name": work.season_name(work.season_at(number), number),
        "networks": work.networks(),
        "overview": work.season_overview(work.season_at(number)),
        "poster_path": work.season_poster(number),
        "season_number": number,
        "vote_average": vote,
    }))
}

/// What a season's document carries under `name`. Its people are not the
/// catalogue's to give — a season's regulars, its crew — and are listed
/// empty; its ids are TheTVDB's of it, as held.
pub fn season_part(work: &Work, number: i32, name: &str, _filters: &Filters) -> Part {
    let id = work.season_id(number);
    let doc = |value: Value| {
        let mut value = value;
        if let Value::Object(map) = &mut value {
            map.shift_insert(0, "id".into(), json!(id));
        }
        Part::Doc(value)
    };
    match name {
        "credits" | "aggregate_credits" => doc(json!({ "cast": [], "crew": [] })),
        "images" => {
            let own: Vec<&Image> = work
                .season_at(number)
                .into_iter()
                .flat_map(|season| season.images.iter())
                .collect();
            // In TMDB's numbering kept apart, the season's poster TMDB's
            // document gives it: the catalogue's seasons are TheTVDB's.
            let theirs: Vec<Value> = match work.season_at(number) {
                Some(_) => Vec::new(),
                None => work
                    .season_poster(number)
                    .as_str()
                    .map(|path| json!({ "file_path": path }))
                    .into_iter()
                    .collect(),
            };
            let posters = work.picture_list(
                "posters",
                &[CoverType::Poster],
                &theirs,
                &own,
                &Filters::all(),
            );
            doc(json!({ "posters": posters }))
        }
        "external_ids" => doc(json!({
            "freebase_mid": null,
            "freebase_id": null,
            "tvdb_id": work.season_at(number).and_then(|s| s.tvdb_id),
            "tvrage_id": null,
            "wikidata_id": null,
        })),
        "videos" => doc(json!({ "results": [] })),
        "translations" => doc(json!({ "translations": season_translations(work, number) })),
        "watch/providers" => elsewhere(name, id),
        _ => Part::Unknown,
    }
}

/// A season's translations: those the catalogue holds of it.
fn season_translations(work: &Work, number: i32) -> Vec<Value> {
    let Some(season) = work.season_at(number) else {
        return Vec::new();
    };
    season
        .translations
        .iter()
        .map(|t| {
            json!({
                "iso_3166_1": "",
                "iso_639_1": language::two_letter(&t.language),
                "name": "",
                "english_name": "",
                "data": {
                    "name": t.title.clone().unwrap_or_default(),
                    "overview": t.overview.clone().unwrap_or_default(),
                },
            })
        })
        .collect()
}

/// An episode's own document, `/3/tv/{id}/season/{n}/episode/{e}`, as the
/// catalogue numbers it: none where it holds no such episode.
pub fn episode(work: &Work, season: i32, number: i32) -> Option<Value> {
    if !work.has_episode(season, number) {
        return None;
    }
    let spot = work.episode_at(season, number)?;
    let mut fields = work.episode_fields(spot);
    let (guests, crew) = work.episode_people(spot);
    fields.insert("crew".into(), json!(crew));
    fields.insert("guest_stars".into(), json!(guests));
    Some(Value::Object(fields))
}

/// What an episode's document carries under `name`.
pub fn episode_part(work: &Work, season: i32, number: i32, name: &str, _filters: &Filters) -> Part {
    let Some(spot) = work.episode_at(season, number) else {
        return Part::Unknown;
    };
    let id = work.episode_id(spot);
    let episode = &spot.episode;
    let doc = |value: Value| {
        let mut value = value;
        if let Value::Object(map) = &mut value {
            map.shift_insert(0, "id".into(), json!(id));
        }
        Part::Doc(value)
    };
    match name {
        // The series' regulars in it are not the catalogue's to say.
        "credits" => {
            let (guests, crew) = work.episode_people(spot);
            doc(json!({ "cast": [], "crew": crew, "guest_stars": guests }))
        }
        "images" => {
            let stills: Vec<Value> = episode
                .image
                .as_deref()
                .and_then(|address| {
                    let path = work.pictures.path(address)?;
                    let (width, height) = work.pictures.size(address).unwrap_or((0, 0));
                    Some(json!({
                        "aspect_ratio": aspect(width, height),
                        "height": height,
                        "iso_3166_1": null,
                        "iso_639_1": null,
                        "file_path": path,
                        "vote_average": 0.0,
                        "vote_count": 0,
                        "width": width,
                    }))
                })
                .into_iter()
                .collect();
            doc(json!({ "stills": stills }))
        }
        "external_ids" => doc(json!({
            "freebase_mid": null,
            "freebase_id": null,
            "imdb_id": null,
            "tvdb_id": episode.tvdb_id,
            "tvrage_id": null,
            "wikidata_id": null,
        })),
        "videos" => doc(json!({ "results": [] })),
        "translations" => doc(json!({ "translations": [] })),
        _ => Part::Unknown,
    }
}

/// A document as it is appended to another: as on its own, less its `id`.
pub fn appended(mut standalone: Value) -> Value {
    if let Value::Object(map) = &mut standalone {
        map.shift_remove("id");
    }
    standalone
}

/// What TMDB answers for a path it has nothing at: status 34.
pub fn not_found() -> Value {
    json!({
        "success": false,
        "status_code": 34,
        "status_message": "The resource you requested could not be found.",
    })
}

/// A work as a page of results lists it — a search's, `find`'s — with its
/// kind beside it where the page lists several (`media_type`).
pub fn result(work: &Work, media_type: bool) -> Value {
    let (vote_average, vote_count) = work.votes();
    let mut result = Map::new();
    result.insert("adult".into(), json!(work.item.is_adult));
    result.insert("backdrop_path".into(), work.lead(CoverType::Fanart));
    result.insert("genre_ids".into(), work.genre_ids());
    result.insert("id".into(), json!(work.id));
    if media_type {
        result.insert(
            "media_type".into(),
            json!(if work.is_movie() { "movie" } else { "tv" }),
        );
    }
    result.insert("original_language".into(), json!(work.original_language()));
    if work.is_movie() {
        result.insert("original_title".into(), json!(work.original_title()));
    } else {
        result.insert("origin_country".into(), work.origin_country());
        result.insert("original_name".into(), json!(work.original_title()));
        result.insert(
            "first_air_date".into(),
            json!(some_text(work.item.first_aired.as_deref()).unwrap_or_default()),
        );
    }
    result.insert("overview".into(), json!(work.text("overview")));
    result.insert("popularity".into(), work.popularity());
    result.insert("poster_path".into(), work.lead(CoverType::Poster));
    if work.is_movie() {
        result.insert(
            "release_date".into(),
            json!(
                some_text(work.item.in_cinemas.as_deref())
                    .and_then(|day| day.get(..10).map(str::to_string))
                    .unwrap_or_default()
            ),
        );
        result.insert("title".into(), json!(work.title()));
        result.insert("video".into(), json!(work.flag("video")));
    } else {
        result.insert("name".into(), json!(work.title()));
    }
    result.insert("softcore".into(), json!(work.flag("softcore")));
    result.insert("vote_average".into(), vote_average);
    result.insert("vote_count".into(), vote_count);
    Value::Object(result)
}

/// `/3/find/{id}` answered with works held here: films, and series.
pub fn found(movies: Vec<Value>, series: Vec<Value>) -> Value {
    json!({
        "movie_results": movies,
        "person_results": [],
        "tv_results": series,
        "tv_episode_results": [],
        "tv_season_results": [],
    })
}

/// Whether the catalogue numbers a series' episodes as TMDB's own document
/// does: as many episodes in each season TMDB lists, and none in a season it
/// does not. What tells TMDB's numbering from a group of TMDB's that the
/// series was numbered by while TheTVDB did not answer for it.
pub fn numbered_alike(item: &MediaItem, snapshot: Option<&Value>) -> bool {
    let Some(seasons) = snapshot
        .and_then(|document| document.get("seasons"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    let mut counts: HashMap<i64, usize> = HashMap::new();
    for episode in &item.episodes {
        *counts.entry(i64::from(episode.season_number)).or_default() += 1;
    }
    let mut listed: HashSet<i64> = HashSet::new();
    for season in seasons {
        let (Some(number), Some(count)) = (
            season.get("season_number").and_then(Value::as_i64),
            season.get("episode_count").and_then(Value::as_u64),
        ) else {
            return false;
        };
        listed.insert(number);
        if counts.get(&number).copied().unwrap_or(0) as u64 != count {
            return false;
        }
    }
    counts.keys().all(|number| listed.contains(number))
}

// ─── vocabulary ──────────────────────────────────────────────────────────────

/// TMDB's genres, by id, as TMDB names them in English and in French.
const MOVIE_GENRES: &[(i64, &str, &str)] = &[
    (28, "Action", "Action"),
    (12, "Adventure", "Aventure"),
    (16, "Animation", "Animation"),
    (35, "Comedy", "Comédie"),
    (80, "Crime", "Crime"),
    (99, "Documentary", "Documentaire"),
    (18, "Drama", "Drame"),
    (10751, "Family", "Familial"),
    (14, "Fantasy", "Fantastique"),
    (36, "History", "Histoire"),
    (27, "Horror", "Horreur"),
    (10402, "Music", "Musique"),
    (9648, "Mystery", "Mystère"),
    (10749, "Romance", "Romance"),
    (878, "Science Fiction", "Science-Fiction"),
    (10770, "TV Movie", "Téléfilm"),
    (53, "Thriller", "Thriller"),
    (10752, "War", "Guerre"),
    (37, "Western", "Western"),
];

const TV_GENRES: &[(i64, &str, &str)] = &[
    (10759, "Action & Adventure", "Action & Adventure"),
    (16, "Animation", "Animation"),
    (35, "Comedy", "Comédie"),
    (80, "Crime", "Crime"),
    (99, "Documentary", "Documentaire"),
    (18, "Drama", "Drame"),
    (10751, "Family", "Familial"),
    (10762, "Kids", "Kids"),
    (9648, "Mystery", "Mystère"),
    (10763, "News", "News"),
    (10764, "Reality", "Reality"),
    (10765, "Sci-Fi & Fantasy", "Science-Fiction & Fantastique"),
    (10766, "Soap", "Soap"),
    (10767, "Talk", "Talk"),
    (10768, "War & Politics", "War & Politics"),
    (37, "Western", "Western"),
];

fn genre_table(kind: MediaKind) -> &'static [(i64, &'static str, &'static str)] {
    match kind {
        MediaKind::Movie => MOVIE_GENRES,
        MediaKind::Series => TV_GENRES,
    }
}

/// TMDB's code for how a film came out in a country.
const fn release_code(kind: ReleaseKind) -> i64 {
    match kind {
        ReleaseKind::Premiere => 1,
        ReleaseKind::Limited => 2,
        ReleaseKind::Theatrical => 3,
        ReleaseKind::Digital => 4,
        ReleaseKind::Physical => 5,
        ReleaseKind::Tv => 6,
    }
}

/// How TMDB's older list of trailers says a video's size: `HD` for 720
/// lines and up to 1080, `HQ` for 480, `SD` below; nothing past 1080.
fn legacy_size(lines: Option<i64>) -> Value {
    match lines {
        Some(720..=1080) => json!("HD"),
        Some(480..=719) => json!("HQ"),
        Some(1..=479) => json!("SD"),
        _ => Value::Null,
    }
}

/// TMDB's kind of episode, from the catalogue's word on the finale.
fn episode_type(finale: Option<&str>) -> &'static str {
    match finale {
        Some("series" | "season") => "finale",
        Some("midseason") => "mid_season",
        _ => "standard",
    }
}

/// A series' status in TMDB's words: TMDB's own where it says what the
/// catalogue does — `Canceled` is ended too — else the catalogue's, in
/// TMDB's words.
fn tv_status(work: &Work) -> String {
    let theirs = work.theirs("/status").and_then(Value::as_str);
    let says = |theirs: &str, ours: &str| match theirs {
        "Ended" | "Canceled" | "Cancelled" => ours == "ended",
        "Planned" | "Pilot" => ours == "upcoming",
        "In Production" => ours == "upcoming" || ours == "continuing",
        _ => ours == "continuing",
    };
    match (work.item.status.as_deref(), theirs) {
        (Some(ours), Some(theirs)) if says(theirs, ours) => theirs.to_string(),
        (Some("ended"), _) => "Ended".into(),
        (Some("upcoming"), _) => "Planned".into(),
        (Some("continuing"), _) => "Returning Series".into(),
        (_, theirs) => theirs.unwrap_or_default().to_string(),
    }
}

/// A film's status in TMDB's words, as a series' is.
fn movie_status(work: &Work) -> String {
    let theirs = work.theirs("/status").and_then(Value::as_str);
    let says = |theirs: &str, ours: &str| match theirs {
        "Released" => ours == "released" || ours == "inCinemas",
        "Post Production" | "In Production" | "Planned" => ours == "announced",
        "Canceled" | "Cancelled" | "Rumored" => ours == "tba",
        _ => false,
    };
    match (work.item.status.as_deref(), theirs) {
        (Some(ours), Some(theirs)) if says(theirs, ours) => theirs.to_string(),
        (Some("released" | "inCinemas"), _) => "Released".into(),
        (Some("announced"), _) => "Planned".into(),
        (Some("tba"), _) => "Rumored".into(),
        (_, theirs) => theirs.unwrap_or_default().to_string(),
    }
}

/// A video as TMDB lists one.
fn video_entry(video: &Video) -> Value {
    json!({
        "iso_639_1": video.language.as_deref().map(language::two_letter).unwrap_or_default(),
        "iso_3166_1": video.country.clone().unwrap_or_default(),
        "name": video.name.clone().unwrap_or_default(),
        "key": video.key,
        "site": video.site,
        "size": video.size.unwrap_or(0),
        "type": video.video_type.clone().unwrap_or_default(),
        "official": video.official,
        "published_at": video.published_at.clone().unwrap_or_default(),
        "id": video.tmdb_id.clone().unwrap_or_default(),
    })
}

/// A company or a network by its name, as TMDB lists it where it does:
/// its id, its logo, its country; nought, none and nothing else.
fn company(theirs: &[Value], name: &str) -> Value {
    theirs
        .iter()
        .find(|c| {
            c.get("name")
                .and_then(Value::as_str)
                .is_some_and(|theirs| folded(theirs) == folded(name))
        })
        .cloned()
        .unwrap_or_else(
            || json!({ "id": 0, "logo_path": null, "name": name, "origin_country": "" }),
        )
}

/// One of TMDB's pictures as a list carries it now: with the country TMDB
/// files it for, which documents kept from before lack.
fn picture_entry(entry: &Value) -> Value {
    let get = |key: &str, default: Value| entry.get(key).cloned().unwrap_or(default);
    json!({
        "aspect_ratio": get("aspect_ratio", json!(0.0)),
        "height": get("height", json!(0)),
        "iso_3166_1": get("iso_3166_1", Value::Null),
        "iso_639_1": get("iso_639_1", Value::Null),
        "file_path": get("file_path", Value::Null),
        "vote_average": get("vote_average", json!(0.0)),
        "vote_count": get("vote_count", json!(0)),
        "width": get("width", json!(0)),
    })
}

/// A picture's width over its height, to three places, as TMDB gives it.
fn aspect(width: u32, height: u32) -> f64 {
    if height == 0 {
        return 0.0;
    }
    (f64::from(width) / f64::from(height) * 1000.0).round() / 1000.0
}

/// What every person of an aggregate carries, from an entry of a list of
/// credits.
fn person_of(entry: &Value) -> Map<String, Value> {
    let mut person = Map::new();
    for key in [
        "adult",
        "gender",
        "id",
        "known_for_department",
        "name",
        "original_name",
        "popularity",
        "profile_path",
    ] {
        person.insert(key.into(), entry.get(key).cloned().unwrap_or(Value::Null));
    }
    person
}

/// Whether a credit is a series' creator, as the catalogue files one: a
/// writer whose job is to have created it.
fn is_creator(credit: &Credit) -> bool {
    credit.credit_type == CreditType::Writer
        && credit
            .job
            .as_deref()
            .or(credit.character_name.as_deref())
            .is_some_and(|job| job.eq_ignore_ascii_case("Creator"))
}

/// The department and the job one of a work's crew is credited in, as TMDB
/// names them: as the catalogue keeps them, else by the kind of credit.
fn department_and_job(credit: &Credit) -> (String, String) {
    let department = credit
        .department
        .clone()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| {
            match credit.credit_type {
                CreditType::Director => "Directing",
                CreditType::Writer => "Writing",
                CreditType::Producer => "Production",
                _ => "Crew",
            }
            .to_string()
        });
    let job = credit
        .job
        .clone()
        .or_else(|| credit.character_name.clone())
        .filter(|j| !j.trim().is_empty())
        .unwrap_or_else(|| {
            match credit.credit_type {
                CreditType::Director => "Director",
                CreditType::Writer => "Writer",
                CreditType::Producer => "Producer",
                _ => "",
            }
            .to_string()
        });
    (department, job)
}

/// A text that says something.
fn some_text(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        domain::{ExternalIds, PrimaryImages},
        providers::tmdb::{map, models},
        wire::contract::{assert_same_shape, gaps},
    };

    /// A document of TMDB's, as it answered in November 2026, reduced: its
    /// lists to three entries, its free texts to a few words.
    fn fixture(name: &str) -> Value {
        let text = match name {
            "movie" => include_str!("fixtures/tmdb/movie.json"),
            "movie-release_dates" => include_str!("fixtures/tmdb/movie-release_dates.json"),
            "movie-videos" => include_str!("fixtures/tmdb/movie-videos.json"),
            "movie-undated" => include_str!("fixtures/tmdb/movie-undated.json"),
            "tv" => include_str!("fixtures/tmdb/tv.json"),
            "tv-nested" => include_str!("fixtures/tmdb/tv-nested.json"),
            "tv-aggregate_credits" => include_str!("fixtures/tmdb/tv-aggregate_credits.json"),
            "tv-external_ids" => include_str!("fixtures/tmdb/tv-external_ids.json"),
            "tv-undated" => include_str!("fixtures/tmdb/tv-undated.json"),
            "season" => include_str!("fixtures/tmdb/season.json"),
            "episode" => include_str!("fixtures/tmdb/episode.json"),
            "find-movie" => include_str!("fixtures/tmdb/find-movie.json"),
            "find-tv" => include_str!("fixtures/tmdb/find-tv.json"),
            "search-movie" => include_str!("fixtures/tmdb/search-movie.json"),
            "search-tv" => include_str!("fixtures/tmdb/search-tv.json"),
            "search-multi" => include_str!("fixtures/tmdb/search-multi.json"),
            other => panic!("no fixture {other}"),
        };
        serde_json::from_str(text).expect("a fixture reads")
    }

    /// Every name TMDB's fixtures carry appended, which a client's request
    /// picks from.
    const APPENDED: &[&str] = &[
        "credits",
        "aggregate_credits",
        "casts",
        "external_ids",
        "videos",
        "keywords",
        "release_dates",
        "releases",
        "trailers",
        "images",
        "alternative_titles",
        "recommendations",
        "translations",
        "reviews",
        "watch/providers",
        "content_ratings",
        "episode_groups",
    ];

    /// TMDB's document as a client asked for it: the work's own fields and
    /// what it appended, out of a document that appended everything.
    fn as_asked(document: &Value, appended: &[&str]) -> Value {
        let mut asked = document.clone();
        if let Value::Object(map) = &mut asked {
            map.retain(|key, _| {
                appended.contains(&key.as_str())
                    || !(APPENDED.contains(&key.as_str()) || key.starts_with("season/"))
            });
        }
        asked
    }

    /// Fight Club as the catalogue keeps it from TMDB's document.
    fn film() -> MediaItem {
        let movie: models::Movie = serde_json::from_value(fixture("movie")).expect("a film");
        map::movie_to_item(&movie, 25)
    }

    /// Breaking Bad as the catalogue keeps it from TMDB's documents, its
    /// first season among them.
    fn series() -> MediaItem {
        let tv: models::Tv = serde_json::from_value(fixture("tv")).expect("a series");
        let season: models::Season = serde_json::from_value(fixture("season")).expect("a season");
        map::tv_to_item(&tv, &[season], 25)
    }

    fn english() -> Language {
        Language::new("en-US", "eng")
    }

    /// Our document for a request: the work's own, and each part appended
    /// as the relay appends it — another resource's empty where TMDB is not
    /// asked here.
    fn written(work: &Work, asked: &[&str], filters: &Filters) -> Value {
        let mut document = match work.kind() {
            MediaKind::Movie => movie(work),
            MediaKind::Series => tv(work),
        };
        for name in asked {
            let value = match title_part(work, name, filters) {
                Part::Doc(value) => Some(value),
                Part::Elsewhere { empty } => empty,
                Part::Unknown => None,
            };
            if let Some(value) = value {
                document[*name] = appended(value);
            }
        }
        document
    }

    /// What our documents may differ from TMDB's by, each for its reason:
    /// where to watch is JustWatch's, through TMDB, and is relayed where
    /// TMDB can be asked — empty here, where it is not; and TMDB itself
    /// gives no size in its older list of trailers for a video past 1080
    /// lines.
    const ELSEWHERE: &[&str] = &["watch/providers.results.*", "trailers.youtube[].size"];

    /// Where TMDB itself writes null when it has nothing — a person's
    /// portrait, a company's logo, a picture's language where it carries no
    /// text — which one sample cannot tell from a value: null is allowed in
    /// ours wherever TMDB's first entry happens to have one.
    const NULL_WHEN_NONE: &[&str] = &[
        "credits.cast[].profile_path",
        "credits.crew[].profile_path",
        "casts.cast[].profile_path",
        "casts.crew[].profile_path",
        "aggregate_credits.cast[].profile_path",
        "aggregate_credits.crew[].profile_path",
        "created_by[].profile_path",
        "cast[].profile_path",
        "crew[].profile_path",
        "production_companies[].logo_path",
        "networks[].logo_path",
        "seasons[].poster_path",
        "images.posters[].iso_639_1",
        "images.posters[].iso_3166_1",
        "images.backdrops[].iso_639_1",
        "images.backdrops[].iso_3166_1",
        "images.logos[].iso_639_1",
        "images.logos[].iso_3166_1",
        "posters[].iso_639_1",
        "posters[].iso_3166_1",
        "images.stills[].iso_639_1",
        "images.stills[].iso_3166_1",
        "stills[].iso_639_1",
        "stills[].iso_3166_1",
    ];

    /// Our document against TMDB's, allowed TMDB's own nulls and `more`.
    #[track_caller]
    fn same_shape(theirs: &Value, ours: &Value, more: &[&str]) {
        let allowed: Vec<&str> = NULL_WHEN_NONE.iter().chain(more).copied().collect();
        assert_same_shape(theirs, ours, &allowed);
    }

    /// A season's or an episode's ids: the catalogue keeps TheTVDB's alone,
    /// and TMDB writes null for those it has none of.
    const IDS_NOT_KEPT: &[&str] = &[
        "external_ids.*",
        "freebase_mid",
        "freebase_id",
        "imdb_id",
        "tvdb_id",
        "tvrage_id",
        "wikidata_id",
    ];

    // ─── the clients' own requests ───────────────────────────────────────

    #[test]
    fn a_film_is_written_in_tmdbs_shape_for_each_client_that_reads_one() {
        let theirs = fixture("movie");
        let item = film();
        let language = english();
        let work = Work::new(&item, 550, &language, &TmdbOnly).with_snapshot(Some(&theirs));
        let filters = Filters::from_query(Some("language=en-US"), Some(&language));
        let requests: &[(&str, &[&str])] = &[
            (
                "Seerr",
                &[
                    "credits",
                    "external_ids",
                    "videos",
                    "keywords",
                    "release_dates",
                    "watch/providers",
                ],
            ),
            (
                "Jellyfin",
                &["credits", "releases", "images", "videos", "keywords"],
            ),
            (
                "Kodi",
                &["trailers", "images", "releases", "credits", "keywords"],
            ),
            (
                "Emby",
                &[
                    "casts",
                    "releases",
                    "images",
                    "keywords",
                    "trailers",
                    "alternative_titles",
                    "reviews",
                ],
            ),
            ("Kometa", &["external_ids", "keywords", "images"]),
            (
                "Yamtrack",
                &[
                    "recommendations",
                    "external_ids",
                    "credits",
                    "watch/providers",
                ],
            ),
        ];
        for (client, appended) in requests {
            let ours = written(&work, appended, &filters);
            let allowed: Vec<&str> = NULL_WHEN_NONE.iter().chain(ELSEWHERE).copied().collect();
            let found = gaps(&as_asked(&theirs, appended), &ours, &allowed);
            assert!(found.is_empty(), "{client}: {found:#?}");
        }
    }

    #[test]
    fn a_series_is_written_in_tmdbs_shape_for_each_client_that_reads_one() {
        let theirs = fixture("tv");
        let item = series();
        let language = english();
        let work = Work::new(&item, 1396, &language, &TmdbOnly)
            .with_snapshot(Some(&theirs))
            .with_tvdb_id(Some(81189));
        let filters = Filters::from_query(Some("language=en-US"), Some(&language));
        let requests: &[(&str, &[&str])] = &[
            (
                "Seerr",
                &[
                    "aggregate_credits",
                    "credits",
                    "external_ids",
                    "keywords",
                    "videos",
                    "content_ratings",
                    "watch/providers",
                ],
            ),
            (
                "Jellyfin",
                &[
                    "credits",
                    "aggregate_credits",
                    "images",
                    "external_ids",
                    "videos",
                    "content_ratings",
                    "episode_groups",
                    "keywords",
                ],
            ),
            (
                "Kodi",
                &[
                    "credits",
                    "content_ratings",
                    "external_ids",
                    "images",
                    "videos",
                    "keywords",
                ],
            ),
            (
                "Emby",
                &[
                    "alternative_titles",
                    "reviews",
                    "credits",
                    "images",
                    "keywords",
                    "external_ids",
                    "videos",
                    "content_ratings",
                    "episode_groups",
                ],
            ),
            ("Kometa", &["external_ids", "keywords", "images"]),
            (
                "Yamtrack",
                &["recommendations", "external_ids", "watch/providers"],
            ),
        ];
        for (client, appended) in requests {
            let ours = written(&work, appended, &filters);
            let allowed: Vec<&str> = NULL_WHEN_NONE.iter().chain(ELSEWHERE).copied().collect();
            let found = gaps(&as_asked(&theirs, appended), &ours, &allowed);
            assert!(found.is_empty(), "{client}: {found:#?}");
        }
    }

    #[test]
    fn what_hangs_off_a_work_asked_on_its_own_carries_the_works_id() {
        let filters = Filters::all();
        let movie_theirs = fixture("movie");
        let item = film();
        let language = english();
        let work = Work::new(&item, 550, &language, &TmdbOnly).with_snapshot(Some(&movie_theirs));
        // Kometa's release dates; Seerr's second call for videos, in
        // English.
        for (name, theirs) in [
            ("release_dates", fixture("movie-release_dates")),
            ("videos", fixture("movie-videos")),
        ] {
            let Part::Doc(ours) = title_part(&work, name, &filters) else {
                panic!("{name} is written here");
            };
            assert_eq!(ours["id"], json!(550), "{name}");
            same_shape(&theirs, &ours, &[]);
        }

        // Kodi's aggregate, asked when a series has no regulars; Seerr's and
        // the parcours' ids.
        let tv_theirs = fixture("tv");
        let item = series();
        let work = Work::new(&item, 1396, &language, &TmdbOnly)
            .with_snapshot(Some(&tv_theirs))
            .with_tvdb_id(Some(81189));
        for (name, theirs) in [
            ("aggregate_credits", fixture("tv-aggregate_credits")),
            ("external_ids", fixture("tv-external_ids")),
        ] {
            let Part::Doc(ours) = title_part(&work, name, &filters) else {
                panic!("{name} is written here");
            };
            assert_eq!(ours["id"], json!(1396), "{name}");
            same_shape(&theirs, &ours, &[]);
        }
    }

    #[test]
    fn a_season_and_an_episode_are_written_as_the_clients_ask_for_them() {
        let item = series();
        let language = english();
        let snapshot = fixture("tv");
        let work = Work::new(&item, 1396, &language, &TmdbOnly)
            .with_snapshot(Some(&snapshot))
            .numbered(Numbering::Own);
        let filters = Filters::from_query(Some("language=en-US"), Some(&language));
        let with = |mut document: Value, parts: Vec<(&str, Part)>| {
            for (name, part) in parts {
                if let Part::Doc(value) = part {
                    document[name] = appended(value);
                }
            }
            document
        };

        let theirs = fixture("season");
        // Seerr, Jellyfin, Emby — whose `keywords` TMDB has none of for a
        // season, and drops.
        for appended_here in [
            &["external_ids"][..],
            &["credits", "images", "external_ids", "videos"],
            &["images", "keywords", "external_ids", "credits", "videos"],
        ] {
            let ours = with(
                season(&work, 1).expect("the first season"),
                appended_here
                    .iter()
                    .map(|name| (*name, season_part(&work, 1, name, &filters)))
                    .collect(),
            );
            assert!(ours.get("keywords").is_none());
            same_shape(&as_asked(&theirs, appended_here), &ours, IDS_NOT_KEPT);
        }

        let theirs = fixture("episode");
        // Jellyfin, Kodi, Emby.
        for appended_here in [
            &["credits", "images", "external_ids", "videos"][..],
            &["images", "external_ids"],
            &["images", "external_ids", "credits", "videos"],
        ] {
            let ours = with(
                episode(&work, 1, 1).expect("the first episode"),
                appended_here
                    .iter()
                    .map(|name| (*name, episode_part(&work, 1, 1, name, &filters)))
                    .collect(),
            );
            same_shape(&as_asked(&theirs, appended_here), &ours, IDS_NOT_KEPT);
        }
    }

    #[test]
    fn the_seasons_asked_with_a_series_are_written_as_on_their_own() {
        // Kodi's TV scraper and Yamtrack: what is appended below the series
        // is TMDB's document at that path, less its id — a season's own
        // `_id` stays.
        let theirs = fixture("tv-nested");
        let item = series();
        let language = english();
        let work = Work::new(&item, 1396, &language, &TmdbOnly).numbered(Numbering::Own);
        let filters = Filters::all();
        let parts: Vec<(&str, Value)> = vec![
            ("season/1", appended(season(&work, 1).unwrap())),
            (
                "season/1/images",
                part_of(season_part(&work, 1, "images", &filters)),
            ),
            (
                "season/1/credits",
                part_of(season_part(&work, 1, "credits", &filters)),
            ),
            (
                "season/1/external_ids",
                part_of(season_part(&work, 1, "external_ids", &filters)),
            ),
            (
                "season/1/episode/1",
                appended(episode(&work, 1, 1).unwrap()),
            ),
            (
                "season/1/episode/1/images",
                part_of(episode_part(&work, 1, 1, "images", &filters)),
            ),
            (
                "season/1/episode/1/external_ids",
                part_of(episode_part(&work, 1, 1, "external_ids", &filters)),
            ),
        ];
        for (key, ours) in parts {
            same_shape(&theirs[key], &ours, IDS_NOT_KEPT);
        }
    }

    fn part_of(part: Part) -> Value {
        match part {
            Part::Doc(value) => appended(value),
            other => panic!("not written here: {other:?}"),
        }
    }

    #[test]
    fn a_work_is_listed_in_a_search_and_found_by_an_id_in_tmdbs_shape() {
        let language = english();
        let film = film();
        let movie_theirs = fixture("movie");
        let work = Work::new(&film, 550, &language, &TmdbOnly).with_snapshot(Some(&movie_theirs));
        let tv_item = series();
        let tv_theirs = fixture("tv");
        let show = Work::new(&tv_item, 1396, &language, &TmdbOnly).with_snapshot(Some(&tv_theirs));

        same_shape(
            &fixture("search-movie")["results"][0],
            &result(&work, false),
            &[],
        );
        same_shape(
            &fixture("search-tv")["results"][0],
            &result(&show, false),
            &[],
        );
        let multi = fixture("search-multi");
        let of = |kind: &str| {
            multi["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["media_type"] == kind)
                .cloned()
                .unwrap()
        };
        same_shape(&of("movie"), &result(&work, true), &[]);
        same_shape(&of("tv"), &result(&show, true), &[]);

        let found_film = found(vec![result(&work, true)], Vec::new());
        same_shape(&fixture("find-movie"), &found_film, &[]);
        let found_show = found(Vec::new(), vec![result(&show, true)]);
        same_shape(&fixture("find-tv"), &found_show, &[]);
    }

    // ─── a work entered by hand ──────────────────────────────────────────

    fn by_hand(kind: MediaKind) -> MediaItem {
        let mut item = MediaItem::empty(kind);
        item.title = "Typed in".into();
        item.is_manual = true;
        if kind == MediaKind::Series {
            item.seasons = vec![Season {
                id: "s1".into(),
                season_number: 1,
                title: None,
                overview: None,
                air_date: None,
                tmdb_id: None,
                tvdb_id: None,
                is_manual: true,
                images: Vec::new(),
                primary_images: PrimaryImages::default(),
                translations: Vec::new(),
            }];
            item.episodes = (1..=2)
                .map(|number| Episode {
                    id: format!("e{number}"),
                    season_number: 1,
                    episode_number: number,
                    absolute_episode_number: None,
                    aired_after_season_number: None,
                    aired_before_season_number: None,
                    aired_before_episode_number: None,
                    title: format!("Typed in, {number}"),
                    overview: None,
                    air_date: None,
                    air_date_utc: None,
                    runtime: None,
                    finale_type: None,
                    image: None,
                    tvdb_id: None,
                    tmdb_id: None,
                    rating: None,
                    is_manual: true,
                })
                .collect();
        }
        item
    }

    /// What a work no source has fills where TMDB writes a value: nothing,
    /// as TMDB writes it for a work it knows nothing of — no picture, no
    /// date, no list of its appended parts' entries — and no collection.
    const NOTHING_KNOWN: &[&str] = &[
        "backdrop_path",
        "poster_path",
        "imdb_id",
        "last_air_date",
        "last_episode_to_air",
        "external_ids.*",
        "seasons[].air_date",
        "seasons[].poster_path",
        "air_date",
        "runtime",
        "still_path",
        "episodes[].air_date",
        "episodes[].runtime",
        "episodes[].still_path",
        "watch/providers.results.*",
    ];

    #[test]
    fn a_work_entered_by_hand_keeps_every_key_tmdb_writes() {
        let language = english();
        let filters = Filters::all();
        for (kind, name, id) in [
            (MediaKind::Movie, "movie", 1_000_000_001_i64),
            (MediaKind::Series, "tv", 1_000_000_002),
        ] {
            let item = by_hand(kind);
            let work = Work::new(&item, id, &language, &TmdbOnly).with_tvdb_id(Some(id));
            let theirs = fixture(name);
            let everything: Vec<&str> = APPENDED
                .iter()
                .copied()
                .filter(|name| theirs.get(*name).is_some())
                .collect();
            let ours = written(&work, &everything, &filters);
            same_shape(&as_asked(&theirs, &everything), &ours, NOTHING_KNOWN);
            // And as TMDB writes a work it knows little of.
            let undated = fixture(&format!("{name}-undated"));
            let asked: Vec<&str> = APPENDED
                .iter()
                .copied()
                .filter(|name| undated.get(*name).is_some())
                .collect();
            let ours = written(&work, &asked, &filters);
            same_shape(&undated, &ours, NOTHING_KNOWN);
            assert_eq!(ours["id"], json!(id));
        }

        let item = by_hand(MediaKind::Series);
        let work = Work::new(&item, 1_000_000_002, &language, &TmdbOnly);
        let ours = season(&work, 1).expect("its season");
        same_shape(&as_asked(&fixture("season"), &[]), &ours, NOTHING_KNOWN);
        assert_eq!(ours["episodes"][1]["name"], json!("Typed in, 2"));
        assert!(ours["id"].as_i64().unwrap() < 0, "{ours}");
        let ours = episode(&work, 1, 2).expect("its episode");
        same_shape(&as_asked(&fixture("episode"), &[]), &ours, NOTHING_KNOWN);
        assert!(season(&work, 2).is_none() && episode(&work, 1, 3).is_none());
    }

    // ─── the catalogue first ─────────────────────────────────────────────

    #[test]
    fn the_catalogues_values_and_locks_come_before_tmdbs_document() {
        let theirs = fixture("movie");
        let mut item = film();
        item.title = "Le Club".into();
        item.locked_fields = vec!["item/title".into(), "item/keywords".into()];
        item.keywords = vec!["dual identity".into(), "soap".into()];
        item.external_ids.imdb = Some("tt0137523".into());
        item.popularity = Some(99.5);
        let language = english();
        let work = Work::new(&item, 550, &language, &TmdbOnly).with_snapshot(Some(&theirs));
        let document = movie(&work);
        assert_eq!(document["title"], json!("Le Club"));
        assert_eq!(document["popularity"], json!(99.5));
        // What the catalogue does not keep is TMDB's.
        assert_eq!(document["budget"], theirs["budget"]);
        assert_eq!(
            document["production_companies"],
            theirs["production_companies"]
        );
        // A keyword locked here is given TMDB's id where TMDB lists it, and
        // nought where it does not.
        let Part::Doc(keywords) = title_part(&work, "keywords", &Filters::all()) else {
            panic!("keywords are written here");
        };
        assert_eq!(
            keywords["keywords"],
            json!([{ "id": 851, "name": "dual identity" }, { "id": 0, "name": "soap" }])
        );
    }

    #[test]
    fn what_a_person_hid_stays_out_of_every_list() {
        let theirs = fixture("movie");
        let mut item = film();
        let poster = theirs["images"]["posters"][0]["file_path"]
            .as_str()
            .unwrap()
            .to_string();
        let narrator = item.credits.remove(0);
        item.hidden.credits.push(narrator.clone());
        item.hidden.images.push(Image {
            id: "hidden".into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: format!("https://image.tmdb.org/t/p/original{poster}"),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        });
        item.images.retain(|image| !image.url.ends_with(&poster));
        let language = english();
        let work = Work::new(&item, 550, &language, &TmdbOnly).with_snapshot(Some(&theirs));
        let Part::Doc(credits) = title_part(&work, "credits", &Filters::all()) else {
            panic!("credits are written here");
        };
        let person = narrator.tmdb_person_id.unwrap();
        assert!(
            credits["cast"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["id"] != json!(person)),
            "{credits}"
        );
        let Part::Doc(images) = title_part(&work, "images", &Filters::all()) else {
            panic!("pictures are written here");
        };
        assert!(
            images["posters"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["file_path"] != json!(poster)),
            "{images}"
        );
        assert_ne!(movie(&work)["poster_path"], json!(poster));
    }

    // ─── languages ───────────────────────────────────────────────────────

    #[test]
    fn a_language_tmdb_holds_a_translation_in_is_written_from_it_and_another_as_tmdb_does() {
        let theirs = fixture("movie");
        let item = film();
        // French: TMDB's translation; its title, empty there, is the
        // original one, as TMDB answers.
        let french = Language::new("fr-FR", "eng");
        let work = Work::new(&item, 550, &french, &TmdbOnly).with_snapshot(Some(&theirs));
        let document = movie(&work);
        assert_eq!(document["title"], json!("Fight Club"));
        assert_eq!(
            document["overview"],
            json!("Le narrateur, sans identité précise, vit…")
        );
        assert_eq!(
            document["tagline"],
            json!("La première règle du Fight Club…")
        );
        assert_eq!(document["genres"][0], json!({ "id": 18, "name": "Drame" }));
        // Yoruba, which TMDB holds nothing in: the original title, nothing
        // else, and the genres in English.
        let yoruba = Language::new("yo-NG", "eng");
        let work = Work::new(&item, 550, &yoruba, &TmdbOnly).with_snapshot(Some(&theirs));
        let document = movie(&work);
        assert_eq!(document["title"], json!("Fight Club"));
        assert_eq!(document["overview"], json!(""));
        assert_eq!(document["tagline"], json!(""));
        assert_eq!(document["homepage"], json!(""));
        assert_eq!(document["genres"][0], json!({ "id": 18, "name": "Drama" }));
    }

    #[test]
    fn a_text_locked_in_a_language_is_written_in_it_alone() {
        let theirs = fixture("movie");
        let mut item = film();
        item.locked_texts = vec![
            LockedText {
                scope: "item".into(),
                field: "title".into(),
                language: "fra".into(),
                value: Some("Le Club des combattants".into()),
            },
            LockedText {
                scope: "item".into(),
                field: "overview".into(),
                language: String::new(),
                value: Some("Everywhere".into()),
            },
        ];
        item.locked_fields = vec!["item/overview".into(), "item/title@fra".into()];
        item.overview = Some("Everywhere".into());
        let french = Language::new("fr", "eng");
        let work = Work::new(&item, 550, &french, &TmdbOnly).with_snapshot(Some(&theirs));
        let document = movie(&work);
        assert_eq!(document["title"], json!("Le Club des combattants"));
        // Locked in every language is the catalogue's own text: TMDB's
        // French stands in French, as the relay writes it.
        assert_eq!(
            document["overview"],
            json!("Le narrateur, sans identité précise, vit…")
        );
        let english = english();
        let work = Work::new(&item, 550, &english, &TmdbOnly).with_snapshot(Some(&theirs));
        assert_eq!(movie(&work)["overview"], json!("Everywhere"));
        assert_eq!(movie(&work)["title"], json!("Fight Club"));
        // TMDB's translations take what was locked in their language.
        let Part::Doc(translations) = title_part(&work, "translations", &Filters::all()) else {
            panic!("translations are written here");
        };
        let fr = translations["translations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["iso_639_1"] == "fr")
            .unwrap();
        assert_eq!(fr["data"]["title"], json!("Le Club des combattants"));
    }

    #[test]
    fn an_episode_in_a_language_held_nowhere_is_named_as_tmdb_names_it() {
        let item = series();
        let yoruba = Language::new("yo", "eng");
        let work = Work::new(&item, 1396, &yoruba, &TmdbOnly).numbered(Numbering::Own);
        let document = season(&work, 1).unwrap();
        assert_eq!(document["name"], json!("Season 1"));
        assert_eq!(document["overview"], json!(""));
        assert_eq!(document["episodes"][0]["name"], json!("Episode 1"));
        assert_eq!(document["episodes"][0]["overview"], json!(""));
        let mut texts = HashMap::new();
        texts.insert(
            (1, 1),
            EpisodeText {
                title: Some("Ìbẹ̀rẹ̀".into()),
                overview: Some("Ọ̀rọ̀".into()),
            },
        );
        let work = Work::new(&item, 1396, &yoruba, &TmdbOnly)
            .numbered(Numbering::Own)
            .with_episode_texts(texts);
        let document = episode(&work, 1, 1).unwrap();
        assert_eq!(document["name"], json!("Ìbẹ̀rẹ̀"));
        assert_eq!(document["overview"], json!("Ọ̀rọ̀"));
    }

    // ─── pictures, ids, numbering ────────────────────────────────────────

    /// The gate as a test sees it: a copy kept here under `kept`.
    struct Gate;

    impl Pictures for Gate {
        fn kept(&self, address: &str) -> Option<String> {
            address.strip_prefix("/media/").map(|key| format!("/{key}"))
        }

        fn size(&self, _address: &str) -> Option<(u32, u32)> {
            Some((1000, 1500))
        }
    }

    #[test]
    fn a_picture_of_another_source_has_a_path_only_through_the_gate() {
        let theirs = fixture("movie");
        let mut item = film();
        let key = format!("{}.jpg", "a".repeat(64));
        item.images.insert(
            0,
            Image {
                id: "upload".into(),
                season_number: None,
                cover_type: CoverType::Poster,
                url: format!("/media/{key}"),
                language: Some("fra".into()),
                sort_order: 0,
                source: None,
                is_manual: true,
            },
        );
        item.primary_images.poster = Some("upload".into());
        let language = english();

        // TMDB's alone: the best of TMDB's posters, and the upload in no
        // list.
        let work = Work::new(&item, 550, &language, &TmdbOnly).with_snapshot(Some(&theirs));
        assert_eq!(movie(&work)["poster_path"], theirs["poster_path"]);

        // Through the gate: the chosen picture leads, and lists first.
        let work = Work::new(&item, 550, &language, &Gate).with_snapshot(Some(&theirs));
        assert_eq!(movie(&work)["poster_path"], json!(format!("/{key}")));
        let filters = Filters::from_query(Some("include_image_language=fr,en,null"), None);
        let Part::Doc(images) = title_part(&work, "images", &filters) else {
            panic!("pictures are written here");
        };
        assert_eq!(images["posters"][0]["file_path"], json!(format!("/{key}")));
        assert_eq!(images["posters"][0]["iso_639_1"], json!("fr"));
        assert_eq!(images["posters"][0]["aspect_ratio"], json!(0.667));
        // In a language the request does not take, it is in none.
        let english_only = Filters::from_query(Some("include_image_language=en"), None);
        let Part::Doc(images) = title_part(&work, "images", &english_only) else {
            panic!("pictures are written here");
        };
        assert!(
            images["posters"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["iso_639_1"] == "en")
        );
    }

    #[test]
    fn a_series_is_given_the_tvdb_id_sonarr_keeps_it_under() {
        let item = by_hand(MediaKind::Series);
        let language = english();
        let handed = crate::service::ids::own(&item.id);
        let work = Work::new(&item, handed, &language, &TmdbOnly).with_tvdb_id(Some(handed));
        let Part::Doc(ids) = title_part(&work, "external_ids", &Filters::all()) else {
            panic!("ids are written here");
        };
        assert_eq!(
            ids,
            json!({
                "id": handed, "imdb_id": null, "freebase_mid": null, "freebase_id": null,
                "tvdb_id": handed, "tvrage_id": null, "wikidata_id": null,
                "facebook_id": null, "instagram_id": null, "twitter_id": null,
            })
        );
    }

    #[test]
    fn tmdbs_seasons_are_listed_where_the_catalogue_numbers_otherwise() {
        let theirs = fixture("tv");
        let item = series();
        let language = english();
        // The catalogue's first season alone, TMDB's three listed: TMDB's.
        let work = Work::new(&item, 1396, &language, &TmdbOnly)
            .with_snapshot(Some(&theirs))
            .numbered(Numbering::Theirs);
        let document = tv(&work);
        assert_eq!(document["seasons"], theirs["seasons"]);
        assert_eq!(document["number_of_episodes"], theirs["number_of_episodes"]);
        assert!(!work.has_season(1));
        assert!(season(&work, 1).is_none());
        // Numbered alike only where every season TMDB lists holds as many.
        assert!(!numbered_alike(&item, Some(&theirs)));
        let mut alike = item.clone();
        alike.episodes.clear();
        let counts = json!({ "id": 1396, "seasons": [
            { "season_number": 1, "episode_count": 0 }
        ]});
        assert!(numbered_alike(&alike, Some(&counts)));
        assert!(!numbered_alike(&alike, None));
    }

    #[test]
    fn a_trailer_chosen_here_leads_the_videos_in_every_language() {
        let theirs = fixture("movie");
        let mut item = film();
        let french = Language::new("fr-FR", "eng");
        let filters = Filters::from_query(Some("language=fr-FR"), Some(&french));
        let keys = |work: &Work| -> Vec<String> {
            let Part::Doc(videos) = title_part(work, "videos", &filters) else {
                panic!("videos are written here");
            };
            videos["results"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["key"].as_str().unwrap().to_string())
                .collect()
        };
        // TMDB's own choice of the catalogue's trailer, in English: none of
        // TMDB's French videos, of which the fixture has none.
        let work = Work::new(&item, 550, &french, &TmdbOnly).with_snapshot(Some(&theirs));
        assert!(keys(&work).is_empty());
        // Locked here: first, in French too.
        item.trailer_youtube_id = Some("lockedTrailerE3".into());
        item.locked_fields = vec!["item/trailerYoutubeId".into()];
        let work = Work::new(&item, 550, &french, &TmdbOnly).with_snapshot(Some(&theirs));
        assert_eq!(keys(&work), ["lockedTrailerE3"]);
    }

    #[test]
    fn tmdbs_seasons_take_the_names_locked_on_the_catalogues_season_of_the_same_id() {
        let theirs = fixture("tv");
        let mut item = series();
        let first = theirs["seasons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["season_number"] == 1)
            .unwrap()
            .clone();
        // The catalogue numbers TMDB's first season as its second.
        for season in &mut item.seasons {
            if season.tmdb_id == first["id"].as_i64() {
                season.season_number = 2;
            } else if season.season_number == 2 {
                season.season_number = 3;
            }
        }
        item.locked_texts = vec![LockedText {
            scope: "season:2".into(),
            field: "title".into(),
            language: "deu".into(),
            value: Some("Die erste".into()),
        }];
        item.locked_fields = vec!["season:2/title@deu".into()];
        let german = Language::new("de-DE", "eng");
        let work = Work::new(&item, 1396, &german, &TmdbOnly)
            .with_snapshot(Some(&theirs))
            .numbered(Numbering::Theirs);
        let seasons = tv(&work)["seasons"].clone();
        let named = |number: i64| {
            seasons
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["season_number"] == number)
                .map(|s| s["name"].clone())
                .unwrap()
        };
        assert_eq!(named(1), json!("Die erste"));
        assert_eq!(named(0), json!("Specials"));
    }

    #[test]
    fn the_catalogues_companies_keywords_videos_and_pictures_come_first_and_tmdbs_stand_in() {
        let theirs = fixture("movie");
        let item = film();
        let language = english();
        let filters = Filters::all();
        let work = Work::new(&item, 550, &language, &TmdbOnly).with_snapshot(Some(&theirs));
        // As the catalogue keeps them, from TMDB's document read again.
        assert!(!item.companies.is_empty() && !item.keyword_entities.is_empty());
        let company = &item.companies[0].company;
        assert_eq!(
            movie(&work)["production_companies"][0]["id"],
            json!(company.tmdb_id)
        );
        let Part::Doc(pictures) = title_part(&work, "images", &filters) else {
            panic!("pictures are written here");
        };
        let first = &pictures["posters"][0];
        assert!(first["width"].as_u64().unwrap() > 0, "{first}");
        assert!(first["vote_count"].as_u64().unwrap() > 0, "{first}");
        let Part::Doc(videos) = title_part(&work, "videos", &Filters::from_query(None, None))
        else {
            panic!("videos are written here");
        };
        assert_eq!(
            videos["results"][0]["key"],
            theirs["videos"]["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["iso_639_1"] == "en")
                .unwrap()["key"]
        );

        // A work not refreshed since the catalogue keeps them: TMDB's
        // document's lists.
        let mut before = item.clone();
        before.companies.clear();
        before.keyword_entities.clear();
        before.videos.clear();
        before.image_details.clear();
        let work = Work::new(&before, 550, &language, &TmdbOnly).with_snapshot(Some(&theirs));
        assert_eq!(
            movie(&work)["production_companies"],
            theirs["production_companies"]
        );
        let Part::Doc(keywords) = title_part(&work, "keywords", &filters) else {
            panic!("keywords are written here");
        };
        assert_eq!(keywords["keywords"], theirs["keywords"]["keywords"]);
        let Part::Doc(pictures) = title_part(&work, "images", &filters) else {
            panic!("pictures are written here");
        };
        assert_eq!(
            pictures["posters"][0]["file_path"],
            theirs["images"]["posters"][0]["file_path"]
        );
    }

    #[test]
    fn a_snapshot_of_another_tmdb_id_is_not_read() {
        let theirs = fixture("movie");
        let mut item = film();
        item.external_ids = ExternalIds {
            tmdb: Some(603),
            ..Default::default()
        };
        let language = english();
        let work = Work::new(&item, 603, &language, &TmdbOnly).with_snapshot(Some(&theirs));
        assert_eq!(movie(&work)["budget"], json!(0));
    }

    #[test]
    fn the_older_forms_are_written_from_the_same_releases_and_videos() {
        let theirs = fixture("movie");
        let item = film();
        let language = english();
        let work = Work::new(&item, 550, &language, &TmdbOnly).with_snapshot(Some(&theirs));
        let filters = Filters::from_query(Some("language=en"), Some(&language));
        let Part::Doc(trailers) = title_part(&work, "trailers", &filters) else {
            panic!("trailers are written here");
        };
        assert_eq!(trailers["youtube"][0]["type"], json!("Trailer"));
        assert_eq!(trailers["quicktime"], json!([]));
        let Part::Doc(releases) = title_part(&work, "releases", &filters) else {
            panic!("releases are written here");
        };
        let us = releases["countries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["iso_3166_1"] == "US")
            .expect("the United States");
        assert_eq!(us["release_date"].as_str().map(str::len), Some(10));
        assert_eq!(us["primary"], json!(false));
        assert_eq!(legacy_size(Some(1080)), json!("HD"));
        assert_eq!(legacy_size(Some(480)), json!("HQ"));
        assert_eq!(legacy_size(Some(2160)), Value::Null);
    }
}

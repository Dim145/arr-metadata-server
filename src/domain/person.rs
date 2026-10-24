//! The people the catalogue credits.
//!
//! A credit names somebody as its source does — by a TMDB id, by TheTVDB's,
//! by a name alone — and is filed under the [`Person`] that is: one record
//! whatever their credits call them, with the ids each source files them
//! under, the other names they go by, and what TMDB says of them.
//!
//! What a source says of a credited person beyond the name travels with the
//! work it was said of, in [`People`]: TheTVDB's id for each of its people,
//! the name in its own script TMDB gives, and the credits of single episodes,
//! which the work's own list leaves out. Read back, it says which person each
//! credit is filed under.

use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::Credit;

/// What somebody did on a work, as their page puts their works together:
/// on screen, or behind the camera in one of three ways.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "camelCase")]
// Named apart from the jobs the server runs, which are `Job`s too: utoipa
// keys schemas on the leaf name alone.
#[schema(as = PersonJob)]
pub enum Job {
    /// A part: in the cast, or a guest on an episode.
    Acting,
    Directing,
    Writing,
    Producing,
}

impl Job {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Acting => "acting",
            Self::Directing => "directing",
            Self::Writing => "writing",
            Self::Producing => "producing",
        }
    }
}

impl fmt::Display for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Job {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "acting" => Ok(Self::Acting),
            "directing" => Ok(Self::Directing),
            "writing" => Ok(Self::Writing),
            "producing" => Ok(Self::Producing),
            other => anyhow::bail!(
                "unknown job {other:?}: expected acting, directing, writing or producing"
            ),
        }
    }
}

/// Somebody the catalogue credits, as it keeps them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named apart from the page the API answers with, which is `Person`.
#[schema(as = PersonRecord)]
pub struct Person {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    /// `nm0186505`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    pub name: String,
    /// What the index files and sorts them under, where it is not their
    /// name: `Toro, Guillermo del`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_name: Option<String>,
    /// Their name in the script of their own language, where it is another
    /// than the one above: 田中真弓.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,
    /// As TMDB has it — 1 a woman, 2 a man, 3 non-binary — for a language
    /// that agrees its participles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gender: Option<u8>,
    /// `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deathday: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_of_birth: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biography: Option<String>,
    /// Their portrait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// A few more of them, in TMDB's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub photos: Vec<String>,
    /// What TMDB files them under: `Acting`, `Directing`, `Writing`, …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known_for: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub popularity: Option<f64>,
    /// When TMDB was last asked about them; none until their page is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
    /// Entered, or put right, by hand: what TMDB says no longer replaces it.
    pub is_manual: bool,
    /// The record this one was merged into, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// The other names they go by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<Alias>,
}

impl Person {
    /// A record of somebody known by a name alone, to be filled in.
    pub fn named(name: &str) -> Self {
        let now = crate::db::now();
        Self {
            id: crate::db::new_id(),
            tmdb_id: None,
            imdb_id: None,
            tvdb_id: None,
            name: name.trim().to_string(),
            sort_name: None,
            original_name: None,
            gender: None,
            birthday: None,
            deathday: None,
            place_of_birth: None,
            biography: None,
            image: None,
            photos: Vec::new(),
            known_for: None,
            homepage: None,
            popularity: None,
            refreshed_at: None,
            is_manual: false,
            merged_into: None,
            created_at: now.clone(),
            updated_at: now,
            aliases: Vec::new(),
        }
    }
}

/// Another name somebody goes by, and where it came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PersonAlias)]
pub struct Alias {
    pub name: String,
    /// `credit` — a spelling one of their credits gives — `tmdb`, one TMDB
    /// lists, `merged`, the name of a record merged into theirs, or
    /// `manual`, one typed by hand.
    pub source: String,
}

/// Who a credit names, beyond the name it gives.
///
/// From a source: what it knows of them that a credit has no room for —
/// TheTVDB's id for its people, the name in its own script and the gender
/// TMDB gives — and what their record is made or completed with when the
/// credit is written. Read back: the record the credit is filed under, with
/// the ids it is known by, for whoever names them on — the NFO's actors.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonRef {
    /// The person here the credit is filed under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gender: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub popularity: Option<f64>,
}

/// A credit on one episode of a series rather than on the series: a guest,
/// the director or a writer of that episode.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeCredit {
    /// Where the source numbers the episode.
    pub season_number: i32,
    pub episode_number: i32,
    /// TMDB's id of the episode: how the credit finds the episode it is of
    /// in the work's own numbering, which may be TheTVDB's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_episode_id: Option<i64>,
    pub credit: Credit,
}

/// What a work's sources say of the people its credits name, and, read back,
/// whom each credit is filed under.
#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct People {
    /// The credits of single episodes, as a source gave them: written with
    /// the work, never read back with it — a long series has thousands, and
    /// a person's page reads its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub episodes: Vec<EpisodeCredit>,
    /// Who each credit names, by the credit's id: its own and its episodes'.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_credit: BTreeMap<String, PersonRef>,
}

impl People {
    pub fn is_empty(&self) -> bool {
        self.episodes.is_empty() && self.by_credit.is_empty()
    }

    /// What is known of the person a credit names.
    pub fn of(&self, credit: &Credit) -> Option<&PersonRef> {
        self.by_credit.get(&credit.id)
    }
}

/// The language a name is written in, as a page's `lang` takes it — `ja`
/// for 田中真弓, `ko` for 봉준호 — read off its script, and, where a script
/// is written in several languages, off the languages the person's works
/// were made in (`made_in`, as the catalogue files a work's: `jpn`, `zho`):
/// Han characters alone are Japanese for somebody whose works are, Chinese
/// for somebody whose works are; the Latin alphabet is Spanish for a
/// Spaniard's. Nothing where neither says: a screen reader is then left to
/// the page's own language rather than told a wrong one.
pub fn name_language(name: &str, made_in: &[String]) -> Option<String> {
    let mut scripts: Vec<Script> = Vec::new();
    for c in name.chars().filter(|c| c.is_alphabetic()) {
        let script = Script::of(c);
        if !scripts.contains(&script) {
            scripts.push(script);
        }
    }
    // Kana make Japanese of whatever Han is beside them, and Hangul Korean.
    for (script, tag) in [(Script::Kana, "ja"), (Script::Hangul, "ko")] {
        if scripts.contains(&script) {
            return Some(tag.to_string());
        }
    }
    let script = scripts
        .iter()
        .copied()
        .find(|s| *s != Script::Latin)
        .or_else(|| scripts.first().copied())?;
    if let Some(tag) = script.only_language() {
        return Some(tag.to_string());
    }

    // The most common language of their works among those the script writes.
    let mut counted: Vec<(&'static str, usize)> = Vec::new();
    for code in made_in {
        let Some(tag) = two_letters(code).filter(|tag| script.writes(tag)) else {
            continue;
        };
        match counted.iter_mut().find(|(seen, _)| *seen == tag) {
            Some((_, n)) => *n += 1,
            None => counted.push((tag, 1)),
        }
    }
    counted
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(tag, _)| tag.to_string())
}

/// Of the other names somebody goes by — TMDB's, in its order — the one in
/// their own script: written in another than the Latin alphabet (the part
/// that is, of a name given with its reading, see [`in_own_script`]), in a
/// language [`name_language`] can tell; the name as it is written in Han
/// characters before a reading of it in kana, `宮崎駿` before `みやざき
/// はやお`; a name given on its own before one given beside a reading, which
/// is how TMDB's lists give the names somebody also wrote or played under;
/// else the first.
pub fn original_name(names: &[String], made_in: &[String]) -> Option<String> {
    names
        .iter()
        .enumerate()
        .filter_map(|(at, name)| {
            let own = in_own_script(name)?;
            let alone = own == name.trim();
            name_language(&own, made_in)
                .is_some()
                .then_some((at, alone, own))
        })
        .min_by_key(|(at, alone, own)| {
            let written = own.chars().any(|c| Script::of(c) == Script::Han);
            (!written, !*alone, *at)
        })
        .map(|(_, _, own)| own)
}

/// What a source gives as somebody's original name, where it is their name
/// in their own script: written in another than the Latin alphabet, every
/// letter of it. TMDB's is typed by whoever filed them, and some write a
/// reading beside it — `Itou Kentarou / 伊藤 健太郎` — of which the part in
/// that script is the name; Han characters before kana, as in
/// [`original_name`]. Nothing where no part is: a spelling in the Latin
/// alphabet is one more spelling, not the name in their own script.
pub fn in_own_script(given: &str) -> Option<String> {
    given
        .split('/')
        .map(str::trim)
        .filter(|part| {
            let mut letters = part.chars().filter(|c| c.is_alphabetic()).peekable();
            letters.peek().is_some() && letters.all(|c| c > '\u{024F}')
        })
        .min_by_key(|part| !part.chars().any(|c| Script::of(c) == Script::Han))
        .map(str::to_string)
}

/// The writing systems [`name_language`] tells apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Script {
    Latin,
    Kana,
    Hangul,
    Han,
    Cyrillic,
    Greek,
    Hebrew,
    Arabic,
    Thai,
    Devanagari,
    Bengali,
    Tamil,
    Telugu,
    Georgian,
    Armenian,
    Other,
}

impl Script {
    fn of(c: char) -> Self {
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Self::Kana,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Self::Hangul,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF => Self::Han,
            0x0370..=0x03FF | 0x1F00..=0x1FFF => Self::Greek,
            0x0400..=0x052F => Self::Cyrillic,
            0x0530..=0x058F => Self::Armenian,
            0x0590..=0x05FF => Self::Hebrew,
            0x0600..=0x06FF | 0x0750..=0x077F => Self::Arabic,
            0x0900..=0x097F => Self::Devanagari,
            0x0980..=0x09FF => Self::Bengali,
            0x0B80..=0x0BFF => Self::Tamil,
            0x0C00..=0x0C7F => Self::Telugu,
            0x0E00..=0x0E7F => Self::Thai,
            0x10A0..=0x10FF => Self::Georgian,
            0x0041..=0x024F | 0x1E00..=0x1EFF => Self::Latin,
            _ => Self::Other,
        }
    }

    /// The one language a script is written in, as far as names go.
    fn only_language(self) -> Option<&'static str> {
        Some(match self {
            Self::Kana => "ja",
            Self::Hangul => "ko",
            Self::Greek => "el",
            Self::Hebrew => "he",
            Self::Thai => "th",
            Self::Bengali => "bn",
            Self::Tamil => "ta",
            Self::Telugu => "te",
            Self::Georgian => "ka",
            Self::Armenian => "hy",
            _ => return None,
        })
    }

    /// Whether a language — a two-letter tag — is written in it.
    fn writes(self, tag: &str) -> bool {
        match self {
            Self::Han => matches!(tag, "ja" | "zh"),
            Self::Cyrillic => matches!(tag, "ru" | "uk" | "be" | "bg" | "sr" | "mk" | "kk" | "mn"),
            Self::Arabic => matches!(tag, "ar" | "fa" | "ur" | "ps"),
            Self::Devanagari => matches!(tag, "hi" | "mr" | "ne"),
            Self::Latin => LATIN.contains(&tag),
            _ => false,
        }
    }
}

/// The languages written in the Latin alphabet whose names a work may be
/// filed under here.
const LATIN: &[&str] = &[
    "en", "fr", "es", "de", "it", "pt", "nl", "sv", "da", "no", "nb", "fi", "pl", "cs", "sk", "hu",
    "ro", "tr", "vi", "id", "ms", "tl", "hr", "sl", "et", "lv", "lt", "is", "ca", "eu", "gl", "ga",
    "cy", "sq", "bs", "af",
];

/// A work's language as the catalogue files it — ISO 639-2, `jpn` — as the
/// two letters a page's `lang` takes; a code of two letters already is one.
fn two_letters(code: &str) -> Option<&'static str> {
    let code = code.trim().to_ascii_lowercase();
    Some(match code.as_str() {
        "jpn" | "ja" => "ja",
        "zho" | "chi" | "cmn" | "yue" | "zh" | "cn" => "zh",
        "kor" | "ko" => "ko",
        "rus" | "ru" => "ru",
        "ukr" | "uk" => "uk",
        "bel" | "be" => "be",
        "bul" | "bg" => "bg",
        "srp" | "sr" => "sr",
        "mkd" | "mac" | "mk" => "mk",
        "kaz" | "kk" => "kk",
        "mon" | "mn" => "mn",
        "ara" | "ar" => "ar",
        "fas" | "per" | "fa" => "fa",
        "urd" | "ur" => "ur",
        "pus" | "ps" => "ps",
        "hin" | "hi" => "hi",
        "mar" | "mr" => "mr",
        "nep" | "ne" => "ne",
        "eng" | "en" => "en",
        "fra" | "fre" | "fr" => "fr",
        "spa" | "es" => "es",
        "deu" | "ger" | "de" => "de",
        "ita" | "it" => "it",
        "por" | "pt" => "pt",
        "nld" | "dut" | "nl" => "nl",
        "swe" | "sv" => "sv",
        "dan" | "da" => "da",
        "nor" | "no" => "no",
        "nob" | "nb" => "nb",
        "fin" | "fi" => "fi",
        "pol" | "pl" => "pl",
        "ces" | "cze" | "cs" => "cs",
        "slk" | "slo" | "sk" => "sk",
        "hun" | "hu" => "hu",
        "ron" | "rum" | "ro" => "ro",
        "tur" | "tr" => "tr",
        "vie" | "vi" => "vi",
        "ind" | "id" => "id",
        "msa" | "may" | "ms" => "ms",
        "tgl" | "fil" | "tl" => "tl",
        "hrv" | "hr" => "hr",
        "slv" | "sl" => "sl",
        "est" | "et" => "et",
        "lav" | "lv" => "lv",
        "lit" | "lt" => "lt",
        "isl" | "ice" | "is" => "is",
        "cat" | "ca" => "ca",
        "eus" | "baq" | "eu" => "eu",
        "glg" | "gl" => "gl",
        "gle" | "ga" => "ga",
        "cym" | "wel" | "cy" => "cy",
        "sqi" | "alb" | "sq" => "sq",
        "bos" | "bs" => "bs",
        "afr" | "af" => "af",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made_in(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn a_job_is_read_back_whatever_its_case() {
        assert_eq!("Writing".parse::<Job>().expect("a job"), Job::Writing);
        assert!("catering".parse::<Job>().is_err());
        for job in [Job::Acting, Job::Directing, Job::Writing, Job::Producing] {
            assert_eq!(job.as_str().parse::<Job>().expect("read back"), job);
        }
    }

    #[test]
    fn the_name_in_their_own_script_is_the_one_written_in_it() {
        let names = |given: &[&str]| given.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let japanese = made_in(&["jpn"]);
        let known = names(&["Tomohiro Suzuki", "すずき ともひろ", "鈴木智博"]);
        assert_eq!(
            original_name(&known, &japanese).as_deref(),
            Some("鈴木智博"),
            "the name as written before its reading"
        );
        // As TMDB lists them: the names they also worked under, each beside
        // its reading, before their own.
        let pen_names = names(&[
            "Hai Kyoutarou / 胚 京太郎",
            "Itou Kentarou / 伊藤 健太郎",
            "Kentarō Itō",
            "いとう けんたろう",
            "伊藤 健太郎",
        ]);
        assert_eq!(
            original_name(&pen_names, &japanese).as_deref(),
            Some("伊藤 健太郎"),
            "the name on its own before one beside a reading"
        );
        let paired = names(&["Kentarō Itō", "Itou Kentarou / 伊藤 健太郎"]);
        assert_eq!(
            original_name(&paired, &japanese).as_deref(),
            Some("伊藤 健太郎"),
            "the part in their script, without the reading"
        );
        let read_only = names(&["すずき ともひろ", "Suzuki Tomohiro"]);
        assert_eq!(
            original_name(&read_only, &japanese).as_deref(),
            Some("すずき ともひろ")
        );
        // Latin letters are never it, and a script no work says the language
        // of is not either.
        assert_eq!(original_name(&names(&["Bryan Lee Cranston"]), &[]), None);
        assert_eq!(original_name(&names(&["成龍"]), &made_in(&["eng"])), None);
    }

    #[test]
    fn a_source_s_original_name_is_kept_where_it_is_written_in_their_own_script() {
        assert_eq!(in_own_script("田中真弓").as_deref(), Some("田中真弓"));
        assert_eq!(
            in_own_script("Иван Охлобыстин").as_deref(),
            Some("Иван Охлобыстин")
        );
        assert_eq!(
            in_own_script("Itou Kentarou / 伊藤 健太郎").as_deref(),
            Some("伊藤 健太郎"),
            "the reading beside it left out"
        );
        assert_eq!(
            in_own_script("いとう けんたろう / 伊藤健太郎").as_deref(),
            Some("伊藤健太郎"),
            "as written before its reading in kana"
        );
        for spelling in ["José García", "Itou Kentarou", "伊藤 Kentarou", " / ", ""] {
            assert_eq!(in_own_script(spelling), None, "{spelling}");
        }
    }

    #[test]
    fn a_name_is_in_the_language_its_script_and_its_works_say() {
        // A script one language writes, whatever the works.
        assert_eq!(name_language("봉준호", &[]).as_deref(), Some("ko"));
        assert_eq!(name_language("たなか まゆみ", &[]).as_deref(), Some("ja"));
        assert_eq!(name_language("Μελίνα Μερκούρη", &[]).as_deref(), Some("el"));
        // Han alone: Japanese for a Japanese career, Chinese for a Chinese
        // one, and nothing for one that says neither.
        assert_eq!(
            name_language("宮崎駿", &made_in(&["jpn", "jpn", "eng"])).as_deref(),
            Some("ja")
        );
        assert_eq!(
            name_language("成龍", &made_in(&["zho", "cn", "eng"])).as_deref(),
            Some("zh")
        );
        assert_eq!(name_language("成龍", &made_in(&["eng"])), None);
        // Kana beside Han make it Japanese.
        assert_eq!(name_language("宮崎 はやお", &[]).as_deref(), Some("ja"));
        // The Latin alphabet: the language most of the works were made in.
        assert_eq!(
            name_language(
                "Pedro Almodóvar Caballero",
                &made_in(&["spa", "spa", "eng"])
            )
            .as_deref(),
            Some("es")
        );
        assert_eq!(name_language("Пётр Тодоровский", &made_in(&["jpn"])), None);
        assert_eq!(name_language("", &made_in(&["eng"])), None);
    }
}

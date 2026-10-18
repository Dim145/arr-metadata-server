//! The override registry — what a human is allowed to edit, and how an edit is
//! applied on top of provider data.
//!
//! A manual edit is stored as `(media_id, scope, field, json_value)` and wins
//! over every provider, forever, until it is deleted. That is the whole locking
//! mechanism: refreshes rewrite provider snapshots, never overrides.
//!
//! Overrides are applied by round-tripping the entity through `serde_json`, so
//! the field names here are exactly the entity's serde names and a new field
//! becomes editable by adding one row to the registry below.
//!
//! A text — a title, an overview — may be locked in one language alone (see
//! [`TRANSLATED`]), and what a source gave the work besides its fields — a
//! credit, an image, an alternative title — is hidden or corrected by a lock
//! that names it by what it is (see [`Child`]).

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::domain::{
    AlternativeTitle, Credit, CreditType, Episode, Image, MediaItem, Season, Translation,
};

/// What a field holds. Drives validation here and the input widget in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FieldType {
    /// Single-line text.
    Text,
    /// Multi-line text.
    LongText,
    Integer,
    Float,
    Boolean,
    /// A day, `YYYY-MM-DD` — or a date-time, as a provider sometimes gives
    /// a release. Kept as a string.
    Date,
    /// An instant, RFC 3339 with its zone: `2009-03-22T21:00:00Z`. What
    /// Sonarr reads as the moment an episode aired.
    DateTime,
    /// `HH:MM`.
    TimeOfDay,
    /// Array of strings.
    TextList,
    /// The identifiers a work goes by elsewhere, as one object by source:
    /// `{"tmdb": 1396, "imdb": "tt0903747", "mal": [1, 2]}`.
    Ids,
}

impl FieldType {
    /// Whether `value` is an acceptable payload for this field.
    ///
    /// JSON `null` is always accepted: it is how a user clears a field, which
    /// is distinct from having no override at all.
    fn accepts(self, value: &Value) -> bool {
        match (self, value) {
            (_, Value::Null) => true,
            (Self::Text | Self::LongText, Value::String(_)) => true,
            // Shaped as the readers of these fields expect: Sonarr parses a
            // date and an instant, and a day typed in as "TBA" was stored,
            // served, and failed there.
            (Self::Date, Value::String(s)) => is_day(s) || is_instant(s),
            (Self::DateTime, Value::String(s)) => is_instant(s),
            (Self::TimeOfDay, Value::String(s)) => {
                chrono::NaiveTime::parse_from_str(s, "%H:%M").is_ok()
            }
            // In `i32` range, because that is what every integer field is. A
            // number that is merely a valid JSON integer passes serde on the
            // way in and fails it on the way out, in `patch_in_place` — which
            // is inside `load`, so the work, the whole catalogue list and the
            // refresh sweep all start erroring, permanently, on one bad paste.
            (Self::Integer, Value::Number(n)) => {
                n.as_i64().is_some_and(|n| i32::try_from(n).is_ok())
            }
            (Self::Float, Value::Number(_)) => true,
            (Self::Boolean, Value::Bool(_)) => true,
            (Self::TextList, Value::Array(items)) => items.iter().all(Value::is_string),
            (Self::Ids, Value::Object(_)) => {
                serde_json::from_value::<crate::domain::ExternalIds>(value.clone()).is_ok()
            }
            _ => false,
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Text => "a string",
            Self::LongText => "a string",
            Self::Integer => "a whole number between -2147483648 and 2147483647",
            Self::Float => "a number",
            Self::Boolean => "true or false",
            Self::Date => "a date, YYYY-MM-DD, or a date-time",
            Self::DateTime => "a date-time with its zone, like 2009-03-22T21:00:00Z",
            Self::TimeOfDay => "a time, HH:MM",
            Self::TextList => "an array of strings",
            Self::Ids => {
                "an object of identifiers by source: tmdb, tvdb, tvmaze, tvrage, trakt and fankai \
                 as numbers, imdb and wikidata as strings, mal and anilist as arrays of numbers"
            }
        }
    }
}

/// `YYYY-MM-DD`, and a real day of the calendar.
fn is_day(value: &str) -> bool {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

/// An RFC 3339 date-time, its zone included.
fn is_instant(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldDef {
    /// The entity's serde name; also the key stored in `media_override.field`.
    pub name: &'static str,
    pub field_type: FieldType,
    /// Shown as the input label in the web UI.
    pub label: &'static str,
}

const fn f(name: &'static str, field_type: FieldType, label: &'static str) -> FieldDef {
    FieldDef {
        name,
        field_type,
        label,
    }
}

use FieldType::*;

/// Editable fields on a work.
pub const ITEM_FIELDS: &[FieldDef] = &[
    f("title", Text, "Title"),
    f("sortTitle", Text, "Sort title"),
    f("originalTitle", Text, "Original title"),
    f("overview", LongText, "Overview"),
    // The line under the title, as TMDB gives it.
    f("tagline", Text, "Tagline"),
    f("status", Text, "Status"),
    f("originalLanguage", Text, "Original language"),
    f("originalCountry", Text, "Original country"),
    // Every country it was made in, `US`, `GB`.
    f("productionCountries", TextList, "Production countries"),
    f("runtime", Integer, "Runtime (minutes)"),
    f("year", Integer, "Year"),
    f("firstAired", Date, "First aired"),
    f("lastAired", Date, "Last aired"),
    f("inCinemas", Date, "In cinemas"),
    f("physicalRelease", Date, "Physical release"),
    f("digitalRelease", Date, "Digital release"),
    f("airTime", TimeOfDay, "Air time"),
    f("network", Text, "Network"),
    f("studio", Text, "Studio"),
    f("productionCompanies", TextList, "Production companies"),
    f("contentRating", Text, "Content rating"),
    // The country the rating is of, as Radarr matches it: `US`, `FR`.
    f("contentRatingCountry", Text, "Content rating country"),
    f("homepage", Text, "Homepage"),
    f("trailerYoutubeId", Text, "YouTube trailer id"),
    f("themeMusic", Text, "Theme music URL"),
    // What TheTVDB adds to a homonym's name — `2023`, `US` — and the Sonarr
    // surface gives back beside the title.
    f("titleQualifier", Text, "Title qualifier"),
    // The TMDB collection a film belongs to.
    f("collectionTmdbId", Integer, "TMDB collection id"),
    f("genres", TextList, "Genres"),
    f("keywords", TextList, "Keywords"),
    // What AniList calls the entry: `TV`, `OVA`, `MOVIE`.
    f("format", Text, "AniList format"),
    // Its identity: written to the row as well as locked, since lists,
    // addresses and the clients' lookups read the row — see `IDENTITY`.
    // The poster and the background to lead with: an image's address, among
    // the work's own — see `CHOSEN_IMAGES`.
    f("primaryPoster", Text, "Primary poster"),
    f("primaryFanart", Text, "Primary background"),
    f("isAdult", Boolean, "Adult"),
    f("slug", Text, "Slug"),
    f("externalIds", Ids, "External identifiers"),
];

/// The fields that are the work's identity: locked like any other, and
/// written through to the row and its identifier rows, where the catalogue's
/// lists, its addresses and the clients' lookups read them.
pub const IDENTITY: &[&str] = &["isAdult", "slug", "externalIds"];

/// The images a person chose to lead with, by the address the work's images
/// go by — a provider's, or an upload's `upload:` origin. Not a field of the
/// work: applied by putting the image first among its kind and naming it.
pub const CHOSEN_IMAGES: &[&str] = &["primaryPoster", "primaryFanart"];

/// The kind of image a choice is for.
fn chosen_kind(field: &str) -> Option<crate::domain::CoverType> {
    match field {
        "primaryPoster" => Some(crate::domain::CoverType::Poster),
        "primaryFanart" => Some(crate::domain::CoverType::Fanart),
        _ => None,
    }
}

/// Put the image a person chose first among the work's own of its kind,
/// and name it — or, when the sources no longer list it, bring it back, as
/// long as its address is one a client can follow: an upload that is gone
/// leaves the work with no choice.
fn lead_with(item: &mut MediaItem, kind: crate::domain::CoverType, address: &str) {
    let found = item
        .images
        .iter()
        .position(|i| i.cover_type == kind && i.season_number.is_none() && i.url == address);
    let mut image = match found {
        Some(at) => item.images.remove(at),
        None if address.starts_with("https://") || address.starts_with("http://") => {
            crate::domain::Image {
                id: format!("chosen-{}", kind.as_str()),
                season_number: None,
                cover_type: kind,
                url: address.to_string(),
                language: None,
                sort_order: 0,
                source: None,
                is_manual: true,
            }
        }
        None => return,
    };
    // First among its kind, and so for anything that re-sorts them.
    let lowest = item
        .images
        .iter()
        .filter(|i| i.cover_type == kind)
        .map(|i| i.sort_order)
        .min()
        .unwrap_or(0);
    image.sort_order = image.sort_order.min(lowest.saturating_sub(1));
    let at = item
        .images
        .iter()
        .position(|i| i.cover_type.priority() >= kind.priority())
        .unwrap_or(item.images.len());
    let id = image.id.clone();
    item.images.insert(at, image);
    match kind {
        crate::domain::CoverType::Poster => item.primary_images.poster = Some(id),
        _ => item.primary_images.fanart = Some(id),
    }
}

/// The same for a season: the poster a person chose leads the season's own
/// posters and is named on the season — or is brought back under it when the
/// sources no longer list it, as long as a client can follow its address.
fn lead_season_with(item: &mut MediaItem, number: i32, address: &str) {
    let kind = crate::domain::CoverType::Poster;
    let Some(season) = item.seasons.iter_mut().find(|s| s.season_number == number) else {
        return;
    };
    let found = season
        .images
        .iter()
        .position(|i| i.cover_type == kind && i.url == address);
    let mut image = match found {
        Some(at) => season.images.remove(at),
        None if address.starts_with("https://") || address.starts_with("http://") => {
            crate::domain::Image {
                id: format!("chosen-season-{number}-{}", kind.as_str()),
                season_number: Some(number),
                cover_type: kind,
                url: address.to_string(),
                language: None,
                sort_order: 0,
                source: None,
                is_manual: true,
            }
        }
        None => return,
    };
    let lowest = season
        .images
        .iter()
        .filter(|i| i.cover_type == kind)
        .map(|i| i.sort_order)
        .min()
        .unwrap_or(0);
    image.sort_order = image.sort_order.min(lowest.saturating_sub(1));
    let at = season
        .images
        .iter()
        .position(|i| i.cover_type.priority() >= kind.priority())
        .unwrap_or(season.images.len());
    let id = image.id.clone();
    season.images.insert(at, image);
    season.primary_images.poster = Some(id);
}

/// The fields that hold an address, and whether an upload's `upload:` origin
/// may stand in it: refused unless a client can follow it, so that nothing a
/// browser would run as a script — `javascript:`, a `data:` page — is served
/// to the public page or a client as a link.
const ADDRESSES: &[(&str, bool)] = &[("homepage", false), ("themeMusic", true), ("image", true)];

fn is_web_address(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

/// A YouTube video id: the eleven-odd characters after `v=`, nothing of the
/// address around them, which the interface and the NFO build themselves.
fn is_youtube_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether a slug is one this server would make: lowercase letters, digits
/// and single hyphens, as `make_slug` writes them.
pub fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Put the locked identity — adult, slug, identifiers — onto a work about
/// to be written, so that a refresh writes the row with what is locked
/// rather than what a provider said.
pub fn pin_identity(item: &mut MediaItem, overrides: &[Override]) {
    for ov in overrides {
        if ov.scope != "item" || ov.in_one_language() {
            continue;
        }
        match (ov.field.as_str(), &ov.value) {
            ("isAdult", Some(Value::Bool(adult))) => item.is_adult = *adult,
            ("slug", Some(Value::String(slug))) if is_slug(slug) => item.slug = slug.clone(),
            ("externalIds", Some(value @ Value::Object(_))) => {
                if let Ok(ids) = serde_json::from_value(value.clone()) {
                    item.external_ids = ids;
                }
            }
            _ => {}
        }
    }
}

/// Editable fields on a season.
pub const SEASON_FIELDS: &[FieldDef] = &[
    f("title", Text, "Title"),
    f("overview", LongText, "Overview"),
    f("airDate", Date, "Air date"),
    // The season as its providers number it: what a link to its page there
    // is built from, and what a hand-made season has none of.
    f("tmdbId", Integer, "TMDB id"),
    f("tvdbId", Integer, "TheTVDB id"),
    // The poster to lead the season with, by address, as the work's own
    // choice is made: not a field of the season but a choice among its images.
    f("primaryPoster", Text, "Primary poster"),
];

/// Editable fields on an episode.
pub const EPISODE_FIELDS: &[FieldDef] = &[
    f("title", Text, "Title"),
    f("overview", LongText, "Overview"),
    f("airDate", Date, "Air date"),
    f("airDateUtc", DateTime, "Air date and time (UTC)"),
    f("runtime", Integer, "Runtime (minutes)"),
    f("finaleType", Text, "Finale type"),
    f("image", Text, "Still image URL"),
    f("absoluteEpisodeNumber", Integer, "Absolute episode number"),
    // Where a special belongs among the regular episodes, as TheTVDB files
    // it and Sonarr slots it: after a season, or before one episode.
    f("airedAfterSeasonNumber", Integer, "Aired after season"),
    f("airedBeforeSeasonNumber", Integer, "Aired before season"),
    f("airedBeforeEpisodeNumber", Integer, "Aired before episode"),
    // The episode as its providers number it.
    f("tvdbId", Integer, "TheTVDB id"),
    f("tmdbId", Integer, "TMDB id"),
];

// ─── scopes ──────────────────────────────────────────────────────────────────

/// What an override addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// The work itself.
    Item,
    Season(i32),
    Episode {
        season: i32,
        episode: i32,
    },
}

impl Scope {
    /// The field registry that applies to this scope.
    pub fn fields(self) -> &'static [FieldDef] {
        match self {
            Self::Item => ITEM_FIELDS,
            Self::Season(_) => SEASON_FIELDS,
            Self::Episode { .. } => EPISODE_FIELDS,
        }
    }

    pub fn field(self, name: &str) -> Option<&'static FieldDef> {
        self.fields().iter().find(|d| d.name == name)
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, fmtr: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Item => fmtr.write_str("item"),
            Self::Season(n) => write!(fmtr, "season:{n}"),
            Self::Episode { season, episode } => write!(fmtr, "episode:{season}x{episode}"),
        }
    }
}

impl FromStr for Scope {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        if s == "item" {
            return Ok(Self::Item);
        }

        if let Some(rest) = s.strip_prefix("season:") {
            let n = rest
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid season number in scope {s:?}"))?;
            return Ok(Self::Season(n));
        }

        if let Some(rest) = s.strip_prefix("episode:") {
            let (season, episode) = rest.split_once('x').ok_or_else(|| {
                anyhow::anyhow!("episode scope must look like episode:1x2, got {s:?}")
            })?;
            return Ok(Self::Episode {
                season: season
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid season number in scope {s:?}"))?,
                episode: episode
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid episode number in scope {s:?}"))?,
            });
        }

        anyhow::bail!("unknown override scope {s:?}")
    }
}

// ─── overrides ───────────────────────────────────────────────────────────────

/// One stored manual edit.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Override {
    /// `item`, `season:3`, `episode:3x7` — or a child the sources gave, as
    /// [`Child`] names one: `credit:actor:tmdb:17419`.
    pub scope: String,
    pub field: String,
    /// The language a text is locked in, as translations are filed — `fra`
    /// — when it is locked in that one alone; absent for every language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// `None` means the user explicitly cleared the field.
    #[schema(required)]
    pub value: Option<Value>,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

impl Override {
    /// Whether the lock holds in one language alone.
    pub fn in_one_language(&self) -> bool {
        self.language.as_deref().is_some_and(|l| !l.is_empty())
    }
}

// ─── languages ───────────────────────────────────────────────────────────────

/// The fields that are text in a language — what a translation gives in
/// another — and so the ones a lock may hold for one language alone: a
/// title and an overview, of the work, a season or an episode, and the
/// work's tagline.
///
/// A work served in a language is given, field by field, what was locked in
/// that language, else what was locked in every language, else the
/// translation — or, with none, the text it is stored in. A lock in every
/// language beats every translation; a lock in one beats both, in it.
pub const TRANSLATED: &[&str] = &["title", "overview", "tagline"];

/// Whether a field may be locked in one language alone.
pub fn validate_language(scope: Scope, field: &str) -> Result<(), String> {
    if scope.field(field).is_some() && TRANSLATED.contains(&field) {
        Ok(())
    } else {
        Err(format!(
            "{field:?} is the same in every language: lock it without one"
        ))
    }
}

/// A text a person locked, in one language or in every one: what a work
/// carries of its locks to the language it is served in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LockedText {
    /// `item`, `season:3` or `episode:3x7`.
    pub scope: String,
    pub field: String,
    /// The language's key, as translations are filed — `fra` — or empty for
    /// every language.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    /// `None` where it was cleared, and locked so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// The lock a text of a work is served with in a language: the one made in
/// that language, else the one made in every language. `None` when neither
/// was made, and the translation or the stored text stands.
pub fn text_lock<'a>(
    item: &'a MediaItem,
    scope: &str,
    field: &str,
    language: &str,
) -> Option<&'a LockedText> {
    let made_in = |wanted: &str| {
        item.locked_texts
            .iter()
            .find(|t| t.scope == scope && t.field == field && t.language == wanted)
    };
    (!language.is_empty())
        .then(|| made_in(language))
        .flatten()
        .or_else(|| made_in(""))
}

/// Lay over a work the texts locked for a language: in it, else in every
/// language. A text locked in another language only is left as it is — the
/// translation laid over it, or the text the work is stored with.
pub fn lay(item: &mut MediaItem, language: &str) {
    let mut chosen: Vec<(String, String, Option<String>)> = Vec::new();
    for text in &item.locked_texts {
        if chosen
            .iter()
            .any(|(scope, field, _)| *scope == text.scope && *field == text.field)
        {
            continue;
        }
        if let Some(lock) = text_lock(item, &text.scope, &text.field, language) {
            chosen.push((lock.scope.clone(), lock.field.clone(), lock.value.clone()));
        }
    }
    for (scope, field, value) in chosen {
        set_text(item, &scope, &field, value);
    }
}

/// Say which of a work's fields are locked as it is served in `language`:
/// a text locked in it, or in every language, as `scope/field`; one locked
/// in another language only as `scope/field@fra`, which counts it without
/// claiming it here. What the language overlay reads before it lays a
/// translation, and what a client is told. The other locks stand.
pub fn mark(item: &mut MediaItem, language: &str) {
    if item.locked_texts.is_empty() {
        return;
    }

    let texts = &item.locked_texts;
    let mut locked: Vec<String> = item
        .locked_fields
        .iter()
        .filter(|path| !names_a_text(path, texts))
        .cloned()
        .collect();
    for text in texts {
        locked.push(if text.language.is_empty() || text.language == language {
            format!("{}/{}", text.scope, text.field)
        } else {
            format!("{}/{}@{}", text.scope, text.field, text.language)
        });
    }
    locked.sort_unstable();
    locked.dedup();
    item.locked_fields = locked;
}

/// Lay over a work, and say locked, the texts locked in `language` alone —
/// another language than the one the catalogue keeps its works in — and no
/// other: what a relay writes into a service's own document in that
/// language. A text locked in every language is the catalogue's own text;
/// in a document in another language the service's text stands, as it did
/// before a lock had a language. The work comes as it is read (the stored
/// language's texts laid and marked), and every text's path is taken out of
/// its locks before those locked in `language` are put back. What is not
/// text stays locked as it was.
pub fn lay_only_in(item: &mut MediaItem, language: &str) {
    let texts = std::mem::take(&mut item.locked_texts);
    item.locked_fields
        .retain(|path| !names_a_text(path, &texts));
    let own: Vec<LockedText> = texts
        .into_iter()
        .filter(|text| !language.is_empty() && text.language == language)
        .collect();
    item.locked_fields.extend(
        own.iter()
            .map(|text| format!("{}/{}", text.scope, text.field)),
    );
    item.locked_fields.sort_unstable();
    item.locked_fields.dedup();
    item.locked_texts = own;
    lay(item, language);
}

/// Whether a locked path — `item/title`, `item/title@fra` — is one of a
/// locked text's.
fn names_a_text(path: &str, texts: &[LockedText]) -> bool {
    let Some((scope, field)) = path.rsplit_once('/') else {
        return false;
    };
    let field = field.split_once('@').map_or(field, |(field, _)| field);
    texts.iter().any(|t| t.scope == scope && t.field == field)
}

/// Make the translations a work carries say what each language is served:
/// in every language held, a text locked in it, or in every language, takes
/// the translation's place, and a language a text was locked in without a
/// translation of its own is given one. Sonarr's and Radarr's lists of the
/// work's languages are made from these, so each entry says what a request
/// in its language is answered with. Laid again leaves them as they are.
pub fn fold_translations(item: &mut MediaItem) {
    // What a translation carries: the work's title and overview.
    let own =
        |t: &&LockedText| t.scope == "item" && matches!(t.field.as_str(), "title" | "overview");
    if !item.locked_texts.iter().any(|t| own(&t)) {
        return;
    }

    let mut missing: Vec<String> = Vec::new();
    for text in item.locked_texts.iter().filter(own) {
        if !text.language.is_empty()
            && !item
                .translations
                .iter()
                .any(|t| t.language == text.language)
            && !missing.contains(&text.language)
        {
            missing.push(text.language.clone());
        }
    }
    item.translations
        .extend(missing.into_iter().map(|language| Translation {
            language,
            title: None,
            overview: None,
            is_manual: true,
        }));

    // What each translation is given, read before any is written: the locks
    // are the work's, and so are its translations.
    let laid: Vec<Laid> = item
        .translations
        .iter()
        .map(|t| {
            let title = text_lock(item, "item", "title", &t.language);
            let overview = text_lock(item, "item", "overview", &t.language);
            Laid {
                title: title.map(|lock| lock.value.clone()),
                overview: overview.map(|lock| lock.value.clone()),
                in_it: [title, overview]
                    .into_iter()
                    .flatten()
                    .any(|lock| lock.language == t.language),
            }
        })
        .collect();
    for (translation, laid) in item.translations.iter_mut().zip(laid) {
        if let Some(title) = laid.title {
            translation.title = title;
        }
        if let Some(overview) = laid.overview {
            translation.overview = overview;
        }
        if laid.in_it {
            translation.is_manual = true;
        }
    }
}

/// What a translation is given of the work's locks: its title and its
/// overview where one is locked for its language, and whether one was locked
/// in that language itself.
struct Laid {
    title: Option<Option<String>>,
    overview: Option<Option<String>>,
    in_it: bool,
}

/// Put a text in its place: the work's own, a season's, an episode's. A
/// title is never cleared, as the registry has it.
fn set_text(item: &mut MediaItem, scope: &str, field: &str, value: Option<String>) {
    let Ok(scope) = scope.parse::<Scope>() else {
        return;
    };
    match scope {
        Scope::Item => match field {
            "title" => {
                if let Some(title) = value {
                    item.title = title;
                }
            }
            "overview" => item.overview = value,
            _ => patch_text(item, field, value),
        },
        Scope::Season(n) => {
            if let Some(season) = item.seasons.iter_mut().find(|s| s.season_number == n) {
                match field {
                    "title" => season.title = value,
                    "overview" => season.overview = value,
                    _ => patch_text(season, field, value),
                }
            }
        }
        Scope::Episode { season, episode } => {
            if let Some(ep) = item
                .episodes
                .iter_mut()
                .find(|e| e.season_number == season && e.episode_number == episode)
            {
                match field {
                    "title" => {
                        if let Some(title) = value {
                            ep.title = title;
                        }
                    }
                    "overview" => ep.overview = value,
                    _ => patch_text(ep, field, value),
                }
            }
        }
    }
}

/// Any other text, by its serde name, as the registry's patches are laid.
fn patch_text<T>(target: &mut T, field: &str, value: Option<String>)
where
    T: Serialize + serde::de::DeserializeOwned,
{
    let mut patch = serde_json::Map::new();
    patch.insert(field.to_string(), value.map_or(Value::Null, Value::String));
    if let Err(e) = patch_in_place(target, patch) {
        tracing::warn!(field, error = %e, "a locked text could not be laid");
    }
}

// ─── what the sources gave ───────────────────────────────────────────────────

/// A child the sources gave a work, as a lock names it: by what it is, not by
/// the row it is kept in, which a refresh writes again under another id. A
/// lock on one hides it, or corrects it; what was added by hand is changed
/// or removed as itself, and none of these apply to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Child {
    /// A credit: what the person did, and who they are.
    Credit { kind: CreditType, person: Person },
    /// An image, by its address as its source gives it.
    Image(String),
    /// An alternative title, by its text and the language it is filed under.
    AlternativeTitle {
        language: Option<String>,
        title: String,
    },
}

/// Who a credit names: by their TMDB id, or by their name where the source
/// gave none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Person {
    Tmdb(i64),
    Named(String),
}

/// What may be locked on a credit a source gave: hidden, the character
/// played, its place among the others of its kind.
pub const CREDIT_FIELDS: &[FieldDef] = &[
    f("hidden", Boolean, "Hidden"),
    f("characterName", Text, "Character"),
    f("sortOrder", Integer, "Order"),
];

/// What may be locked on an image or an alternative title a source gave:
/// hidden.
pub const HIDDEN_ONLY: &[FieldDef] = &[f("hidden", Boolean, "Hidden")];

impl Child {
    pub fn of_credit(credit: &Credit) -> Self {
        Self::Credit {
            kind: credit.credit_type,
            person: match credit.tmdb_person_id {
                Some(id) => Person::Tmdb(id),
                None => Person::Named(credit.person_name.trim().to_string()),
            },
        }
    }

    pub fn of_image(image: &Image) -> Self {
        Self::Image(image.url.clone())
    }

    pub fn of_title(title: &AlternativeTitle) -> Self {
        Self::AlternativeTitle {
            language: title.language.clone().filter(|l| !l.is_empty()),
            title: title.title.clone(),
        }
    }

    /// What may be locked on it.
    pub fn fields(&self) -> &'static [FieldDef] {
        match self {
            Self::Credit { .. } => CREDIT_FIELDS,
            Self::Image(_) | Self::AlternativeTitle { .. } => HIDDEN_ONLY,
        }
    }

    fn noun(&self) -> &'static str {
        match self {
            Self::Credit { .. } => "credit",
            Self::Image(_) => "image",
            Self::AlternativeTitle { .. } => "alternative title",
        }
    }
}

impl fmt::Display for Child {
    fn fmt(&self, fmtr: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credit { kind, person } => match person {
                Person::Tmdb(id) => write!(fmtr, "credit:{}:tmdb:{id}", kind.as_str()),
                Person::Named(name) => write!(fmtr, "credit:{}:name:{name}", kind.as_str()),
            },
            Self::Image(address) => write!(fmtr, "image:{address}"),
            Self::AlternativeTitle { language, title } => write!(
                fmtr,
                "alternativeTitle:{}:{title}",
                language.as_deref().unwrap_or_default()
            ),
        }
    }
}

impl FromStr for Child {
    type Err = anyhow::Error;

    /// `credit:actor:tmdb:17419`, `credit:director:name:Jane Doe`,
    /// `image:https://…`, `alternativeTitle:fr:Les Experts` — or
    /// `alternativeTitle::CSI` for a title filed under no language.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(rest) = s.strip_prefix("credit:") {
            let (kind, person) = rest
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("a credit is named credit:actor:tmdb:17419"))?;
            let kind = match kind {
                "actor" => CreditType::Actor,
                "director" => CreditType::Director,
                "writer" => CreditType::Writer,
                "producer" => CreditType::Producer,
                "guest" => CreditType::Guest,
                other => anyhow::bail!("unknown credit type {other:?}"),
            };
            let person = if let Some(id) = person.strip_prefix("tmdb:") {
                Person::Tmdb(
                    id.parse()
                        .map_err(|_| anyhow::anyhow!("a TMDB person id is a number"))?,
                )
            } else if let Some(name) = person.strip_prefix("name:")
                && !name.trim().is_empty()
            {
                Person::Named(name.to_string())
            } else {
                anyhow::bail!("a credit names its person by tmdb:<id> or name:<name>")
            };
            return Ok(Self::Credit { kind, person });
        }

        if let Some(address) = s.strip_prefix("image:")
            && !address.is_empty()
        {
            return Ok(Self::Image(address.to_string()));
        }

        if let Some(rest) = s.strip_prefix("alternativeTitle:")
            && let Some((language, title)) = rest.split_once(':')
            && !title.is_empty()
        {
            return Ok(Self::AlternativeTitle {
                language: (!language.is_empty()).then(|| language.to_string()),
                title: title.to_string(),
            });
        }

        anyhow::bail!("{s:?} names no credit, image or alternative title")
    }
}

/// Reject a lock on a child that names something it does not have, or
/// carries the wrong value: hidden is `true` — showing it again is lifting
/// the lock — and an order is a number, never cleared. A character cleared
/// is a credit with no character.
pub fn validate_child(child: &Child, field: &str, value: Option<&Value>) -> Result<(), String> {
    let Some(def) = child.fields().iter().find(|d| d.name == field) else {
        return Err(format!("{field:?} is not locked on an {}", child.noun()));
    };
    match (field, value) {
        ("hidden", Some(Value::Bool(true))) => Ok(()),
        ("hidden", _) => Err("hidden is true: to show it again, lift the lock".into()),
        ("sortOrder", None | Some(Value::Null)) => {
            Err("an order is a number: set one, or lift the lock".into())
        }
        (_, None) => Ok(()),
        (_, Some(v)) if def.field_type.accepts(v) => Ok(()),
        _ => Err(format!("{field:?} expects {}", def.field_type.describe())),
    }
}

/// What the sources gave a work and a person hid. Served to nobody: kept
/// on the work as it is read, for whoever maintains the catalogue to show
/// again, and left out of every answer by
/// [`crate::service::redact_for_reader`].
#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Hidden {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credits: Vec<Credit>,
    /// The work's own and its seasons', a season's with its number.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternative_titles: Vec<AlternativeTitle>,
}

impl Hidden {
    pub fn is_empty(&self) -> bool {
        self.credits.is_empty() && self.images.is_empty() && self.alternative_titles.is_empty()
    }
}

/// Hide and correct what the sources gave, as the locks on it say: the
/// hidden kept apart on the work, the credits corrected — a character, a
/// place among the others — and put back in the order they are read in.
fn edit_children(item: &mut MediaItem, edits: &[(Child, &str, Value)]) {
    if edits.is_empty() {
        return;
    }
    let hides = |key: &Child| {
        edits.iter().any(|(child, field, value)| {
            child == key && *field == "hidden" && value == &Value::Bool(true)
        })
    };

    let mut reordered = false;
    let mut credits = Vec::with_capacity(item.credits.len());
    for mut credit in std::mem::take(&mut item.credits) {
        if credit.is_manual {
            credits.push(credit);
            continue;
        }
        let key = Child::of_credit(&credit);
        reordered |= correct(&mut credit, &key, edits);
        if hides(&key) {
            item.hidden.credits.push(credit);
        } else {
            credits.push(credit);
        }
    }
    // As they are read from the store: by kind, then by place.
    if reordered {
        credits.sort_by(|a, b| {
            a.credit_type
                .as_str()
                .cmp(b.credit_type.as_str())
                .then(a.sort_order.cmp(&b.sort_order))
        });
    }
    item.credits = credits;

    let mut hidden_images = Vec::new();
    for images in std::iter::once(&mut item.images)
        .chain(item.seasons.iter_mut().map(|season| &mut season.images))
    {
        let (shown, hidden): (Vec<Image>, Vec<Image>) = std::mem::take(images)
            .into_iter()
            .partition(|image| image.is_manual || !hides(&Child::of_image(image)));
        *images = shown;
        hidden_images.extend(hidden);
    }
    item.hidden.images.extend(hidden_images);

    let (shown, hidden): (Vec<AlternativeTitle>, Vec<AlternativeTitle>) =
        std::mem::take(&mut item.alternative_titles)
            .into_iter()
            .partition(|title| title.is_manual || !hides(&Child::of_title(title)));
    item.alternative_titles = shown;
    item.hidden.alternative_titles.extend(hidden);
}

/// Correct a credit as the locks on it say: the character it is played as,
/// its place among the others of its kind. Whether its place moved.
fn correct(credit: &mut Credit, key: &Child, edits: &[(Child, &str, Value)]) -> bool {
    let mut moved = false;
    for (_, field, value) in edits.iter().filter(|(child, ..)| child == key) {
        match *field {
            "characterName" => {
                credit.character_name = value
                    .as_str()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .map(str::to_string);
            }
            "sortOrder" => {
                if let Some(order) = value.as_i64().and_then(|n| i32::try_from(n).ok()) {
                    credit.sort_order = order;
                    moved = true;
                }
            }
            _ => {}
        }
    }
    moved
}

/// What a credit is served as when it is read apart from its work — on the
/// person's own page: as the work's locks correct it, or `None` where they
/// hide it. One added by hand is itself.
pub fn edit_credit(mut credit: Credit, locks: &[Override]) -> Option<Credit> {
    if credit.is_manual {
        return Some(credit);
    }
    let key = Child::of_credit(&credit);
    let edits: Vec<(Child, &str, Value)> = locks
        .iter()
        .filter(|lock| !lock.in_one_language())
        .filter_map(|lock| {
            let child = lock.scope.parse::<Child>().ok().filter(|c| *c == key)?;
            validate_child(&child, &lock.field, lock.value.as_ref()).ok()?;
            Some((
                child,
                lock.field.as_str(),
                lock.value.clone().unwrap_or(Value::Null),
            ))
        })
        .collect();
    let hidden = edits
        .iter()
        .any(|(_, field, value)| *field == "hidden" && value == &Value::Bool(true));
    correct(&mut credit, &key, &edits);
    (!hidden).then_some(credit)
}

/// Reject an edit that names an unknown field or carries the wrong type, before
/// it ever reaches the database.
/// Fields a work cannot be read without: cleared, they would leave the work
/// unreadable — not on its own page, not by Sonarr — so a lock on one holds
/// a value or is not held.
const NEVER_CLEARED: &[&str] = &[
    "title",
    "genres",
    "keywords",
    "isAdult",
    "slug",
    "externalIds",
    "primaryPoster",
    "primaryFanart",
];

pub fn validate(scope: Scope, field: &str, value: Option<&Value>) -> Result<(), String> {
    let Some(def) = scope.field(field) else {
        return Err(format!("{field:?} is not editable on scope {scope}"));
    };

    if value.is_none_or(Value::is_null) && NEVER_CLEARED.contains(&field) {
        return Err(format!("{field:?} cannot be cleared: set it, or unlock it"));
    }

    if CHOSEN_IMAGES.contains(&field)
        && let Some(Value::String(address)) = value
        && !(address.starts_with("https://")
            || address.starts_with("http://")
            || address.starts_with("upload:"))
    {
        return Err(format!(
            "{field:?} names one of the work's images, by its address"
        ));
    }

    if field == "slug"
        && let Some(Value::String(slug)) = value
        && !is_slug(slug)
    {
        return Err(
            "a slug is lowercase letters, digits and single hyphens, like breaking-bad-2008".into(),
        );
    }

    if let Some((_, uploads)) = ADDRESSES.iter().find(|(name, _)| *name == field)
        && let Some(Value::String(address)) = value
        && !(is_web_address(address) || (*uploads && address.starts_with("upload:")))
    {
        return Err(format!(
            "{field:?} is an address a client can follow: http:// or https://{}",
            if *uploads {
                ", or an upload's origin"
            } else {
                ""
            }
        ));
    }

    if field == "trailerYoutubeId"
        && let Some(Value::String(id)) = value
        && !is_youtube_id(id)
    {
        return Err(
            "a YouTube id is the letters, digits, - and _ after v=, not the address".into(),
        );
    }

    // Uppercase alpha-2, the form Radarr matches a rating's country against.
    if field == "contentRatingCountry"
        && let Some(Value::String(code)) = value
        && !(code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase()))
    {
        return Err("a rating's country is its two-letter code, in capitals: US, FR".into());
    }

    match value {
        None => Ok(()),
        Some(v) if def.field_type.accepts(v) => Ok(()),
        Some(_) => Err(format!("{field:?} expects {}", def.field_type.describe())),
    }
}

/// Apply every override to `item`, in place.
///
/// Unknown fields and values that no longer typecheck are skipped with a warning
/// rather than failing the read: a registry change must not make already-stored
/// entities unreadable.
pub fn apply(item: &mut MediaItem, overrides: &[Override]) -> Result<(), serde_json::Error> {
    apply_in(item, overrides, "")
}

/// [`apply`], for a work served in `stored` when no other language is asked
/// for — the key of the language the catalogue keeps its works in, `fra` —
/// so that a text locked in that language is laid over the one the work is
/// stored with, as a text locked in another language is when the work is
/// served in that one (see `service::language`). Left empty, no text locked
/// in one language alone is laid.
///
/// A text locked in one language is carried on the work, in
/// [`MediaItem::locked_texts`], to the language it is served in; what the
/// sources gave and a person hid is kept apart, in [`MediaItem::hidden`].
pub fn apply_in(
    item: &mut MediaItem,
    overrides: &[Override],
    stored: &str,
) -> Result<(), serde_json::Error> {
    if overrides.is_empty() {
        return Ok(());
    }

    let mut item_patch = serde_json::Map::new();
    let mut season_patches: Vec<(i32, serde_json::Map<String, Value>)> = Vec::new();
    let mut episode_patches: Vec<((i32, i32), serde_json::Map<String, Value>)> = Vec::new();
    let mut locked: Vec<String> = Vec::new();
    let mut chosen: Vec<(crate::domain::CoverType, String)> = Vec::new();
    let mut chosen_for_seasons: Vec<(i32, String)> = Vec::new();
    let mut texts: Vec<LockedText> = Vec::new();
    let mut children: Vec<(Child, &str, Value)> = Vec::new();

    for ov in overrides {
        // A credit, an image, a title the sources gave.
        if let Ok(child) = ov.scope.parse::<Child>() {
            if ov.in_one_language() || validate_child(&child, &ov.field, ov.value.as_ref()).is_err()
            {
                tracing::warn!(
                    scope = %ov.scope,
                    field = %ov.field,
                    "skipping a lock on a child that no longer reads"
                );
                continue;
            }
            locked.push(format!("{}/{}", ov.scope, ov.field));
            children.push((
                child,
                ov.field.as_str(),
                ov.value.clone().unwrap_or(Value::Null),
            ));
            continue;
        }

        let Ok(scope) = ov.scope.parse::<Scope>() else {
            tracing::warn!(scope = %ov.scope, "skipping override with an unparsable scope");
            continue;
        };

        if validate(scope, &ov.field, ov.value.as_ref()).is_err()
            || (ov.in_one_language() && validate_language(scope, &ov.field).is_err())
        {
            tracing::warn!(
                scope = %ov.scope,
                field = %ov.field,
                "skipping override that no longer matches the field registry"
            );
            continue;
        }

        // A text is carried to the language it is served in: in that one
        // alone, laid there and nowhere else; in every language, laid here
        // as any field is, and again over a lock in the stored language when
        // the work is served in another.
        if TRANSLATED.contains(&ov.field.as_str()) {
            texts.push(LockedText {
                scope: scope.to_string(),
                field: ov.field.clone(),
                language: ov.language.clone().unwrap_or_default(),
                value: ov
                    .value
                    .as_ref()
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
            if ov.in_one_language() {
                continue;
            }
        } else {
            locked.push(format!("{scope}/{}", ov.field));
        }

        let value = ov.value.clone().unwrap_or(Value::Null);

        match scope {
            Scope::Item if CHOSEN_IMAGES.contains(&ov.field.as_str()) => {
                if let (Some(kind), Value::String(address)) = (chosen_kind(&ov.field), &value) {
                    chosen.push((kind, address.clone()));
                }
            }
            Scope::Item => {
                item_patch.insert(ov.field.clone(), value);
            }
            Scope::Season(n) if ov.field == "primaryPoster" => {
                if let Value::String(address) = &value {
                    chosen_for_seasons.push((n, address.clone()));
                }
            }
            Scope::Season(n) => {
                entry(&mut season_patches, n).insert(ov.field.clone(), value);
            }
            Scope::Episode { season, episode } => {
                entry(&mut episode_patches, (season, episode)).insert(ov.field.clone(), value);
            }
        }
    }

    if !item_patch.is_empty() {
        patch_in_place(item, item_patch)?;
    }

    // Before the pictures chosen to lead with: a choice brings back the
    // picture it names, hidden or not.
    edit_children(item, &children);

    for (kind, address) in &chosen {
        lead_with(item, *kind, address);
    }

    for (number, patch) in season_patches {
        if let Some(season) = item.seasons.iter_mut().find(|s| s.season_number == number) {
            patch_in_place::<Season>(season, patch)?;
        }
    }

    for (number, address) in &chosen_for_seasons {
        lead_season_with(item, *number, address);
    }

    for ((season, episode), patch) in episode_patches {
        if let Some(ep) = item
            .episodes
            .iter_mut()
            .find(|e| e.season_number == season && e.episode_number == episode)
        {
            patch_in_place::<Episode>(ep, patch)?;
        }
    }

    // The work as it is served in the language it is stored in: a text
    // locked there over the stored one, and the translations saying the same.
    item.locked_texts = texts;
    lay(item, stored);
    fold_translations(item);

    locked.sort_unstable();
    locked.dedup();
    item.locked_fields = locked;
    mark(item, stored);

    Ok(())
}

/// Merge `patch` into `target`'s JSON form and deserialize back over it.
fn patch_in_place<T>(
    target: &mut T,
    patch: serde_json::Map<String, Value>,
) -> Result<(), serde_json::Error>
where
    T: Serialize + serde::de::DeserializeOwned,
{
    let mut doc = serde_json::to_value(&*target)?;

    if let Value::Object(map) = &mut doc {
        for (k, v) in patch {
            map.insert(k, v);
        }
    }

    *target = serde_json::from_value(doc)?;
    Ok(())
}

/// Get-or-insert a patch map for `key`, preserving insertion order.
fn entry<K: PartialEq>(
    patches: &mut Vec<(K, serde_json::Map<String, Value>)>,
    key: K,
) -> &mut serde_json::Map<String, Value> {
    if let Some(pos) = patches.iter().position(|(k, _)| *k == key) {
        return &mut patches[pos].1;
    }
    patches.push((key, serde_json::Map::new()));
    &mut patches.last_mut().expect("just pushed").1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MediaKind;

    fn item() -> MediaItem {
        let mut it = MediaItem::empty(MediaKind::Series);
        it.title = "Provider Title".into();
        it.overview = Some("Provider overview".into());
        it.runtime = Some(42);
        it.genres = vec!["Drama".into()];
        it
    }

    fn ov(scope: &str, field: &str, value: Option<Value>) -> Override {
        Override {
            scope: scope.into(),
            field: field.into(),
            language: None,
            value,
            updated_at: "2026-09-20T00:00:00.000Z".into(),
            updated_by: None,
        }
    }

    #[test]
    fn a_number_too_big_for_the_field_is_refused_at_the_door() {
        // Every integer field is an `i32`. A larger one is a valid JSON integer
        // and passes serde on the way in, then fails it on the way out — inside
        // `load`, which is how a single pasted timestamp made a work, the whole
        // catalogue list and the refresh sweep error until somebody found it.
        assert!(validate(Scope::Item, "year", Some(&Value::from(2026))).is_ok());
        assert!(validate(Scope::Item, "runtime", Some(&Value::from(-30))).is_ok());

        assert!(validate(Scope::Item, "year", Some(&Value::from(3_000_000_000i64))).is_err());
        assert!(validate(Scope::Item, "runtime", Some(&Value::from(i64::MIN))).is_err());
        assert!(
            validate(Scope::Item, "runtime", Some(&Value::from(u64::MAX))).is_err(),
            "an unsigned value past i64 is not a runtime either"
        );
    }

    #[test]
    fn an_override_replaces_the_provider_value() {
        let mut it = item();
        apply(&mut it, &[ov("item", "title", Some("Mine".into()))]).unwrap();
        assert_eq!(it.title, "Mine");
        assert_eq!(it.overview.as_deref(), Some("Provider overview"));
    }

    #[test]
    fn a_null_override_clears_the_field() {
        let mut it = item();
        apply(&mut it, &[ov("item", "overview", None)]).unwrap();
        assert_eq!(it.overview, None);
    }

    #[test]
    fn overridden_fields_are_reported_as_locked() {
        let mut it = item();
        apply(
            &mut it,
            &[
                ov("item", "title", Some("Mine".into())),
                ov("item", "runtime", Some(50.into())),
            ],
        )
        .unwrap();
        assert_eq!(it.locked_fields, vec!["item/runtime", "item/title"]);
    }

    #[test]
    fn list_fields_are_replaced_wholesale() {
        let mut it = item();
        apply(
            &mut it,
            &[ov(
                "item",
                "genres",
                Some(serde_json::json!(["Comedy", "Sci-Fi"])),
            )],
        )
        .unwrap();
        assert_eq!(it.genres, vec!["Comedy", "Sci-Fi"]);
    }

    #[test]
    fn an_unknown_field_is_skipped_rather_than_fatal() {
        let mut it = item();
        apply(&mut it, &[ov("item", "notAField", Some("x".into()))]).unwrap();
        assert_eq!(it.title, "Provider Title");
        assert!(it.locked_fields.is_empty());
    }

    #[test]
    fn a_wrongly_typed_override_is_skipped() {
        let mut it = item();
        apply(
            &mut it,
            &[ov("item", "runtime", Some("not a number".into()))],
        )
        .unwrap();
        assert_eq!(it.runtime, Some(42));
    }

    #[test]
    fn dates_times_and_instants_are_checked_for_their_shape() {
        let episode = Scope::Episode {
            season: 1,
            episode: 1,
        };
        // A day, or a date-time a provider gave, for a date.
        assert!(validate(Scope::Item, "firstAired", Some(&"2008-01-20".into())).is_ok());
        assert!(
            validate(
                Scope::Item,
                "digitalRelease",
                Some(&"2016-06-30T00:00:00Z".into())
            )
            .is_ok()
        );
        assert!(validate(Scope::Item, "firstAired", Some(&"TBA".into())).is_err());
        assert!(validate(Scope::Item, "firstAired", Some(&"2008-13-40".into())).is_err());
        // An instant, with its zone, for the moment an episode aired.
        assert!(validate(episode, "airDateUtc", Some(&"2009-03-22T21:00:00Z".into())).is_ok());
        assert!(
            validate(
                episode,
                "airDateUtc",
                Some(&"2009-03-22T23:00:00+02:00".into())
            )
            .is_ok()
        );
        assert!(validate(episode, "airDateUtc", Some(&"2009-03-22".into())).is_err());
        assert!(validate(episode, "airDateUtc", Some(&"2009-03-22T21:00".into())).is_err());
        // A time of day.
        assert!(validate(Scope::Item, "airTime", Some(&"21:00".into())).is_ok());
        assert!(validate(Scope::Item, "airTime", Some(&"9pm".into())).is_err());
        // Clearing any of them is always allowed.
        assert!(validate(episode, "airDateUtc", Some(&Value::Null)).is_ok());
    }

    #[test]
    fn validation_rejects_bad_input_up_front() {
        assert!(validate(Scope::Item, "title", Some(&"ok".into())).is_ok());
        // A field a work cannot be read without is never cleared; one it can
        // do without is.
        assert!(validate(Scope::Item, "title", None).is_err());
        assert!(validate(Scope::Item, "title", Some(&serde_json::Value::Null)).is_err());
        assert!(validate(Scope::Item, "overview", None).is_ok());
        assert!(validate(Scope::Item, "runtime", Some(&"nope".into())).is_err());
        assert!(validate(Scope::Item, "nonexistent", Some(&"x".into())).is_err());
        // `network` belongs to the work, not to an episode.
        assert!(
            validate(
                Scope::Episode {
                    season: 1,
                    episode: 1
                },
                "network",
                Some(&"x".into())
            )
            .is_err()
        );
    }

    /// An address a client cannot follow is refused at the door: a script
    /// scheme locked into a homepage would otherwise reach the public page
    /// as a link, and whoever clicked it.
    #[test]
    fn an_address_is_one_a_client_can_follow() {
        let episode = Scope::Episode {
            season: 1,
            episode: 1,
        };
        assert!(
            validate(
                Scope::Item,
                "homepage",
                Some(&"https://example.org/".into())
            )
            .is_ok()
        );
        assert!(validate(Scope::Item, "homepage", Some(&"http://example.org".into())).is_ok());
        assert!(validate(Scope::Item, "homepage", Some(&"javascript:alert(1)".into())).is_err());
        assert!(validate(Scope::Item, "homepage", Some(&"example.org".into())).is_err());
        assert!(
            validate(Scope::Item, "homepage", Some(&"upload:0123".into())).is_err(),
            "a site is not uploaded"
        );
        assert!(validate(Scope::Item, "themeMusic", Some(&"upload:0123".into())).is_ok());
        assert!(validate(Scope::Item, "themeMusic", Some(&"ftp://x/theme.mp3".into())).is_err());
        assert!(
            validate(
                episode,
                "image",
                Some(&"http://stills.example/1.jpg".into())
            )
            .is_ok()
        );
        assert!(validate(episode, "image", Some(&"upload:0123".into())).is_ok());
        assert!(validate(episode, "image", Some(&"data:text/html,<script>".into())).is_err());
        assert!(
            validate(Scope::Item, "homepage", Some(&Value::Null)).is_ok(),
            "cleared is not an address"
        );

        assert!(validate(Scope::Item, "trailerYoutubeId", Some(&"dQw4w9WgXcQ".into())).is_ok());
        assert!(
            validate(
                Scope::Item,
                "trailerYoutubeId",
                Some(&"https://youtu.be/x".into())
            )
            .is_err()
        );

        assert!(validate(Scope::Item, "contentRatingCountry", Some(&"US".into())).is_ok());
        assert!(validate(Scope::Item, "contentRatingCountry", Some(&"usa".into())).is_err());
        assert!(validate(Scope::Item, "contentRatingCountry", Some(&"fr".into())).is_err());
    }

    #[test]
    fn scopes_round_trip() {
        for s in [
            Scope::Item,
            Scope::Season(3),
            Scope::Episode {
                season: 3,
                episode: 7,
            },
        ] {
            assert_eq!(s.to_string().parse::<Scope>().unwrap(), s);
        }
        assert!("season:x".parse::<Scope>().is_err());
        assert!("episode:1-2".parse::<Scope>().is_err());
        assert!("nonsense".parse::<Scope>().is_err());
    }

    #[test]
    fn episode_overrides_reach_the_right_episode() {
        let mut it = item();
        it.episodes = vec![
            Episode {
                id: "a".into(),
                season_number: 1,
                episode_number: 1,
                absolute_episode_number: None,
                aired_after_season_number: None,
                aired_before_season_number: None,
                aired_before_episode_number: None,
                title: "One".into(),
                overview: None,
                air_date: None,
                air_date_utc: None,
                runtime: None,
                finale_type: None,
                image: None,
                tvdb_id: None,
                tmdb_id: None,
                rating: None,
                is_manual: false,
            },
            Episode {
                id: "b".into(),
                season_number: 1,
                episode_number: 2,
                absolute_episode_number: None,
                aired_after_season_number: None,
                aired_before_season_number: None,
                aired_before_episode_number: None,
                title: "Two".into(),
                overview: None,
                air_date: None,
                air_date_utc: None,
                runtime: None,
                finale_type: None,
                image: None,
                tvdb_id: None,
                tmdb_id: None,
                rating: None,
                is_manual: false,
            },
        ];

        apply(
            &mut it,
            &[ov("episode:1x2", "title", Some("Patched".into()))],
        )
        .unwrap();

        assert_eq!(it.episodes[0].title, "One");
        assert_eq!(it.episodes[1].title, "Patched");
    }

    /// The identity fields are locked like any other, never cleared, a slug
    /// only in the shape this server writes — and pinned onto a work about
    /// to be written, so a refresh keeps them.
    #[test]
    fn the_identity_is_locked_pinned_and_never_cleared() {
        use serde_json::json;
        assert!(validate(Scope::Item, "isAdult", Some(&json!(true))).is_ok());
        assert!(validate(Scope::Item, "isAdult", Some(&json!("yes"))).is_err());
        assert!(
            validate(Scope::Item, "isAdult", None).is_err(),
            "never cleared"
        );
        assert!(validate(Scope::Item, "slug", Some(&json!("breaking-bad-2008"))).is_ok());
        assert!(validate(Scope::Item, "slug", Some(&json!("Breaking Bad"))).is_err());
        assert!(validate(Scope::Item, "slug", Some(&json!("a--b"))).is_err());
        assert!(
            validate(
                Scope::Item,
                "externalIds",
                Some(&json!({"tmdb": 1396, "imdb": "tt0903747", "mal": [1, 2]}))
            )
            .is_ok()
        );
        assert!(
            validate(
                Scope::Item,
                "externalIds",
                Some(&json!({"tmdb": "not a number"}))
            )
            .is_err()
        );
        assert!(
            validate(Scope::Item, "externalIds", Some(&json!({}))).is_ok(),
            "none is a set too"
        );

        let mut item = MediaItem::empty(crate::domain::MediaKind::Series);
        item.slug = "from-a-provider".into();
        let locks = vec![
            Override {
                scope: "item".into(),
                field: "isAdult".into(),
                language: None,
                value: Some(json!(true)),
                updated_at: String::new(),
                updated_by: None,
            },
            Override {
                scope: "item".into(),
                field: "slug".into(),
                language: None,
                value: Some(json!("by-hand")),
                updated_at: String::new(),
                updated_by: None,
            },
            Override {
                scope: "item".into(),
                field: "externalIds".into(),
                language: None,
                value: Some(json!({"tvdb": 81189})),
                updated_at: String::new(),
                updated_by: None,
            },
            Override {
                scope: "season:1".into(),
                field: "slug".into(),
                language: None,
                value: Some(json!("not-a-work")),
                updated_at: String::new(),
                updated_by: None,
            },
        ];
        pin_identity(&mut item, &locks);
        assert!(item.is_adult);
        assert_eq!(item.slug, "by-hand");
        assert_eq!(item.external_ids.tvdb, Some(81189));
        assert_eq!(item.external_ids.tmdb, None, "the lock is the whole set");

        // Read back, the same locks patch the served work the same way.
        let mut served = MediaItem::empty(crate::domain::MediaKind::Series);
        apply(&mut served, &locks[..3]).unwrap();
        assert!(served.is_adult);
        assert_eq!(served.slug, "by-hand");
        assert_eq!(served.external_ids.tvdb, Some(81189));
    }

    /// A season's chosen poster leads the season's own and is named on it;
    /// one the sources no longer list is brought back under the season, and
    /// the season's other fields are patched as before.
    #[test]
    fn a_seasons_chosen_poster_leads_its_season() {
        use crate::domain::{CoverType, Image};
        use serde_json::json;
        let image = |id: &str, season: i32, url: &str, order: i32| Image {
            id: id.into(),
            season_number: Some(season),
            cover_type: CoverType::Poster,
            url: url.into(),
            language: None,
            sort_order: order,
            source: Some("tmdb".into()),
            is_manual: false,
        };
        let lock = |scope: &str, field: &str, value: serde_json::Value| Override {
            scope: scope.into(),
            field: field.into(),
            language: None,
            value: Some(value),
            updated_at: String::new(),
            updated_by: None,
        };
        assert!(
            validate(
                Scope::Season(2),
                "primaryPoster",
                Some(&json!("https://s/2b.jpg"))
            )
            .is_ok()
        );
        assert!(
            validate(
                Scope::Season(2),
                "primaryPoster",
                Some(&json!("upload:0123"))
            )
            .is_ok()
        );
        assert!(
            validate(
                Scope::Season(2),
                "primaryPoster",
                Some(&json!("not an address"))
            )
            .is_err()
        );
        assert!(
            validate(Scope::Season(2), "primaryPoster", None).is_err(),
            "unlocked, not cleared"
        );

        let mut item = MediaItem::empty(crate::domain::MediaKind::Series);
        let mut one = crate::db::repo::child::blank_season(1);
        one.is_manual = false;
        one.images = vec![image("s1a", 1, "https://s/1a.jpg", 0)];
        let mut two = crate::db::repo::child::blank_season(2);
        two.is_manual = false;
        two.images = vec![
            image("s2a", 2, "https://s/2a.jpg", 0),
            image("s2b", 2, "https://s/2b.jpg", 1),
        ];
        item.seasons = vec![one, two];
        apply(
            &mut item,
            &[
                lock("season:2", "primaryPoster", json!("https://s/2b.jpg")),
                lock("season:2", "title", json!("The second")),
                lock("season:1", "primaryPoster", json!("https://s/gone.jpg")),
            ],
        )
        .unwrap();

        let two = &item.seasons[1];
        let order: Vec<&str> = two.images.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(order, ["s2b", "s2a"], "the chosen poster leads the season");
        assert_eq!(two.primary_images.poster.as_deref(), Some("s2b"));
        assert_eq!(
            two.title.as_deref(),
            Some("The second"),
            "the other fields still patch"
        );
        let one = &item.seasons[0];
        assert_eq!(
            one.images[0].url, "https://s/gone.jpg",
            "brought back under the season"
        );
        assert!(one.images[0].is_manual);
        assert_eq!(
            one.primary_images.poster.as_deref(),
            Some("chosen-season-1-poster")
        );
        assert!(
            item.locked_fields
                .iter()
                .any(|f| f == "season:2/primaryPoster")
        );
        assert!(
            item.primary_images.is_empty(),
            "a season's choice is not the work's"
        );
    }

    /// A chosen poster leads its kind and is named; one the sources no
    /// longer list is brought back when a client can follow it, and an
    /// upload that is gone leaves no choice.
    #[test]
    fn a_chosen_image_leads_its_kind_and_is_named() {
        use crate::domain::{CoverType, Image};
        use serde_json::json;
        let image = |id: &str, kind: CoverType, url: &str, order: i32| Image {
            id: id.into(),
            season_number: None,
            cover_type: kind,
            url: url.into(),
            language: None,
            sort_order: order,
            source: Some("tmdb".into()),
            is_manual: false,
        };
        let lock = |field: &str, value: &str| Override {
            scope: "item".into(),
            field: field.into(),
            language: None,
            value: Some(json!(value)),
            updated_at: String::new(),
            updated_by: None,
        };
        assert!(
            validate(
                Scope::Item,
                "primaryPoster",
                Some(&json!("https://p/2.jpg"))
            )
            .is_ok()
        );
        assert!(validate(Scope::Item, "primaryPoster", Some(&json!("upload:0123"))).is_ok());
        assert!(validate(Scope::Item, "primaryPoster", Some(&json!("not an address"))).is_err());
        assert!(
            validate(Scope::Item, "primaryFanart", None).is_err(),
            "unlocked, not cleared"
        );

        let mut item = MediaItem::empty(crate::domain::MediaKind::Series);
        item.images = vec![
            image("p1", CoverType::Poster, "https://p/1.jpg", 0),
            image("p2", CoverType::Poster, "https://p/2.jpg", 1),
            image("f1", CoverType::Fanart, "https://f/1.jpg", 0),
        ];
        apply(
            &mut item,
            &[
                lock("primaryPoster", "https://p/2.jpg"),
                lock("primaryFanart", "https://f/gone.jpg"),
            ],
        )
        .unwrap();
        let order: Vec<&str> = item.images.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(order, ["p2", "p1", "chosen-fanart", "f1"]);
        assert_eq!(item.primary_images.poster.as_deref(), Some("p2"));
        assert_eq!(item.primary_images.fanart.as_deref(), Some("chosen-fanart"));
        assert!(
            item.locked_fields
                .contains(&"item/primaryPoster".to_string())
        );

        let mut gone = MediaItem::empty(crate::domain::MediaKind::Movie);
        gone.images = vec![image("p1", CoverType::Poster, "https://p/1.jpg", 0)];
        apply(&mut gone, &[lock("primaryPoster", "upload:0123")]).unwrap();
        assert_eq!(
            gone.primary_images.poster, None,
            "an upload that is gone is no choice"
        );
        assert_eq!(gone.images.len(), 1);
    }

    /// A lock in one language.
    fn in_language(scope: &str, field: &str, language: &str, value: &str) -> Override {
        Override {
            language: Some(language.into()),
            ..ov(scope, field, Some(value.into()))
        }
    }

    /// A text is locked in one language alone, and only a text: what a
    /// translation has no say in is the same in every language.
    #[test]
    fn only_a_text_is_locked_in_one_language() {
        let episode = Scope::Episode {
            season: 1,
            episode: 2,
        };
        assert!(validate_language(Scope::Item, "title").is_ok());
        assert!(validate_language(Scope::Item, "overview").is_ok());
        assert!(validate_language(Scope::Item, "tagline").is_ok());
        assert!(validate_language(Scope::Season(1), "title").is_ok());
        assert!(validate_language(episode, "overview").is_ok());
        assert!(validate_language(Scope::Item, "runtime").is_err());
        assert!(validate_language(Scope::Item, "slug").is_err());
        assert!(
            validate_language(episode, "tagline").is_err(),
            "an episode has no tagline"
        );
    }

    /// Locked in French alone, the title is set aside for French: the work
    /// keeps its own text, says the lock is another language's, and is given
    /// the French title when it is laid in French.
    #[test]
    fn a_lock_in_one_language_is_laid_in_that_one_alone() {
        let mut it = item();
        apply_in(
            &mut it,
            &[in_language("item", "title", "fra", "Mon titre")],
            "eng",
        )
        .unwrap();
        assert_eq!(it.title, "Provider Title", "English is not French");
        assert_eq!(it.locked_fields, ["item/title@fra"]);

        mark(&mut it, "fra");
        lay(&mut it, "fra");
        assert_eq!(it.title, "Mon titre");
        assert_eq!(it.locked_fields, ["item/title"], "locked, in French");
    }

    /// Locked in the language the work is stored in, the text is the one it
    /// is read with — and the one a language with nothing of its own falls
    /// back to.
    #[test]
    fn a_lock_in_the_stored_language_is_the_stored_text() {
        let mut it = item();
        apply_in(
            &mut it,
            &[in_language("item", "overview", "eng", "Mine, in English")],
            "eng",
        )
        .unwrap();
        assert_eq!(it.overview.as_deref(), Some("Mine, in English"));
        assert_eq!(it.locked_fields, ["item/overview"]);

        mark(&mut it, "deu");
        lay(&mut it, "deu");
        assert_eq!(it.overview.as_deref(), Some("Mine, in English"));
        assert_eq!(
            it.locked_fields,
            ["item/overview@eng"],
            "not locked in German"
        );
    }

    /// A lock in every language is every language's, but a lock in one beats
    /// it there: French is given its own, German the one of every language,
    /// and the stored language the one of every language too.
    #[test]
    fn a_lock_in_one_language_beats_one_in_every_language_there_alone() {
        let mut it = item();
        let locks = [
            ov("item", "title", Some("Everywhere".into())),
            in_language("item", "title", "fra", "En France"),
        ];
        apply_in(&mut it, &locks, "eng").unwrap();
        assert_eq!(it.title, "Everywhere");
        assert_eq!(it.locked_fields, ["item/title", "item/title@fra"]);

        let mut french = it.clone();
        mark(&mut french, "fra");
        lay(&mut french, "fra");
        assert_eq!(french.title, "En France");
        assert_eq!(french.locked_fields, ["item/title"]);

        let mut german = it.clone();
        mark(&mut german, "deu");
        lay(&mut german, "deu");
        assert_eq!(german.title, "Everywhere");

        // Locked in the stored language and in every one: the stored text is
        // the stored language's lock, and another language is given the one
        // of every language back.
        let mut both = item();
        let locks = [
            ov("item", "title", Some("Everywhere".into())),
            in_language("item", "title", "eng", "In English"),
        ];
        apply_in(&mut both, &locks, "eng").unwrap();
        assert_eq!(both.title, "In English");
        mark(&mut both, "deu");
        lay(&mut both, "deu");
        assert_eq!(both.title, "Everywhere");
    }

    /// The translations say what each language is served: a lock in it, or
    /// in every language, in the translation's place; a language locked in
    /// without a translation is given one.
    #[test]
    fn the_translations_say_what_each_language_is_served() {
        let mut it = item();
        it.translations = vec![
            Translation {
                language: "fra".into(),
                title: Some("Titre du fournisseur".into()),
                overview: Some("Résumé du fournisseur".into()),
                is_manual: false,
            },
            Translation {
                language: "deu".into(),
                title: Some("Anbietertitel".into()),
                overview: None,
                is_manual: false,
            },
        ];
        apply_in(
            &mut it,
            &[
                in_language("item", "title", "fra", "Mon titre"),
                in_language("item", "title", "ita", "Il mio titolo"),
                ov("item", "overview", Some("Everywhere".into())),
            ],
            "eng",
        )
        .unwrap();
        // Laid again, as a list does once its translations are read: the same.
        fold_translations(&mut it);

        let of = |language: &str| {
            it.translations
                .iter()
                .find(|t| t.language == language)
                .expect("held")
        };
        assert_eq!(of("fra").title.as_deref(), Some("Mon titre"));
        assert_eq!(of("fra").overview.as_deref(), Some("Everywhere"));
        assert!(of("fra").is_manual);
        assert_eq!(of("deu").title.as_deref(), Some("Anbietertitel"));
        assert_eq!(of("deu").overview.as_deref(), Some("Everywhere"));
        assert!(!of("deu").is_manual, "only the one of every language");
        assert_eq!(of("ita").title.as_deref(), Some("Il mio titolo"));
        assert_eq!(it.translations.len(), 3);
    }

    /// A season's and an episode's texts are locked in one language as the
    /// work's are.
    #[test]
    fn a_seasons_and_an_episodes_texts_are_laid_in_their_language() {
        let mut it = item();
        let mut season = crate::db::repo::child::blank_season(1);
        season.title = Some("Season 1".into());
        it.seasons = vec![season];
        let mut episode = crate::db::repo::child::blank_episode(1, 1);
        episode.title = "Pilot".into();
        it.episodes = vec![episode];

        apply_in(
            &mut it,
            &[
                in_language("season:1", "title", "fra", "Première saison"),
                in_language("episode:1x1", "title", "fra", "Le pilote"),
                in_language("episode:1x1", "overview", "eng", "It starts."),
            ],
            "eng",
        )
        .unwrap();
        assert_eq!(it.seasons[0].title.as_deref(), Some("Season 1"));
        assert_eq!(it.episodes[0].title, "Pilot");
        assert_eq!(it.episodes[0].overview.as_deref(), Some("It starts."));

        mark(&mut it, "fra");
        lay(&mut it, "fra");
        assert_eq!(it.seasons[0].title.as_deref(), Some("Première saison"));
        assert_eq!(it.episodes[0].title, "Le pilote");
        assert_eq!(
            it.episodes[0].overview.as_deref(),
            Some("It starts."),
            "nothing of its own in French: the stored text"
        );
        assert!(
            it.locked_fields
                .contains(&"episode:1x1/overview@eng".to_string())
        );
    }

    /// A child the sources gave is named the one way, read back the same,
    /// and refused when it is no child.
    #[test]
    fn a_child_is_named_by_what_it_is() {
        for named in [
            "credit:actor:tmdb:17419",
            "credit:director:name:Vince Gilligan",
            "image:https://image.tmdb.org/t/p/original/a.jpg",
            "alternativeTitle:fr:Les Experts",
            "alternativeTitle::Fate/stay night: UBW",
        ] {
            let child: Child = named.parse().expect(named);
            assert_eq!(child.to_string(), named);
        }
        assert_eq!(
            "credit:actor:tmdb:17419".parse::<Child>().unwrap(),
            Child::Credit {
                kind: CreditType::Actor,
                person: Person::Tmdb(17419),
            }
        );
        for nonsense in [
            "credit:hero:tmdb:1",
            "credit:actor:tmdb:x",
            "credit:actor:name:",
            "image:",
            "alternativeTitle:fr",
            "item",
            "season:1",
        ] {
            assert!(nonsense.parse::<Child>().is_err(), "{nonsense}");
        }

        let credit = Child::Credit {
            kind: CreditType::Actor,
            person: Person::Tmdb(1),
        };
        assert!(validate_child(&credit, "hidden", Some(&Value::Bool(true))).is_ok());
        assert!(
            validate_child(&credit, "hidden", Some(&Value::Bool(false))).is_err(),
            "shown again is a lock lifted"
        );
        assert!(validate_child(&credit, "characterName", Some(&"Walt".into())).is_ok());
        assert!(
            validate_child(&credit, "characterName", None).is_ok(),
            "no character"
        );
        assert!(validate_child(&credit, "sortOrder", Some(&Value::from(3))).is_ok());
        assert!(validate_child(&credit, "sortOrder", None).is_err());
        let image = Child::Image("https://x/a.jpg".into());
        assert!(validate_child(&image, "characterName", Some(&"x".into())).is_err());
    }

    /// What the sources gave and a person hid is kept apart; a credit is
    /// played and placed as corrected; what was added by hand is not
    /// touched; and each lock is counted.
    #[test]
    fn what_the_sources_gave_is_hidden_and_corrected() {
        use crate::domain::{CoverType, Image};

        let credit = |id: &str, person: Option<i64>, name: &str, order: i32, manual: bool| Credit {
            id: id.into(),
            credit_type: CreditType::Actor,
            person_name: name.into(),
            character_name: Some("Someone".into()),
            image: None,
            tmdb_person_id: person,
            credit_tmdb_id: None,
            sort_order: order,
            is_manual: manual,
        };
        let picture = |id: &str, season: Option<i32>, url: &str, manual: bool| Image {
            id: id.into(),
            season_number: season,
            cover_type: CoverType::Poster,
            url: url.into(),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: manual,
        };
        let title = |id: &str, text: &str, language: Option<&str>| AlternativeTitle {
            id: id.into(),
            title: text.into(),
            title_type: None,
            language: language.map(str::to_string),
            is_manual: false,
        };

        let mut it = item();
        it.credits = vec![
            credit("walt", Some(17419), "Bryan Cranston", 0, false),
            credit("jesse", Some(84497), "Aaron Paul", 1, false),
            credit("hank", None, "Dean Norris", 2, false),
            credit("mine", None, "Dean Norris", 3, true),
        ];
        it.images = vec![
            picture("p1", None, "https://p/1.jpg", false),
            picture("p2", None, "https://p/2.jpg", false),
            picture("p3", None, "https://p/1.jpg", true),
        ];
        let mut season = crate::db::repo::child::blank_season(1);
        season.images = vec![picture("s1", Some(1), "https://p/2.jpg", false)];
        it.seasons = vec![season];
        it.alternative_titles = vec![
            title("t1", "Breaking Bad", Some("fr")),
            title("t2", "Breaking Bad", None),
        ];

        apply(
            &mut it,
            &[
                ov("credit:actor:tmdb:17419", "hidden", Some(Value::Bool(true))),
                ov(
                    "credit:actor:tmdb:84497",
                    "characterName",
                    Some("Jesse Pinkman".into()),
                ),
                ov("credit:actor:tmdb:84497", "sortOrder", Some(Value::from(9))),
                ov("credit:actor:name:Dean Norris", "characterName", None),
                ov("image:https://p/1.jpg", "hidden", Some(Value::Bool(true))),
                ov("image:https://p/2.jpg", "hidden", Some(Value::Bool(true))),
                ov(
                    "alternativeTitle:fr:Breaking Bad",
                    "hidden",
                    Some(Value::Bool(true)),
                ),
            ],
        )
        .unwrap();

        let ids = |credits: &[Credit]| credits.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
        assert_eq!(
            ids(&it.credits),
            ["hank", "mine", "jesse"],
            "moved to its place"
        );
        assert_eq!(ids(&it.hidden.credits), ["walt"]);
        assert_eq!(
            it.credits[2].character_name.as_deref(),
            Some("Jesse Pinkman")
        );
        assert_eq!(
            it.credits[0].character_name, None,
            "no character, as locked"
        );
        assert_eq!(
            it.credits[1].character_name.as_deref(),
            Some("Someone"),
            "the one added by hand is its own"
        );

        let urls = |images: &[Image]| images.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
        assert_eq!(urls(&it.images), ["p3"], "the one added by hand stays");
        assert!(it.seasons[0].images.is_empty(), "a season's is hidden too");
        assert_eq!(urls(&it.hidden.images), ["p1", "p2", "s1"]);

        assert_eq!(it.alternative_titles.len(), 1);
        assert_eq!(
            it.alternative_titles[0].id, "t2",
            "the title in no language stays"
        );
        assert_eq!(it.hidden.alternative_titles[0].id, "t1");

        assert!(
            it.locked_fields
                .contains(&"credit:actor:tmdb:17419/hidden".to_string())
        );
        assert!(
            it.locked_fields
                .contains(&"image:https://p/1.jpg/hidden".to_string())
        );
        assert_eq!(it.locked_fields.len(), 7);

        // Read apart from its work, a credit is served as on it.
        let locks = [
            ov("credit:actor:tmdb:17419", "hidden", Some(Value::Bool(true))),
            ov(
                "credit:actor:tmdb:84497",
                "characterName",
                Some("Jesse Pinkman".into()),
            ),
        ];
        assert!(
            edit_credit(
                credit("walt", Some(17419), "Bryan Cranston", 0, false),
                &locks
            )
            .is_none()
        );
        assert_eq!(
            edit_credit(credit("jesse", Some(84497), "Aaron Paul", 1, false), &locks)
                .and_then(|c| c.character_name)
                .as_deref(),
            Some("Jesse Pinkman")
        );
    }
}

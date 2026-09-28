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

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::domain::{Episode, MediaItem, Season};

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
                 as numbers, imdb as a string, mal and anilist as arrays of numbers"
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
    f("status", Text, "Status"),
    f("originalLanguage", Text, "Original language"),
    f("originalCountry", Text, "Original country"),
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
    f("contentRating", Text, "Content rating"),
    f("homepage", Text, "Homepage"),
    f("trailerYoutubeId", Text, "YouTube trailer id"),
    f("themeMusic", Text, "Theme music URL"),
    f("genres", TextList, "Genres"),
    f("keywords", TextList, "Keywords"),
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
        if ov.scope != "item" {
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
    pub scope: String,
    pub field: String,
    /// `None` means the user explicitly cleared the field.
    pub value: Option<Value>,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
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
    if overrides.is_empty() {
        return Ok(());
    }

    let mut item_patch = serde_json::Map::new();
    let mut season_patches: Vec<(i32, serde_json::Map<String, Value>)> = Vec::new();
    let mut episode_patches: Vec<((i32, i32), serde_json::Map<String, Value>)> = Vec::new();
    let mut locked: Vec<String> = Vec::new();
    let mut chosen: Vec<(crate::domain::CoverType, String)> = Vec::new();

    for ov in overrides {
        let Ok(scope) = ov.scope.parse::<Scope>() else {
            tracing::warn!(scope = %ov.scope, "skipping override with an unparsable scope");
            continue;
        };

        if validate(scope, &ov.field, ov.value.as_ref()).is_err() {
            tracing::warn!(
                scope = %ov.scope,
                field = %ov.field,
                "skipping override that no longer matches the field registry"
            );
            continue;
        }

        let value = ov.value.clone().unwrap_or(Value::Null);
        locked.push(format!("{scope}/{}", ov.field));

        match scope {
            Scope::Item if CHOSEN_IMAGES.contains(&ov.field.as_str()) => {
                if let (Some(kind), Value::String(address)) = (chosen_kind(&ov.field), &value) {
                    chosen.push((kind, address.clone()));
                }
            }
            Scope::Item => {
                item_patch.insert(ov.field.clone(), value);
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

    for (kind, address) in &chosen {
        lead_with(item, *kind, address);
    }

    for (number, patch) in season_patches {
        if let Some(season) = item.seasons.iter_mut().find(|s| s.season_number == number) {
            patch_in_place::<Season>(season, patch)?;
        }
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

    locked.sort_unstable();
    locked.dedup();
    item.locked_fields = locked;

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
                value: Some(json!(true)),
                updated_at: String::new(),
                updated_by: None,
            },
            Override {
                scope: "item".into(),
                field: "slug".into(),
                value: Some(json!("by-hand")),
                updated_at: String::new(),
                updated_by: None,
            },
            Override {
                scope: "item".into(),
                field: "externalIds".into(),
                value: Some(json!({"tvdb": 81189})),
                updated_at: String::new(),
                updated_by: None,
            },
            Override {
                scope: "season:1".into(),
                field: "slug".into(),
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
}

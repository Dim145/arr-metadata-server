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
    f("genres", TextList, "Genres"),
    f("keywords", TextList, "Keywords"),
];

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
pub fn validate(scope: Scope, field: &str, value: Option<&Value>) -> Result<(), String> {
    let Some(def) = scope.field(field) else {
        return Err(format!("{field:?} is not editable on scope {scope}"));
    };

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
        assert!(validate(Scope::Item, "title", None).is_ok());
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
}

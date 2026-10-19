//! The orders a series' episodes come in.
//!
//! A series aired in one order and is numbered by it: the work's own episode
//! list. Others number it besides — the DVDs', straight through, a streaming
//! service's, the order a person prefers — each a sequence of places, a
//! season and a number, that names an episode the work holds or carries the
//! record of one it does not: TheTVDB files a DVD's hour-long cut of two
//! aired parts as an episode of its own, and The Office's Superfan order is
//! nothing but extended cuts.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What the key of an order made here begins with.
pub const MADE_HERE: &str = "ams-";

/// What the key of a TMDB episode group begins with, before the group's id.
pub const TMDB_GROUP: &str = "tmdb-";

/// The longest key an order is asked for by: a TMDB group's is `tmdb-` and
/// twenty-four hexadecimal digits.
pub const KEY_MOST: usize = 40;

/// Whether `key` can be an order's: what TheTVDB names its orders by, and
/// what this server names the others by — letters, digits and hyphens, in
/// lower case. It is a segment of Sonarr's address for a series, and a part
/// of the key Plex files an episode under.
pub fn is_key(key: &str) -> bool {
    (1..=KEY_MOST).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// What an order is, whatever its source calls it. TheTVDB's further orders
/// are slots — `alternate`, `regional`, `altdvd`, `alttwo` — whose meaning
/// is the series' own name for each, so they are one kind here; TMDB types
/// its episode groups one to seven.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrderKind {
    /// As the episodes aired: the work's own order, and a TMDB group of
    /// type 1, "Original air date".
    Aired,
    /// As the DVDs put them.
    Dvd,
    /// Straight through, from the first.
    Absolute,
    /// As a streaming service puts them: TMDB's type 4.
    Digital,
    /// As they were made: TMDB's type 6.
    Production,
    /// By story: TMDB's type 5.
    StoryArc,
    /// As a channel shows them: TMDB's type 7.
    Tv,
    /// One of TheTVDB's further orders, named by the series.
    TvdbAlt,
    /// Made here.
    Custom,
}

impl OrderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aired => "aired",
            Self::Dvd => "dvd",
            Self::Absolute => "absolute",
            Self::Digital => "digital",
            Self::Production => "production",
            Self::StoryArc => "story_arc",
            Self::Tv => "tv",
            Self::TvdbAlt => "tvdb_alt",
            Self::Custom => "custom",
        }
    }

    /// The kind of one of TheTVDB's season types: its DVD and absolute
    /// orders are what they say; the rest are the series' own.
    pub fn of_tvdb(season_type: &str) -> Self {
        match season_type {
            "official" | "default" => Self::Aired,
            "dvd" => Self::Dvd,
            "absolute" => Self::Absolute,
            _ => Self::TvdbAlt,
        }
    }

    /// The kind of a TMDB episode group, by its type: 1 original air date,
    /// 2 absolute, 3 DVD, 4 digital, 5 story arc, 6 production, 7 TV.
    pub const fn of_tmdb(group_type: i64) -> Option<Self> {
        match group_type {
            1 => Some(Self::Aired),
            2 => Some(Self::Absolute),
            3 => Some(Self::Dvd),
            4 => Some(Self::Digital),
            5 => Some(Self::StoryArc),
            6 => Some(Self::Production),
            7 => Some(Self::Tv),
            _ => None,
        }
    }
}

impl FromStr for OrderKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim() {
            "aired" => Self::Aired,
            "dvd" => Self::Dvd,
            "absolute" => Self::Absolute,
            "digital" => Self::Digital,
            "production" => Self::Production,
            "story_arc" => Self::StoryArc,
            "tv" => Self::Tv,
            "tvdb_alt" => Self::TvdbAlt,
            "custom" => Self::Custom,
            other => anyhow::bail!("unknown kind of order: {other}"),
        })
    }
}

/// One of the orders a series' episodes come in: one of TheTVDB's besides
/// the aired one, a TMDB episode group, one made here — or the aired order
/// itself, whose places are the work's own episodes and are not listed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeOrder {
    /// What a client asks for it by — Sonarr v5's season type, Plex's
    /// episode order, the TheTVDB relay's season type: `official` for the
    /// aired order, TheTVDB's own type for its others (`dvd`, `absolute`,
    /// `alternate`, `regional`, `altdvd`, `alttwo`), `tmdb-` and the id of a
    /// TMDB episode group, `ams-` and a name for one made here.
    pub key: String,
    pub kind: OrderKind,
    /// Where it was read: `tvdb`, `tmdb`, or `local` for one made here. The
    /// aired order's is whoever numbers the work's episodes — `tmdb` while
    /// TMDB numbers a series TheTVDB stopped answering for.
    pub source: String,
    /// The order's id at its source: a TMDB episode group's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// Its name: the series' own for one of TheTVDB's ("Superfan Order",
    /// "BluRay Order") or TheTVDB's for the type ("DVD Order"), the group's
    /// on TMDB, the one it was given here.
    pub label: String,
    /// Whether it is the aired order.
    pub is_default: bool,
    /// Whether it is what the aired order answers with: Sonarr asking for
    /// `official`, Sonarr v4, Plex with no order named and the TheTVDB relay
    /// are given its numbering, frozen until it is thawed. One order of a
    /// work at the most.
    pub is_frozen: bool,
    /// How many episodes its source says it holds, where they are yet to be
    /// read: a TMDB group's are read the first time it is served or shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announced: Option<i32>,
    /// When it was last written: read from its source, saved, frozen. An
    /// episode that airs after it is served in a frozen or a home-made order
    /// under its aired numbers, where the order leaves them free.
    pub updated_at: String,
    /// Its places, in its own sequence. None for the aired order: its
    /// places are the work's episodes.
    pub episodes: Vec<PlacedEpisode>,
}

/// Where one episode stands in an order: a season and a number, and either
/// an episode the work holds or the record the order carries of one it does
/// not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlacedEpisode {
    /// The place's own id: what a record a person wrote is known by from one
    /// save to the next.
    #[serde(default)]
    #[schema(required)]
    pub id: String,
    pub season_number: i32,
    pub episode_number: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_number: Option<i32>,
    /// The held episode the place names, by the row it is kept in. A
    /// refresh writes a source's episodes again under new rows, so the ids
    /// below are what name one of those; the row names one made by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<String>,
    /// The episode's id on TheTVDB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    /// The episode's id on TMDB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    /// What the order says of an episode the work does not hold: served in
    /// its place, as an episode of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<OrderRecord>,
}

/// An episode as an order's source describes it, kept with its place: the
/// record served where no episode the work holds stands for the place.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrderRecord {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    /// The day it aired, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aired: Option<String>,
    /// In minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<i32>,
    /// Its still, an address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// `season`, `midseason` or `series`, as TheTVDB marks a finale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finale_type: Option<String>,
}

impl PlacedEpisode {
    /// A place naming an episode by its TheTVDB id, as TheTVDB places one.
    pub fn of_tvdb(tvdb_id: i64, season_number: i32, episode_number: i32) -> Self {
        Self {
            id: crate::db::new_id(),
            season_number,
            episode_number,
            absolute_number: None,
            episode_id: None,
            tvdb_id: Some(tvdb_id),
            tmdb_id: None,
            record: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_is_written_as_it_is_read() {
        for kind in [
            OrderKind::Aired,
            OrderKind::Dvd,
            OrderKind::Absolute,
            OrderKind::Digital,
            OrderKind::Production,
            OrderKind::StoryArc,
            OrderKind::Tv,
            OrderKind::TvdbAlt,
            OrderKind::Custom,
        ] {
            assert_eq!(kind.as_str().parse::<OrderKind>().unwrap(), kind);
            // The same word in the documents the API sends.
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(kind.as_str())
            );
        }
        assert!("blu-ray".parse::<OrderKind>().is_err());
    }

    #[test]
    fn thetvdbs_slots_are_one_kind_and_tmdbs_types_seven() {
        assert_eq!(OrderKind::of_tvdb("dvd"), OrderKind::Dvd);
        assert_eq!(OrderKind::of_tvdb("absolute"), OrderKind::Absolute);
        for slot in ["alternate", "regional", "altdvd", "alttwo"] {
            assert_eq!(OrderKind::of_tvdb(slot), OrderKind::TvdbAlt);
        }
        assert_eq!(OrderKind::of_tmdb(1), Some(OrderKind::Aired));
        assert_eq!(OrderKind::of_tmdb(3), Some(OrderKind::Dvd));
        assert_eq!(OrderKind::of_tmdb(5), Some(OrderKind::StoryArc));
        assert_eq!(OrderKind::of_tmdb(7), Some(OrderKind::Tv));
        assert_eq!(OrderKind::of_tmdb(8), None);
    }
}

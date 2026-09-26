//! External identifier namespaces.
//!
//! TMDB and TVDB number movies and series independently, so a bare `(tmdb, 42)`
//! is ambiguous. Every source here names the entity type it addresses; the
//! database enforces the same list with a CHECK constraint.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::MediaKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSource {
    TmdbMovie,
    TmdbTv,
    TvdbSeries,
    TvdbMovie,
    Imdb,
    TvMaze,
    TvRage,
    Mal,
    AniList,
    TraktShow,
    TraktMovie,
    /// Fankai's id for a Fan-Kai production — a recut TheTVDB and TMDB list
    /// nothing of, so it is the one id such a work has.
    Fankai,
}

impl ExternalSource {
    pub const ALL: &'static [Self] = &[
        Self::TmdbMovie,
        Self::TmdbTv,
        Self::TvdbSeries,
        Self::TvdbMovie,
        Self::Imdb,
        Self::TvMaze,
        Self::TvRage,
        Self::Mal,
        Self::AniList,
        Self::TraktShow,
        Self::TraktMovie,
        Self::Fankai,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TmdbMovie => "tmdb_movie",
            Self::TmdbTv => "tmdb_tv",
            Self::TvdbSeries => "tvdb_series",
            Self::TvdbMovie => "tvdb_movie",
            Self::Imdb => "imdb",
            Self::TvMaze => "tvmaze",
            Self::TvRage => "tvrage",
            Self::Mal => "mal",
            Self::AniList => "anilist",
            Self::TraktShow => "trakt_show",
            Self::TraktMovie => "trakt_movie",
            Self::Fankai => "fankai",
        }
    }

    /// The source that names `kind` on the given provider.
    pub const fn tmdb_for(kind: MediaKind) -> Self {
        match kind {
            MediaKind::Series => Self::TmdbTv,
            MediaKind::Movie => Self::TmdbMovie,
        }
    }

    pub const fn tvdb_for(kind: MediaKind) -> Self {
        match kind {
            MediaKind::Series => Self::TvdbSeries,
            MediaKind::Movie => Self::TvdbMovie,
        }
    }
}

impl fmt::Display for ExternalSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ExternalSource {
    type Err = UnknownSource;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_ascii_lowercase();
        Self::ALL
            .iter()
            .copied()
            .find(|c| c.as_str() == normalized)
            .ok_or_else(|| UnknownSource(s.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown external id source: {0}")]
pub struct UnknownSource(pub String);

/// The identifiers known for one work, in the shape the compatibility surfaces need.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ExternalIds {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvmaze: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvrage: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mal: Vec<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anilist: Vec<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trakt: Option<i64>,
    /// Fankai's id, for a Fan-Kai production; see `providers::fankai`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fankai: Option<i64>,
}

impl ExternalIds {
    /// Fold one stored `(source, value)` row into the struct.
    ///
    /// Values that do not parse as the source's expected type are dropped rather
    /// than failing the whole read: one malformed row should not hide a work.
    pub fn apply(&mut self, source: ExternalSource, value: &str) {
        let as_int = || value.parse::<i64>().ok();

        match source {
            ExternalSource::TmdbMovie | ExternalSource::TmdbTv => self.tmdb = as_int(),
            ExternalSource::TvdbSeries | ExternalSource::TvdbMovie => self.tvdb = as_int(),
            ExternalSource::Imdb => self.imdb = Some(value.to_string()),
            ExternalSource::TvMaze => self.tvmaze = as_int(),
            ExternalSource::TvRage => self.tvrage = as_int(),
            ExternalSource::Mal => self.mal.extend(as_int()),
            ExternalSource::AniList => self.anilist.extend(as_int()),
            ExternalSource::TraktShow | ExternalSource::TraktMovie => self.trakt = as_int(),
            ExternalSource::Fankai => self.fankai = as_int(),
        }
    }

    /// Expand back into storable rows for a work of the given kind.
    pub fn rows(&self, kind: MediaKind) -> Vec<(ExternalSource, String)> {
        let mut out = Vec::new();

        if let Some(v) = self.tmdb {
            out.push((ExternalSource::tmdb_for(kind), v.to_string()));
        }
        if let Some(v) = self.tvdb {
            out.push((ExternalSource::tvdb_for(kind), v.to_string()));
        }
        if let Some(v) = &self.imdb {
            out.push((ExternalSource::Imdb, v.clone()));
        }
        if let Some(v) = self.tvmaze {
            out.push((ExternalSource::TvMaze, v.to_string()));
        }
        if let Some(v) = self.tvrage {
            out.push((ExternalSource::TvRage, v.to_string()));
        }
        for v in &self.mal {
            out.push((ExternalSource::Mal, v.to_string()));
        }
        for v in &self.anilist {
            out.push((ExternalSource::AniList, v.to_string()));
        }
        if let Some(v) = self.trakt {
            let src = match kind {
                MediaKind::Series => ExternalSource::TraktShow,
                MediaKind::Movie => ExternalSource::TraktMovie,
            };
            out.push((src, v.to_string()));
        }
        if let Some(v) = self.fankai {
            out.push((ExternalSource::Fankai, v.to_string()));
        }

        out
    }

    /// True when at least one id exists, which is what makes an entry refreshable.
    pub fn is_empty(&self) -> bool {
        self.tmdb.is_none()
            && self.tvdb.is_none()
            && self.imdb.is_none()
            && self.tvmaze.is_none()
            && self.tvrage.is_none()
            && self.mal.is_empty()
            && self.anilist.is_empty()
            && self.trakt.is_none()
            && self.fankai.is_none()
    }
}

/// Normalise an IMDb id to its canonical `tt0000000` form.
///
/// Accepts a bare number, a `tt`-prefixed id, or a full imdb.com URL.
pub fn normalize_imdb_id(raw: &str) -> Option<String> {
    let s = raw.trim();

    // Pull the id out of a URL if that is what we were given. `!seg.is_empty()`
    // matters: a trailing slash yields an empty segment, and `all()` on an empty
    // iterator is vacuously true.
    let s = s
        .rsplit('/')
        .find(|seg| {
            !seg.is_empty() && (seg.starts_with("tt") || seg.chars().all(|c| c.is_ascii_digit()))
        })
        .unwrap_or(s);

    let digits = s
        .strip_prefix("tt")
        .or_else(|| s.strip_prefix("TT"))
        .unwrap_or(s);

    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    // IMDb pads to at least 7 digits and grows beyond that for newer titles.
    Some(format!("tt{digits:0>7}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_roundtrips_through_its_string_form() {
        for &s in ExternalSource::ALL {
            assert_eq!(s.as_str().parse::<ExternalSource>().unwrap(), s);
        }
    }

    #[test]
    fn imdb_ids_are_normalised() {
        assert_eq!(normalize_imdb_id("tt0903747").as_deref(), Some("tt0903747"));
        assert_eq!(normalize_imdb_id("903747").as_deref(), Some("tt0903747"));
        assert_eq!(normalize_imdb_id("tt903747").as_deref(), Some("tt0903747"));
        assert_eq!(
            normalize_imdb_id("https://www.imdb.com/title/tt12345678/").as_deref(),
            Some("tt12345678")
        );
        assert_eq!(normalize_imdb_id("not-an-id"), None);
        assert_eq!(normalize_imdb_id(""), None);
    }

    #[test]
    fn ids_round_trip_through_rows() {
        let mut ids = ExternalIds {
            tmdb: Some(1396),
            tvdb: Some(81189),
            imdb: Some("tt0903747".into()),
            mal: vec![7, 9],
            fankai: Some(12),
            ..Default::default()
        };

        let rows = ids.rows(MediaKind::Series);
        assert!(rows.contains(&(ExternalSource::TmdbTv, "1396".into())));
        assert!(rows.contains(&(ExternalSource::TvdbSeries, "81189".into())));
        assert!(rows.contains(&(ExternalSource::Fankai, "12".into())));

        let mut rebuilt = ExternalIds::default();
        for (source, value) in &rows {
            rebuilt.apply(*source, value);
        }
        ids.mal.sort_unstable();
        assert_eq!(rebuilt, ids);
    }

    #[test]
    fn a_movie_stores_its_tmdb_id_under_the_movie_namespace() {
        let ids = ExternalIds {
            tmdb: Some(42),
            ..Default::default()
        };
        assert_eq!(
            ids.rows(MediaKind::Movie),
            vec![(ExternalSource::TmdbMovie, "42".to_string())]
        );
    }

    #[test]
    fn malformed_numeric_values_are_dropped_not_fatal() {
        let mut ids = ExternalIds::default();
        ids.apply(ExternalSource::TmdbTv, "not-a-number");
        assert_eq!(ids.tmdb, None);
    }
}

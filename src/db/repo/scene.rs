//! What Sonarr's alternate titles are made from: every series held, by the id
//! Sonarr keeps it under, with each title it goes by.
//!
//! Read in a handful of queries over the whole catalogue rather than work by
//! work: the list is asked for every few hours, and loading each series with
//! its episodes to read a few titles would be most of a catalogue's rows.

use std::collections::HashMap;

use anyhow::Result;

use crate::{
    db::{Db, RowExt as _},
    domain::ExternalSource,
};

/// One series, as the scene-mapping list needs it.
#[derive(Clone, Debug, Default)]
pub struct SeriesTitles {
    /// Its title, a locked one included.
    pub title: String,
    /// Whether that title is locked: served as it is, in every language.
    pub title_locked: bool,
    /// Its original title, a locked one included.
    pub original_title: Option<String>,
    /// The language of that original title.
    pub original_language: Option<String>,
    /// Whether it is shown at all: a hidden work is on no surface.
    pub enabled: bool,
    /// What TheTVDB adds to its name to tell it from a homonym.
    pub title_qualifier: Option<String>,
    /// The year it first aired.
    pub year: Option<i32>,
    pub tvdb: Option<i64>,
    pub tmdb: Option<i64>,
    pub fankai: Option<i64>,
    /// Every alternative title it is known by, with the language its source
    /// gave it, when it gave one.
    pub alternative: Vec<(Option<String>, String)>,
    /// Its translated titles, with the language each is in.
    pub translated: Vec<(String, String)>,
}

/// Every series held, with its titles.
pub async fn series_titles(db: &Db) -> Result<Vec<SeriesTitles>> {
    let mut by_id: HashMap<String, SeriesTitles> = HashMap::new();
    let mut order = Vec::new();

    let rows = sqlx::query(db.sql(
        "SELECT id, title, original_title, original_language, is_enabled, title_qualifier, year \
         FROM media_item \
         WHERE kind = 'series' ORDER BY id",
    ))
    .fetch_all(db.pool())
    .await?;
    for row in rows {
        let id = row.text("id")?;
        order.push(id.clone());
        by_id.insert(
            id.clone(),
            SeriesTitles {
                title: row.text("title")?,
                original_title: row.opt_text("original_title")?,
                original_language: row.opt_text("original_language")?,
                enabled: row.flag("is_enabled")?,
                title_qualifier: row.opt_text("title_qualifier")?,
                year: row.opt_int("year")?,
                ..Default::default()
            },
        );
    }

    // A locked title is the one every client is served, Sonarr included.
    let rows = sqlx::query(db.sql(
        "SELECT o.media_id, o.field, o.value FROM media_override o \
         JOIN media_item i ON i.id = o.media_id \
         WHERE i.kind = 'series' AND o.scope = 'item' \
         AND o.field IN ('title', 'originalTitle')",
    ))
    .fetch_all(db.pool())
    .await?;
    for row in rows {
        let Some(series) = by_id.get_mut(&row.text("media_id")?) else {
            continue;
        };
        let value = row
            .opt_text("value")?
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
        let text = value
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        match row.text("field")?.as_str() {
            "title" => {
                if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
                    series.title = text;
                    series.title_locked = true;
                }
            }
            _ => series.original_title = text,
        }
    }

    let rows = sqlx::query(db.sql(
        "SELECT e.media_id, e.source, e.value FROM media_external_id e \
         JOIN media_item i ON i.id = e.media_id \
         WHERE i.kind = 'series' AND e.source IN (?, ?, ?)",
    ))
    .bind(ExternalSource::TvdbSeries.as_str())
    .bind(ExternalSource::TmdbTv.as_str())
    .bind(ExternalSource::Fankai.as_str())
    .fetch_all(db.pool())
    .await?;
    for row in rows {
        let Some(series) = by_id.get_mut(&row.text("media_id")?) else {
            continue;
        };
        let Ok(value) = row.text("value")?.trim().parse::<i64>() else {
            continue;
        };
        match row.text("source")?.as_str() {
            s if s == ExternalSource::TvdbSeries.as_str() => series.tvdb = Some(value),
            s if s == ExternalSource::TmdbTv.as_str() => series.tmdb = Some(value),
            _ => series.fankai = Some(value),
        }
    }

    let rows = sqlx::query(db.sql(
        "SELECT a.media_id, a.language, a.title FROM media_alternative_title a \
         JOIN media_item i ON i.id = a.media_id \
         WHERE i.kind = 'series' ORDER BY a.media_id, a.title",
    ))
    .fetch_all(db.pool())
    .await?;
    for row in rows {
        if let Some(series) = by_id.get_mut(&row.text("media_id")?) {
            series
                .alternative
                .push((row.opt_text("language")?, row.text("title")?));
        }
    }

    let rows = sqlx::query(db.sql(
        "SELECT t.media_id, t.language, t.title FROM media_translation t \
         JOIN media_item i ON i.id = t.media_id \
         WHERE i.kind = 'series' AND t.title IS NOT NULL \
         ORDER BY t.media_id, t.language",
    ))
    .fetch_all(db.pool())
    .await?;
    for row in rows {
        let Some(title) = row.opt_text("title")? else {
            continue;
        };
        if let Some(series) = by_id.get_mut(&row.text("media_id")?) {
            series.translated.push((row.text("language")?, title));
        }
    }

    Ok(order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect())
}

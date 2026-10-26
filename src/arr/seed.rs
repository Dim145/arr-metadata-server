//! The catalogue filled from the operator's own library.
//!
//! Until Sonarr and Radarr are pointed here they ask Skyhook and Radarr's
//! server, and this one knows nothing of what they hold: the first library
//! refresh after the switch would fetch every series at once, each while
//! Sonarr waits for it. The seed reads both libraries first
//! (`/api/v3/series`, `/api/v3/movie`) and imports what is missing the way the
//! import page does — by the id they hold it under, through the same lookups
//! a client's request takes — one work after another, a pause between two
//! fetches. What is held already is passed over, so it can be run again at
//! any time and only fetches what came since; and stopped between two works.
//!
//! Each work is written down for the run — `imported`, `skipped` when it was
//! held already, `failed` when no provider had it or it is held there under
//! no id to look it up by — and the run says how many of each:
//! `12 already here, 30 imported, 2 not found, 0 failed, of 44`.

use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};

use anyhow::Result;

use super::client::{App, Connection, Held};
use crate::{
    db::repo::{self, job},
    domain::{ExternalSource, MediaItem, MediaKind},
    error::{AppError, AppResult},
    jobs::cancel,
    service::{self, ids},
    state::AppState,
};

/// The task's name, as the history files its runs.
pub const KIND: &str = "arr.seed";

/// What a run is held under among several instances.
pub const HOLD: &str = KIND;

/// The pause after each work that had to be fetched: the providers are asked
/// for a whole library, and are in no hurry.
const PAUSE: Duration = Duration::from_millis(500);

/// Held by a run, so that two never read the same library at once.
static SEEDING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Whether a run holds the seed's lock on this instance.
pub fn is_busy() -> bool {
    SEEDING.try_lock().is_err()
}

/// Whether there is a library to read: Sonarr's or Radarr's connected.
pub fn connected(state: &AppState) -> bool {
    App::ALL
        .into_iter()
        .any(|app| Connection::configured(state, app).is_some())
}

/// The notes a run leaves on each work, which the interface puts in words.
pub mod notes {
    pub const IMPORTED: &str = "imported";
    pub const HERE: &str = "already held here";
    pub const NOT_FOUND: &str = "no provider could resolve it";
    pub const NO_ID: &str = "held there under no id to look it up by";
}

/// Start a run in the background, as `by` asked: the run's id at once.
pub async fn start(state: &AppState, by: &str) -> AppResult<Option<String>> {
    if !connected(state) {
        return Err(AppError::Disabled {
            code: "arr_off",
            message: "connect Sonarr or Radarr first, on their page".into(),
        });
    }
    let held = SEEDING.clone().try_lock_owned().map_err(|_| {
        AppError::Conflict("the library is being read already; its run is in the history".into())
    })?;
    let shared = state
        .coord
        .hold_for(HOLD, "the library is being read")
        .await?;
    let record = job::start_by(&state.db, KIND, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let flag = cancel::register(&record);
        let ended = run(&state, &record, &flag).await;
        let closed = match &ended {
            Ok(Pass::Done { summary, unread }) => {
                job::finish(&state.db, &record, Some(summary), unread.as_deref()).await
            }
            Ok(Pass::Stopped(summary)) => job::stop(&state.db, &record, summary).await,
            Err(e) => job::finish(&state.db, &record, None, Some(&format!("{e:#}"))).await,
        };
        if let Err(e) = closed {
            tracing::warn!(error = %e, "could not close the seed's run");
        }
        if let Err(e) = ended {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "the library could not be read"
            );
        }
    });

    Ok(Some(id))
}

/// How a run ended.
enum Pass {
    /// Through every work: the counts, and the library that could not be
    /// read, when one could not.
    Done {
        summary: String,
        unread: Option<String>,
    },
    Stopped(String),
}

/// What came of each work, counted.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub here: usize,
    pub imported: usize,
    pub missing: usize,
    pub failed: usize,
}

impl Tally {
    fn said(&self) -> String {
        format!(
            "{} already here, {} imported, {} not found, {} failed",
            self.here, self.imported, self.missing, self.failed
        )
    }

    fn done(&self) -> usize {
        self.here + self.imported + self.missing + self.failed
    }
}

async fn run(state: &AppState, record: &str, flag: &cancel::Registered) -> Result<Pass> {
    // Both libraries first: one that cannot be read is said at the end, and
    // the other is seeded all the same.
    let mut wanted: Vec<(App, Held)> = Vec::new();
    let mut unread: Vec<String> = Vec::new();
    for app in App::ALL {
        let Some(connection) = Connection::configured(state, app) else {
            continue;
        };
        match connection.library().await {
            Ok(held) => wanted.extend(held.into_iter().map(|held| (app, held))),
            Err(e) => unread.push(format!("{} could not be read: {e:#}", app.name())),
        }
    }
    if wanted.is_empty() && !unread.is_empty() {
        anyhow::bail!(unread.join("; "));
    }

    let total = wanted.len();
    let mut tally = Tally::default();
    for (position, (app, held)) in wanted.iter().enumerate() {
        if flag.stopped() {
            return Ok(Pass::Stopped(format!(
                "stopped: {}, of {total}",
                tally.said()
            )));
        }

        let (seeded, fetched) = seed_one(state, *app, held).await;
        note(state, record, position, *app, held, &seeded).await;
        match seeded {
            Seeded::Here(_) => tally.here += 1,
            Seeded::Imported(_) => tally.imported += 1,
            Seeded::Missing | Seeded::NoId => tally.missing += 1,
            Seeded::Failed(_) => tally.failed += 1,
        }

        if tally.done() % 5 == 0
            && let Err(e) = job::progress(
                &state.db,
                record,
                &format!("{} of {total}: {}", tally.done(), tally.said()),
            )
            .await
        {
            tracing::debug!(error = %e, "could not say how far the seed got");
        }
        if fetched {
            tokio::time::sleep(PAUSE).await;
        }
    }

    Ok(Pass::Done {
        summary: format!("{}, of {total}", tally.said()),
        unread: (!unread.is_empty()).then(|| unread.join("; ")),
    })
}

/// What came of one work.
#[derive(Debug)]
pub enum Seeded {
    /// Held here already, under this id.
    Here(String),
    /// Held there under no id this server could look it up by.
    NoId,
    Imported(Box<MediaItem>),
    /// No provider had it.
    Missing,
    Failed(String),
}

/// Import one work of the library unless it is held: what came of it, and
/// whether a provider was asked.
pub async fn seed_one(state: &AppState, app: App, held: &Held) -> (Seeded, bool) {
    // A work entered by hand, under the number made of its id here: held
    // here, or nowhere a source could be asked for it.
    let client_id = match app {
        App::Sonarr => held.tvdb_id,
        App::Radarr => held.tmdb_id,
    };
    if let Some(client_id) = client_id.filter(|id| ids::is_own(*id)) {
        let kind = match app {
            App::Sonarr => MediaKind::Series,
            App::Radarr => MediaKind::Movie,
        };
        return match service::by_own_id(state, kind, client_id).await {
            Ok(Some(item)) => (Seeded::Here(item.id), false),
            Ok(None) => (Seeded::NoId, false),
            Err(e) => (Seeded::Failed(format!("{e:#}")), false),
        };
    }

    let ids = lookup(app, held);
    let Some((source, value)) = ids.first() else {
        return (Seeded::NoId, false);
    };

    for (source, value) in &ids {
        match repo::item::find_id_by_external(&state.db, *source, value).await {
            Ok(Some(id)) => return (Seeded::Here(id), false),
            Ok(None) => {}
            Err(e) => return (Seeded::Failed(format!("{e:#}")), false),
        }
    }

    let fetched = match (app, *source) {
        (App::Sonarr, _) => match held.tvdb_id.filter(|id| *id > 0) {
            // A task nobody waits on: a slow source is given its time.
            Some(client_id) => {
                crate::providers::unhurried(service::series::by_client_id(state, client_id)).await
            }
            None => crate::providers::unhurried(service::series::by_imdb_id(state, value)).await,
        },
        (App::Radarr, ExternalSource::Imdb) => service::movie::by_imdb_id(state, value).await,
        (App::Radarr, _) => match value.parse::<i64>() {
            Ok(tmdb_id) => service::movie::by_tmdb_id(state, tmdb_id).await,
            Err(_) => return (Seeded::NoId, false),
        },
    };
    let seeded = match fetched {
        Ok(Some(item)) => Seeded::Imported(Box::new(item)),
        Ok(None) => Seeded::Missing,
        Err(e) => Seeded::Failed(crate::providers::clip(&format!("{e:#}"), 300)),
    };
    (seeded, true)
}

/// The ids a work of the library is looked up by here, the one it is
/// imported by first: for Sonarr, the id it holds the series under —
/// TheTVDB's, or one this server made for a TMDB-only series or a Fan-Kai —
/// then IMDb's; for Radarr, TMDB's, then IMDb's.
fn lookup(app: App, held: &Held) -> Vec<(ExternalSource, String)> {
    let mut ids = Vec::new();
    match app {
        App::Sonarr => {
            if let Some(client_id) = held.tvdb_id.filter(|id| *id > 0) {
                ids.push(if let Some(fankai) = ids::from_fankai(client_id) {
                    (ExternalSource::Fankai, fankai.to_string())
                } else if let Some(tmdb) = ids::from_synthetic(client_id) {
                    (ExternalSource::TmdbTv, tmdb.to_string())
                } else {
                    (ExternalSource::TvdbSeries, client_id.to_string())
                });
            }
        }
        App::Radarr => {
            if let Some(tmdb) = held.tmdb_id.filter(|id| *id > 0) {
                ids.push((ExternalSource::TmdbMovie, tmdb.to_string()));
            }
        }
    }
    if let Some(imdb) = held
        .imdb_id
        .as_deref()
        .and_then(crate::domain::ids::normalize_imdb_id)
    {
        ids.push((ExternalSource::Imdb, imdb));
    }
    ids
}

/// One work, written down for the run: as it is held here when it is, by
/// the title the library gives it when it is not.
async fn note(
    state: &AppState,
    record: &str,
    position: usize,
    app: App,
    held: &Held,
    seeded: &Seeded,
) {
    let kind = match app {
        App::Sonarr => MediaKind::Series,
        App::Radarr => MediaKind::Movie,
    };
    let (id, outcome, note) = match seeded {
        Seeded::Here(id) => (id.as_str(), job::Outcome::Skipped, notes::HERE),
        Seeded::NoId => ("", job::Outcome::Failed, notes::NO_ID),
        Seeded::Imported(item) => (item.id.as_str(), job::Outcome::Imported, notes::IMPORTED),
        Seeded::Missing => ("", job::Outcome::Failed, notes::NOT_FOUND),
        Seeded::Failed(why) => ("", job::Outcome::Failed, why.as_str()),
    };
    let title = match seeded {
        Seeded::Imported(item) => Some(item.title.clone()),
        _ => held.title.clone(),
    };
    let title = title.map(|title| match held.year.filter(|year| *year > 0) {
        Some(year) if !title.ends_with(&format!("({year})")) => format!("{title} ({year})"),
        _ => title,
    });
    let touched = job::Touched {
        id,
        title: title.as_deref(),
        kind: Some(kind),
    };
    let position = i64::try_from(position).unwrap_or(i64::MAX);
    if let Err(e) = job::add_entry(&state.db, record, position, touched, outcome, Some(note)).await
    {
        tracing::debug!(error = %e, "could not write down what the seed did to a work");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        arr::client::tests::{Seen, stand_in},
        domain::ExternalIds,
        settings::Scope,
    };

    fn held(id: i64, tvdb: Option<i64>, tmdb: Option<i64>, imdb: Option<&str>) -> Held {
        Held {
            id,
            title: Some(format!("Work {id}")),
            year: Some(2008),
            tvdb_id: tvdb,
            tmdb_id: tmdb,
            imdb_id: imdb.map(String::from),
        }
    }

    #[test]
    fn a_work_is_looked_up_by_the_id_it_is_held_under() {
        assert_eq!(
            lookup(
                App::Sonarr,
                &held(1, Some(81189), Some(1396), Some("tt0903747"))
            ),
            vec![
                (ExternalSource::TvdbSeries, "81189".to_string()),
                (ExternalSource::Imdb, "tt0903747".to_string()),
            ]
        );
        // The ids this server made: a TMDB-only series, a Fan-Kai.
        assert_eq!(
            lookup(App::Sonarr, &held(2, Some(100_001_396), None, None)),
            vec![(ExternalSource::TmdbTv, "1396".to_string())]
        );
        assert_eq!(
            lookup(App::Sonarr, &held(3, Some(200_000_012), None, Some(""))),
            vec![(ExternalSource::Fankai, "12".to_string())]
        );
        assert_eq!(
            lookup(App::Radarr, &held(4, None, Some(603), Some("0133093"))),
            vec![
                (ExternalSource::TmdbMovie, "603".to_string()),
                (ExternalSource::Imdb, "tt0133093".to_string()),
            ]
        );
        assert!(lookup(App::Radarr, &held(5, None, Some(0), None)).is_empty());
    }

    /// A work entered by hand is held by Sonarr and Radarr under the number
    /// made of its id here: found here by it, and no source asked — none
    /// could have it, not even by the IMDb id it was typed in with.
    #[tokio::test]
    async fn a_work_entered_by_hand_is_found_here_by_the_number_it_was_handed() {
        let state = crate::api::native::testing::server().await;
        let mut made = Vec::new();
        for kind in [MediaKind::Series, MediaKind::Movie] {
            let mut work = MediaItem::empty(kind);
            work.title = "Vicineko".into();
            work.slug = format!("vicineko-{kind}");
            work.is_manual = true;
            repo::item::upsert(
                &state.db,
                repo::item::ItemWrite {
                    item: &work,
                    replace_children: true,
                },
            )
            .await
            .unwrap();
            made.push(work.id);
        }
        let series = ids::own(&made[0]);
        let film = ids::own(&made[1]);

        let (seeded, asked) =
            seed_one(&state, App::Sonarr, &held(1, Some(series), None, None)).await;
        assert!(
            matches!(&seeded, Seeded::Here(id) if *id == made[0]),
            "{seeded:?}"
        );
        assert!(!asked);
        let (seeded, asked) = seed_one(&state, App::Radarr, &held(2, None, Some(film), None)).await;
        assert!(
            matches!(&seeded, Seeded::Here(id) if *id == made[1]),
            "{seeded:?}"
        );
        assert!(!asked);

        // A film's number is no series': Sonarr holding it names nothing here.
        let (seeded, asked) = seed_one(
            &state,
            App::Sonarr,
            &held(3, Some(film), None, Some("tt0903747")),
        )
        .await;
        assert!(matches!(seeded, Seeded::NoId), "{seeded:?}");
        assert!(!asked);
    }

    #[tokio::test]
    async fn what_is_held_is_passed_over_and_nothing_is_fetched_twice() {
        let state = crate::api::native::testing::server().await;
        let (at, seen) = stand_in(Arc::new(|request: &Seen| match request.target.as_str() {
            "/api/v3/series" => (
                200,
                r#"[{"id":1,"title":"Breaking Bad","year":2008,"tvdbId":81189},
                    {"id":2,"title":"A series nobody has","tvdbId":0}]"#
                    .into(),
            ),
            _ => (404, String::new()),
        }))
        .await;
        for (key, value) in [
            ("arr.sonarrUrl", at.as_str()),
            ("arr.sonarrKey", "seed-key-0001"),
        ] {
            state
                .settings
                .set(Scope::Server, "", key, value, None)
                .await
                .unwrap();
        }

        let mut series = MediaItem::empty(MediaKind::Series);
        series.title = "Breaking Bad".into();
        series.slug = "breaking-bad".into();
        series.external_ids = ExternalIds {
            tvdb: Some(81189),
            ..Default::default()
        };
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &series,
                replace_children: true,
            },
        )
        .await
        .unwrap();

        // Twice: the second run finds what the first did, and fetches nothing.
        for _ in 0..2 {
            let record = job::start_by(&state.db, KIND, None, Some("admin:test"))
                .await
                .unwrap();
            let flag = cancel::register(&record);
            let Pass::Done { summary, unread } = run(&state, &record, &flag).await.unwrap() else {
                panic!("the run was not stopped");
            };
            assert_eq!(
                summary,
                "1 already here, 0 imported, 1 not found, 0 failed, of 2"
            );
            assert_eq!(unread, None);

            let entries = job::entries(&state.db, &record, 10).await.unwrap();
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].media_id.as_deref(), Some(series.id.as_str()));
            assert_eq!(entries[0].outcome, job::Outcome::Skipped);
            assert_eq!(entries[1].media_id, None);
            assert_eq!(entries[1].outcome, job::Outcome::Failed);
            assert_eq!(entries[1].note.as_deref(), Some(notes::NO_ID));
        }
        assert!(
            seen.lock().iter().all(|s| s.target == "/api/v3/series"),
            "the library was read, and nothing else asked of it"
        );
    }
}

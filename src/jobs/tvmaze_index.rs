//! TVmaze's index of every series it lists, read into the database
//! (`tvmaze.index`), weekly (wave P, P3).
//!
//! The first time, the index is read whole: `/shows?page=N` from the first
//! page until TVmaze answers 404, a page of two hundred and fifty ids at a
//! time — three hundred and eighty-two pages for the ninety-five thousand ids
//! of November 2026, a third of a megabyte each. After that, TVmaze's list of
//! the series it changed (`/updates/shows`), and each of those whose date is
//! not the one kept asked for again (`/shows/{id}`): seventeen hundred in the
//! week measured, where the pages are four hundred. The list asked is the
//! shortest that reaches back to the last reading: the week's when it was
//! read less than a week ago; the month's for a reading a week ago to the
//! minute, which the week's would leave a few minutes short of, and which
//! costs no more calls, the series kept as they are being passed over; and
//! past a month, the pages again.
//!
//! Every request goes through the TVmaze client's pacer, shared with the
//! series it fetches one by one, so that the two together keep to TVmaze's
//! twenty calls in ten seconds; a 429 is waited out for as long as it asks.
//! Nobody waits on a reading — the requests of the clients take their turns
//! between its own — and one can be stopped from the tasks page: what it
//! wrote stays, and the next one begins where it would have.
//!
//! A reading that fails keeps what was there (D1): a series is written as it
//! is read, and one is forgotten only once the whole index has been read
//! without it, or once TVmaze answers 404 for it. A series TVmaze gives
//! something unreadable for is passed over and counted, kept as it was; and
//! a whole reading that stops well short of the highest id kept — a page
//! answered 404 midway, not the index's end — forgets nothing.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::{
    db::{now, parse_rfc3339, repo},
    jobs::datasets::{Ended, Run, TVMAZE_INDEX},
    providers::tvmaze::{IndexShow, Indexed, Since},
    state::AppState,
};

/// The most pages a reading goes through: ten times as many as TVmaze has
/// today is a TVmaze that never answers 404.
const MOST_PAGES: u32 = 4_000;

/// How many series read again one by one are written at a time.
const WRITE_EVERY: usize = 25;

/// How far below the highest id kept a whole reading of the index may end
/// and still be the index's end: four pages' worth. TVmaze numbers its
/// series as it adds them, so the last page holds the newest; a reading that
/// stops further down met a 404 midway, and forgets nothing.
const SHORT_BY: i64 = 4 * 250;

/// How often a reading says how far it has got.
const SAY_EVERY: Duration = Duration::from_secs(2);

/// The list of the series TVmaze changed that reaches back to a reading this
/// long ago, with an hour to spare; `None` past a month, when the pages are
/// read again.
fn window(age: chrono::Duration) -> Option<Since> {
    let spare = chrono::Duration::hours(1);
    [
        (Since::Day, chrono::Duration::days(1)),
        (Since::Week, chrono::Duration::days(7)),
        (Since::Month, chrono::Duration::days(30)),
    ]
    .into_iter()
    .find(|(_, reach)| age <= *reach - spare)
    .map(|(since, _)| since)
}

/// Read TVmaze's index, and keep it: whole the first time — or after a month
/// without a reading — and otherwise the series it changed since the last.
/// What the reading did, as the history of the task says it.
pub(crate) async fn import(state: &AppState, run: &Run<'_>) -> Result<Ended> {
    crate::providers::unhurried(read(state, run)).await
}

async fn read(state: &AppState, run: &Run<'_>) -> Result<Ended> {
    // Before anything is read: a series TVmaze changes meanwhile is newer
    // than the reading, and the next one asks for it.
    let started = now();
    let held = repo::tvmaze::count(&state.db).await?;
    let last = repo::import::get(&state.db, TVMAZE_INDEX).await?;
    let since = last
        .filter(|_| held > 0)
        .and_then(|last| parse_rfc3339(&last.imported_at))
        .and_then(|at| window(chrono::Utc::now() - at));

    match since {
        Some(since) => read_changes(state, run, since, &started).await,
        None => read_pages(state, run, held, &started).await,
    }
}

/// Says how far a reading has got, every [`SAY_EVERY`] at most.
struct Saying {
    last: Option<Instant>,
}

impl Saying {
    async fn say(&mut self, run: &Run<'_>, detail: impl FnOnce() -> String) {
        if self.last.is_none_or(|at| at.elapsed() >= SAY_EVERY) {
            run.say(&detail()).await;
            self.last = Some(Instant::now());
        }
    }
}

/// Read the index whole, a page at a time, writing each page as it comes;
/// then forget the series kept that no page listed.
async fn read_pages(
    state: &AppState,
    run: &Run<'_>,
    held_before: i64,
    started: &str,
) -> Result<Ended> {
    let mut seen: HashSet<i64> = HashSet::new();
    // Listed, though what TVmaze gives for them does not read: kept as they
    // were, not forgotten.
    let mut unread_ids: HashSet<i64> = HashSet::new();
    let mut unread = 0usize;
    let mut page: u32 = 0;
    let mut saying = Saying { last: None };

    loop {
        if run.stopped() {
            return Ok(Ended::Stopped(format!(
                "stopped: {} series from {page} pages",
                seen.len()
            )));
        }
        if page >= MOST_PAGES {
            bail!("TVmaze's index ran past {MOST_PAGES} pages without an end");
        }
        let Some(read) = state.tvmaze.index_page(page).await? else {
            break;
        };
        unread += read.unread;
        unread_ids.extend(read.unread_ids);
        repo::tvmaze::write(&state.db, &read.shows).await?;
        seen.extend(read.shows.iter().map(|show| show.id));
        page += 1;
        saying
            .say(run, || {
                format!("{page} pages read, {} series kept", seen.len())
            })
            .await;
    }

    if seen.is_empty() {
        bail!("TVmaze's index listed no series; what was kept stays");
    }
    // Far fewer than were kept: a reading cut short or misread, not series
    // TVmaze dropped. What it brought is written; nothing is forgotten.
    if (seen.len() as i64) < held_before / 2 {
        bail!(
            "TVmaze's index listed {} series against {held_before} kept; none is forgotten",
            seen.len()
        );
    }

    let kept = repo::tvmaze::ids(&state.db).await?;
    // Ended well below the highest id kept: a page answered 404 midway.
    let highest_seen = seen.iter().chain(&unread_ids).max().copied().unwrap_or(0);
    if let Some(highest_kept) = kept.iter().max().copied()
        && highest_kept - highest_seen > SHORT_BY
    {
        bail!(
            "TVmaze's index ended at the series {highest_seen}, short of the {highest_kept} kept; \
             none is forgotten"
        );
    }
    let gone: Vec<i64> = kept
        .into_iter()
        .filter(|id| !seen.contains(id) && !unread_ids.contains(id))
        .collect();
    repo::tvmaze::forget(&state.db, &gone).await?;
    let kept = repo::tvmaze::count(&state.db).await?;
    repo::import::record(&state.db, TVMAZE_INDEX, kept, started).await?;

    let mut summary = format!("{} series from {page} pages", seen.len());
    if !gone.is_empty() {
        summary.push_str(&format!(", {} no longer listed", gone.len()));
    }
    if unread > 0 {
        summary.push_str(&format!(", {unread} unreadable"));
    }
    Ok(Ended::Done(summary))
}

/// Read again the series TVmaze changed within `since` whose date is not the
/// one kept, writing them a few at a time; forget those TVmaze answers 404
/// for.
async fn read_changes(
    state: &AppState,
    run: &Run<'_>,
    since: Since,
    started: &str,
) -> Result<Ended> {
    let changed = state.tvmaze.changed(since).await?;
    let ids: Vec<i64> = changed.iter().map(|(id, _)| *id).collect();
    let kept = repo::tvmaze::updated_of(&state.db, &ids).await?;
    let due: Vec<i64> = changed
        .iter()
        .filter(|(id, at)| kept.get(id).copied().flatten() != Some(*at))
        .map(|(id, _)| *id)
        .collect();

    let mut batch: Vec<IndexShow> = Vec::with_capacity(WRITE_EVERY);
    let mut gone: Vec<i64> = Vec::new();
    let mut read = 0usize;
    // Passed over, kept as they were: what TVmaze gives for them does not read.
    let mut unread = 0usize;
    let mut saying = Saying { last: None };

    let reading: Result<bool> = async {
        for (done, id) in due.iter().enumerate() {
            if run.stopped() {
                return Ok(false);
            }
            match state.tvmaze.indexed(*id).await? {
                Indexed::Listed(show) => batch.push(*show),
                Indexed::Gone => gone.push(*id),
                Indexed::Unreadable => unread += 1,
            }
            if batch.len() >= WRITE_EVERY {
                repo::tvmaze::write(&state.db, &batch).await?;
                read += batch.len();
                batch.clear();
            }
            saying
                .say(run, || {
                    format!("{} of {}: series read again", done + 1, due.len())
                })
                .await;
        }
        Ok(true)
    }
    .await;

    // What was read stays, however the reading ended.
    repo::tvmaze::write(&state.db, &batch).await?;
    read += batch.len();
    repo::tvmaze::forget(&state.db, &gone).await?;

    let unreadable = match unread {
        0 => String::new(),
        n => format!(", {n} unreadable"),
    };
    if !reading? {
        return Ok(Ended::Stopped(format!(
            "stopped: {read} of {} changed series read again, {} gone{unreadable}",
            due.len(),
            gone.len()
        )));
    }

    let held = repo::tvmaze::count(&state.db).await?;
    repo::import::record(&state.db, TVMAZE_INDEX, held, started).await?;
    Ok(Ended::Done(format!(
        "{read} of {} series changed in the {} read again, {} gone{unreadable}: {held} in all",
        changed.len(),
        since.as_str(),
        gone.len()
    )))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use axum::{
        http::{StatusCode, Uri},
        response::IntoResponse as _,
    };
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        db::{repo::tvmaze::Key, to_rfc3339},
        jobs::{
            datasets::{Asked, import_now},
            tasks::{self, TaskId},
        },
        settings::registry::Scope,
    };

    /// What the stand-in for TVmaze serves, and what it was asked.
    #[derive(Default)]
    struct Served {
        /// Each page of the index, by its number; past the last, 404.
        pages: Vec<Value>,
        /// The series it says it changed, by id, and when.
        changed: Value,
        /// Each series asked for alone, by id; one not here, 404.
        shows: HashMap<i64, Value>,
        /// A request answered 429 once before it is served: its path and query.
        refuse_once: Option<String>,
        /// What it was asked, path and query, in order.
        asked: Vec<String>,
    }

    /// TVmaze, served from here as TVmaze serves it.
    async fn tvmaze_at(served: Served) -> (String, Arc<Mutex<Served>>) {
        let served = Arc::new(Mutex::new(served));
        let kept = Arc::clone(&served);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let at = format!("http://{}", listener.local_addr().expect("its address"));
        let app = axum::Router::new().fallback(move |uri: Uri| {
            let served = Arc::clone(&kept);
            async move {
                let query = uri.query().unwrap_or_default();
                let asked = match query {
                    "" => uri.path().to_string(),
                    query => format!("{}?{query}", uri.path()),
                };
                let mut served = served.lock().expect("served");
                served.asked.push(asked.clone());
                if served.refuse_once.as_deref() == Some(asked.as_str()) {
                    served.refuse_once = None;
                    return (StatusCode::TOO_MANY_REQUESTS, [("Retry-After", "1")], "")
                        .into_response();
                }
                let found = match uri.path() {
                    "/shows" => query
                        .strip_prefix("page=")
                        .and_then(|page| page.parse::<usize>().ok())
                        .and_then(|page| served.pages.get(page).cloned()),
                    "/updates/shows" => Some(served.changed.clone()),
                    path => path
                        .strip_prefix("/shows/")
                        .and_then(|id| id.parse::<i64>().ok())
                        .and_then(|id| served.shows.get(&id).cloned()),
                };
                match found {
                    Some(body) => axum::Json(body).into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
        });
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("served");
        });
        (at, served)
    }

    /// A series as TVmaze's index lists it, on a network.
    fn show(id: i64, name: &str, weight: i64, updated: i64) -> Value {
        json!({
            "id": id,
            "url": format!("https://www.tvmaze.com/shows/{id}"),
            "name": name,
            "type": "Scripted",
            "language": "English",
            "genres": ["Drama"],
            "status": "Running",
            "premiered": "2019-01-04",
            "ended": null,
            "network": {
                "id": 1,
                "name": "Tidewater Broadcasting",
                "country": { "name": "United Kingdom", "code": "GB", "timezone": "Europe/London" }
            },
            "webChannel": null,
            "externals": { "tvrage": null, "thetvdb": 700_000 + id, "imdb": format!("tt{:07}", 7_000_000 + id) },
            "weight": weight,
            "updated": updated,
            "summary": "<p>Not kept.</p>",
            "image": { "medium": "m.jpg", "original": "o.jpg" }
        })
    }

    /// One that streams on a web channel of no country.
    fn streamed(id: i64, name: &str, weight: i64, updated: i64) -> Value {
        let mut show = show(id, name, weight, updated);
        show["network"] = Value::Null;
        show["webChannel"] = json!({ "id": 9, "name": "Lanternlight+", "country": null });
        show
    }

    /// The server, TVmaze at `at`, its index switched on.
    async fn server(at: &str) -> AppState {
        let mut config = crate::config::Config::from_env().expect("a configuration");
        config.mode = crate::config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = crate::config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = None;
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;
        config.tvmaze.upstream = at.to_string();

        let state = AppState::bootstrap(config).await.expect("a server");
        state
            .settings
            .set(Scope::Server, "", "tvmaze.index", "true", None)
            .await
            .expect("switched on");
        state
    }

    /// The index read now, waiting its turn: the import lock is the
    /// process's, and another test's import may be holding it.
    async fn import_when_free(state: &AppState) -> Asked {
        for _ in 0..1200 {
            match import_now(state, TVMAZE_INDEX, None).await {
                Asked::Busy => tokio::time::sleep(Duration::from_millis(50)).await,
                asked => return asked,
            }
        }
        panic!("the import lock stayed taken for a minute");
    }

    /// What reading the index now came to, which it did.
    async fn read_when_free(state: &AppState) -> String {
        match import_when_free(state).await {
            Asked::Imported(summary) => summary,
            Asked::Failed(e) => panic!("{e:#}"),
            _ => panic!("not read"),
        }
    }

    #[tokio::test]
    async fn tvmazes_index_is_read_whole_then_by_what_changed_and_whole_again_a_month_on() {
        let mut broken = show(3, "Unreadable", 1, 1);
        broken["id"] = json!("three");
        let (at, served) = tvmaze_at(Served {
            pages: vec![
                json!([
                    show(1, "Quillmere", 60, 1_000),
                    streamed(2, "Lanternfall", 40, 1_000),
                    broken
                ]),
                json!([show(250, "Harrowgate Close", 90, 1_000)]),
            ],
            // Asked to wait a second, the reading does.
            refuse_once: Some("/shows?page=1".into()),
            ..Served::default()
        })
        .await;
        let state = server(&at).await;
        let asked = || std::mem::take(&mut served.lock().expect("served").asked);

        // The first time, every page, until the one TVmaze answers 404.
        assert_eq!(
            read_when_free(&state).await,
            "3 series from 2 pages, 1 unreadable"
        );
        assert_eq!(
            asked(),
            [
                "/shows?page=0",
                "/shows?page=1",
                "/shows?page=1",
                "/shows?page=2"
            ]
        );
        let kept = repo::tvmaze::by(&state.db, Key::Tvmaze, &[1, 2, 250])
            .await
            .unwrap();
        assert_eq!(
            kept[&1],
            IndexShow {
                id: 1,
                name: "Quillmere".into(),
                kind: Some("Scripted".into()),
                language: Some("English".into()),
                status: Some("Running".into()),
                premiered: Some("2019-01-04".into()),
                ended: None,
                network: Some("Tidewater Broadcasting".into()),
                country: Some("GB".into()),
                web: false,
                tvdb_id: Some(700_001),
                imdb_id: Some("tt7000001".into()),
                tvrage_id: None,
                weight: Some(60),
                updated: Some(1_000),
            }
        );
        assert_eq!(
            (
                kept[&2].network.as_deref(),
                kept[&2].country.as_deref(),
                kept[&2].web
            ),
            (Some("Lanternlight+"), None, true)
        );

        // Then what TVmaze changed since — today's list, the last reading
        // being a moment ago: the series whose date moved, or new; one it
        // answers 404 for is gone.
        {
            let mut served = served.lock().expect("served");
            served.changed = json!({ "1": 1_000, "2": 2_000, "251": 2_000, "9": 2_000 });
            served.shows = HashMap::from([
                (2, streamed(2, "Lanternfall", 80, 2_000)),
                (251, show(251, "Ninefold Reach", 30, 2_000)),
            ]);
        }
        assert_eq!(
            read_when_free(&state).await,
            "2 of 4 series changed in the day read again, 1 gone: 4 in all"
        );
        assert_eq!(
            asked(),
            [
                "/updates/shows?since=day",
                "/shows/2",
                "/shows/9",
                "/shows/251"
            ]
        );
        let kept = repo::tvmaze::by(&state.db, Key::Tvmaze, &[2, 251])
            .await
            .unwrap();
        assert_eq!(kept[&2].weight, Some(80));
        assert_eq!(kept[&251].name, "Ninefold Reach");

        // A month on, the pages again: what they no longer list goes.
        let long_ago = to_rfc3339(chrono::Utc::now() - chrono::Duration::days(40));
        repo::import::record(&state.db, TVMAZE_INDEX, 4, &long_ago)
            .await
            .unwrap();
        assert_eq!(
            read_when_free(&state).await,
            "3 series from 2 pages, 1 no longer listed, 1 unreadable"
        );
        assert_eq!(repo::tvmaze::ids(&state.db).await.unwrap(), [1, 2, 250]);

        // Searched by name, with the ids TVmaze files it under elsewhere.
        let found = crate::service::tvmaze_index::search(&state, "harrowgate", 5)
            .await
            .unwrap();
        assert_eq!(
            found
                .iter()
                .map(|show| (show.id, show.tvdb_id))
                .collect::<Vec<_>>(),
            [(250, Some(700_250))]
        );

        // The task says it ran, and comes again in a week.
        let task = tasks::state_named(&state, TaskId::ImportTvmaze)
            .await
            .unwrap();
        assert_eq!(task.mode, tasks::Mode::Scheduled);
        assert_eq!(task.every_seconds, Some(7 * 24 * 60 * 60));
        assert_eq!(
            task.last.map(|run| run.status),
            Some(repo::job::Status::Succeeded)
        );
        assert!(task.schedulable);
        assert!(tasks::stops_partway(TaskId::ImportTvmaze.as_str()));
    }

    #[tokio::test]
    async fn a_reading_that_finds_far_fewer_series_than_were_kept_forgets_none() {
        let (at, _served) = tvmaze_at(Served {
            pages: vec![json!([show(1, "Quillmere", 60, 1_000)])],
            ..Served::default()
        })
        .await;
        let state = server(&at).await;
        let kept: Vec<IndexShow> = (10..20)
            .map(|id| IndexShow {
                id,
                name: format!("Kept {id}"),
                ..IndexShow::default()
            })
            .collect();
        repo::tvmaze::write(&state.db, &kept).await.unwrap();

        let Asked::Failed(e) = import_when_free(&state).await else {
            panic!("taken");
        };
        assert!(e.to_string().contains("1 series against 10 kept"), "{e:#}");
        assert_eq!(repo::tvmaze::count(&state.db).await.unwrap(), 11);
        assert!(
            repo::import::get(&state.db, TVMAZE_INDEX)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// A series TVmaze lists but gives something unreadable for is kept as
    /// it was: passed over by a whole reading, which forgets it not, and by
    /// a reading of what changed, which goes on with the others.
    #[tokio::test]
    async fn a_series_that_does_not_read_is_kept_as_it_was_and_passed_over() {
        let mut broken = show(2, "Unreadable", 1, 1);
        broken["name"] = json!(42);
        let (at, served) = tvmaze_at(Served {
            pages: vec![json!([
                show(1, "Quillmere", 60, 1_000),
                broken,
                show(3, "Harrowgate Close", 90, 1_000)
            ])],
            ..Served::default()
        })
        .await;
        let state = server(&at).await;
        let before = IndexShow {
            id: 2,
            name: "Kept Before".into(),
            ..IndexShow::default()
        };
        repo::tvmaze::write(&state.db, &[before]).await.unwrap();

        assert_eq!(
            read_when_free(&state).await,
            "2 series from 1 pages, 1 unreadable"
        );
        assert_eq!(
            repo::tvmaze::ids(&state.db).await.unwrap(),
            [1, 2, 3],
            "the one that did not read is kept"
        );

        {
            let mut served = served.lock().expect("served");
            served.changed = json!({ "1": 2_000, "3": 2_000 });
            let mut unreadable = show(1, "Quillmere", 70, 2_000);
            unreadable["name"] = json!(["not", "a", "name"]);
            served.shows =
                HashMap::from([(1, unreadable), (3, show(3, "Harrowgate Close", 95, 2_000))]);
        }
        assert_eq!(
            read_when_free(&state).await,
            "1 of 2 series changed in the day read again, 0 gone, 1 unreadable: 3 in all"
        );
        let kept = repo::tvmaze::by(&state.db, Key::Tvmaze, &[1, 3])
            .await
            .unwrap();
        assert_eq!(kept[&1].weight, Some(60), "kept as it was");
        assert_eq!(kept[&3].weight, Some(95));
    }

    /// A whole reading that ends far below the highest id kept met a 404
    /// midway, not the index's end: nothing is forgotten.
    #[tokio::test]
    async fn a_reading_that_ends_far_below_the_highest_id_kept_forgets_none() {
        let (at, _served) = tvmaze_at(Served {
            pages: vec![json!([
                show(1, "Quillmere", 60, 1_000),
                show(2, "Lanternfall", 40, 1_000)
            ])],
            ..Served::default()
        })
        .await;
        let state = server(&at).await;
        let far_up = IndexShow {
            id: 5_000,
            name: "Far Up".into(),
            ..IndexShow::default()
        };
        repo::tvmaze::write(&state.db, &[far_up]).await.unwrap();

        let Asked::Failed(e) = import_when_free(&state).await else {
            panic!("taken");
        };
        assert!(
            e.to_string()
                .contains("ended at the series 2, short of the 5000 kept"),
            "{e:#}"
        );
        assert_eq!(repo::tvmaze::ids(&state.db).await.unwrap(), [1, 2, 5_000]);
    }

    #[test]
    fn the_list_of_changes_asked_is_the_shortest_that_reaches_back_to_the_last_reading() {
        let hours = chrono::Duration::hours;
        assert_eq!(window(hours(2)), Some(Since::Day));
        assert_eq!(window(hours(23)), Some(Since::Day));
        assert_eq!(window(hours(24)), Some(Since::Week));
        assert_eq!(window(hours(6 * 24 + 22)), Some(Since::Week));
        // A week ago to the minute: the week's list would stop short of it.
        assert_eq!(window(hours(7 * 24)), Some(Since::Month));
        assert_eq!(window(hours(29 * 24)), Some(Since::Month));
        assert_eq!(window(hours(30 * 24)), None);
    }
}

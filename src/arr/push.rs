//! Telling the operator's Sonarr and Radarr that a work changed here.
//!
//! Sonarr reads a series again twelve hours after it last did, Radarr a film
//! about as often: a title locked here, an episode put right, waits that long
//! to reach them — or for somebody to press their refresh button. With
//! `arr.pushRefresh` on, the change is told instead: the work is found there
//! by the id it is served under (TheTVDB's, or the synthetic one a TMDB-only
//! series or a Fan-Kai is handed, for Sonarr; TMDB's for Radarr) and asked to
//! be read again, `RefreshSeries` or `RefreshMovie`.
//!
//! What changes a work for its clients: a lock put on it, on one of its
//! seasons or its episodes, changed or taken off; an edit of its seasons or
//! episodes; a sync; a refresh that changed what it is served as — not one
//! that found everything as it was, which would have Sonarr read its library
//! again after every sweep. A refresh is the schedule's or one asked for by
//! hand; the fetch a client's own request makes of a work gone stale is not
//! told, since the client asking has the answer already.
//!
//! Gathered by work over a short quiet, so that a burst is one command: an
//! editor locking six fields of a work is told ten seconds after the last of
//! them — or a minute after the first, however long the burst goes on. What
//! is due at once is one command for each of the two, every work's id in it.
//! Each instance tells of the changes made on it, from a loop of its own,
//! kept like the others; what came of each work is written down for the
//! page, and a failure is said in the log. An app that does not answer at all
//! is asked once, not once for each work: the works not yet told of wait for
//! the next pass.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::LazyLock,
    time::{Duration, Instant},
};

use serde_json::{Map, Value};

use super::{
    client::{App, Connection, unanswered},
    history::{self, Outcome, Told},
};
use crate::{
    db::repo::audit::Action,
    domain::{MediaItem, MediaKind},
    service,
    state::AppState,
};

/// The switch.
pub const SETTING: &str = "arr.pushRefresh";

/// How long a work waits after the last change to it before it is told of.
const QUIET: Duration = Duration::from_secs(10);

/// How long a work that goes on changing waits at most.
const LONGEST: Duration = Duration::from_secs(60);

/// The most works one command carries.
const MOST_PER_COMMAND: usize = 100;

/// The most works waiting at once: past it a change is not told, which the
/// log says — a bulk edit of the whole catalogue is better told by the
/// next refresh than held in memory.
const MOST_WAITING: usize = 10_000;

/// Why a work is told of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// A lock put on it, changed or taken off.
    Lock,
    /// Its seasons or episodes edited by hand, or the work switched on or off.
    Edit,
    /// Asked of some of its sources again.
    Sync,
    /// Refreshed, and served differently since.
    Refresh,
}

impl Reason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lock => "lock",
            Self::Edit => "edit",
            Self::Sync => "sync",
            Self::Refresh => "refresh",
        }
    }
}

/// One work waiting to be told of.
#[derive(Debug)]
struct Pending {
    first: Instant,
    last: Instant,
    reasons: BTreeSet<Reason>,
}

impl Pending {
    fn due(&self) -> Instant {
        (self.last + QUIET).min(self.first + LONGEST)
    }
}

/// The works waiting, by id.
#[derive(Debug, Default)]
pub struct Waiting {
    works: HashMap<String, Pending>,
}

impl Waiting {
    /// A change to a work, at `now`. False when too many are waiting to take
    /// another.
    pub fn add(&mut self, id: &str, reason: Reason, now: Instant) -> bool {
        if let Some(pending) = self.works.get_mut(id) {
            pending.last = now;
            pending.reasons.insert(reason);
            return true;
        }
        if self.works.len() >= MOST_WAITING {
            return false;
        }
        self.works.insert(
            id.to_string(),
            Pending {
                first: now,
                last: now,
                reasons: BTreeSet::from([reason]),
            },
        );
        true
    }

    /// When the next work is due, if one waits.
    pub fn next_due(&self) -> Option<Instant> {
        self.works.values().map(Pending::due).min()
    }

    /// The works due by `now`, taken out, each with why — in the order their
    /// first change came in.
    pub fn take_due(&mut self, now: Instant) -> Vec<(String, Vec<Reason>)> {
        let mut due: Vec<(Instant, String)> = self
            .works
            .iter()
            .filter(|(_, pending)| pending.due() <= now)
            .map(|(id, pending)| (pending.first, id.clone()))
            .collect();
        due.sort();
        due.into_iter()
            .filter_map(|(_, id)| {
                let pending = self.works.remove(&id)?;
                Some((id, pending.reasons.into_iter().collect()))
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.works.len()
    }
}

/// This process's works waiting, and the word that one was added.
struct Queue {
    waiting: parking_lot::Mutex<Waiting>,
    wake: tokio::sync::Notify,
}

static QUEUE: LazyLock<Queue> = LazyLock::new(|| Queue {
    waiting: parking_lot::Mutex::new(Waiting::default()),
    wake: tokio::sync::Notify::new(),
});

/// How many works wait to be told of, on this instance.
pub fn waiting() -> usize {
    QUEUE.waiting.lock().len()
}

/// Tell Sonarr or Radarr of this work, soon, when telling is on.
pub fn offer(state: &AppState, id: &str, reason: Reason) {
    if !state.flag(SETTING, false) {
        return;
    }
    if QUEUE.waiting.lock().add(id, reason, Instant::now()) {
        QUEUE.wake.notify_one();
    } else {
        tracing::warn!(
            %id,
            "too many works wait to be told to Sonarr and Radarr; this one is told at its next \
             change"
        );
    }
}

/// What an entry of the trail changed for a work's clients, offered: a lock
/// put, changed or taken off (`override.*`), an edit, a sync — each naming
/// the work by the id its target opens with.
pub fn heard(state: &AppState, action: Action, target: Option<&str>) {
    let reason = match action {
        Action::OverrideSet | Action::OverrideRemoved | Action::OverridesCleared => Reason::Lock,
        Action::ItemUpdated => Reason::Edit,
        Action::ItemSynced => Reason::Sync,
        _ => return,
    };
    if let Some(id) = target.and_then(work_id) {
        offer(state, id, reason);
    }
}

/// The work id a target opens with — `{id}`, or `{id}#{scope}/{field}` for a
/// lock — if it opens with one.
fn work_id(target: &str) -> Option<&str> {
    let id = target.get(..36)?;
    let shaped = id.bytes().enumerate().all(|(i, b)| match i {
        8 | 13 | 18 | 23 => b == b'-',
        _ => b.is_ascii_hexdigit(),
    });
    let ends = target.len() == 36 || target[36..].starts_with('#');
    (shaped && ends).then_some(id)
}

/// A refresh that came back: offered when it changed what the work is
/// served as. Compared only with telling on and the work's Sonarr or Radarr
/// connected — it reads both copies whole.
pub fn refreshed(state: &AppState, before: &MediaItem, after: &MediaItem) {
    if !state.flag(SETTING, false)
        || Connection::configured(state, App::of(after.kind)).is_none()
        || !served_differently(before, after)
    {
        return;
    }
    offer(state, &after.id, Reason::Refresh);
}

/// What changes at every refresh and is nobody's news: when it was fetched
/// and is due again, the ids its rows are written under — new at each write
/// — a popularity and vote counts that move every day.
const UNSERVED: &[&str] = &[
    "id",
    "createdAt",
    "updatedAt",
    "refreshedAt",
    "refreshAfter",
    "refreshError",
    "popularity",
    "ratings",
    "rating",
];

/// Whether two copies of a work are served differently.
pub fn served_differently(before: &MediaItem, after: &MediaItem) -> bool {
    served(before) != served(after)
}

fn served(item: &MediaItem) -> Value {
    let mut value = serde_json::to_value(item).unwrap_or_default();
    chosen_by_address(&mut value);
    strip(&mut value);
    value
}

/// The pictures chosen to lead with — the work's, each season's — named by
/// their address rather than by the id of the row each is kept under, which
/// a refresh writes anew: what a client is given of a choice is the picture,
/// and the same picture under a new row is no news. Read before the ids are
/// stripped, while they still say which picture each names.
fn chosen_by_address(value: &mut Value) {
    fn name_by_address(holder: &mut Map<String, Value>) {
        let addresses: Vec<(String, Value)> = match holder.get("images") {
            Some(Value::Array(images)) => images
                .iter()
                .filter_map(|image| {
                    let id = image.get("id")?.as_str()?.to_string();
                    Some((id, image.get("url").cloned().unwrap_or(Value::Null)))
                })
                .collect(),
            _ => Vec::new(),
        };
        if let Some(Value::Object(chosen)) = holder.get_mut("primaryImages") {
            for slot in chosen.values_mut() {
                let address = slot.as_str().and_then(|id| {
                    addresses
                        .iter()
                        .find(|(held, _)| held == id)
                        .map(|(_, url)| url.clone())
                });
                *slot = address.unwrap_or(Value::Null);
            }
        }
    }

    let Value::Object(work) = value else {
        return;
    };
    name_by_address(work);
    if let Some(Value::Array(seasons)) = work.get_mut("seasons") {
        for season in seasons {
            if let Value::Object(season) = season {
                name_by_address(season);
            }
        }
    }
}

fn strip(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in UNSERVED {
                map.remove(*key);
            }
            map.values_mut().for_each(strip);
        }
        Value::Array(entries) => entries.iter_mut().for_each(strip),
        _ => {}
    }
}

/// Tell what is due, until the process stops.
pub async fn run(state: AppState) {
    loop {
        let next = QUEUE.waiting.lock().next_due();
        match next {
            None => QUEUE.wake.notified().await,
            Some(at) => {
                tokio::select! {
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(at)) => {}
                    () = QUEUE.wake.notified() => {}
                }
            }
        }

        let due = QUEUE.waiting.lock().take_due(Instant::now());
        if !due.is_empty() {
            send(&state, due).await;
        }
    }
}

/// One work about to be told of.
struct Target {
    id: String,
    title: String,
    client_id: Option<i64>,
    reasons: Vec<String>,
    /// The same reasons, as the queue names them: what a work that could not
    /// be told of yet is offered again for.
    kinds: Vec<Reason>,
}

/// Tell each app of its works: found there one by one, then asked to read
/// them again in one command.
pub async fn send(state: &AppState, due: Vec<(String, Vec<Reason>)>) {
    // Switched off while they waited: nobody is told.
    if !state.flag(SETTING, false) {
        return;
    }

    let mut by_app: BTreeMap<App, Vec<Target>> = BTreeMap::new();
    for (id, reasons) in due {
        let item = match service::load(state, &id).await {
            Ok(Some(item)) => item,
            // Deleted meanwhile: nothing to tell.
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(%id, error = format_args!("{e:#}"), "could not read a work to tell Sonarr or Radarr of");
                continue;
            }
        };
        let client_id = match item.kind {
            MediaKind::Series => service::series::client_id(&item),
            MediaKind::Movie => item.external_ids.tmdb,
        };
        by_app.entry(App::of(item.kind)).or_default().push(Target {
            id,
            title: item.title,
            client_id,
            reasons: reasons.iter().map(|r| r.as_str().to_string()).collect(),
            kinds: reasons,
        });
    }

    for (app, targets) in by_app {
        // Not connected: telling is on for the other one.
        let Some(connection) = Connection::configured(state, app) else {
            tracing::debug!(
                app = app.as_str(),
                works = targets.len(),
                "not connected; not told"
            );
            continue;
        };
        tell(state, &connection, targets).await;
    }
}

/// Tell one app of its works: each found there, then every one found asked to
/// read again in one command.
///
/// An app that does not answer — nobody connects, or it takes too long — is
/// asked once, not once for each work, each waiting out the same timeout and
/// each with a line in the log: the work that found out is written down as
/// failed, and the rest are offered again, to be told at the next pass.
async fn tell(state: &AppState, connection: &Connection, targets: Vec<Target>) {
    let app = connection.app;
    // Each row as written, with what came of it.
    let mut rows: Vec<(Target, Option<i64>, Outcome, Option<String>)> = Vec::new();
    let mut found: Vec<(Target, i64)> = Vec::new();

    let mut targets = targets.into_iter();
    while let Some(target) = targets.next() {
        let Some(client_id) = target.client_id else {
            let why = format!("no id {} could know it by", app.name());
            rows.push((target, None, Outcome::Absent, Some(why)));
            continue;
        };
        match connection.find(client_id).await {
            Ok(Some(held)) => found.push((target, held.id)),
            Ok(None) => rows.push((target, None, Outcome::Absent, None)),
            Err(e) if unanswered(&e) => {
                // Those already found are put back too: the command that
                // would tell of them goes to the address that just did not
                // answer.
                let why = crate::providers::clip(&format!("{e:#}"), 300);
                let mut back: Vec<Target> = found.drain(..).map(|(target, _)| target).collect();
                back.extend(targets.by_ref());
                tracing::warn!(
                    app = app.as_str(),
                    id = %target.id,
                    error = %why,
                    waiting = back.len(),
                    "nobody answered; the works not yet told of wait for the next pass"
                );
                rows.push((target, None, Outcome::Failed, Some(why)));
                for target in back {
                    for reason in target.kinds {
                        offer(state, &target.id, reason);
                    }
                }
                break;
            }
            Err(e) => {
                let why = crate::providers::clip(&format!("{e:#}"), 300);
                tracing::warn!(app = app.as_str(), id = %target.id, error = %why, "could not find a work to tell of");
                rows.push((target, None, Outcome::Failed, Some(why)));
            }
        }
    }

    let mut found = found.into_iter().peekable();
    while found.peek().is_some() {
        let chunk: Vec<(Target, i64)> = found.by_ref().take(MOST_PER_COMMAND).collect();
        let ids: Vec<i64> = chunk.iter().map(|(_, id)| *id).collect();
        match connection.refresh(&ids).await {
            Ok(command) => {
                tracing::info!(
                    app = app.as_str(),
                    works = ids.len(),
                    ?command,
                    "asked to read works again"
                );
                let said = command.map(|command| format!("command {command}"));
                for (target, arr_id) in chunk {
                    rows.push((target, Some(arr_id), Outcome::Sent, said.clone()));
                }
            }
            Err(e) => {
                let why = crate::providers::clip(&format!("{e:#}"), 300);
                tracing::warn!(app = app.as_str(), works = ids.len(), error = %why, "could not ask to read works again");
                for (target, arr_id) in chunk {
                    rows.push((target, Some(arr_id), Outcome::Failed, Some(why.clone())));
                }
            }
        }
    }

    let told: Vec<Told<'_>> = rows
        .iter()
        .map(|(target, arr_id, outcome, detail)| Told {
            app,
            media_id: &target.id,
            title: &target.title,
            client_id: target.client_id,
            arr_id: *arr_id,
            outcome: *outcome,
            reasons: &target.reasons,
            detail: detail.as_deref(),
        })
        .collect();
    if let Err(e) = history::add(&state.db, &told).await {
        tracing::warn!(
            error = format_args!("{e:#}"),
            "could not write down what was told"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        arr::client::tests::{Seen, stand_in},
        domain::{Episode, ExternalIds},
        settings::Scope,
    };

    const SERIES: &str = "0192aa52-8f62-7cc3-a1f1-6a1b2c3d4e5f";
    const FILM: &str = "0192aa52-8f62-7cc3-a1f1-6a1b2c3d4e60";

    #[test]
    fn a_burst_is_told_once_after_its_quiet_and_a_stream_after_a_minute() {
        let t0 = Instant::now();
        let at = |seconds: u64| t0 + Duration::from_secs(seconds);
        let mut waiting = Waiting::default();

        // Six locks on one work over four seconds, a sync among them; one
        // edit of another.
        for (second, reason) in [
            (0, Reason::Lock),
            (1, Reason::Lock),
            (2, Reason::Sync),
            (4, Reason::Lock),
        ] {
            assert!(waiting.add(SERIES, reason, at(second)));
        }
        assert!(waiting.add(FILM, Reason::Edit, at(1)));
        assert_eq!(waiting.len(), 2);

        assert!(
            waiting.take_due(at(5)).is_empty(),
            "nothing before the quiet"
        );
        assert_eq!(waiting.next_due(), Some(at(11)));
        assert_eq!(
            waiting.take_due(at(11)),
            vec![(FILM.to_string(), vec![Reason::Edit])]
        );
        // One command for the whole burst, with every reason it had.
        assert_eq!(
            waiting.take_due(at(14)),
            vec![(SERIES.to_string(), vec![Reason::Lock, Reason::Sync])]
        );
        assert_eq!(waiting.len(), 0);
        assert_eq!(waiting.next_due(), None);

        // A work that changes every five seconds is told a minute after the
        // first change, however long it goes on.
        for second in (0..=70).step_by(5) {
            waiting.add(SERIES, Reason::Lock, at(second));
            let taken = waiting.take_due(at(second));
            if second < 60 {
                assert!(taken.is_empty(), "at {second} s");
            } else {
                assert_eq!(taken.len(), 1, "at {second} s");
                break;
            }
        }
    }

    #[test]
    fn a_target_names_its_work_by_the_id_it_opens_with() {
        assert_eq!(work_id(SERIES), Some(SERIES));
        assert_eq!(
            work_id(&format!("{SERIES}#episode:1x2/title")),
            Some(SERIES)
        );
        assert_eq!(work_id("admin"), None);
        assert_eq!(work_id(&format!("{SERIES}tail")), None);
    }

    /// A provider's episode, under a row id of its own as every write gives.
    fn episode(season: i32, number: i32, tvdb: i64) -> Episode {
        let mut episode = crate::db::repo::child::blank_episode(season, number);
        episode.id = crate::db::new_id();
        episode.tvdb_id = Some(tvdb);
        episode.is_manual = false;
        episode
    }

    #[test]
    fn a_refresh_that_moved_nothing_served_is_no_change() {
        let mut before = MediaItem::empty(MediaKind::Series);
        before.id = SERIES.into();
        before.title = "Breaking Bad".into();
        before.popularity = Some(150.2);
        before.episodes = vec![episode(1, 1, 349232)];
        before.refreshed_at = Some("2026-10-01T00:00:00.000Z".into());

        // Fetched again: new row ids, a new date, another popularity.
        let mut after = before.clone();
        after.popularity = Some(171.9);
        after.refreshed_at = Some("2026-10-11T00:00:00.000Z".into());
        after.episodes = vec![episode(1, 1, 349232)];
        assert_ne!(before.episodes[0].id, after.episodes[0].id);
        assert!(!served_differently(&before, &after));

        // An episode retitled is news.
        after.episodes[0].title = "Pilot".into();
        assert!(served_differently(&before, &after));
    }

    /// A provider's picture, under a row id of its own as every write gives.
    fn picture(url: &str, season: Option<i32>) -> crate::domain::Image {
        crate::domain::Image {
            id: crate::db::new_id(),
            season_number: season,
            cover_type: crate::domain::CoverType::Poster,
            url: url.into(),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        }
    }

    /// The work's own pictures and its first season's, each leading with
    /// the first, as a person chose — under the row ids of this write.
    fn written(mut item: MediaItem) -> MediaItem {
        item.images = vec![
            picture("https://img.example/a.jpg", None),
            picture("https://img.example/b.jpg", None),
        ];
        item.primary_images.poster = Some(item.images[0].id.clone());
        let mut season = crate::db::repo::child::blank_season(1);
        season.images = vec![
            picture("https://img.example/s1.jpg", Some(1)),
            picture("https://img.example/s1b.jpg", Some(1)),
        ];
        season.primary_images.poster = Some(season.images[0].id.clone());
        item.seasons = vec![season];
        item
    }

    #[test]
    fn a_picture_chosen_is_the_same_news_under_a_new_row() {
        let mut work = MediaItem::empty(MediaKind::Series);
        work.id = SERIES.into();
        work.title = "Breaking Bad".into();
        let before = written(work.clone());
        // Fetched again: every picture written under a new row id, the
        // choices following them.
        let after = written(work);
        assert_ne!(before.primary_images.poster, after.primary_images.poster);
        assert_ne!(
            before.seasons[0].primary_images.poster,
            after.seasons[0].primary_images.poster
        );
        assert!(!served_differently(&before, &after));

        // Another picture chosen is news.
        let mut other = after.clone();
        other.primary_images.poster = Some(other.images[1].id.clone());
        assert!(served_differently(&before, &other));
        // A season's too.
        let mut other = after;
        other.seasons[0].primary_images.poster = Some(other.seasons[0].images[1].id.clone());
        assert!(served_differently(&before, &other));
    }

    /// A series and a film held here, a Sonarr that holds the series and a
    /// Radarr that does not hold the film.
    async fn setting_up() -> (AppState, Arc<parking_lot::Mutex<Vec<Seen>>>) {
        let state = crate::api::native::testing::server().await;
        let (at, seen) = stand_in(Arc::new(|request: &Seen| {
            match (request.method.as_str(), request.target.as_str()) {
                ("GET", "/api/v3/series?tvdbId=81189") => (
                    200,
                    r#"[{"id":12,"title":"Breaking Bad","tvdbId":81189}]"#.into(),
                ),
                ("GET", _) => (200, "[]".into()),
                ("POST", "/api/v3/command") => (201, r#"{"id":77}"#.into()),
                _ => (404, String::new()),
            }
        }))
        .await;
        for (key, value) in [
            ("arr.sonarrUrl", at.as_str()),
            ("arr.sonarrKey", "sonarr-key-0001"),
            ("arr.radarrUrl", at.as_str()),
            ("arr.radarrKey", "radarr-key-0001"),
            (SETTING, "true"),
        ] {
            state
                .settings
                .set(Scope::Server, "", key, value, None)
                .await
                .unwrap();
        }

        for (id, kind, title, ids) in [
            (
                SERIES,
                MediaKind::Series,
                "Breaking Bad",
                ExternalIds {
                    tvdb: Some(81189),
                    tmdb: Some(1396),
                    ..Default::default()
                },
            ),
            (
                FILM,
                MediaKind::Movie,
                "The Matrix",
                ExternalIds {
                    tmdb: Some(603),
                    ..Default::default()
                },
            ),
        ] {
            let mut item = MediaItem::empty(kind);
            item.id = id.into();
            item.title = title.into();
            item.slug = crate::domain::make_slug(title, None);
            item.created_at = crate::db::now();
            item.updated_at = crate::db::now();
            item.external_ids = ids;
            crate::db::repo::item::upsert(
                &state.db,
                crate::db::repo::item::ItemWrite {
                    item: &item,
                    replace_children: true,
                },
            )
            .await
            .unwrap();
        }
        (state, seen)
    }

    #[tokio::test]
    async fn the_series_is_found_and_told_and_the_film_said_absent() {
        let (state, seen) = setting_up().await;

        send(
            &state,
            vec![
                (SERIES.to_string(), vec![Reason::Lock, Reason::Sync]),
                (FILM.to_string(), vec![Reason::Edit]),
            ],
        )
        .await;

        let asked: Vec<(String, String)> = seen
            .lock()
            .iter()
            .map(|s| (s.method.clone(), s.target.clone()))
            .collect();
        assert!(asked.contains(&("GET".into(), "/api/v3/series?tvdbId=81189".into())));
        assert!(asked.contains(&("GET".into(), "/api/v3/movie?tmdbId=603".into())));
        let commands: Vec<Seen> = seen
            .lock()
            .iter()
            .filter(|s| s.method == "POST")
            .cloned()
            .collect();
        assert_eq!(
            commands.len(),
            1,
            "no command for a film Radarr does not hold"
        );
        let body: Value = serde_json::from_str(&commands[0].body).unwrap();
        assert_eq!(body["name"], "RefreshSeries");
        assert_eq!(body["seriesIds"], serde_json::json!([12]));
        assert_eq!(commands[0].key.as_deref(), Some("sonarr-key-0001"));

        let pushes = history::latest(&state.db, 10).await.unwrap();
        let sonarr = pushes.iter().find(|p| p.app == App::Sonarr).unwrap();
        assert_eq!(sonarr.outcome, Outcome::Sent);
        assert_eq!(sonarr.arr_id, Some(12));
        assert_eq!(sonarr.detail.as_deref(), Some("command 77"));
        assert_eq!(sonarr.reasons, ["lock", "sync"]);
        let radarr = pushes.iter().find(|p| p.app == App::Radarr).unwrap();
        assert_eq!(radarr.outcome, Outcome::Absent);
        assert_eq!(radarr.client_id, Some(603));

        // Switched off: nobody is asked anything.
        state
            .settings
            .set(Scope::Server, "", SETTING, "false", None)
            .await
            .unwrap();
        let before = seen.lock().len();
        send(&state, vec![(SERIES.to_string(), vec![Reason::Lock])]).await;
        assert_eq!(seen.lock().len(), before);
    }

    const SECOND: &str = "0192aa52-8f62-7cc3-a1f1-6a1b2c3d4e61";
    const THIRD: &str = "0192aa52-8f62-7cc3-a1f1-6a1b2c3d4e62";

    /// A series held here, by the id it is served under.
    async fn series(state: &AppState, id: &str, title: &str, tvdb: i64) {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.id = id.into();
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        item.created_at = crate::db::now();
        item.updated_at = crate::db::now();
        item.external_ids = ExternalIds {
            tvdb: Some(tvdb),
            ..Default::default()
        };
        crate::db::repo::item::upsert(
            &state.db,
            crate::db::repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();
    }

    /// Counts what this module logs as a warning on the thread that set it up,
    /// while it lives: what a test says "once" with.
    struct Warnings {
        count: Arc<std::sync::atomic::AtomicUsize>,
        _kept: tracing::subscriber::DefaultGuard,
    }

    struct CountWarnings(Arc<std::sync::atomic::AtomicUsize>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountWarnings {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let at = event.metadata();
            if *at.level() == tracing::Level::WARN && at.target().ends_with("arr::push") {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    impl Warnings {
        fn counted() -> Self {
            use tracing_subscriber::layer::SubscriberExt as _;

            let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let subscriber = tracing_subscriber::registry().with(CountWarnings(Arc::clone(&count)));
            Self {
                count,
                _kept: tracing::subscriber::set_default(subscriber),
            }
        }

        fn so_far(&self) -> usize {
            self.count.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// The reasons a work waits in the queue for, if it waits there.
    fn waits_for(id: &str) -> Option<Vec<Reason>> {
        QUEUE
            .waiting
            .lock()
            .works
            .get(id)
            .map(|pending| pending.reasons.iter().copied().collect())
    }

    /// What the queue of this process keeps is every test's: each one leaves
    /// its own works out of it.
    fn forget(ids: &[&str]) {
        QUEUE
            .waiting
            .lock()
            .works
            .retain(|id, _| !ids.contains(&id.as_str()));
    }

    #[tokio::test]
    async fn a_sonarr_nobody_answers_is_asked_once_and_the_rest_wait_for_the_next_pass() {
        let (state, _) = setting_up().await;
        // An address nothing listens at: a port that was free a moment ago.
        let closed = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        state
            .settings
            .set(Scope::Server, "", "arr.sonarrUrl", &closed, None)
            .await
            .unwrap();
        series(&state, SECOND, "Better Call Saul", 273_181).await;
        series(&state, THIRD, "Dark", 334_551).await;

        let warnings = Warnings::counted();
        send(
            &state,
            vec![
                (SERIES.to_string(), vec![Reason::Lock]),
                (SECOND.to_string(), vec![Reason::Edit]),
                (THIRD.to_string(), vec![Reason::Lock, Reason::Sync]),
            ],
        )
        .await;

        // The first to find out is written down as failed; the others were
        // never looked for, and are not.
        let pushes = history::latest(&state.db, 10).await.unwrap();
        assert_eq!(pushes.len(), 1, "{pushes:?}");
        assert_eq!(pushes[0].outcome, Outcome::Failed);
        assert_eq!(pushes[0].media_id.as_deref(), Some(SERIES));
        assert!(
            pushes[0]
                .detail
                .as_deref()
                .is_some_and(|why| why.contains("could not connect")),
            "{pushes:?}"
        );
        // They wait for the next pass, for what they were told of.
        assert_eq!(waits_for(SERIES), None, "the failed one is not put back");
        assert_eq!(waits_for(SECOND), Some(vec![Reason::Edit]));
        assert_eq!(waits_for(THIRD), Some(vec![Reason::Lock, Reason::Sync]));
        // One line in the log, not one for each work.
        assert_eq!(warnings.so_far(), 1);

        forget(&[SERIES, SECOND, THIRD]);
    }

    #[tokio::test]
    async fn a_sonarr_that_answers_no_is_asked_for_each_work_and_none_is_put_back() {
        let (state, _) = setting_up().await;
        let (refusing, asked) = stand_in(Arc::new(|_: &Seen| (401, String::new()))).await;
        state
            .settings
            .set(Scope::Server, "", "arr.sonarrUrl", &refusing, None)
            .await
            .unwrap();
        series(&state, SECOND, "Better Call Saul", 273_181).await;

        let warnings = Warnings::counted();
        send(
            &state,
            vec![
                (SERIES.to_string(), vec![Reason::Lock]),
                (SECOND.to_string(), vec![Reason::Edit]),
            ],
        )
        .await;

        // An address that answers refused the key, whichever work asks: each
        // is looked for and written down, and waiting would change nothing.
        assert_eq!(asked.lock().len(), 2);
        let pushes = history::latest(&state.db, 10).await.unwrap();
        assert_eq!(pushes.len(), 2);
        assert!(pushes.iter().all(|push| push.outcome == Outcome::Failed));
        assert_eq!(waits_for(SERIES), None);
        assert_eq!(waits_for(SECOND), None);
        assert_eq!(warnings.so_far(), 2);

        forget(&[SERIES, SECOND]);
    }
}

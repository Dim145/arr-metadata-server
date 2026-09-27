//! What fetches the media in line, and what sweeps the ones nobody points
//! at any more.
//!
//! One worker from the start, woken by whatever puts an address in line and
//! looking on its own every half minute, taking a few at a time. The task
//! "store everything" does the same with a record and a way to stop it; the
//! two share a lock, so a batch is never fetched twice. The sweep runs on
//! its own schedule, daily, and on request.

use std::{
    collections::HashSet,
    sync::{Arc, LazyLock},
    time::Duration,
};

use anyhow::{Context, Result};
use bytes::Bytes;

use crate::{
    db::repo::{
        self,
        asset::{Asset, Kind, Stored, Thumb},
        job,
    },
    error::{AppError, AppResult},
    jobs::cancel,
    state::AppState,
};

use super::file;

/// How many fetches at once: enough to be quick, few enough that a provider
/// sees a reader, not a crawler.
const AT_ONCE: usize = 4;
/// How many are taken from the line at a time.
const BATCH: i64 = 16;
/// How long the worker sleeps between looks, when nothing rang.
const LOOK_EVERY: Duration = Duration::from_secs(30);
/// The tries an address gets before it is given up on.
const MAX_ATTEMPTS: i32 = 5;
/// The sweep's own schedule.
pub const SWEEP_EVERY: Duration = Duration::from_secs(24 * 3600);
/// A file the store lists that no row names is left this long: it may be
/// one just put there whose row is about to be written.
const ORPHAN_GRACE: chrono::Duration = chrono::Duration::hours(1);

/// Held by whatever is fetching: the worker for a batch, the task for its
/// whole run.
static STORING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));
static SWEEPING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

pub fn is_storing() -> bool {
    STORING.try_lock().is_err()
}

pub fn is_sweeping() -> bool {
    SWEEPING.try_lock().is_err()
}

/// The worker: from the start, for as long as the server runs.
pub async fn run(state: AppState) {
    if !state.media.is_on() {
        return;
    }

    loop {
        // Woken, or looking on its own.
        let _ = tokio::time::timeout(LOOK_EVERY, state.media.notify.notified()).await;

        if !state.flag("media.store", true) {
            continue;
        }
        // A task is fetching everything: it will get to these.
        let Ok(_held) = STORING.try_lock() else {
            continue;
        };

        loop {
            match batch(&state).await {
                Ok(0) => break,
                Ok(n) if n < BATCH as usize => break,
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!(
                        error = format_args!("{e:#}"),
                        "the media worker could not read its line"
                    );
                    break;
                }
            }
        }
    }
}

/// Fetch the next few in line. How many were taken.
async fn batch(state: &AppState) -> Result<usize> {
    let due = repo::asset::due(&state.db, BATCH).await?;
    if due.is_empty() {
        return Ok(0);
    }
    let taken = due.len();
    let _ = fetch_all(state, due, None).await;
    Ok(taken)
}

/// Fetch these, a few at a time. How many were stored, and how many failed.
async fn fetch_all(
    state: &AppState,
    assets: Vec<Asset>,
    flag: Option<&cancel::Registered>,
) -> (usize, usize) {
    use futures::StreamExt as _;

    let outcomes: Vec<bool> = futures::stream::iter(assets)
        .map(|asset| async move {
            if flag.is_some_and(|f| f.stopped()) {
                return false;
            }
            fetch_one(state, &asset).await
        })
        .buffer_unordered(AT_ONCE)
        .collect()
        .await;

    // Whatever cards were drawn of these works are drawn again with the
    // copies kept.
    state.caches.searches.invalidate_all();

    let stored = outcomes.iter().filter(|ok| **ok).count();
    (stored, outcomes.len() - stored)
}

/// Fetch one, store it, note it — or note why not. Whether it was stored.
async fn fetch_one(state: &AppState, asset: &Asset) -> bool {
    match store_one(state, asset).await {
        Ok(()) => {
            if let Some(work) = &asset.wanted_by {
                state.caches.items.invalidate(&format!("item:{work}")).await;
            }
            true
        }
        Err(e) => {
            // An address that answers with the wrong thing is given up on at
            // once, and filed past every try, so "try everything again"
            // passes it by: asking again would get the same answer.
            let attempts = match e {
                Trouble::Unfit(_) => repo::asset::UNFIT,
                Trouble::Passing(_) => asset.attempts + 1,
            };
            let next = (attempts < MAX_ATTEMPTS).then(|| {
                let hours = 2i64.pow(u32::try_from(attempts - 1).unwrap_or(0)).min(24);
                crate::db::to_rfc3339(chrono::Utc::now() + chrono::Duration::hours(hours))
            });
            tracing::info!(
                origin = %asset.origin,
                attempts,
                error = %e,
                "a medium could not be fetched"
            );
            let error = e.to_string();
            let error = truncated(&error, 300);
            if let Err(e) =
                repo::asset::mark_failed(&state.db, &asset.id, attempts, error, next.as_deref())
                    .await
            {
                tracing::warn!(error = %e, "could not note the failure");
            }
            false
        }
    }
}

fn truncated(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// Why one was not stored: see [`super::fetch::Failure`].
enum Trouble {
    Unfit(String),
    Passing(anyhow::Error),
}

impl std::fmt::Display for Trouble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unfit(why) => f.write_str(why),
            Self::Passing(e) => write!(f, "{e:#}"),
        }
    }
}

impl From<anyhow::Error> for Trouble {
    fn from(e: anyhow::Error) -> Self {
        Self::Passing(e)
    }
}

async fn store_one(state: &AppState, asset: &Asset) -> Result<(), Trouble> {
    let store = state.media.store().context("no media store")?;
    let fetched = super::fetch::download(state, &asset.origin, asset.kind)
        .await
        .map_err(|e| match e {
            super::fetch::Failure::Unfit(why) => Trouble::Unfit(why),
            super::fetch::Failure::Passing(e) => Trouble::Passing(e),
        })?;
    let stored = keep(state, store, fetched.bytes, &fetched.inspected).await?;
    // Forgotten meanwhile — a reset, a sweep — it is not remembered either:
    // the bytes are the next sweep's to remove.
    if repo::asset::mark_stored(&state.db, &asset.id, &stored.as_row()).await? {
        state.media.remember(
            &asset.origin,
            &stored.key,
            stored.thumb,
            fetched.inspected.content_type,
        );
    }
    Ok(())
}

/// What was kept of some bytes.
pub struct Kept {
    pub key: String,
    pub sha256: String,
    pub content_type: &'static str,
    pub bytes: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub thumb: Thumb,
}

impl Kept {
    pub fn as_row(&self) -> Stored<'_> {
        Stored {
            key: &self.key,
            content_type: self.content_type,
            bytes: self.bytes,
            sha256: &self.sha256,
            width: self.width,
            height: self.height,
            thumb: self.thumb,
        }
    }
}

/// Put bytes in the store under their key, and a thumbnail beside a
/// picture's. Bytes already there — the same picture from two providers —
/// are not put again.
pub async fn keep(
    state: &AppState,
    store: &super::store::Store,
    bytes: Bytes,
    inspected: &file::Inspected,
) -> Result<Kept> {
    let (key, sha256) = file::key_for(&bytes, inspected.ext);
    let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);

    let already = repo::asset::thumb_of_key(&state.db, &key).await?;
    let thumb = match already {
        Some(thumb) if store.exists(&key).await? => thumb,
        _ => {
            store
                .put(&key, bytes.clone(), inspected.content_type)
                .await?;
            match inspected.kind {
                Kind::Image => {
                    // A thumbnail that cannot be made is no thumbnail: the
                    // picture is kept and served whole.
                    let made = tokio::task::spawn_blocking(move || file::thumbnail(&bytes))
                        .await
                        .context("the thumbnail task was cancelled")?;
                    match made {
                        Some((small, kind)) => {
                            let (thumb_key, content_type) = file::thumb_key(&key, kind)
                                .zip(kind.content_type())
                                .expect("a thumbnail made has a kind");
                            store
                                .put(&thumb_key, Bytes::from(small), content_type)
                                .await?;
                            kind
                        }
                        None => Thumb::None,
                    }
                }
                Kind::Audio => Thumb::None,
            }
        }
    };

    Ok(Kept {
        key,
        sha256,
        content_type: inspected.content_type,
        bytes: size,
        width: inspected.width.and_then(|w| i32::try_from(w).ok()),
        height: inspected.height.and_then(|h| i32::try_from(h).ok()),
        thumb,
    })
}

fn busy(what: &str) -> AppError {
    AppError::Conflict(format!("{what} already; wait for it to end, or stop it"))
}

/// Everything not kept yet, fetched now, as `by` asked: every address the
/// catalogue holds is put in line, then the line is worked through, the run
/// saying how far it has got. It can be stopped between two batches.
pub async fn store_all(state: &AppState, by: &str) -> AppResult<Option<String>> {
    if !state.media.is_on() {
        return Err(AppError::Disabled {
            code: "media_off",
            message: "no media store is configured".into(),
        });
    }
    let held = STORING
        .clone()
        .try_lock_owned()
        .map_err(|_| busy("media are being fetched"))?;
    let record = job::start_by(&state.db, job::kinds::MEDIA_STORE, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let flag = cancel::register(&record);
        let outcome = everything(&state, &record, &flag).await;
        close(&state, &record, outcome).await;
    });

    Ok(Some(id))
}

/// How a pass ended.
enum Pass {
    Done(String),
    Stopped(String),
}

async fn everything(state: &AppState, record: &str, flag: &cancel::Registered) -> Result<Pass> {
    let people = state.flag("media.people", true);
    let audio = state.flag("media.audio", true);

    // Everything the catalogue points at that no row knows, in line — and
    // everything given up on, asked again.
    let unknown = repo::asset::unknown_origins(&state.db, people, audio).await?;
    let wanted: Vec<repo::asset::Wanted<'_>> = unknown
        .iter()
        // Not what would fail at once: an SVG logo, an address of this
        // server's own.
        .filter(|(origin, _)| super::fetch::fetchable(origin))
        .map(|(origin, kind)| repo::asset::Wanted {
            origin,
            kind: *kind,
            wanted_by: None,
        })
        .collect();
    for slice in wanted.chunks(500) {
        repo::asset::enqueue(&state.db, slice).await?;
    }
    repo::asset::retry_troubled(&state.db).await?;

    let total = repo::asset::pending_count(&state.db).await?;
    let (mut stored, mut failed) = (0usize, 0usize);

    loop {
        if flag.stopped() {
            return Ok(Pass::Stopped(format!(
                "stopped: {stored} stored, {failed} failed, of {total}"
            )));
        }
        let due = repo::asset::due(&state.db, BATCH).await?;
        if due.is_empty() {
            break;
        }
        let (ok, ko) = fetch_all(state, due, Some(flag)).await;
        stored += ok;
        failed += ko;

        if let Err(e) = job::progress(
            &state.db,
            record,
            &format!(
                "{} of {total}: {stored} stored, {failed} failed",
                stored + failed
            ),
        )
        .await
        {
            tracing::debug!(error = %e, "could not say how far the fetch got");
        }
    }

    Ok(Pass::Done(format!(
        "{stored} stored, {failed} failed, of {total}"
    )))
}

async fn close(state: &AppState, record: &str, outcome: Result<Pass>) {
    let closed = match outcome {
        Ok(Pass::Done(summary)) => job::finish(&state.db, record, Some(&summary), None).await,
        Ok(Pass::Stopped(summary)) => job::stop(&state.db, record, &summary).await,
        Err(e) => {
            tracing::warn!(error = format_args!("{e:#}"), "a media run failed");
            job::finish(&state.db, record, None, Some(&format!("{e:#}"))).await
        }
    };
    if let Err(e) = closed {
        tracing::warn!(error = %e, "could not close the media run");
    }
}

/// The sweep, on request.
pub async fn sweep_now(state: &AppState, by: &str) -> AppResult<Option<String>> {
    if !state.media.is_on() {
        return Err(AppError::Disabled {
            code: "media_off",
            message: "no media store is configured".into(),
        });
    }
    let held = SWEEPING
        .clone()
        .try_lock_owned()
        .map_err(|_| busy("the media are being swept"))?;
    let record = job::start_by(&state.db, job::kinds::MEDIA_SWEEP, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let outcome = sweep(&state).await.map(Pass::Done);
        close(&state, &record, outcome).await;
    });

    Ok(Some(id))
}

/// The sweep, on its schedule: a day after the last, and an hour after the
/// start, so a server that runs a day at a time still sweeps.
pub async fn run_sweeps(state: AppState) {
    if !state.media.is_on() {
        return;
    }
    tokio::time::sleep(Duration::from_secs(3600)).await;
    loop {
        if let Ok(held) = SWEEPING.clone().try_lock_owned() {
            let record = job::start_by(&state.db, job::kinds::MEDIA_SWEEP, None, None)
                .await
                .inspect_err(|e| tracing::warn!(error = %e, "could not open the sweep's run"))
                .ok();
            let outcome = sweep(&state).await.map(Pass::Done);
            match record {
                Some(record) => close(&state, &record, outcome).await,
                None => {
                    if let Err(e) = outcome {
                        tracing::warn!(error = format_args!("{e:#}"), "the media sweep failed");
                    }
                }
            }
            drop(held);
        }
        tokio::time::sleep(SWEEP_EVERY).await;
    }
}

/// Forget what nobody points at any more, and delete what nobody names.
///
/// A row goes when no work's rows hold its address and no locked value does;
/// its file goes with it unless another row holds the same bytes. Then the
/// store is listed, and a file no row names — one whose row was lost
/// between the put and the write, say — goes too, once it has had its
/// hour.
pub async fn sweep(state: &AppState) -> Result<String> {
    let store = state.media.store().context("no media store")?;

    let locked = locked_addresses(state).await?;
    let lost = repo::asset::unreferenced(&state.db, &locked).await?;
    let (mut rows, mut files) = (0usize, 0usize);
    for asset in &lost {
        if let Some(key) = &asset.key
            && !repo::asset::key_shared(&state.db, key, &asset.id).await?
        {
            store.delete(key).await?;
            files += 1;
            if let Some(thumb) = file::thumb_key(key, asset.thumb) {
                store.delete(&thumb).await?;
                files += 1;
            }
        }
        repo::asset::delete(&state.db, &asset.id).await?;
        state.media.forget(&asset.origin);
        rows += 1;
    }

    let named = repo::asset::all_keys(&state.db).await?;
    let cutoff = chrono::Utc::now() - ORPHAN_GRACE;
    for (key, modified) in store.list().await? {
        if !named.contains(&key) && modified < cutoff {
            store.delete(&key).await?;
            files += 1;
        }
    }

    Ok(format!("{rows} forgotten, {files} files removed"))
}

/// The addresses locked values hold: a still put on an episode by hand,
/// a theme put on a work. Not what the sweep looks through in SQL, since a
/// locked value is JSON.
async fn locked_addresses(state: &AppState) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    for (_, locked) in repo::override_field::all(&state.db).await? {
        if matches!(locked.field.as_str(), "image" | "themeMusic")
            && let Some(serde_json::Value::String(url)) = locked.value
        {
            out.insert(state.media.unlocalize(&url));
        }
    }
    Ok(out)
}

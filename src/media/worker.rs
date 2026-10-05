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
        asset::{Asset, Claim, Kind, Stored, Thumb},
        job,
    },
    error::{AppError, AppResult},
    jobs::cancel,
    state::AppState,
};

use super::{file, store::Store};

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
/// Thumbnails are made one at a time, whoever asks — the worker's fetches,
/// an upload, a run of "store everything": decoding a large picture takes a
/// few hundred megabytes, and four at once took more than a small server
/// has.
static THUMBNAILING: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));

pub fn is_storing() -> bool {
    STORING.try_lock().is_err()
}

pub fn is_sweeping() -> bool {
    SWEEPING.try_lock().is_err()
}

/// Fetching held off for as long as what this returns is kept: no batch
/// starts on this instance, and no other instance runs "store everything".
/// Refused while either is under way — what a reset needs, so that nothing
/// it forgets is stored again behind it.
pub async fn hold_off_fetching(
    state: &AppState,
) -> AppResult<(tokio::sync::OwnedMutexGuard<()>, crate::coord::Held)> {
    let held = STORING
        .clone()
        .try_lock_owned()
        .map_err(|_| busy("media are being fetched"))?;
    let shared = state
        .coord
        .hold_for(job::kinds::MEDIA_STORE, "media are being fetched")
        .await?;
    Ok((held, shared))
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
    let (stored, failed) = fetch_all(state, due, None).await;
    // None of them taken — another instance's, or a claim that could not be
    // made: the line is looked at again on the next ring, not at once.
    Ok(if stored + failed == 0 { 0 } else { taken })
}

/// Fetch these, a few at a time — each claimed first, so that among
/// several instances no two fetch the same. How many were stored, and how
/// many failed.
async fn fetch_all(
    state: &AppState,
    assets: Vec<Asset>,
    flag: Option<&cancel::Registered>,
) -> (usize, usize) {
    use futures::StreamExt as _;

    let outcomes: Vec<Option<bool>> = futures::stream::iter(assets)
        .map(|asset| async move {
            if flag.is_some_and(|f| f.stopped()) {
                return Some(false);
            }
            // This try, counted as it is taken, and the next put off as a
            // failure would put it off: one that never comes back is not
            // taken again at once.
            let tries = asset.attempts + 1;
            match repo::asset::claim(
                &state.db,
                &asset.id,
                &state.coord.instance.name,
                MAX_ATTEMPTS,
                &retry_after(tries),
            )
            .await
            {
                Ok(Claim::Taken) => Some(fetch_one(state, &asset, tries).await),
                Ok(Claim::GivenUp) => {
                    tracing::warn!(
                        origin = %asset.origin,
                        "a medium was given up on: every fetch of it was cut short"
                    );
                    Some(false)
                }
                // Another instance got there first, or it was forgotten.
                Ok(Claim::Lost) => None,
                Err(e) => {
                    tracing::warn!(error = %e, "could not claim a medium in line");
                    None
                }
            }
        })
        .buffer_unordered(AT_ONCE)
        .collect()
        .await;
    let outcomes: Vec<bool> = outcomes.into_iter().flatten().collect();

    // Whatever cards were drawn of these works are drawn again with the
    // copies kept.
    state.caches.searches.invalidate_all().await;

    let stored = outcomes.iter().filter(|ok| **ok).count();
    (stored, outcomes.len() - stored)
}

/// When a try that failed — or never came back — is made again: an hour
/// after the first, then two, four, eight; a day at most.
fn retry_after(tries: i32) -> String {
    let hours = 2i64.pow(u32::try_from(tries - 1).unwrap_or(0)).min(24);
    crate::db::to_rfc3339(chrono::Utc::now() + chrono::Duration::hours(hours))
}

/// Fetch one, store it, note it — or note why not. Whether it was stored.
/// `tries` counts this one.
async fn fetch_one(state: &AppState, asset: &Asset, tries: i32) -> bool {
    match store_one(state, asset).await {
        Ok(()) => {
            if let Some(work) = &asset.wanted_by {
                state.caches.touched(work).await;
            }
            true
        }
        Err(e) => {
            // An address that answers with the wrong thing is given up on at
            // once, and filed past every try, so "try everything again"
            // passes it by: asking again would get the same answer.
            let unfit = matches!(e, Trouble::Unfit(_));
            let next = (!unfit && tries < MAX_ATTEMPTS).then(|| retry_after(tries));
            tracing::info!(
                origin = %asset.origin,
                attempts = if unfit { repo::asset::UNFIT } else { tries },
                error = %e,
                "a medium could not be fetched"
            );
            let error = e.to_string();
            let error = truncated(&error, 300);
            if let Err(e) =
                repo::asset::mark_failed(&state.db, &asset.id, error, next.as_deref(), unfit).await
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
    let (key, sha256) = file::key_for(&fetched.bytes, fetched.inspected.ext);
    // The row names the bytes before they are put: nothing that deletes a
    // file nobody names takes them meanwhile. Forgotten already — a reset,
    // a sweep — there is nothing to keep them for.
    if !repo::asset::reserve_key(&state.db, &asset.id, &key).await? {
        return Ok(());
    }
    let kept = keep(store, key, sha256, fetched.bytes, &fetched.inspected).await?;
    // Forgotten meanwhile, it is not remembered either: the bytes are the
    // next sweep's to remove.
    if repo::asset::mark_stored(&state.db, &asset.id, &kept.as_row()).await? {
        settle(store, &kept).await;
        state
            .media
            .remember(&asset.origin, &kept.key, kept.thumb, kept.content_type);
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
    /// The bytes put, and their thumbnail's, held until the row naming
    /// them is written: see [`settle`].
    data: Bytes,
    small: Option<Bytes>,
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

/// Put bytes in the store under their key — `file::key_for`'s — and a
/// thumbnail beside a picture's. Put every time, though the same picture
/// from two providers may be there already: the same key is the same bytes,
/// and a put is what tells a sweep that looked a moment ago that they are
/// wanted.
pub async fn keep(
    store: &Store,
    key: String,
    sha256: String,
    bytes: Bytes,
    inspected: &file::Inspected,
) -> Result<Kept> {
    let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    store
        .put(&key, bytes.clone(), inspected.content_type)
        .await?;

    let made = match inspected.kind {
        Kind::Image => thumbnail(bytes.clone()).await,
        Kind::Audio => None,
    };
    let (thumb, small) = match made {
        Some((small, kind)) => {
            let (thumb_key, content_type) = file::thumb_key(&key, kind)
                .zip(kind.content_type())
                .expect("a thumbnail made has a kind");
            store.put(&thumb_key, small.clone(), content_type).await?;
            (kind, Some(small))
        }
        None => (Thumb::None, None),
    };

    Ok(Kept {
        key,
        sha256,
        content_type: inspected.content_type,
        bytes: size,
        width: inspected.width.and_then(|w| i32::try_from(w).ok()),
        height: inspected.height.and_then(|h| i32::try_from(h).ok()),
        thumb,
        data: bytes,
        small,
    })
}

/// A picture's thumbnail: made one at a time, off the server's own threads.
/// None for one too large to be worth decoding, one the decoders here
/// cannot read — or one they panic on, which is caught there: the picture
/// is kept and served whole.
async fn thumbnail(bytes: Bytes) -> Option<(Bytes, Thumb)> {
    // Held by the decoding itself, so a request given up on meanwhile does
    // not let another start beside it.
    let permit = THUMBNAILING.clone().acquire_owned().await.ok()?;
    let made = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        file::thumbnail(&bytes)
    })
    .await;
    match made {
        Ok(made) => made.map(|(small, kind)| (Bytes::from(small), kind)),
        Err(e) => {
            tracing::warn!(error = %e, "no thumbnail: the decoder gave up on the picture");
            None
        }
    }
}

/// Once the row naming them is written: the files are in the store, or are
/// put back. A removal or a sweep that looked before the row was written
/// may have taken them in between; after it, none will. Quiet — the files
/// were put a moment ago, and a store that cannot be asked now is said so
/// in the log.
pub async fn settle(store: &Store, kept: &Kept) {
    if let Err(e) = settle_files(store, kept).await {
        tracing::warn!(
            key = %kept.key,
            error = format_args!("{e:#}"),
            "could not make sure a medium kept is in the store"
        );
    }
}

async fn settle_files(store: &Store, kept: &Kept) -> Result<()> {
    if !store.exists(&kept.key).await? {
        tracing::info!(key = %kept.key, "a medium was taken from under its row; put back");
        store
            .put(&kept.key, kept.data.clone(), kept.content_type)
            .await?;
    }
    if let (Some(small), Some((thumb_key, content_type))) = (
        &kept.small,
        file::thumb_key(&kept.key, kept.thumb).zip(kept.thumb.content_type()),
    ) && !store.exists(&thumb_key).await?
    {
        store.put(&thumb_key, small.clone(), content_type).await?;
    }
    Ok(())
}

/// Delete a key's files — the bytes and their thumbnail — unless a row
/// still names the key: another holding the same bytes, or one in line for
/// them whose fetch is under way. Called once the row they were kept for is
/// gone. Best effort: what could not be deleted is said in the log and left
/// to the sweep's pass over the store. How many went.
pub async fn delete_files(state: &AppState, store: &Store, key: &str, thumb: Thumb) -> usize {
    match repo::asset::key_named(&state.db, key).await {
        Ok(false) => {}
        Ok(true) => return 0,
        Err(e) => {
            tracing::warn!(key, error = %e, "could not tell whether a medium's file is still named");
            return 0;
        }
    }
    let mut gone = 0;
    for key in file::thumb_key(key, thumb)
        .into_iter()
        .chain(std::iter::once(key.to_string()))
    {
        match store.delete(&key).await {
            Ok(()) => gone += 1,
            Err(e) => tracing::warn!(
                key,
                error = format_args!("{e:#}"),
                "could not delete a medium's file; the sweep will"
            ),
        }
    }
    gone
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
    let shared = state
        .coord
        .hold_for(job::kinds::MEDIA_STORE, "media are being fetched")
        .await?;
    let record = job::start_by(&state.db, job::kinds::MEDIA_STORE, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
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
        .filter(|(origin, _)| super::fetch::fetchable(origin, state.media.guard))
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
        // Every one of them taken by another instance, or not to be taken:
        // asking again at once would be asked the same.
        if ok + ko == 0 {
            break;
        }

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
    let shared = state
        .coord
        .hold_for(job::kinds::MEDIA_SWEEP, "the media are being swept")
        .await?;
    let record = job::start_by(&state.db, job::kinds::MEDIA_SWEEP, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let outcome = sweep(&state).await.map(Pass::Done);
        close(&state, &record, outcome).await;
    });

    Ok(Some(id))
}

/// The sweep, on its schedule: a day after the last, and an hour after the
/// start, so a server that runs a day at a time still sweeps. The leader's
/// to run; the others look again in an hour.
pub async fn run_sweeps(state: AppState) {
    if !state.media.is_on() {
        return;
    }
    tokio::time::sleep(Duration::from_secs(3600)).await;
    loop {
        if !state.coord.leads() {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            continue;
        }
        if let Ok(held) = SWEEPING.clone().try_lock_owned()
            && let Some(shared) = state.coord.hold(job::kinds::MEDIA_SWEEP).await
        {
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
            drop(shared);
            drop(held);
        }
        tokio::time::sleep(SWEEP_EVERY).await;
    }
}

/// Forget what nobody points at any more, and delete what nobody names.
///
/// A row goes when no work's rows hold its address and no locked value does
/// — asked again as it is deleted, since something may have come to name it
/// since the list was read — once it has had its hour: an upload's row is
/// written a moment before what points at it. Its file goes with it unless
/// another row holds the same bytes. Then the store is listed, and a file
/// no row names — one whose row was lost between the put and the write,
/// say — goes too, once it has had its hour: asked again just before it
/// goes, of the rows and of the store, as a row may have been written for
/// it, or the file put again, since the list was read. One that cannot be
/// deleted is counted and passed by; the rest are still swept.
pub async fn sweep(state: &AppState) -> Result<String> {
    let store = state.media.store().context("no media store")?;
    let cutoff = chrono::Utc::now() - ORPHAN_GRACE;

    let locked = locked_addresses(state).await?;
    let lost =
        repo::asset::unreferenced(&state.db, &locked, &crate::db::to_rfc3339(cutoff)).await?;
    let (mut rows, mut files, mut troubles) = (0usize, 0usize, 0usize);
    for asset in &lost {
        match repo::asset::delete_unless_referenced(&state.db, asset).await {
            Ok(true) => {
                state.media.forget(&asset.origin);
                rows += 1;
                if let Some(key) = &asset.key {
                    files += delete_files(state, store, key, asset.thumb).await;
                }
            }
            // Named again since the list was read.
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(origin = %asset.origin, error = %e, "the sweep could not forget a medium");
                troubles += 1;
            }
        }
    }

    let held = repo::asset::held_stems(&state.db).await?;
    let listed = match store.list().await {
        Ok(listed) => listed,
        Err(e) => {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "the sweep could not list the store"
            );
            troubles += 1;
            Vec::new()
        }
    };
    for (key, modified) in listed {
        if modified >= cutoff || held.contains(file::stem_of(&key)) {
            continue;
        }
        let outcome = match orphaned(state, store, &key, cutoff).await {
            Ok(true) => store.delete(&key).await.map(|()| true),
            other => other,
        };
        match outcome {
            Ok(true) => files += 1,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(
                    key,
                    error = format_args!("{e:#}"),
                    "the sweep could not remove a file"
                );
                troubles += 1;
            }
        }
    }

    let mut summary = format!("{rows} forgotten, {files} files removed");
    if troubles > 0 {
        summary.push_str(&format!(", {troubles} could not be (see the log)"));
    }
    Ok(summary)
}

/// Whether a file the store listed is still nobody's, asked just before it
/// goes: no row names its bytes, and nothing has put it there again since
/// `cutoff`.
async fn orphaned(
    state: &AppState,
    store: &Store,
    key: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Result<bool> {
    if repo::asset::stem_named(&state.db, file::stem_of(key)).await? {
        return Ok(false);
    }
    Ok(store.modified(key).await?.is_some_and(|at| at < cutoff))
}

/// The addresses locked values hold: a still put on an episode by hand,
/// a theme put on a work. Not what the sweep looks through in SQL, since a
/// locked value is JSON.
async fn locked_addresses(state: &AppState) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    for (_, locked) in repo::override_field::all(&state.db).await? {
        if super::ADDRESS_FIELDS.contains(&locked.field.as_str())
            && let Some(serde_json::Value::String(url)) = locked.value
        {
            out.insert(state.media.unlocalize(&url));
        }
    }
    Ok(out)
}

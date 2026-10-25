//! The two tasks that make copies of the pictures kept.
//!
//! - `media.variants`, by hand only: every picture stored before this
//!   version — or before there were copies at all — prepared as one stored
//!   now is: its placeholder worked out, and the copy its kind is first
//!   asked for made, a poster's 480 and a background's 1280. A few at a
//!   time, each picture noted on its row once done, so that a run stopped
//!   — by hand, or by the server going down — is taken up by the next where
//!   it stopped. Never at start: a catalogue's worth of pictures is hours
//!   of decoding on a small machine, and when to spend them is the
//!   administrator's choice. Until then a page is given what it asks for,
//!   made as it asks.
//! - `media.avif`, at night once `media.avif` is on, and by hand: an AVIF
//!   copy of every poster at 480 and 960, and of every background at 1280
//!   — a third lighter than the WebP, and seconds to make, one after
//!   another. A page offers one only where it exists.

use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};

use anyhow::{Context as _, Result};
use bytes::Bytes;
use chrono::{DateTime, Timelike as _, Utc};

use crate::{
    db::repo::{
        self,
        asset::{Asset, Variant},
        job::{self, Outcome, Touched},
    },
    error::{AppError, AppResult},
    jobs::cancel,
    media::{
        Look,
        file::Made,
        variants::{self, Decoded, Format},
    },
    state::AppState,
};

/// The tasks' names, as the history files their runs, and what each is
/// held under among several instances.
pub const VARIANTS: &str = "media.variants";
pub const AVIF: &str = "media.avif";

/// The hour, UTC, the night's AVIF copies are made at.
pub const NIGHT_HOUR: u32 = 3;
/// How often the night comes round.
pub const NIGHT_EVERY: Duration = Duration::from_secs(24 * 3600);

/// How many pictures are read from the line at once.
const BATCH: i64 = 16;

static PREPARING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));
static MAKING_AVIF: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Whether the pictures are being prepared on this instance.
pub fn is_preparing() -> bool {
    PREPARING.try_lock().is_err()
}

/// Whether the night's AVIF copies are being made on this instance.
pub fn is_making_avif() -> bool {
    MAKING_AVIF.try_lock().is_err()
}

/// Whether either task runs here.
pub fn is_busy() -> bool {
    is_preparing() || is_making_avif()
}

fn media_off() -> AppError {
    AppError::Disabled {
        code: "media_off",
        message: "no media store is configured".into(),
    }
}

/// Prepare every picture kept that this version has not, in the background,
/// as `by` asked.
pub async fn prepare_now(state: &AppState, by: &str) -> AppResult<Option<String>> {
    if !state.media.is_on() {
        return Err(media_off());
    }
    if state.maintenance.is_on() {
        return Err(AppError::Conflict("a backup is being restored".into()));
    }
    let held = PREPARING
        .clone()
        .try_lock_owned()
        .map_err(|_| AppError::Conflict("the pictures are being prepared already".into()))?;
    let shared = state
        .coord
        .hold_for(VARIANTS, "the pictures are being prepared")
        .await?;
    let record = job::start_by(&state.db, VARIANTS, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let flag = cancel::register(&record);
        let outcome = prepare_all(&state, &record, &flag).await;
        drawn_again(&state).await;
        close(&state, &record, outcome).await;
    });
    Ok(Some(id))
}

/// How a run ended.
enum Pass {
    Done(String),
    Stopped(String),
}

/// What became of one picture: `written` for one prepared or copied, as a
/// run's entries say of a file made.
struct Done {
    outcome: Outcome,
    note: Option<String>,
}

async fn prepare_all(state: &AppState, record: &str, flag: &cancel::Registered) -> Result<Pass> {
    let store = state.media.store().context("no media store")?;
    let total = repo::asset::unprepared_count(&state.db, variants::PREPARED).await?;
    let (mut prepared, mut failed, mut position) = (0i64, 0i64, 0i64);
    let mut after = String::new();
    loop {
        if flag.stopped() {
            return Ok(Pass::Stopped(format!(
                "stopped: {prepared} prepared, {failed} failed, of {total}"
            )));
        }
        let line = repo::asset::unprepared(&state.db, variants::PREPARED, &after, BATCH).await?;
        let Some(last) = line.last() else {
            break;
        };
        after.clone_from(&last.id);
        for asset in &line {
            if flag.stopped() {
                break;
            }
            let done = prepare_one(state, store, asset).await;
            match done.outcome {
                Outcome::Written => prepared += 1,
                Outcome::Failed => failed += 1,
                _ => {}
            }
            position += 1;
            entry(state, record, position, asset, &done).await;
        }
        if let Err(e) = job::progress(
            &state.db,
            record,
            &format!("{position} of {total}: {prepared} prepared, {failed} failed"),
        )
        .await
        {
            tracing::debug!(error = %e, "could not say how far the preparing got");
        }
    }
    Ok(Pass::Done(format!(
        "{prepared} prepared, {failed} failed, of {total}"
    )))
}

/// Prepare one picture: its placeholder and its first copy, noted on its
/// row. A picture the store does not have, or that cannot be read, is noted
/// all the same — another run would find it no better — and one the store
/// could not be asked for is left for the next run.
async fn prepare_one(state: &AppState, store: &crate::media::store::Store, asset: &Asset) -> Done {
    let failed = |note: String| Done {
        outcome: Outcome::Failed,
        note: Some(note),
    };
    let Some(key) = asset.key.clone() else {
        return Done {
            outcome: Outcome::Skipped,
            note: None,
        };
    };
    let bytes = match store.read(&key).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            mark(state, asset, None, None).await;
            return Done {
                outcome: Outcome::Skipped,
                note: Some("not in the store".into()),
            };
        }
        Err(e) => return failed(format!("{e:#}")),
    };
    let cover = repo::asset::cover_of(&state.db, &asset.origin)
        .await
        .unwrap_or(None);
    let prepared = variants::off_thread(move || variants::prepare(&bytes, cover)).await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(e) => {
            mark(state, asset, None, None).await;
            return failed(format!("{e:#}"));
        }
    };

    let stem = crate::media::file::stem_of(&key).to_string();
    for copy in prepared.copies {
        let copy_key = copy.key(&stem);
        let content_type = copy.content_type();
        let data = Bytes::from(copy.bytes);
        if let Err(e) = store.put(&copy_key, data.clone(), content_type).await {
            return failed(format!("{e:#}"));
        }
        note(
            state,
            &copy_key,
            &stem,
            copy.width,
            copy.height,
            copy.format,
            data.len(),
        )
        .await;
    }
    let (color, thumbhash) = match &prepared.placeholder {
        Some(placeholder) => (
            Some(placeholder.color_hex()),
            Some(placeholder.thumbhash_base64()),
        ),
        None => (None, None),
    };
    mark(state, asset, color.as_deref(), thumbhash.as_deref()).await;
    state.media.describe(
        &key,
        Look::of_row(&repo::asset::Indexed {
            origin: asset.origin.clone(),
            key: key.clone(),
            thumb: asset.thumb,
            width: asset.width,
            height: asset.height,
            color,
            thumbhash,
        }),
    );
    Done {
        outcome: Outcome::Written,
        note: None,
    }
}

/// Note a picture prepared by this version: quiet, as the next run would
/// only take it again.
async fn mark(state: &AppState, asset: &Asset, color: Option<&str>, thumbhash: Option<&str>) {
    if let Err(e) =
        repo::asset::mark_prepared(&state.db, &asset.id, color, thumbhash, variants::PREPARED).await
    {
        tracing::warn!(origin = %asset.origin, error = %e, "could not note a picture prepared");
    }
}

/// Note a copy put in the store.
async fn note(
    state: &AppState,
    key: &str,
    stem: &str,
    width: u32,
    height: u32,
    format: Format,
    bytes: usize,
) {
    let variant = Variant {
        key: key.to_string(),
        stem: stem.to_string(),
        width: i32::try_from(width).unwrap_or(i32::MAX),
        height: i32::try_from(height).unwrap_or(i32::MAX),
        format: format.as_str().to_string(),
        bytes: i64::try_from(bytes).unwrap_or(i64::MAX),
    };
    if let Err(e) = repo::asset::add_variant(&state.db, &variant).await {
        tracing::warn!(key, error = %e, "could not note a copy made");
    }
}

/// Write down what a run did to one picture: under the work that wanted it
/// first, named by its address.
async fn entry(state: &AppState, record: &str, position: i64, asset: &Asset, done: &Done) {
    let named = short(&asset.origin);
    if let Err(e) = job::add_entry(
        &state.db,
        record,
        position,
        Touched {
            id: asset.wanted_by.as_deref().unwrap_or(""),
            title: Some(&named),
            kind: None,
        },
        done.outcome,
        done.note.as_deref(),
    )
    .await
    {
        tracing::debug!(error = %e, "could not write down what became of a picture");
    }
}

/// An address as a run's entry names it: its host and the end of its path,
/// cut at a slash when it is long.
fn short(origin: &str) -> String {
    const MOST: usize = 60;
    let Ok(url) = url::Url::parse(origin) else {
        return origin.to_string();
    };
    let Some(host) = url.host_str() else {
        return origin.to_string();
    };
    let path = url.path();
    if path.len() <= MOST {
        return format!("{host}{path}");
    }
    let mut from = path.len() - MOST;
    while !path.is_char_boundary(from) {
        from += 1;
    }
    let tail = &path[from..];
    let tail = tail.find('/').map_or(tail, |slash| &tail[slash..]);
    format!("{host}…{tail}")
}

/// Once a run is over, done or stopped: the works and the pages of cards
/// kept as they were read are read again, so that the placeholders and the
/// AVIF copies made are told to the pages — here and on every instance.
async fn drawn_again(state: &AppState) {
    for space in ["items", "searches"] {
        state.caches.flush(space).await;
    }
}

async fn close(state: &AppState, record: &str, outcome: Result<Pass>) {
    let closed = match outcome {
        Ok(Pass::Done(summary)) => job::finish(&state.db, record, Some(&summary), None).await,
        Ok(Pass::Stopped(summary)) => job::stop(&state.db, record, &summary).await,
        Err(e) => {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "a run over the pictures failed"
            );
            job::finish(&state.db, record, None, Some(&format!("{e:#}"))).await
        }
    };
    if let Err(e) = closed {
        tracing::warn!(error = %e, "could not close the run over the pictures");
    }
}

// ─── the night's AVIF copies ─────────────────────────────────────────────────

/// Make the AVIF copies now, in the background, as `by` asked — or, with
/// `by` none, as the night's schedule does.
pub async fn avif_now(state: &AppState, by: Option<&str>) -> AppResult<Option<String>> {
    if !state.media.is_on() {
        return Err(media_off());
    }
    if !state.flag("media.avif", false) {
        return Err(AppError::Disabled {
            code: "avif_off",
            message: "AVIF copies are off: set media.avif".into(),
        });
    }
    if state.maintenance.is_on() {
        return Err(AppError::Conflict("a backup is being restored".into()));
    }
    let held = MAKING_AVIF
        .clone()
        .try_lock_owned()
        .map_err(|_| AppError::Conflict("the AVIF copies are being made already".into()))?;
    let shared = state
        .coord
        .hold_for(AVIF, "the AVIF copies are being made")
        .await?;
    let record = job::start_by(&state.db, AVIF, None, by).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let flag = cancel::register(&record);
        let outcome = avif_all(&state, &record, &flag).await;
        drawn_again(&state).await;
        close(&state, &record, outcome).await;
    });
    Ok(Some(id))
}

async fn avif_all(state: &AppState, record: &str, flag: &cancel::Registered) -> Result<Pass> {
    let store = state.media.store().context("no media store")?;
    let (mut seen, mut made, mut failed, mut position) = (0i64, 0i64, 0i64, 0i64);
    let mut after = String::new();
    loop {
        if flag.stopped() {
            return Ok(Pass::Stopped(format!(
                "stopped: {made} copies made, {failed} failed, of {seen} pictures"
            )));
        }
        let line = repo::asset::avif_candidates(&state.db, &after, BATCH).await?;
        let Some((last, _)) = line.last() else {
            break;
        };
        after.clone_from(&last.id);
        for (asset, cover) in &line {
            if flag.stopped() {
                break;
            }
            seen += 1;
            let Some((done, copies)) = avif_one(state, store, asset, *cover).await else {
                continue;
            };
            made += copies;
            if done.outcome == Outcome::Failed {
                failed += 1;
            }
            position += 1;
            entry(state, record, position, asset, &done).await;
        }
        if let Err(e) = job::progress(
            &state.db,
            record,
            &format!("{seen} looked at: {made} copies made, {failed} failed"),
        )
        .await
        {
            tracing::debug!(error = %e, "could not say how far the AVIF copies got");
        }
    }
    Ok(Pass::Done(format!(
        "{made} copies made, {failed} failed, of {seen} pictures"
    )))
}

/// The AVIF copies of one picture that are not made yet, made: `None` when
/// none was wanted, else what became of it — the widths made as its note —
/// and how many were.
async fn avif_one(
    state: &AppState,
    store: &crate::media::store::Store,
    asset: &Asset,
    cover: crate::domain::CoverType,
) -> Option<(Done, i64)> {
    let key = asset.key.clone()?;
    let stem = crate::media::file::stem_of(&key).to_string();
    let own = asset.width.and_then(|w| u32::try_from(w).ok());
    let mut wanted: Vec<u32> = variants::avif_widths(Some(cover))
        .iter()
        .filter_map(|width| match own {
            Some(own) => variants::width_made(*width, own),
            None => Some(*width),
        })
        .collect();
    wanted.dedup();
    let failed = |note: String, made: i64| {
        Some((
            Done {
                outcome: Outcome::Failed,
                note: Some(note),
            },
            made,
        ))
    };
    let made: Vec<u32> = match repo::asset::variants_of(&state.db, &stem).await {
        Ok(variants) => variants
            .iter()
            .filter(|v| v.format == Format::Avif.as_str())
            .filter_map(|v| u32::try_from(v.width).ok())
            .collect(),
        Err(e) => return failed(format!("{e:#}"), 0),
    };
    wanted.retain(|width| !made.contains(width));
    if wanted.is_empty() {
        return None;
    }

    let bytes = match store.read(&key).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return Some((
                Done {
                    outcome: Outcome::Skipped,
                    note: Some("not in the store".into()),
                },
                0,
            ));
        }
        Err(e) => return failed(format!("{e:#}"), 0),
    };
    let copies = variants::off_thread_avif(move || {
        let decoded = Decoded::read(&bytes)?;
        wanted
            .into_iter()
            .map(|width| variants::make(&decoded, Made::Width(width), Format::Avif, Some(cover)))
            .collect::<Result<Vec<_>>>()
    })
    .await;
    let copies = match copies {
        Ok(copies) => copies,
        Err(e) => return failed(format!("{e:#}"), 0),
    };
    let mut widths = Vec::new();
    for copy in copies {
        let copy_key = copy.key(&stem);
        let data = Bytes::from(copy.bytes);
        if let Err(e) = store.put(&copy_key, data.clone(), "image/avif").await {
            return failed(format!("{e:#}"), widths.len() as i64);
        }
        note(
            state,
            &copy_key,
            &stem,
            copy.width,
            copy.height,
            Format::Avif,
            data.len(),
        )
        .await;
        state.media.add_avif(&stem, copy.width);
        widths.push(copy.width.to_string());
    }
    let made = widths.len() as i64;
    Some((
        Done {
            outcome: Outcome::Written,
            note: Some(widths.join(", ")),
        },
        made,
    ))
}

/// When the night next comes: the next [`NIGHT_HOUR`] o'clock, UTC.
pub fn next_night(now: DateTime<Utc>) -> DateTime<Utc> {
    let today = now
        .with_hour(NIGHT_HOUR)
        .and_then(|at| at.with_minute(0))
        .and_then(|at| at.with_second(0))
        .and_then(|at| at.with_nanosecond(0))
        .unwrap_or(now);
    if today > now {
        today
    } else {
        today + chrono::Duration::days(1)
    }
}

/// The night's schedule, for as long as the server runs: at
/// [`NIGHT_HOUR`] each night, the leader makes the AVIF copies when
/// `media.avif` is on. The switch is read at the hour, so turning it on
/// takes effect the next night, and off at once.
pub async fn run_nights(state: AppState) {
    if !state.media.is_on() {
        return;
    }
    loop {
        let now = Utc::now();
        let wait = (next_night(now) - now)
            .to_std()
            .unwrap_or(Duration::from_secs(60));
        tokio::time::sleep(wait).await;
        if !state.flag("media.avif", false) || !state.coord.leads() || is_making_avif() {
            continue;
        }
        match avif_now(&state, None).await {
            Ok(_) => {}
            Err(e) => tracing::info!(error = %e, "the night's AVIF copies were not made"),
        }
        // Past the hour before looking for the next night.
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::{
        db::repo::asset::{Kind, Stored, Thumb, Wanted},
        domain::{CoverType, Image, MediaItem, MediaKind},
        media::file,
    };

    /// The whole server, on a database of its own, keeping its media in
    /// `dir`.
    async fn server(dir: &std::path::Path) -> AppState {
        let mut config = crate::config::Config::from_env().expect("a configuration");
        config.mode = crate::config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = crate::config::MediaStorage::Filesystem;
        config.media.dir = dir.to_path_buf();
        config.clients = None;
        config.tmdb.api_key = None;
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;
        AppState::bootstrap(config).await.expect("a server")
    }

    /// A poster of its own: a gradient this wide.
    fn poster(width: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::from_fn(width, width * 3 / 2, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 70])
        })
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
        out
    }

    /// A film whose posters this server keeps as an earlier version kept
    /// them: stored, with no placeholder and no copy.
    async fn kept_as_before(state: &AppState, widths: &[u32]) -> Vec<String> {
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.title = "Paris, Texas".into();
        film.slug = film.id.clone();
        let origins: Vec<String> = widths
            .iter()
            .map(|w| format!("https://images.example/poster-{w}.png"))
            .collect();
        for origin in &origins {
            film.images.push(Image {
                id: crate::db::new_id(),
                season_number: None,
                cover_type: CoverType::Poster,
                url: origin.clone(),
                language: None,
                sort_order: 0,
                source: Some("tmdb".into()),
                is_manual: false,
            });
        }
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &film,
                replace_children: true,
            },
        )
        .await
        .unwrap();

        let store = state.media.store().unwrap();
        for (origin, width) in origins.iter().zip(widths) {
            let bytes = poster(*width);
            let (key, sha) = file::key_for(&bytes, "png");
            store
                .put(&key, Bytes::from(bytes.clone()), "image/png")
                .await
                .unwrap();
            repo::asset::enqueue(
                &state.db,
                &[Wanted {
                    origin,
                    kind: Kind::Image,
                    wanted_by: Some(&film.id),
                }],
            )
            .await
            .unwrap();
            let row = repo::asset::by_origin(&state.db, origin)
                .await
                .unwrap()
                .unwrap();
            repo::asset::mark_stored(
                &state.db,
                &row.id,
                &Stored {
                    key: &key,
                    content_type: "image/png",
                    bytes: bytes.len() as i64,
                    sha256: &sha,
                    width: Some(*width as i32),
                    height: Some((*width * 3 / 2) as i32),
                    thumb: Thumb::None,
                    color: None,
                    thumbhash: None,
                    prepared: 0,
                },
            )
            .await
            .unwrap();
            state.media.remember(origin, &key, Thumb::None, "image/png");
        }
        origins
    }

    /// The task prepares what no version prepared, a few at a time: a
    /// picture prepared before a run was stopped is not prepared again, the
    /// copy a poster is first asked for is made and noted, and a page is
    /// told what the picture looks like.
    #[tokio::test]
    async fn the_pictures_are_prepared_once_and_a_stopped_run_is_taken_up() {
        let dir = crate::testing::scratch_path("variants");
        let state = server(&dir).await;
        let store = state.media.store().unwrap().clone();
        let origins = kept_as_before(&state, &[600, 620, 640]).await;
        assert_eq!(
            repo::asset::unprepared_count(&state.db, variants::PREPARED)
                .await
                .unwrap(),
            3
        );

        // One prepared, as a run that stopped after it would have left it.
        let first = repo::asset::unprepared(&state.db, variants::PREPARED, "", 1)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(
            prepare_one(&state, &store, &first).await.outcome,
            Outcome::Written
        );

        // The next run takes the other two, and them alone.
        let record = job::start(&state.db, VARIANTS, None).await.unwrap();
        let flag = cancel::register(&record);
        match prepare_all(&state, &record, &flag).await.unwrap() {
            Pass::Done(summary) => assert_eq!(summary, "2 prepared, 0 failed, of 2"),
            Pass::Stopped(summary) => panic!("stopped: {summary}"),
        }
        assert_eq!(job::count_entries(&state.db, &record).await.unwrap(), 2);
        assert_eq!(
            repo::asset::unprepared_count(&state.db, variants::PREPARED)
                .await
                .unwrap(),
            0
        );

        for origin in &origins {
            let asset = repo::asset::by_origin(&state.db, origin)
                .await
                .unwrap()
                .unwrap();
            assert!(
                asset.color.is_some() && asset.thumbhash.is_some(),
                "{origin}"
            );
            let key = asset.key.unwrap();
            let stem = file::stem_of(&key);
            let copies = repo::asset::variants_of(&state.db, stem).await.unwrap();
            assert_eq!(copies.len(), 1, "{origin}");
            assert_eq!((copies[0].width, copies[0].format.as_str()), (480, "webp"));
            assert!(store.exists(&copies[0].key).await.unwrap());
            let look = state.media.keyed(&key).unwrap().look;
            assert!(look.color.is_some() && look.thumbhash.is_some() && look.width.is_some());
        }

        // A run asked to stop before its first picture stops there.
        let stopped = job::start(&state.db, VARIANTS, None).await.unwrap();
        let flag = cancel::register(&stopped);
        assert!(cancel::cancel(&stopped));
        match prepare_all(&state, &stopped, &flag).await.unwrap() {
            Pass::Stopped(summary) => assert_eq!(summary, "stopped: 0 prepared, 0 failed, of 0"),
            Pass::Done(summary) => panic!("not stopped: {summary}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The night's AVIF copies of a poster are made once — at 480 and 960,
    /// and at its own width where it is narrower: a small one here, as an
    /// unoptimised AVIF encoder takes seconds over a 480 — and offered from
    /// then on.
    #[tokio::test]
    async fn a_posters_avif_copies_are_made_once() {
        assert_eq!(variants::avif_widths(Some(CoverType::Poster)), [480, 960]);
        assert_eq!(variants::avif_widths(Some(CoverType::Fanart)), [1280, 1920]);
        assert!(variants::avif_widths(Some(CoverType::Clearlogo)).is_empty());

        let dir = crate::testing::scratch_path("avif");
        let state = server(&dir).await;
        let origins = kept_as_before(&state, &[160]).await;

        let record = job::start(&state.db, AVIF, None).await.unwrap();
        let flag = cancel::register(&record);
        match avif_all(&state, &record, &flag).await.unwrap() {
            Pass::Done(summary) => {
                assert_eq!(summary, "1 copies made, 0 failed, of 1 pictures")
            }
            Pass::Stopped(summary) => panic!("stopped: {summary}"),
        }
        let asset = repo::asset::by_origin(&state.db, &origins[0])
            .await
            .unwrap()
            .unwrap();
        let key = asset.key.unwrap();
        let widths: Vec<i32> = repo::asset::variants_of(&state.db, file::stem_of(&key))
            .await
            .unwrap()
            .iter()
            .filter(|v| v.format == "avif")
            .map(|v| v.width)
            .collect();
        assert_eq!(widths, [160], "480 and 960 are its own 160");
        assert_eq!(state.media.keyed(&key).unwrap().look.avif, [160]);
        let copy = state
            .media
            .store()
            .unwrap()
            .read(&format!("{}-160.avif", file::stem_of(&key)))
            .await
            .unwrap()
            .expect("in the store");
        assert_eq!(&copy[4..12], b"ftypavif");

        let again = job::start(&state.db, AVIF, None).await.unwrap();
        let flag = cancel::register(&again);
        match avif_all(&state, &again, &flag).await.unwrap() {
            Pass::Done(summary) => {
                assert_eq!(summary, "0 copies made, 0 failed, of 1 pictures")
            }
            Pass::Stopped(summary) => panic!("stopped: {summary}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_night_comes_at_three_utc_and_never_twice_at_once() {
        let at = |text: &str| {
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&Utc)
        };
        assert_eq!(
            next_night(at("2026-10-24T01:30:00Z")),
            at("2026-10-24T03:00:00Z")
        );
        assert_eq!(
            next_night(at("2026-10-24T03:00:00Z")),
            at("2026-10-25T03:00:00Z"),
            "at the hour itself, the next one"
        );
        assert_eq!(
            next_night(at("2026-10-24T22:10:00Z")),
            at("2026-10-25T03:00:00Z")
        );
    }

    #[test]
    fn a_runs_entry_names_a_picture_by_its_host_and_its_end() {
        assert_eq!(
            short("https://image.tmdb.org/t/p/original/voD93lzFZrr9xfAggwFcPRBi.jpg"),
            "image.tmdb.org/t/p/original/voD93lzFZrr9xfAggwFcPRBi.jpg"
        );
        assert_eq!(
            short(
                "https://assets.fanart.tv/fanart/tv/121361/seasonposter/game-of-thrones-50b3f6ccb1b0c.jpg"
            ),
            "assets.fanart.tv…/tv/121361/seasonposter/game-of-thrones-50b3f6ccb1b0c.jpg"
        );
        assert_eq!(short("upload:0190f2a8"), "upload:0190f2a8");
    }
}

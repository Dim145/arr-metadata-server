//! Whether a newer release is out, asked of GitHub when the operator says
//! so: `updates.check`, off until somebody turns it on.
//!
//! What leaves the server is one anonymous GET of the project's latest
//! release on GitHub's API — a minute after the start, then daily — with the
//! User-Agent every request of this server carries, its name and version,
//! and nothing else: no identifier, no figure about the instance or what it
//! holds. What comes back is kept in memory: the version, its page and when
//! it was published. The dashboard says so, the scrape counts it
//! (`ams_update_available`) and the webhook is told (`update.available`)
//! once per version, whichever instance finds it first — the version told is
//! written down, so a restart does not tell it again.
//!
//! The latest release is GitHub's: never a draft, never one marked a
//! pre-release, and one whose version reads as a pre-release all the same is
//! passed by too. It is newer when its numbers are higher than those of the
//! version running, or the same when what runs is a pre-release of them.

use std::{
    sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use crate::{
    db::{
        Db,
        repo::{job, keystore},
    },
    error::{AppError, AppResult},
    state::AppState,
};

/// The task's name, as the history files its runs.
pub const KIND: &str = job::kinds::UPDATE_CHECK;

/// The setting that turns the check on.
pub const SETTING: &str = "updates.check";

/// What is asked: the latest release that is neither a draft nor marked a
/// pre-release.
const LATEST: &str = "https://api.github.com/repos/Dim145/arr-metadata-server/releases/latest";

/// Where the project's releases are, and so the only place a release's page
/// is taken to be.
const RELEASES: &str = "https://github.com/Dim145/arr-metadata-server/releases/";

/// How long after the start the first check is made.
const FIRST_AFTER: Duration = Duration::from_secs(60);

/// How long between two checks.
pub const EVERY: Duration = Duration::from_secs(24 * 3600);

/// How often the switch is looked at: turned on, the check comes within it.
const TICK: Duration = Duration::from_secs(60);

/// How long GitHub is given to answer.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The most of GitHub's answer that is read: the release's notes are in it.
const MOST_BYTES: u64 = 1024 * 1024;

/// The run's note when nothing newer is out.
pub const NOTHING_NEWER: &str = "nothing newer is out";

/// A release newer than the version running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Its version, as `major.minor.patch`.
    pub version: String,
    /// Its page, on GitHub.
    pub url: String,
    /// When GitHub says it was published.
    pub published_at: Option<String>,
}

/// What the last check found newer, if anything. Kept while a check fails:
/// what was out is still out.
static NEWER: Mutex<Option<Release>> = Mutex::new(None);

/// When the schedule checks next, as near as can be said.
static NEXT: Mutex<Option<DateTime<Utc>>> = Mutex::new(None);

/// One check at a time: the schedule's, or one somebody asked for.
static CHECKING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// A value kept here, whatever a panic left it as: a release or an instant
/// is never half written.
fn kept<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether the operator asked for the check.
pub fn on(state: &AppState) -> bool {
    state.flag(SETTING, false)
}

/// The newer release the last check found: none while the check is off.
pub fn newer(state: &AppState) -> Option<Release> {
    if !on(state) {
        return None;
    }
    kept(&NEWER).clone()
}

/// When the schedule checks next.
pub fn next_at() -> Option<DateTime<Utc>> {
    *kept(&NEXT)
}

/// Whether a check is under way, on this instance.
pub fn is_checking() -> bool {
    CHECKING.try_lock().is_err()
}

/// The schedule: a minute after the start, then a day after the last check,
/// for as long as the switch is on. Each instance asks for itself — what it
/// found is what its dashboard and its scrape say — and the webhook is told
/// once whichever instance asks first.
pub async fn run(state: AppState) {
    *kept(&NEXT) = Some(Utc::now() + delta(FIRST_AFTER));
    tokio::time::sleep(FIRST_AFTER).await;
    let mut was_on = on(&state);
    loop {
        let is_on = on(&state);
        if is_on && !was_on {
            // Turned on since the last tick: a minute from now, not at once,
            // so that the switch can be flipped and flipped back without
            // GitHub being asked in between.
            *kept(&NEXT) = Some(Utc::now() + delta(FIRST_AFTER));
        }
        was_on = is_on;
        if is_on
            && next_at().is_none_or(|at| at <= Utc::now())
            && let Ok(_held) = CHECKING.clone().try_lock_owned()
        {
            let record = job::start(&state.db, KIND, None)
                .await
                .inspect_err(|e| tracing::warn!(error = %e, "could not open the check's run"))
                .ok();
            close(&state, record, check(&state).await).await;
        }
        tokio::time::sleep(TICK).await;
    }
}

/// A check now, as `by` asked, in the background: the run's id at once.
pub async fn check_now(state: &AppState, by: &str) -> AppResult<Option<String>> {
    if !on(state) {
        return Err(AppError::Disabled {
            code: "updates_off",
            message: "the check for a newer version is switched off in the settings".into(),
        });
    }
    let held = CHECKING
        .clone()
        .try_lock_owned()
        .map_err(|_| AppError::Conflict("a check for a newer version is under way".into()))?;
    let record = job::start_by(&state.db, KIND, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        close(&state, Some(record), check(&state).await).await;
    });
    Ok(Some(id))
}

/// Ask, keep what was found, and tell the webhook of a release it has not
/// been told of: the run's note.
async fn check(state: &AppState) -> Result<String> {
    // From this check, whatever comes of it: a failure is tried again with
    // the next day's, not with every tick until GitHub answers.
    *kept(&NEXT) = Some(Utc::now() + delta(EVERY));

    let latest = latest(&state.http).await?;
    let found = newer_than(env!("CARGO_PKG_VERSION"), &latest);
    kept(&NEWER).clone_from(&found);

    let Some(release) = found else {
        return Ok(NOTHING_NEWER.to_string());
    };
    match told_first(&state.db, &release.version).await {
        Ok(true) => crate::service::webhook::update_available(state, &release),
        Ok(false) => {}
        Err(e) => tracing::warn!(
            error = format_args!("{e:#}"),
            "could not write down which release the webhook was told of"
        ),
    }
    Ok(format!("{} is out", release.version))
}

/// Close the run with what came of the check.
async fn close(state: &AppState, record: Option<String>, outcome: Result<String>) {
    let Some(record) = record else {
        if let Err(e) = outcome {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "the check for a newer version failed"
            );
        }
        return;
    };
    let closed = match &outcome {
        Ok(summary) => job::finish(&state.db, &record, Some(summary), None).await,
        Err(e) => job::finish(&state.db, &record, None, Some(&format!("{e:#}"))).await,
    };
    if let Err(e) = closed {
        tracing::warn!(error = %e, "could not close the check's run");
    }
}

/// GitHub's latest release, as much of it as is read.
#[derive(Debug, Deserialize)]
struct Latest {
    tag_name: String,
    html_url: Option<String>,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Ask GitHub for the latest release.
async fn latest(http: &reqwest::Client) -> Result<Latest> {
    let response = http
        .get(LATEST)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "GitHub could not be reached: {}",
                crate::providers::describe_request_error(&e)
            )
        })?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("GitHub answered {status}");
    }
    let body = crate::providers::read_body(response, MOST_BYTES).await?;
    serde_json::from_slice(&body).context("GitHub's answer could not be read")
}

/// The release, when it is newer than `running`: see the module's notes.
fn newer_than(running: &str, latest: &Latest) -> Option<Release> {
    if latest.draft || latest.prerelease {
        return None;
    }
    let found = Version::parse(&latest.tag_name)?;
    if !found.is_newer_than(Version::parse(running)?) {
        return None;
    }
    let (major, minor, patch) = found.numbers;
    // Its page as GitHub gives it, when it is one of the project's; made
    // from the tag otherwise. Shown as a link, so never anything else.
    let url = latest
        .html_url
        .clone()
        .filter(|url| url.starts_with(RELEASES) && !url.chars().any(char::is_whitespace))
        .unwrap_or_else(|| {
            format!(
                "{RELEASES}tag/{}",
                crate::providers::segment(latest.tag_name.trim())
            )
        });
    let published_at = latest
        .published_at
        .as_deref()
        .and_then(crate::db::parse_rfc3339)
        .map(crate::db::to_rfc3339);
    Some(Release {
        version: format!("{major}.{minor}.{patch}"),
        url,
        published_at,
    })
}

/// A version as semver orders it: its three numbers, and whether it is a
/// pre-release. What follows a `+` is build metadata, and no part of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Version {
    numbers: (u64, u64, u64),
    pre: bool,
}

impl Version {
    /// `0.8.0`, `v0.8.0`, `0.8.0-rc.1`, `0.8.0+build.5`; nothing else.
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let text = text.split('+').next().unwrap_or(text);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, true),
            Some(_) => return None,
            None => (text, false),
        };
        let mut parts = core.split('.').map(|part| {
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse::<u64>().ok())
                .flatten()
        });
        let numbers = (parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(Self { numbers, pre })
    }

    /// Whether this, a release, comes after `running`. A pre-release never
    /// does: it is not one to point an operator at.
    fn is_newer_than(self, running: Self) -> bool {
        !self.pre
            && (self.numbers > running.numbers || (self.numbers == running.numbers && running.pre))
    }
}

/// Whether this caller is the first to write `version` down as the release
/// the webhook was told of. Two instances finding it at once both read what
/// was there before; only one replaces it.
async fn told_first(db: &Db, version: &str) -> Result<bool> {
    let name = keystore::names::UPDATE_TOLD;
    let told = keystore::put_if_absent(db, name, "").await?;
    if told == version {
        return Ok(false);
    }
    keystore::replace_if(db, name, &told, version).await
}

/// A wait, as a span of the clock.
fn delta(wait: Duration) -> TimeDelta {
    TimeDelta::from_std(wait).unwrap_or(TimeDelta::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> Latest {
        Latest {
            tag_name: tag.into(),
            html_url: Some(format!("{RELEASES}tag/{tag}")),
            published_at: Some("2026-10-01T12:00:00Z".into()),
            draft: false,
            prerelease: false,
        }
    }

    #[test]
    fn a_version_reads_as_semver_does() {
        let v = |text| Version::parse(text);
        assert_eq!(
            v("0.7.0"),
            Some(Version {
                numbers: (0, 7, 0),
                pre: false
            })
        );
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert_eq!(v("1.2.3+build.5"), v("1.2.3"));
        assert!(v("1.2.3-rc.1").is_some_and(|v| v.pre));
        for refused in [
            "", "1.2", "1.2.3.4", "1.2.x", "1.-2.3", "1.2.3-", "latest", " ",
        ] {
            assert_eq!(v(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn newer_means_higher_numbers_or_the_release_of_a_pre_release() {
        let newer = |found: &str, running: &str| {
            Version::parse(found)
                .unwrap()
                .is_newer_than(Version::parse(running).unwrap())
        };
        assert!(newer("0.8.0", "0.7.0"));
        assert!(newer("0.7.1", "0.7.0"));
        assert!(newer("1.0.0", "0.10.4"));
        // Numbers, not text: 0.10 comes after 0.9.
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("0.7.0", "0.7.0"));
        assert!(!newer("0.6.9", "0.7.0"));
        // The release a pre-release was the run-up to comes after it…
        assert!(newer("0.8.0", "0.8.0-rc.1"));
        // …but a pre-release is passed by, whatever its numbers.
        assert!(!newer("0.9.0-beta.1", "0.8.0"));
    }

    #[test]
    fn the_release_found_is_kept_as_its_numbers_and_its_page() {
        let found = newer_than("0.7.0", &release("v0.8.0")).expect("newer");
        assert_eq!(found.version, "0.8.0");
        assert_eq!(found.url, format!("{RELEASES}tag/v0.8.0"));
        // Written as this server writes every instant.
        assert_eq!(
            found.published_at.as_deref(),
            Some("2026-10-01T12:00:00.000Z")
        );

        assert_eq!(newer_than("0.8.0", &release("v0.8.0")), None);
        assert_eq!(newer_than("0.9.0", &release("v0.8.0")), None);
        assert_eq!(newer_than("0.7.0", &release("nightly")), None);

        // Marked a draft or a pre-release on GitHub: passed by.
        let mut marked = release("v0.8.0");
        marked.prerelease = true;
        assert_eq!(newer_than("0.7.0", &marked), None);
        let mut draft = release("v0.8.0");
        draft.draft = true;
        assert_eq!(newer_than("0.7.0", &draft), None);
    }

    #[test]
    fn a_page_elsewhere_is_never_the_one_linked_to() {
        for elsewhere in [
            "https://example.com/Dim145/arr-metadata-server/releases/tag/v0.8.0",
            "javascript:alert(1)",
            "https://github.com/someone-else/arr-metadata-server/releases/tag/v0.8.0",
        ] {
            let mut answer = release("v0.8.0");
            answer.html_url = Some(elsewhere.into());
            let found = newer_than("0.7.0", &answer).expect("newer");
            assert_eq!(found.url, format!("{RELEASES}tag/v0.8.0"), "{elsewhere}");
        }
        let mut answer = release("v0.8.0");
        answer.html_url = None;
        answer.published_at = Some("not a date".into());
        let found = newer_than("0.7.0", &answer).expect("newer");
        assert_eq!(found.url, format!("{RELEASES}tag/v0.8.0"));
        assert_eq!(found.published_at, None);
    }

    #[test]
    fn github_s_answer_is_read_for_what_it_needs() {
        let answer: Latest = serde_json::from_str(
            r#"{"tag_name":"v0.8.0","html_url":"https://github.com/Dim145/arr-metadata-server/releases/tag/v0.8.0",
                "published_at":"2026-10-01T12:00:00Z","name":"0.8.0","body":"Notes","assets":[]}"#,
        )
        .unwrap();
        assert_eq!(answer.tag_name, "v0.8.0");
        assert!(!answer.draft && !answer.prerelease);
    }

    #[tokio::test]
    async fn a_version_is_told_once_whichever_instance_finds_it() {
        let db = Db::connect(&crate::config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: Duration::from_secs(5),
        })
        .await
        .unwrap();
        db.migrate().await.unwrap();

        assert!(told_first(&db, "0.8.0").await.unwrap());
        assert!(!told_first(&db, "0.8.0").await.unwrap());
        assert!(told_first(&db, "0.9.0").await.unwrap());
        assert!(!told_first(&db, "0.9.0").await.unwrap());
        assert_eq!(
            keystore::get(&db, keystore::names::UPDATE_TOLD)
                .await
                .unwrap()
                .as_deref(),
            Some("0.9.0")
        );
    }
}

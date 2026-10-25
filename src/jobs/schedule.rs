//! When a periodic task runs, where somebody said otherwise than its own
//! cadence.
//!
//! Two ways to say it. A *window* of the day — `01:00` to `06:00` — keeps a
//! task to its own cadence but only inside it: a sweep due at noon waits for
//! one in the morning. A *cron expression* — `0 4 * * 0`, four on Sunday
//! mornings — replaces the cadence with the moments it names. Both in UTC, as
//! the night's AVIF copies are: a server has no time zone of its own to go
//! by, and a schedule that moved with the clocks would run twice, or not at
//! all, on the two nights a year they change.
//!
//! A cron expression is the five fields everybody writes: minute, hour, day
//! of the month, month, day of the week. Each is `*`, a number, a range
//! `1-5`, a list `1,15`, a step `*/15` or `10-50/20`; months and days go by
//! their English names too (`JAN`, `SUN`), Sunday is 0 or 7; and `@hourly`,
//! `@daily`, `@weekly`, `@monthly` and `@yearly` stand for theirs. When both
//! day fields are restricted, a day that matches either is one, as every
//! cron reads them.
//!
//! The schedules are kept in the database, so the leader of several
//! instances reads the one somebody set on another: read again at each of
//! its ticks, never held.

use std::str::FromStr;

use chrono::{DateTime, Datelike as _, Duration, NaiveDate, TimeZone as _, Timelike as _, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{db::repo, state::AppState};

// ─── cron ────────────────────────────────────────────────────────────────────

/// A cron expression, read: the moments it names, field by field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cron {
    /// Bit `m` for minute `m`, 0 to 59.
    minutes: u64,
    /// Bit `h` for hour `h`, 0 to 23.
    hours: u32,
    /// Bit `d` for day `d` of the month, 1 to 31.
    days: u32,
    /// Bit `m` for month `m`, 1 to 12.
    months: u16,
    /// Bit `w` for weekday `w`, 0 (Sunday) to 6.
    weekdays: u8,
    /// Whether the day of the month was left open (`*`): with the day of the
    /// week restricted, then, the day of the week alone decides.
    any_day: bool,
    /// Whether the day of the week was left open.
    any_weekday: bool,
}

/// How far ahead the next moment is looked for: a 29th of February that must
/// also be a Monday comes round once in twenty-eight years, which nobody
/// means; past this, an expression is said never to come.
const HORIZON_YEARS: i64 = 10;

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];
const WEEKDAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

impl FromStr for Cron {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let expanded = match text.to_ascii_lowercase().as_str() {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            other if other.starts_with('@') => {
                return Err(format!(
                    "{text} is not a shorthand this server knows: @hourly, @daily, @weekly, @monthly or @yearly"
                ));
            }
            _ => text,
        };
        let fields: Vec<&str> = expanded.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!(
                "a cron expression has five fields — minute, hour, day of the month, month, day of the week — and this one has {}",
                fields.len()
            ));
        }

        let minutes = field(fields[0], 0, 59, None, "minute")?;
        let hours = field(fields[1], 0, 23, None, "hour")?;
        let days = field(fields[2], 1, 31, None, "day of the month")?;
        let months = field(fields[3], 1, 12, Some((&MONTHS, 1)), "month")?;
        // Sunday is 0 or 7: read up to 7, and 7 folded onto 0.
        let weekdays = field(fields[4], 0, 7, Some((&WEEKDAYS, 0)), "day of the week")?;
        let weekdays = (weekdays & 0x7f) | (weekdays >> 7);

        let cron = Self {
            minutes,
            hours: hours as u32,
            days: days as u32,
            months: months as u16,
            weekdays: weekdays as u8,
            any_day: fields[2].starts_with('*'),
            any_weekday: fields[4].starts_with('*'),
        };
        // A day that never comes: the 30th of February, the 31st of a month
        // of thirty days alone.
        if cron.next_after(Utc::now()).is_none() {
            return Err("this expression names a day that never comes".into());
        }
        Ok(cron)
    }
}

/// One field read into the bits of the values it allows, from `low` to `high`
/// inclusive. `names`, where the field has them, with the value the first one
/// stands for.
fn field(
    text: &str,
    low: u32,
    high: u32,
    names: Option<(&[&str], u32)>,
    what: &str,
) -> Result<u64, String> {
    let value = |part: &str| -> Result<u32, String> {
        if let Some((names, first)) = names
            && let Some(at) = names.iter().position(|n| n.eq_ignore_ascii_case(part))
        {
            return Ok(first + at as u32);
        }
        part.parse::<u32>()
            .ok()
            .filter(|n| (low..=high).contains(n))
            .ok_or_else(|| format!("{part} is not a {what}: it runs from {low} to {high}"))
    };

    let mut bits = 0u64;
    for part in text.split(',') {
        if part.is_empty() {
            return Err(format!("the {what} has an empty part: {text}"));
        }
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step
                    .parse()
                    .ok()
                    .filter(|step| *step > 0)
                    .ok_or_else(|| format!("{step} is not a step of the {what}"))?;
                (range, step)
            }
            None => (part, 1),
        };
        let (from, to) = if range == "*" {
            (low, high)
        } else if let Some((a, b)) = range.split_once('-') {
            let (a, b) = (value(a)?, value(b)?);
            if a > b {
                return Err(format!(
                    "{range}: a range of the {what} runs from the lower to the higher"
                ));
            }
            (a, b)
        } else {
            let a = value(range)?;
            // `5/15`: from five onwards, as most crons read it.
            (a, if step > 1 { high } else { a })
        };
        let mut at = from;
        while at <= to {
            bits |= 1 << at;
            at += step;
        }
    }
    Ok(bits)
}

impl Cron {
    fn day_matches(&self, date: NaiveDate) -> bool {
        let day = self.days & (1 << date.day()) != 0;
        let weekday = self.weekdays & (1 << date.weekday().num_days_from_sunday()) != 0;
        match (self.any_day, self.any_weekday) {
            (true, true) => true,
            (true, false) => weekday,
            (false, true) => day,
            (false, false) => day || weekday,
        }
    }

    /// The first moment it names strictly after `after`, to the minute;
    /// nothing within ten years.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut at = after
            .with_second(0)
            .and_then(|at| at.with_nanosecond(0))
            .unwrap_or(after)
            + Duration::minutes(1);
        let limit = at + Duration::days(366 * HORIZON_YEARS);

        while at <= limit {
            if self.months & (1 << at.month()) == 0 {
                // The first of the next month.
                let (year, month) = if at.month() == 12 {
                    (at.year() + 1, 1)
                } else {
                    (at.year(), at.month() + 1)
                };
                at = Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).single()?;
                continue;
            }
            if !self.day_matches(at.date_naive()) {
                at = midnight(at.date_naive().succ_opt()?)?;
                continue;
            }
            if self.hours & (1 << at.hour()) == 0 {
                at = at.with_minute(0)? + Duration::hours(1);
                continue;
            }
            if self.minutes & (1 << at.minute()) == 0 {
                at += Duration::minutes(1);
                continue;
            }
            return Some(at);
        }
        None
    }
}

fn midnight(date: NaiveDate) -> Option<DateTime<Utc>> {
    Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?))
}

// ─── window ──────────────────────────────────────────────────────────────────

/// A stretch of the day, in minutes since midnight UTC: `end` before `start`
/// crosses midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    start: u32,
    end: u32,
}

/// A time of day written `HH:MM`, as minutes since midnight.
pub fn minutes_of(text: &str) -> Result<u32, String> {
    let bad = || format!("{text} is not a time of day: write it HH:MM, 00:00 to 23:59");
    let (hours, minutes) = text.trim().split_once(':').ok_or_else(bad)?;
    let (hours, minutes): (u32, u32) = (
        hours.parse().map_err(|_| bad())?,
        minutes.parse().map_err(|_| bad())?,
    );
    if hours > 23 || minutes > 59 || text.trim().len() != 5 {
        return Err(bad());
    }
    Ok(hours * 60 + minutes)
}

fn hhmm(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

impl Window {
    pub fn new(start: &str, end: &str) -> Result<Self, String> {
        let (start, end) = (minutes_of(start)?, minutes_of(end)?);
        if start == end {
            return Err("a window that ends when it starts is no window: leave the task to its own cadence instead".into());
        }
        Ok(Self { start, end })
    }

    pub fn start(self) -> String {
        hhmm(self.start)
    }

    pub fn end(self) -> String {
        hhmm(self.end)
    }

    /// Whether a moment falls inside: from the start, up to the end.
    pub fn contains(self, at: DateTime<Utc>) -> bool {
        let minute = at.hour() * 60 + at.minute();
        if self.start < self.end {
            (self.start..self.end).contains(&minute)
        } else {
            minute >= self.start || minute < self.end
        }
    }

    /// `at` itself when it falls inside; else the window's next opening.
    pub fn next_open(self, at: DateTime<Utc>) -> DateTime<Utc> {
        if self.contains(at) {
            return at;
        }
        let opening = |date: NaiveDate| {
            midnight(date).map(|day| day + Duration::minutes(i64::from(self.start)))
        };
        match opening(at.date_naive()) {
            Some(today) if today > at => today,
            Some(today) => today + Duration::days(1),
            None => at,
        }
    }
}

// ─── a task's schedule ───────────────────────────────────────────────────────

/// What a schedule is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = ScheduleKind)]
pub enum Kind {
    /// The task's own cadence, inside a window of the day.
    Window,
    /// The moments a cron expression names.
    Cron,
}

impl Kind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Cron => "cron",
        }
    }
}

/// A schedule as it is set and shown: a window's two ends, or an expression,
/// in UTC.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TaskSchedule)]
pub struct Setting {
    pub kind: Kind,
    /// Where a window starts, `HH:MM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Where it ends, `HH:MM`: before `from`, it crosses midnight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// The five fields, for a cron schedule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// Who set it, as the journal names them: read, never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    /// When: read, never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// A schedule read, ready to say when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Schedule {
    Window(Window),
    Cron(Cron),
}

impl Setting {
    /// What it says, checked: a window of two times that differ, or an
    /// expression with a moment to come.
    pub fn read(&self) -> Result<Schedule, String> {
        match self.kind {
            Kind::Window => {
                let (Some(from), Some(to)) = (self.from.as_deref(), self.to.as_deref()) else {
                    return Err("a window needs where it starts and where it ends".into());
                };
                Window::new(from, to).map(Schedule::Window)
            }
            Kind::Cron => self
                .cron
                .as_deref()
                .ok_or_else(|| "a cron schedule needs its expression".to_string())?
                .parse()
                .map(Schedule::Cron),
        }
    }

    /// The same, written the one way it is kept: times as `HH:MM`, the
    /// expression's spaces single.
    pub fn normalized(&self) -> Result<Self, String> {
        Ok(match self.read()? {
            Schedule::Window(window) => Self {
                kind: Kind::Window,
                from: Some(window.start()),
                to: Some(window.end()),
                cron: None,
                updated_by: None,
                updated_at: None,
            },
            Schedule::Cron(_) => Self {
                kind: Kind::Cron,
                from: None,
                to: None,
                cron: self
                    .cron
                    .as_deref()
                    .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" ")),
                updated_by: None,
                updated_at: None,
            },
        })
    }
}

/// What a task's own cadence says of it, for the schedule to go by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    /// When its own cadence would run it next — in the past when it is due.
    pub natural: DateTime<Utc>,
    /// When it last did its work: a run that went through, or the list it
    /// imported. What a cron schedule counts its next moment from.
    pub last_done: Option<DateTime<Utc>>,
    /// Not before this: after a failure, the wait before it is tried again.
    pub not_before: Option<DateTime<Utc>>,
}

/// When a task runs next: as its own cadence says without a schedule; at the
/// window's next opening when that falls outside it; at the expression's
/// first moment since it last did its work — or since the schedule was set,
/// so a new schedule does not run at once for a moment it never named —
/// with a cron schedule. `None` when the expression never comes again.
pub fn next(
    schedule: Option<(&Schedule, Option<DateTime<Utc>>)>,
    clock: Clock,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let floor = |at: DateTime<Utc>| clock.not_before.map_or(at, |not| at.max(not));
    match schedule {
        None => Some(floor(clock.natural)),
        Some((Schedule::Window(window), _)) => {
            Some(window.next_open(floor(clock.natural).max(now)))
        }
        Some((Schedule::Cron(cron), set_at)) => {
            let since = match (clock.last_done, set_at) {
                (Some(done), Some(set)) => done.max(set),
                (Some(at), None) | (None, Some(at)) => at,
                // Never done, and no date to the schedule: from now.
                (None, None) => now,
            };
            cron.next_after(since).map(floor)
        }
    }
}

/// The tasks a schedule can be set for: those that run on a cadence of their
/// own, from a loop of this module's. The others keep theirs — the media
/// sweep, a certificate's renewal, the check for a newer version — or have
/// none: what runs by hand, the import rules, each on its own schedule.
pub const SCHEDULABLE: &[&str] = &[
    repo::job::kinds::REFRESH_SWEEP,
    repo::job::kinds::IMPORT_ANIME,
    repo::job::kinds::IMPORT_IMDB,
    repo::job::kinds::IMDB_AKAS,
    repo::job::kinds::IMDB_EPISODES,
    crate::jobs::backup::KIND,
];

pub fn is_schedulable(task: &str) -> bool {
    SCHEDULABLE.contains(&task)
}

/// A task's schedule as it is kept: what was set, and when — `None` for a
/// task left to its own cadence, or one whose row this version cannot read,
/// which is said once in the log.
pub async fn of(state: &AppState, task: &str) -> anyhow::Result<Option<(Setting, Schedule)>> {
    let Some(row) = repo::task_schedule::get(&state.db, task).await? else {
        return Ok(None);
    };
    Ok(read_row(row))
}

/// Every schedule set, by task.
pub async fn all(
    state: &AppState,
) -> anyhow::Result<std::collections::HashMap<String, (Setting, Schedule)>> {
    Ok(repo::task_schedule::all(&state.db)
        .await?
        .into_iter()
        .filter_map(|row| {
            let task = row.task.clone();
            read_row(row).map(|read| (task, read))
        })
        .collect())
}

fn read_row(row: repo::task_schedule::Row) -> Option<(Setting, Schedule)> {
    let kind = match row.kind.as_str() {
        "window" => Kind::Window,
        "cron" => Kind::Cron,
        _ => return None,
    };
    let setting = Setting {
        kind,
        from: row.window_start,
        to: row.window_end,
        cron: row.cron,
        updated_by: row.updated_by,
        updated_at: Some(row.updated_at),
    };
    match setting.read() {
        Ok(schedule) => Some((setting, schedule)),
        Err(why) => {
            tracing::warn!(task = %row.task, why, "a task's schedule could not be read; its own cadence stands");
            None
        }
    }
}

/// When the schedule set for `task` — or its own cadence — runs it next.
pub fn next_of(
    read: Option<&(Setting, Schedule)>,
    clock: Clock,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let set_at = |setting: &Setting| {
        setting
            .updated_at
            .as_deref()
            .and_then(crate::db::parse_rfc3339)
    };
    next(
        read.map(|(setting, schedule)| (schedule, set_at(setting))),
        clock,
        now,
    )
}

/// Whether `task` is due now, by its schedule and its own cadence: the
/// question each loop asks at its tick. A schedule that cannot be read is no
/// reason to stop: its own cadence answers.
pub async fn due(state: &AppState, task: &str, clock: Clock) -> bool {
    let read = match of(state, task).await {
        Ok(read) => read,
        Err(e) => {
            tracing::warn!(
                task,
                error = format_args!("{e:#}"),
                "could not read a task's schedule; waiting for the next tick"
            );
            return false;
        }
    };
    let now = Utc::now();
    next_of(read.as_ref(), clock, now).is_some_and(|at| at <= now)
}

/// The next few moments a schedule names from now — what the page shows
/// beside a schedule being written, so its author sees what they wrote.
pub fn upcoming(schedule: &Schedule, now: DateTime<Utc>, count: usize) -> Vec<DateTime<Utc>> {
    let mut out = Vec::with_capacity(count);
    match schedule {
        Schedule::Cron(cron) => {
            let mut at = now;
            while out.len() < count {
                let Some(next) = cron.next_after(at) else {
                    break;
                };
                out.push(next);
                at = next;
            }
        }
        Schedule::Window(window) => {
            // The window's next openings: the first is now, when it is open;
            // each after it, the first start strictly later.
            let mut at = window.next_open(now);
            while out.len() < count {
                out.push(at);
                let Some(start) = midnight(at.date_naive())
                    .map(|day| day + Duration::minutes(i64::from(window.start)))
                else {
                    break;
                };
                at = if start > at {
                    start
                } else {
                    start + Duration::days(1)
                };
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn a_cron_expression_names_the_moments_it_says() {
        let sundays: Cron = "0 4 * * 0".parse().unwrap();
        // Saturday the 24th of October 2026, at noon: Sunday at four.
        assert_eq!(
            sundays.next_after(at("2026-10-24T12:00:00Z")),
            Some(at("2026-10-25T04:00:00Z"))
        );
        // Strictly after: at four on Sunday, the next Sunday.
        assert_eq!(
            sundays.next_after(at("2026-10-25T04:00:00Z")),
            Some(at("2026-11-01T04:00:00Z"))
        );
        // Names and seven for Sunday read the same.
        assert_eq!("0 4 * * SUN".parse::<Cron>().unwrap(), sundays);
        assert_eq!("0 4 * * 7".parse::<Cron>().unwrap(), sundays);

        let quarters: Cron = "*/15 9-17 * * mon-fri".parse().unwrap();
        assert_eq!(
            quarters.next_after(at("2026-10-23T17:50:00Z")),
            // Friday evening, past five: Monday at nine.
            Some(at("2026-10-26T09:00:00Z"))
        );
        assert_eq!(
            quarters.next_after(at("2026-10-26T09:07:30Z")),
            Some(at("2026-10-26T09:15:00Z"))
        );

        let monthly: Cron = "@monthly".parse().unwrap();
        assert_eq!(
            monthly.next_after(at("2026-12-15T00:00:00Z")),
            Some(at("2027-01-01T00:00:00Z"))
        );

        // The 31st, in a month that has one only.
        let last: Cron = "30 2 31 * *".parse().unwrap();
        assert_eq!(
            last.next_after(at("2026-11-01T00:00:00Z")),
            Some(at("2026-12-31T02:30:00Z"))
        );
    }

    #[test]
    fn both_days_restricted_is_either_of_them() {
        // The first of the month, and every Monday.
        let either: Cron = "0 0 1 * 1".parse().unwrap();
        assert_eq!(
            either.next_after(at("2026-10-24T00:00:00Z")),
            Some(at("2026-10-26T00:00:00Z")),
            "Monday the 26th"
        );
        assert_eq!(
            either.next_after(at("2026-10-27T00:00:00Z")),
            Some(at("2026-11-01T00:00:00Z")),
            "the first, a Sunday"
        );
    }

    #[test]
    fn what_is_no_cron_expression_is_said_why() {
        for bad in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "* * * * 8",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
            "1,,2 * * * *",
            "@reboot",
            // The 30th of February.
            "0 0 30 2 *",
        ] {
            assert!(bad.parse::<Cron>().is_err(), "{bad:?} should be refused");
        }
        // Leap days do come.
        assert!("0 0 29 2 *".parse::<Cron>().is_ok());
    }

    #[test]
    fn a_window_holds_its_own_hours_and_says_when_it_opens() {
        let night = Window::new("01:00", "06:00").unwrap();
        assert!(night.contains(at("2026-10-24T01:00:00Z")));
        assert!(night.contains(at("2026-10-24T05:59:00Z")));
        assert!(!night.contains(at("2026-10-24T06:00:00Z")));
        assert_eq!(
            night.next_open(at("2026-10-24T12:00:00Z")),
            at("2026-10-25T01:00:00Z")
        );
        assert_eq!(
            night.next_open(at("2026-10-24T00:30:00Z")),
            at("2026-10-24T01:00:00Z")
        );
        assert_eq!(
            night.next_open(at("2026-10-24T03:00:00Z")),
            at("2026-10-24T03:00:00Z")
        );

        // Across midnight.
        let late = Window::new("22:00", "02:00").unwrap();
        assert!(late.contains(at("2026-10-24T23:30:00Z")));
        assert!(late.contains(at("2026-10-24T01:59:00Z")));
        assert!(!late.contains(at("2026-10-24T02:00:00Z")));
        assert_eq!(
            late.next_open(at("2026-10-24T12:00:00Z")),
            at("2026-10-24T22:00:00Z")
        );

        for bad in [
            ("01:00", "01:00"),
            ("25:00", "02:00"),
            ("1:00", "02:00"),
            ("01:60", "02:00"),
        ] {
            assert!(Window::new(bad.0, bad.1).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_next_run_follows_the_cadence_the_window_or_the_expression() {
        let now = at("2026-10-24T12:00:00Z");
        let due = Clock {
            natural: at("2026-10-24T11:00:00Z"),
            last_done: Some(at("2026-10-24T05:00:00Z")),
            not_before: None,
        };
        // Its own cadence: due an hour ago, so now.
        assert_eq!(next(None, due, now), Some(at("2026-10-24T11:00:00Z")));

        // At night only: the window's next opening.
        let night = Schedule::Window(Window::new("01:00", "06:00").unwrap());
        assert_eq!(
            next(Some((&night, None)), due, now),
            Some(at("2026-10-25T01:00:00Z"))
        );
        // Due within the window: when it is due.
        let inside = Clock {
            natural: at("2026-10-25T03:00:00Z"),
            ..due
        };
        assert_eq!(
            next(Some((&night, None)), inside, now),
            Some(at("2026-10-25T03:00:00Z"))
        );

        // Sundays at four: from the last run that went through.
        let sundays = Schedule::Cron("0 4 * * 0".parse().unwrap());
        assert_eq!(
            next(Some((&sundays, None)), due, now),
            Some(at("2026-10-25T04:00:00Z"))
        );
        // A Sunday missed while the server was down is run once it is back.
        let missed = Clock {
            last_done: Some(at("2026-10-10T04:00:00Z")),
            ..due
        };
        assert_eq!(
            next(Some((&sundays, None)), missed, now),
            Some(at("2026-10-11T04:00:00Z"))
        );
        // But a schedule set since then counts from when it was set.
        assert_eq!(
            next(
                Some((&sundays, Some(at("2026-10-24T10:00:00Z")))),
                missed,
                now
            ),
            Some(at("2026-10-25T04:00:00Z"))
        );
        // After a failure, the wait comes first.
        let failed = Clock {
            not_before: Some(at("2026-10-24T13:00:00Z")),
            ..missed
        };
        assert_eq!(
            next(Some((&sundays, None)), failed, now),
            Some(at("2026-10-24T13:00:00Z"))
        );
    }

    #[test]
    fn a_setting_is_kept_in_one_form_and_shows_its_next_moments() {
        let window = Setting {
            kind: Kind::Window,
            from: Some("01:00".into()),
            to: Some("06:00".into()),
            cron: None,
            updated_by: None,
            updated_at: None,
        };
        assert_eq!(window.normalized().unwrap().from.as_deref(), Some("01:00"));
        let cron = Setting {
            kind: Kind::Cron,
            from: None,
            to: None,
            cron: Some("  0   4 * *  0 ".into()),
            updated_by: None,
            updated_at: None,
        };
        assert_eq!(
            cron.normalized().unwrap().cron.as_deref(),
            Some("0 4 * * 0")
        );
        let read = cron.read().unwrap();
        let now = at("2026-10-24T12:00:00Z");
        assert_eq!(
            upcoming(&read, now, 3),
            vec![
                at("2026-10-25T04:00:00Z"),
                at("2026-11-01T04:00:00Z"),
                at("2026-11-08T04:00:00Z"),
            ]
        );
        assert_eq!(
            upcoming(&window.read().unwrap(), now, 2),
            vec![at("2026-10-25T01:00:00Z"), at("2026-10-26T01:00:00Z")]
        );
        // A window open now opens now.
        assert_eq!(
            upcoming(&window.read().unwrap(), at("2026-10-24T02:00:00Z"), 1),
            vec![at("2026-10-24T02:00:00Z")]
        );
    }
}

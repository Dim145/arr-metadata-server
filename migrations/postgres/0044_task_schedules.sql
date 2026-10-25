-- When the periodic tasks run, where an administrator said otherwise than
-- each task's own cadence: a window of the day the task keeps to, or the
-- moments a cron expression names. A task without a row runs as it always
-- has. Every time here is UTC, as the night's AVIF copies are.
CREATE TABLE task_schedule (
    task          TEXT    PRIMARY KEY,
    -- 'window': its own cadence, but only inside the window. 'cron': at the
    -- moments the expression names, instead of its own cadence.
    kind          TEXT    NOT NULL CHECK (kind IN ('window', 'cron')),
    -- 'HH:MM'. An end before the start crosses midnight: 22:00 to 06:00.
    window_start  TEXT,
    window_end    TEXT,
    -- Five fields: minute, hour, day of the month, month, day of the week.
    cron          TEXT,
    updated_by    TEXT,
    updated_at    TEXT    NOT NULL
);

-- What is brought into the catalogue on a schedule, and into which
-- selection: TMDB's most popular of a recent stretch, or a list another
-- server publishes, followed. Each run is a job run of its own,
-- `import.rule`, naming the rule as its target.
CREATE TABLE import_rule (
    id            TEXT    PRIMARY KEY,
    name          TEXT    NOT NULL,
    -- 'tmdb': TMDB's most popular works of the last days. 'feed': another
    -- server's list, as a text file of ids or an Atom feed.
    source        TEXT    NOT NULL CHECK (source IN ('tmdb', 'feed')),
    -- What the source is asked, as JSON in its own vocabulary.
    params        TEXT    NOT NULL DEFAULT '{}',
    -- The selection the works go into; none for the catalogue alone. A rule
    -- goes with the selection it fills.
    list_id       TEXT    REFERENCES curated_list (id) ON DELETE CASCADE,
    -- 'add': the works join the selection. 'replace': the selection becomes
    -- what the source gave, in its order.
    list_mode     TEXT    NOT NULL DEFAULT 'add' CHECK (list_mode IN ('add', 'replace')),
    -- When it runs: five cron fields, UTC.
    cron          TEXT    NOT NULL,
    -- A language the works' episode texts are fetched in as they come in,
    -- besides the catalogue's own.
    language      TEXT,
    -- Whether the works it brings in are switched on.
    enable_works  INTEGER NOT NULL DEFAULT 1,
    is_enabled    INTEGER NOT NULL DEFAULT 1,
    last_run_at   TEXT,
    last_job_id   TEXT,
    created_by    TEXT,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL
);

CREATE INDEX ix_import_rule_list ON import_rule (list_id);

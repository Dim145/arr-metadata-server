-- What a run did to each work, one row a work, in the order they were taken:
-- the detail behind a run's one-line summary, for the history to open. The
-- work's title and kind are kept beside its id, so a run reads the same once
-- the work is gone. Pruned with the runs.
CREATE TABLE job_entry (
    id         TEXT PRIMARY KEY,
    job_id     TEXT NOT NULL,
    position   BIGINT NOT NULL,
    media_id   TEXT,
    title      TEXT,
    kind       TEXT,
    outcome    TEXT NOT NULL,
    note       TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX ix_job_entry_job ON job_entry (job_id, position);

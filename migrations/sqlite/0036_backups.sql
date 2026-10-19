-- The archives this server made of itself: one row an archive, written once
-- the archive is whole where it is kept — a directory, or among several
-- instances the bucket — and taken away with it. The archive is what
-- matters; the row is how its date, its size, what it holds and why it was
-- made are listed without opening it, and how the dashboard says when the
-- last one was made. A restore keeps the rows it found, since the archives
-- did not move: the copy it brings back carries the list as it stood then.
CREATE TABLE backup (
    id             TEXT    PRIMARY KEY,
    -- The file's name where it is kept, e.g.
    -- `ams-20261019-020000-scheduled.tar.zst`, `.age` after it when it is
    -- encrypted.
    name           TEXT    NOT NULL UNIQUE,
    created_at     TEXT    NOT NULL,
    -- scheduled, manual, safety (before a restore) or command (the command
    -- line's).
    reason         TEXT    NOT NULL,
    -- directory or bucket.
    place          TEXT    NOT NULL,
    size_bytes     INTEGER NOT NULL,
    -- Of the archive as kept, encrypted or not.
    sha256         TEXT    NOT NULL,
    -- The engine the database was read from: sqlite or postgres.
    engine         TEXT    NOT NULL,
    server_version TEXT    NOT NULL,
    -- The last migration the database had: what a restore needs this
    -- server's to reach at least.
    migration      INTEGER NOT NULL,
    includes_media INTEGER NOT NULL DEFAULT 0,
    media_files    INTEGER NOT NULL DEFAULT 0,
    encrypted      INTEGER NOT NULL DEFAULT 0,
    -- Who asked, as the journal names them; nothing for the schedule.
    created_by     TEXT,
    -- The run that made it, when one did.
    job_id         TEXT
);

CREATE INDEX ix_backup_created ON backup (created_at);

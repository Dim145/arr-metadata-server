-- What several instances of this server share through the database, besides
-- the catalogue itself.

-- Material every instance must hold the same of: the key the identity
-- provider's cookies are sealed with, the authority the clients trust and the
-- certificate it issued. Kept here so an instance started afresh serves what
-- the others do, rather than one of its own.
CREATE TABLE IF NOT EXISTS keystore (
    name       TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Which instance runs a job, so that one starting closes the runs its own
-- previous life left open — and not the ones another instance is running.
ALTER TABLE job_run ADD COLUMN instance TEXT;

-- A medium in line is taken by one worker at a time: whoever claimed it, and
-- when — a claim older than a few minutes is a worker that died mid-fetch,
-- and the medium is anybody's again.
ALTER TABLE media_asset ADD COLUMN claimed_by TEXT;
ALTER TABLE media_asset ADD COLUMN claimed_at TEXT;

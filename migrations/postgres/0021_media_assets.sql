-- The media kept here: every picture and theme a work points at, fetched
-- once and served from this server or its bucket, so the catalogue reads
-- without its providers. A row per address the work holds — the address is
-- what the work's rows carry, and stays what they carry — or per upload.
CREATE TABLE media_asset (
    id              TEXT PRIMARY KEY,
    origin          TEXT NOT NULL UNIQUE,
    kind            TEXT NOT NULL CHECK (kind IN ('image', 'audio')),
    status          TEXT NOT NULL CHECK (status IN ('pending', 'stored', 'failed')),
    -- Where the bytes are: "<sha256>.<ext>", the same for the same bytes.
    key             TEXT,
    content_type    TEXT,
    bytes           BIGINT,
    sha256          TEXT,
    width           INTEGER,
    height          INTEGER,
    has_thumb       INTEGER NOT NULL DEFAULT 0,
    attempts        INTEGER NOT NULL DEFAULT 0,
    error           TEXT,
    next_attempt_at TEXT,
    -- The work that wanted it first, to refresh once it is here.
    wanted_by       TEXT,
    uploaded_by     TEXT,
    created_at      TEXT NOT NULL,
    stored_at       TEXT
);

CREATE INDEX ix_media_asset_status ON media_asset (status, next_attempt_at);
CREATE INDEX ix_media_asset_key    ON media_asset (key);

-- The sweep looks for every address in every column that can hold one:
-- each column by itself, or a catalogue's worth of pictures is walked once
-- per asset.
CREATE INDEX ix_media_image_url      ON media_image (url);
CREATE INDEX ix_media_episode_image  ON media_episode (image);
CREATE INDEX ix_media_credit_image   ON media_credit (image);
CREATE INDEX ix_media_relation_image ON media_relation (image);
CREATE INDEX ix_media_item_theme     ON media_item (theme_music);

-- A run stopped partway is filed as 'stopped' since the tasks page, which
-- the check refused: the run stayed 'running' in the history.
ALTER TABLE job_run DROP CONSTRAINT job_run_status_check;
ALTER TABLE job_run ADD CONSTRAINT job_run_status_check
    CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'stopped'));

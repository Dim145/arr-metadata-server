-- ============================================================================
-- arr-metadata-server — initial schema (PostgreSQL)
--
-- Mirrors migrations/sqlite/0001_init.sql. Two deliberate non-idiomatic choices
-- keep a single Rust code path across both engines:
--
--   * Booleans are INTEGER, not BOOLEAN. sqlx's `Any` driver widens integers
--     across SMALLINT/INTEGER/BIGINT but will not decode an integer into `bool`,
--     so a real BOOLEAN column here would need a second read path.
--   * JSON-valued columns are TEXT, not JSONB, for the same reason. Nothing
--     currently queries inside those documents; switching to JSONB later is a
--     migration plus a decode branch, not a redesign.
--
-- Timestamps are RFC 3339 in UTC (lexicographically sortable).
-- Identifiers are UUIDv7 in canonical hyphenated form.
-- ============================================================================

-- ─── canonical entity ───────────────────────────────────────────────────────

CREATE TABLE media_item (
    id                  TEXT     PRIMARY KEY,
    kind                TEXT     NOT NULL CHECK (kind IN ('series', 'movie')),
    slug                TEXT     NOT NULL,
    title               TEXT     NOT NULL,
    sort_title          TEXT,
    original_title      TEXT,
    overview            TEXT,
    status              TEXT,
    original_language   TEXT,
    original_country    TEXT,
    runtime             INTEGER,
    year                INTEGER,
    first_aired         TEXT,
    last_aired          TEXT,
    in_cinemas          TEXT,
    physical_release    TEXT,
    digital_release     TEXT,
    air_time            TEXT,
    network             TEXT,
    studio              TEXT,
    content_rating      TEXT,
    homepage            TEXT,
    trailer_youtube_id  TEXT,
    popularity          DOUBLE PRECISION,
    genres              TEXT     NOT NULL DEFAULT '[]',
    keywords            TEXT     NOT NULL DEFAULT '[]',
    collection_tmdb_id  INTEGER,

    -- Entries created in the UI with no provider behind them.
    is_manual           INTEGER  NOT NULL DEFAULT 0,
    -- Soft-disable: kept in the database, hidden from every read surface.
    is_enabled          INTEGER  NOT NULL DEFAULT 1,

    created_at          TEXT     NOT NULL,
    updated_at          TEXT     NOT NULL,
    refreshed_at        TEXT,
    -- When the scheduler should look at this entry again. NULL = never.
    refresh_after       TEXT,
    refresh_error       TEXT
);

CREATE UNIQUE INDEX ux_media_item_kind_slug ON media_item (kind, slug);
CREATE        INDEX ix_media_item_title     ON media_item (title);
CREATE        INDEX ix_media_item_updated   ON media_item (updated_at);
-- Drives the refresh scheduler's "what is due?" scan.
CREATE        INDEX ix_media_item_refresh   ON media_item (refresh_after)
    WHERE refresh_after IS NOT NULL;

-- ─── external identifiers ───────────────────────────────────────────────────
--
-- `source` is namespaced per entity type because TMDB and TVDB number movies
-- and series independently: tmdb movie 42 and tmdb series 42 are different works.

CREATE TABLE media_external_id (
    media_id    TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    source      TEXT NOT NULL CHECK (source IN (
                    'tmdb_movie', 'tmdb_tv', 'tvdb_series', 'tvdb_movie',
                    'imdb', 'tvmaze', 'tvrage', 'mal', 'anilist',
                    'trakt_show', 'trakt_movie'
                )),
    value       TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (source, value)
);

CREATE INDEX ix_media_external_id_media ON media_external_id (media_id);

-- ─── provider snapshots ─────────────────────────────────────────────────────

CREATE TABLE media_provider_snapshot (
    media_id    TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    provider    TEXT NOT NULL,
    payload     TEXT NOT NULL,
    fetched_at  TEXT NOT NULL,
    etag        TEXT,
    PRIMARY KEY (media_id, provider)
);

CREATE INDEX ix_snapshot_fetched ON media_provider_snapshot (fetched_at);

-- ─── manual overrides (the lock) ────────────────────────────────────────────
--
-- `scope` addresses what the field belongs to:
--   'item'              the work itself
--   'season:3'          season 3
--   'episode:3x7'       season 3, episode 7
-- `value` is a JSON-encoded value; SQL NULL means "the user cleared this field",
-- which is distinct from having no override at all.

CREATE TABLE media_override (
    media_id    TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    scope       TEXT NOT NULL DEFAULT 'item',
    field       TEXT NOT NULL,
    value       TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    updated_by  TEXT,
    PRIMARY KEY (media_id, scope, field)
);

CREATE INDEX ix_override_media ON media_override (media_id);

-- ─── seasons & episodes ─────────────────────────────────────────────────────

CREATE TABLE media_season (
    id             TEXT    PRIMARY KEY,
    media_id       TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    season_number  INTEGER NOT NULL,
    title          TEXT,
    overview       TEXT,
    air_date       TEXT,
    tmdb_id        INTEGER,
    tvdb_id        INTEGER,
    is_manual      INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT    NOT NULL,
    updated_at     TEXT    NOT NULL,
    UNIQUE (media_id, season_number)
);

CREATE TABLE media_episode (
    id                          TEXT    PRIMARY KEY,
    media_id                    TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    season_number               INTEGER NOT NULL,
    episode_number              INTEGER NOT NULL,
    absolute_episode_number     INTEGER,
    aired_after_season_number   INTEGER,
    aired_before_season_number  INTEGER,
    aired_before_episode_number INTEGER,
    title                       TEXT    NOT NULL DEFAULT '',
    overview                    TEXT,
    air_date                    TEXT,
    air_date_utc                TEXT,
    runtime                     INTEGER,
    finale_type                 TEXT,
    image                       TEXT,
    tvdb_id                     INTEGER,
    tmdb_id                     INTEGER,
    rating_value                DOUBLE PRECISION,
    rating_count                INTEGER,
    is_manual                   INTEGER NOT NULL DEFAULT 0,
    created_at                  TEXT    NOT NULL,
    updated_at                  TEXT    NOT NULL,
    UNIQUE (media_id, season_number, episode_number)
);

CREATE INDEX ix_episode_media ON media_episode (media_id, season_number, episode_number);
CREATE INDEX ix_episode_air   ON media_episode (air_date_utc) WHERE air_date_utc IS NOT NULL;

-- ─── images ─────────────────────────────────────────────────────────────────

CREATE TABLE media_image (
    id             TEXT    PRIMARY KEY,
    media_id       TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    -- NULL means the image belongs to the work rather than to one season.
    season_number  INTEGER,
    cover_type     TEXT    NOT NULL,
    url            TEXT    NOT NULL,
    language       TEXT,
    sort_order     INTEGER NOT NULL DEFAULT 0,
    source         TEXT,
    is_manual      INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT    NOT NULL
);

CREATE INDEX        ix_image_media ON media_image (media_id, cover_type, sort_order);
CREATE UNIQUE INDEX ux_image_url   ON media_image (media_id, cover_type, url);

-- ─── credits ────────────────────────────────────────────────────────────────

CREATE TABLE media_credit (
    id               TEXT    PRIMARY KEY,
    media_id         TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    credit_type      TEXT    NOT NULL DEFAULT 'actor',
    person_name      TEXT    NOT NULL,
    character_name   TEXT,
    image            TEXT,
    tmdb_person_id   INTEGER,
    sort_order       INTEGER NOT NULL DEFAULT 0,
    is_manual        INTEGER NOT NULL DEFAULT 0,
    created_at       TEXT    NOT NULL
);

CREATE INDEX ix_credit_media ON media_credit (media_id, credit_type, sort_order);

-- ─── alternative titles, ratings, translations ──────────────────────────────

CREATE TABLE media_alternative_title (
    id          TEXT PRIMARY KEY,
    media_id    TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    title_type  TEXT,
    language    TEXT,
    is_manual   INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);

CREATE UNIQUE INDEX ux_alt_title ON media_alternative_title (media_id, title, COALESCE(language, ''));

CREATE TABLE media_rating (
    media_id  TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    source    TEXT NOT NULL,
    value     DOUBLE PRECISION,
    votes     INTEGER,
    rating_type TEXT,
    PRIMARY KEY (media_id, source)
);

CREATE TABLE media_translation (
    media_id  TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    language  TEXT NOT NULL,
    title     TEXT,
    overview  TEXT,
    is_manual INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (media_id, language)
);

-- ─── API clients ────────────────────────────────────────────────────────────
--
-- The key itself is never stored. `key_prefix` is the plaintext head shown in
-- the UI so a key can be recognised at a glance; `key_hash` is a hex SHA-256 of
-- the whole key.
--
-- SHA-256 rather than argon2 is deliberate. Argon2 exists to make *guessing*
-- expensive, which matters for human-chosen passwords. These keys are 256 bits
-- from the OS CSPRNG, so there is nothing to guess — and argon2 on every
-- metadata request would add tens of milliseconds to a hot path that Sonarr
-- hits in bursts. Admin passwords, which are human-chosen, do use argon2id.
--
-- The unique index on `key_hash` is also what makes authentication a single
-- indexed lookup instead of a scan.

CREATE TABLE api_client (
    id            TEXT    PRIMARY KEY,
    name          TEXT    NOT NULL,
    key_prefix    TEXT    NOT NULL,
    key_hash      TEXT    NOT NULL,
    scopes        TEXT    NOT NULL DEFAULT '[]',
    is_enabled    INTEGER NOT NULL DEFAULT 1,
    expires_at    TEXT,
    created_at    TEXT    NOT NULL,
    last_used_at  TEXT,
    last_used_ip  TEXT,
    note          TEXT
);

CREATE UNIQUE INDEX ux_api_client_name ON api_client (name);
CREATE UNIQUE INDEX ux_api_client_hash ON api_client (key_hash);

-- ─── admin users & sessions ─────────────────────────────────────────────────

CREATE TABLE admin_user (
    id             TEXT    PRIMARY KEY,
    username       TEXT    NOT NULL,
    password_hash  TEXT    NOT NULL,
    is_admin       INTEGER NOT NULL DEFAULT 1,
    created_at     TEXT    NOT NULL,
    last_login_at  TEXT
);

CREATE UNIQUE INDEX ux_admin_user_username ON admin_user (username);

CREATE TABLE admin_session (
    -- SHA-256 of the session token; the token itself only lives in the cookie.
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES admin_user (id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    user_agent  TEXT,
    ip          TEXT
);

CREATE INDEX ix_session_user    ON admin_session (user_id);
CREATE INDEX ix_session_expires ON admin_session (expires_at);

-- ─── jobs & audit ───────────────────────────────────────────────────────────

CREATE TABLE job_run (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    target       TEXT,
    status       TEXT NOT NULL CHECK (status IN ('queued', 'running', 'succeeded', 'failed')),
    started_at   TEXT,
    finished_at  TEXT,
    error        TEXT,
    detail       TEXT,
    created_at   TEXT NOT NULL
);

CREATE INDEX ix_job_status  ON job_run (status, created_at);
CREATE INDEX ix_job_created ON job_run (created_at);

CREATE TABLE audit_log (
    id         TEXT PRIMARY KEY,
    at         TEXT NOT NULL,
    actor      TEXT,
    action     TEXT NOT NULL,
    target     TEXT,
    detail     TEXT,
    ip         TEXT
);

CREATE INDEX ix_audit_at ON audit_log (at);

-- ─── settings & cached searches ─────────────────────────────────────────────

CREATE TABLE setting (
    setting_key  TEXT PRIMARY KEY,
    value        TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE search_cache (
    cache_key   TEXT PRIMARY KEY,
    payload     TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL
);

CREATE INDEX ix_search_cache_expires ON search_cache (expires_at);

-- What this server asked of the operator's own Sonarr and Radarr: one row a
-- work told of, newest last, for their page to list what was sent lately and
-- how it went. Kept in the database rather than in memory because the change
-- may have been made on one instance and the page read on another, and
-- because a restart should not forget a failure the operator has not read.
-- Only the latest few hundred are kept: rows past them go as rows arrive.
CREATE TABLE arr_push (
    id        TEXT    PRIMARY KEY,
    at        TEXT    NOT NULL,
    -- sonarr or radarr.
    app       TEXT    NOT NULL,
    -- The work, by its id here, and its title as it stood: the row reads the
    -- same once the work is gone.
    media_id  TEXT,
    title     TEXT,
    -- The id the work is served under — TheTVDB's, a synthetic one, TMDB's —
    -- and the one Sonarr or Radarr knows it by, when it holds it.
    client_id INTEGER,
    arr_id    INTEGER,
    -- sent, absent (not held there) or failed.
    outcome   TEXT    NOT NULL,
    -- Why it was told: a JSON array of lock, edit, sync, refresh.
    reasons   TEXT    NOT NULL DEFAULT '[]',
    -- The command it was sent in, or what went wrong.
    detail    TEXT
);

CREATE INDEX ix_arr_push_at ON arr_push (at);

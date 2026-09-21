-- Network authorisation, and who has been knocking.
--
-- Sonarr and Radarr have their metadata URLs compiled in and cannot present a
-- key, so those surfaces are guarded by address. That list used to live only in
-- `AMS_ALLOWLIST`, which meant changing who may call required a restart and a
-- file edit — while an API key, the other half of the same decision, could be
-- issued from the interface. This table makes both the same kind of thing.
--
-- The environment variable still seeds this table the first time the server
-- starts against an empty one, so an existing deployment keeps working. After
-- that the table is the truth and the variable is ignored: two sources for one
-- decision is how a server ends up refusing a client nobody can explain.

CREATE TABLE network_rule (
    id          TEXT NOT NULL PRIMARY KEY,
    -- An address or a CIDR block, stored as written so the interface can show
    -- the operator what they typed rather than a normalised form.
    cidr        TEXT NOT NULL UNIQUE,
    note        TEXT,
    created_at  TEXT NOT NULL,
    created_by  TEXT
);

-- Everyone who has called a guarded surface, whether or not they were let in.
--
-- The point is the refusals: a client that cannot reach this server gives no
-- clue why, and its address is the one thing needed to fix that. Keeping the
-- allowed ones too means the interface can show what is actually using the
-- server rather than only what failed to.
--
-- One row per address. `hits` counts, `last_seen` is updated at most once a
-- minute, so a Sonarr refreshing a library does not turn every read into a
-- write.
CREATE TABLE network_caller (
    ip              TEXT    NOT NULL PRIMARY KEY,
    -- Resolved from the system resolver and the hosts file, best effort. In a
    -- compose network this is the container's name, which is what an operator
    -- actually recognises; it is refreshed occasionally rather than per request.
    hostname        TEXT,
    -- Sonarr and Radarr identify themselves here, which names the application
    -- where the hostname names the machine.
    user_agent      TEXT,
    last_surface    TEXT    NOT NULL,
    last_path       TEXT,
    last_allowed    INTEGER NOT NULL,
    hits            INTEGER NOT NULL DEFAULT 1,
    refusals        INTEGER NOT NULL DEFAULT 0,
    first_seen      TEXT    NOT NULL,
    last_seen       TEXT    NOT NULL
);

CREATE INDEX ix_network_caller_seen ON network_caller (last_seen DESC);

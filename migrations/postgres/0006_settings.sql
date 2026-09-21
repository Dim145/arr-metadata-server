-- Settings that belong to the operator rather than to the deployment.
--
-- Some configuration is about the machine — where the database is, what port to
-- bind, which keys to use — and belongs in the environment, where it can be
-- version-controlled with the compose file. The rest is about how this server
-- behaves, and an operator should be able to change it without editing a file
-- and restarting: which language to answer in, whether to fall back to Skyhook,
-- how often to refresh, whether adult titles are served at all.
--
-- One table rather than a column per setting, because the set grows and because
-- the same key has to be settable at more than one scope.
--
--   server  — the default for everything, `scope_id` empty
--   client  — an API key, `scope_id` is its id
--   peer    — an allowlist rule, `scope_id` is its id
--
-- A peer is how a client that cannot present a key is identified. Sonarr and
-- Radarr have their URLs compiled in and send no credential, so the only thing
-- that distinguishes them is the address they call from — which is exactly what
-- an allowlist rule already records. Naming the rule names the client, and the
-- name survives a container taking a new address, which an IP does not.
--
-- The environment still seeds the server scope on a first start, so an existing
-- deployment keeps its configuration across the upgrade.

-- The initial schema declared a `setting` table — one key, one value, no
-- scope — and nothing ever wrote to it. Dropping it loses nothing and keeps the
-- name for the table that does the job.
DROP TABLE IF EXISTS setting;

CREATE TABLE setting (
    scope       TEXT NOT NULL,
    scope_id    TEXT NOT NULL DEFAULT '',
    key         TEXT NOT NULL,
    -- Stored as text and parsed against the registry, which knows each key's
    -- type. A typed column per kind would mean three nullable columns and a
    -- check constraint to keep exactly one of them filled.
    value       TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    updated_by  TEXT,
    PRIMARY KEY (scope, scope_id, key)
);

-- What to call the client an allowlist rule stands for.
ALTER TABLE network_rule ADD COLUMN name TEXT;

-- Whether a work is an adult title, as the provider that supplied it said.
--
-- Nothing in the canonical model recorded this, so there was nothing to filter
-- on: the only adult handling was a parameter passed through to TMDB's search.
-- Serving a catalogue means being able to decide per request who sees what,
-- which means storing it.
ALTER TABLE media_item ADD COLUMN is_adult INTEGER NOT NULL DEFAULT 0;

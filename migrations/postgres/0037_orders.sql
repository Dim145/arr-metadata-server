-- The orders a series' episodes come in, as orders of their own.
--
-- 0013 kept TheTVDB's other orders as placings of the aired episodes, by
-- their TheTVDB id: a DVD's, straight through, an alternate or a regional
-- one. That model could not hold what TheTVDB files in one order alone — a
-- DVD's hour-long cut of two aired parts, the 194 extended cuts of The
-- Office's Superfan order — since a place named an aired episode or
-- nothing; nor an order from elsewhere, nor one a person made. An order is
-- now a row of its own: what clients ask for it by (`key`), what it is
-- (`kind`), where it was read (`source`, `source_ref`), the name it goes by
-- (`label`) and whether it is frozen as the aired numbering. Each of its
-- places names a held episode when there is one, and otherwise carries the
-- record its source gave of it.
--
-- The aired order is the work's own episode list and is not kept here: it is
-- told apart by `is_default` when an order stands for it.
CREATE TABLE media_order (
    id           TEXT    PRIMARY KEY,
    media_id     TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    -- What a client asks for it by: TheTVDB's own type for its orders
    -- (`dvd`, `alternate`, `alttwo`…), `tmdb-` and the id of a TMDB episode
    -- group, `ams-` and a name for one made here.
    key          TEXT    NOT NULL,
    kind         TEXT    NOT NULL CHECK (kind IN ('aired', 'dvd', 'absolute', 'digital', 'production', 'story_arc', 'tv', 'tvdb_alt', 'custom')),
    -- tvdb, tmdb, or local for one made here.
    source       TEXT    NOT NULL,
    -- The order's id at its source: a TMDB episode group's.
    source_ref   TEXT,
    -- Its name: TheTVDB's for the series' order ("Superfan Order"), the
    -- group's on TMDB, or the one it was given here.
    label        TEXT    NOT NULL,
    is_default   INTEGER NOT NULL DEFAULT 0,
    -- What the aired order answers with, while it is set: one order of a
    -- work at the most.
    is_frozen    INTEGER NOT NULL DEFAULT 0,
    -- How many episodes its source says it holds, before they are read: a
    -- TMDB group's are read the first time the group is served or shown.
    announced    INTEGER,
    -- When its places were last read from its source; empty while they are
    -- still to be read.
    read_at      TEXT,
    created_at   TEXT    NOT NULL,
    updated_at   TEXT    NOT NULL,
    UNIQUE (media_id, key)
);

-- One place of an order, in the order's own sequence. `episode_id` names a
-- held episode by the row it is kept in, which only an episode made by hand
-- keeps across refreshes; TheTVDB's and TMDB's ids name it whatever row it
-- is in. The record — title to finale — is what the order's source said of
-- the episode, or what a person wrote of one the work does not hold.
CREATE TABLE media_order_episode (
    id               TEXT    PRIMARY KEY,
    order_id         TEXT    NOT NULL REFERENCES media_order (id) ON DELETE CASCADE,
    position         INTEGER NOT NULL,
    season_number    INTEGER NOT NULL,
    episode_number   INTEGER NOT NULL,
    absolute_number  INTEGER,
    episode_id       TEXT,
    tvdb_episode_id  BIGINT,
    tmdb_episode_id  BIGINT,
    title            TEXT,
    overview         TEXT,
    aired            TEXT,
    runtime          INTEGER,
    image            TEXT,
    finale_type      TEXT
);

CREATE INDEX ix_order_episode_order ON media_order_episode (order_id, position);

-- Who numbers a series when TheTVDB does not answer for it: how many
-- refreshes in a row it has not, twenty hours apart at the least (`silent`,
-- the last counted at `silent_at`), and since when TMDB numbers it in its
-- place (`since`, empty while TheTVDB does). No row for a series TheTVDB
-- answers for.
CREATE TABLE media_numbering (
    media_id    TEXT    PRIMARY KEY REFERENCES media_item (id) ON DELETE CASCADE,
    silent      INTEGER NOT NULL DEFAULT 0,
    silent_at   TEXT,
    since       TEXT,
    updated_at  TEXT    NOT NULL
);

-- What 0013 kept, each order a row of its own, named as TheTVDB names its
-- types until the next refresh reads the series' own names for them. The
-- order's id is made of the work's and the type, which name it once; its
-- places keep their ids, in the order of their numbers.
INSERT INTO media_order
    (id, media_id, key, kind, source, source_ref, label, is_default, is_frozen, announced,
     read_at, created_at, updated_at)
SELECT media_id || ':' || order_type,
       media_id,
       order_type,
       CASE order_type
           WHEN 'dvd' THEN 'dvd'
           WHEN 'absolute' THEN 'absolute'
           ELSE 'tvdb_alt'
       END,
       'tvdb',
       NULL,
       CASE order_type
           WHEN 'dvd' THEN 'DVD Order'
           WHEN 'absolute' THEN 'Absolute Order'
           WHEN 'alternate' THEN 'Alternate Order'
           WHEN 'regional' THEN 'Regional Order'
           WHEN 'altdvd' THEN 'Alternate DVD Order'
           WHEN 'alttwo' THEN 'Alternate Order 2'
           ELSE order_type
       END,
       0,
       0,
       NULL,
       MIN(created_at),
       MIN(created_at),
       MIN(created_at)
  FROM media_episode_order
 GROUP BY media_id, order_type;

INSERT INTO media_order_episode
    (id, order_id, position, season_number, episode_number, absolute_number, episode_id,
     tvdb_episode_id, tmdb_episode_id, title, overview, aired, runtime, image, finale_type)
SELECT id,
       media_id || ':' || order_type,
       ROW_NUMBER() OVER (
           PARTITION BY media_id, order_type
           ORDER BY season_number, episode_number, tvdb_episode_id
       ) - 1,
       season_number,
       episode_number,
       absolute_number,
       NULL,
       tvdb_episode_id,
       NULL,
       NULL,
       NULL,
       NULL,
       NULL,
       NULL,
       NULL
  FROM media_episode_order;

-- What 0013 kept was read before an order kept the name the series gives it
-- on TheTVDB, or the episodes TheTVDB files in it alone — The Office's
-- Superfan cuts —, and its orders would be served so until the series is
-- refreshed again: a week, for an ended one. Each series that had an order
-- is due for a refresh from the upgrade on, and the sweep takes it among the
-- first works it finds due. One due already keeps its place; one never
-- scheduled is left so.
UPDATE media_item
   SET refresh_after = to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"')
 WHERE id IN (SELECT media_id FROM media_episode_order)
   AND refresh_after > to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"');

DROP TABLE media_episode_order;

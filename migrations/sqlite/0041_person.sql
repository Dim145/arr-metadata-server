-- The people the catalogue credits, each a record of their own.
--
-- Until now a person was whatever a credit named: a TMDB id where the source
-- gave one, a name where it did not, put together again on every read of
-- their page. Nothing could say that two spellings were one person, or that
-- one name was two, and nothing was kept of them between two reads. A person
-- is now a row, with the ids each source files them under, what TMDB says of
-- them (fetched the first time their page is read, and again once it is a
-- month old), and the other names they go by; every credit names the person
-- it is of.
--
-- `merged_into` is set on a record somebody merged into another: kept, ids
-- and all, so that a credit filed under its TMDB or TheTVDB id finds the
-- record it went into, and an address naming it leads there. Nothing lists
-- it, and nothing is credited to it any more. Checked as a transaction ends,
-- since a copy of the table into another database writes its rows in no
-- order: a record merged may come before the one it went into.
CREATE TABLE person (
    id              TEXT     PRIMARY KEY,
    tmdb_id         INTEGER,
    imdb_id         TEXT,
    tvdb_id         INTEGER,
    name            TEXT     NOT NULL,
    -- What the index files and sorts them under, where it is not their name:
    -- set by hand, `Toro, Guillermo del`.
    sort_name       TEXT,
    -- The name in the script of their own language, as TMDB has it: 田中真弓.
    original_name   TEXT,
    -- As TMDB has it: 1 a woman, 2 a man, 3 non-binary.
    gender          INTEGER,
    birthday        TEXT,
    deathday        TEXT,
    place_of_birth  TEXT,
    biography       TEXT,
    -- Their portrait, and a few more of them, as JSON: TMDB's.
    image           TEXT,
    photos          TEXT     NOT NULL DEFAULT '[]',
    -- What TMDB files them under — `Acting`, `Directing` — and their page.
    known_for       TEXT,
    homepage        TEXT,
    popularity      REAL,
    -- When TMDB was last asked about them; empty until their page is read.
    refreshed_at    TEXT,
    -- Entered, or put right, by hand: what TMDB says no longer replaces it.
    is_manual       INTEGER  NOT NULL DEFAULT 0,
    -- What the index of people files and counts them by, worked out by the
    -- server whenever their credits change: the name they sort by, folded as
    -- the search folds it, and its first letter (`#` for any but A to Z);
    -- how many works anybody may see credit them by a credit no lock hides,
    -- in all and for each job. Read whole, these are an index's page; added
    -- up from the credits for every request, they cost seconds.
    sort_key        TEXT     NOT NULL DEFAULT '',
    letter          TEXT     NOT NULL DEFAULT '#',
    works           INTEGER  NOT NULL DEFAULT 0,
    acting          INTEGER  NOT NULL DEFAULT 0,
    directing       INTEGER  NOT NULL DEFAULT 0,
    writing         INTEGER  NOT NULL DEFAULT 0,
    producing       INTEGER  NOT NULL DEFAULT 0,
    merged_into     TEXT     REFERENCES person (id) DEFERRABLE INITIALLY DEFERRED,
    created_at      TEXT     NOT NULL,
    updated_at      TEXT     NOT NULL
);

-- One record for each TMDB id; any number without one.
CREATE UNIQUE INDEX ux_person_tmdb ON person (tmdb_id);
CREATE INDEX ix_person_tvdb ON person (tvdb_id);
CREATE INDEX ix_person_name ON person (name);
CREATE INDEX ix_person_merged ON person (merged_into);
CREATE INDEX ix_person_listed ON person (letter, sort_key);

-- The other names somebody goes by, and where each came from: `credit`, a
-- spelling one of their credits gives; `tmdb`, one TMDB lists; `merged`, the
-- name of a record merged into theirs; `manual`, one typed by hand. They are
-- found by each of them, and a credit naming one of them alone is theirs.
CREATE TABLE person_alias (
    person_id  TEXT NOT NULL REFERENCES person (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    source     TEXT NOT NULL,
    PRIMARY KEY (person_id, name)
);

CREATE INDEX ix_person_alias_name ON person_alias (name);

-- Who a credit is of. And where it is: on the work, or on one episode of it —
-- a guest, the director of one episode — which the work's own cast and crew
-- leave out.
ALTER TABLE media_credit ADD COLUMN person_id TEXT REFERENCES person (id);
ALTER TABLE media_credit ADD COLUMN season_number INTEGER;
ALTER TABLE media_credit ADD COLUMN episode_number INTEGER;

CREATE INDEX ix_credit_person_id ON media_credit (person_id, media_id);

-- The people the credits held so far name. Their ids are random rather than
-- ordered by time, as the server's own are: nothing reads an order into a
-- person's id.
--
-- First one record for each TMDB id, under the name most of its credits give
-- it — the first in order of those as common — with the latest portrait one
-- of them carries.
INSERT INTO person (id, tmdb_id, name, image, is_manual, created_at, updated_at)
SELECT lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4'
           || substr(lower(hex(randomblob(2))), 2) || '-'
           || substr('89ab', 1 + (abs(random()) % 4), 1)
           || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))),
       named.tmdb_person_id,
       named.name,
       (SELECT c.image FROM media_credit c
         WHERE c.tmdb_person_id = named.tmdb_person_id AND c.image IS NOT NULL
         ORDER BY c.created_at DESC LIMIT 1),
       0,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
  FROM (SELECT tmdb_person_id, trim(person_name) AS name,
               ROW_NUMBER() OVER (PARTITION BY tmdb_person_id
                                  ORDER BY COUNT(*) DESC, trim(person_name)) AS place
          FROM media_credit
         WHERE tmdb_person_id IS NOT NULL AND trim(person_name) <> ''
         GROUP BY tmdb_person_id, trim(person_name)) AS named
 WHERE named.place = 1;

UPDATE media_credit
   SET person_id = (SELECT p.id FROM person p WHERE p.tmdb_id = media_credit.tmdb_person_id)
 WHERE tmdb_person_id IS NOT NULL;

-- Every other spelling their credits give, as a name they go by.
INSERT INTO person_alias (person_id, name, source)
SELECT DISTINCT c.person_id, trim(c.person_name), 'credit'
  FROM media_credit c JOIN person p ON p.id = c.person_id
 WHERE trim(c.person_name) <> p.name AND trim(c.person_name) <> '';

-- A credit that gives a name and no TMDB id is taken for the one person
-- known by a TMDB id whom some credit names that way: a guest TheTVDB lists,
-- an actor typed in by hand, are the actor TMDB credits elsewhere far more
-- often than they are somebody else. Not when two people known by TMDB ids
-- go by the name: which of them it is, nothing here says. A homonym is the
-- risk — somebody else of the same name, credited by name alone, filed under
-- the better-known one — and the way out is to split the credit off by hand
-- (`POST /api/v1/people/{id}/split`), which a refresh then leaves as it was
-- split: a credit written again keeps the person it had.
UPDATE media_credit
   SET person_id = known.person_id
  FROM (SELECT trim(person_name) AS name, MIN(person_id) AS person_id
          FROM media_credit
         WHERE tmdb_person_id IS NOT NULL
         GROUP BY trim(person_name)
        HAVING COUNT(DISTINCT person_id) = 1) AS known
 WHERE media_credit.tmdb_person_id IS NULL
   AND trim(media_credit.person_name) = known.name;

-- Then one record for each name the other credits give, and those credits
-- filed under it.
INSERT INTO person (id, name, image, is_manual, created_at, updated_at)
SELECT lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4'
           || substr(lower(hex(randomblob(2))), 2) || '-'
           || substr('89ab', 1 + (abs(random()) % 4), 1)
           || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))),
       named.name,
       named.image,
       0,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
  FROM (SELECT trim(person_name) AS name, MAX(image) AS image
          FROM media_credit
         WHERE tmdb_person_id IS NULL AND person_id IS NULL AND trim(person_name) <> ''
         GROUP BY trim(person_name)) AS named;

UPDATE media_credit
   SET person_id = (SELECT p.id FROM person p
                     WHERE p.tmdb_id IS NULL AND p.name = trim(media_credit.person_name))
 WHERE person_id IS NULL AND trim(person_name) <> '';

-- A credit hidden by a lock, or shown again: the person it names is found
-- by one work fewer, or one more, once the work's rows are written again —
-- and not at all once every credit of theirs is hidden.
CREATE TRIGGER search_stale_credit_lock_ai AFTER INSERT ON media_override
WHEN NEW.field = 'hidden' AND NEW.scope LIKE 'credit:%' BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = NEW.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_credit_lock_ad AFTER DELETE ON media_override
WHEN OLD.field = 'hidden' AND OLD.scope LIKE 'credit:%' BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = OLD.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_credit_lock_au AFTER UPDATE ON media_override
WHEN (NEW.field = 'hidden' AND NEW.scope LIKE 'credit:%')
  OR (OLD.field = 'hidden' AND OLD.scope LIKE 'credit:%') BEGIN
    UPDATE media_item SET search_stale = 1
     WHERE id IN (OLD.media_id, NEW.media_id) AND search_stale = 0;
END;

-- A work switched on or off, given to adults or taken from them: who its
-- credits name is counted by one work more, or one fewer, in the index of
-- people once its rows are written again.
CREATE TRIGGER search_stale_item_shown_au AFTER UPDATE OF is_enabled, is_adult ON media_item BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = NEW.id AND search_stale = 0;
END;

-- The search index's people were their credits' keys, `tmdb:<id>` and
-- `name:<name>`; they are the records now. Taken out, and every work marked
-- for the server to write its rows again, which writes the people its credits
-- name under their records — in the background, from the first start.
DELETE FROM search_doc WHERE kind = 'person';
UPDATE media_item SET search_stale = 1 WHERE search_stale = 0;

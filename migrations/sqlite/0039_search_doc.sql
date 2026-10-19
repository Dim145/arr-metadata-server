-- The search index: one row for every name something is found by — each
-- title a work goes by, each person credited, each episode — with its text
-- as the server folds it (`norm`: decomposed, the combining accents of
-- U+0300 to U+036F taken off so that kana keep their marks, composed again,
-- in lower case, every mark of punctuation a space). The server writes it,
-- since neither engine folds text the way the other does, and ranks what it
-- finds itself; see `src/search`.
--
-- `id` is not a counter: it is worked out from what the row is, with its kind
-- in the top bits, so that a row written again keeps its number, a copy into
-- another database needs no sequence put right, and a search of one kind
-- reads one range of the FTS5 table below. `ref_id` is what the row names —
-- the work's id, the episode's, or `tmdb:<id>` and `name:<name>` for a person,
-- as a lock on a credit names one — and `work_id` the work a work's or an
-- episode's row belongs to, gone with it.
CREATE TABLE search_doc (
    id          INTEGER PRIMARY KEY,
    kind        TEXT    NOT NULL CHECK (kind IN ('work', 'person', 'episode')),
    ref_id      TEXT    NOT NULL,
    work_id     TEXT    REFERENCES media_item (id) ON DELETE CASCADE,
    lang        TEXT,
    norm        TEXT    NOT NULL,
    text        TEXT    NOT NULL,
    popularity  REAL,
    year        INTEGER
);

-- A term shorter than a trigram is looked for at the start of a name, which
-- this reads in order; the other two are how a work's rows and a person's are
-- found again to be written — a person's by its key, which nothing reads a
-- work's or an episode's row by: over every row, that index weighed a third
-- of the table.
CREATE INDEX ix_search_doc_prefix ON search_doc (kind, norm);
CREATE INDEX ix_search_doc_work ON search_doc (work_id);
CREATE INDEX ix_search_doc_person_key ON search_doc (ref_id) WHERE kind = 'person';

-- Trigrams over the folded text, read for a term found inside a name and for
-- the names that share most of a misspelt one's trigrams. Its content is the
-- table above, which the three triggers keep it in step with, as the FTS5
-- documentation has them. `detail = full`: a term longer than three letters
-- is a phrase of trigrams, and a phrase needs their places.
CREATE VIRTUAL TABLE search_fts USING fts5(
    norm,
    content = 'search_doc',
    content_rowid = 'id',
    tokenize = 'trigram',
    detail = 'full'
);

CREATE TRIGGER search_doc_ai AFTER INSERT ON search_doc BEGIN
    INSERT INTO search_fts (rowid, norm) VALUES (NEW.id, NEW.norm);
END;

CREATE TRIGGER search_doc_ad AFTER DELETE ON search_doc BEGIN
    INSERT INTO search_fts (search_fts, rowid, norm) VALUES ('delete', OLD.id, OLD.norm);
END;

-- Only what the index holds: a row's popularity or year written again is
-- nothing the trigrams need to hear of.
CREATE TRIGGER search_doc_au AFTER UPDATE OF id, norm ON search_doc BEGIN
    INSERT INTO search_fts (search_fts, rowid, norm) VALUES ('delete', OLD.id, OLD.norm);
    INSERT INTO search_fts (rowid, norm) VALUES (NEW.id, NEW.norm);
END;

-- Which works the index is behind on. A refresh writes a work's rows in its
-- own transaction; anything else that changes what a work is found by — a
-- title locked, an alternative title or an episode added by hand, a credit
-- removed — marks the work, and the server writes its rows again within the
-- minute. Every work held before this migration starts marked: that is how
-- the index is first filled, in the background, by the running server.
ALTER TABLE media_item ADD COLUMN search_stale INTEGER NOT NULL DEFAULT 1;

CREATE INDEX ix_media_item_search_stale ON media_item (id) WHERE search_stale = 1;

-- A credit named without a TMDB id is a person by their name alone: found
-- again by it when the people's rows are written, and by the list's filter.
CREATE INDEX ix_credit_person_name ON media_credit (person_name);

CREATE TRIGGER search_stale_alt_ai AFTER INSERT ON media_alternative_title BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = NEW.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_alt_ad AFTER DELETE ON media_alternative_title BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = OLD.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_alt_au AFTER UPDATE ON media_alternative_title BEGIN
    UPDATE media_item SET search_stale = 1
     WHERE id IN (OLD.media_id, NEW.media_id) AND search_stale = 0;
END;

CREATE TRIGGER search_stale_translation_ai AFTER INSERT ON media_translation BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = NEW.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_translation_ad AFTER DELETE ON media_translation BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = OLD.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_translation_au AFTER UPDATE OF media_id, title ON media_translation BEGIN
    UPDATE media_item SET search_stale = 1
     WHERE id IN (OLD.media_id, NEW.media_id) AND search_stale = 0;
END;

CREATE TRIGGER search_stale_episode_ai AFTER INSERT ON media_episode BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = NEW.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_episode_ad AFTER DELETE ON media_episode BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = OLD.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_episode_au
AFTER UPDATE OF media_id, season_number, episode_number, title, air_date ON media_episode BEGIN
    UPDATE media_item SET search_stale = 1
     WHERE id IN (OLD.media_id, NEW.media_id) AND search_stale = 0;
END;

CREATE TRIGGER search_stale_credit_ai AFTER INSERT ON media_credit BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = NEW.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_credit_ad AFTER DELETE ON media_credit BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = OLD.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_credit_au
AFTER UPDATE OF media_id, person_name, tmdb_person_id ON media_credit BEGIN
    UPDATE media_item SET search_stale = 1
     WHERE id IN (OLD.media_id, NEW.media_id) AND search_stale = 0;
END;

-- The locks a work is found by: a title in any language, an episode's, and
-- an alternative title hidden.
CREATE TRIGGER search_stale_override_ai AFTER INSERT ON media_override
WHEN NEW.field IN ('title', 'originalTitle', 'sortTitle') OR NEW.scope LIKE 'alternativeTitle:%' BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = NEW.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_override_ad AFTER DELETE ON media_override
WHEN OLD.field IN ('title', 'originalTitle', 'sortTitle') OR OLD.scope LIKE 'alternativeTitle:%' BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = OLD.media_id AND search_stale = 0;
END;
CREATE TRIGGER search_stale_override_au AFTER UPDATE ON media_override
WHEN NEW.field IN ('title', 'originalTitle', 'sortTitle') OR NEW.scope LIKE 'alternativeTitle:%'
  OR OLD.field IN ('title', 'originalTitle', 'sortTitle') OR OLD.scope LIKE 'alternativeTitle:%' BEGIN
    UPDATE media_item SET search_stale = 1
     WHERE id IN (OLD.media_id, NEW.media_id) AND search_stale = 0;
END;

-- And the work's own names, year and popularity, wherever they are written.
CREATE TRIGGER search_stale_item_au
AFTER UPDATE OF kind, title, sort_title, original_title, original_language, year, popularity
ON media_item BEGIN
    UPDATE media_item SET search_stale = 1 WHERE id = NEW.id AND search_stale = 0;
END;

-- The index of 0022, which the search no longer reads: every title of a
-- work in one row, matched whole and unranked. Its triggers first, which
-- name it.
DROP TRIGGER IF EXISTS media_search_item_ai;
DROP TRIGGER IF EXISTS media_search_item_au;
DROP TRIGGER IF EXISTS media_search_item_ad;
DROP TRIGGER IF EXISTS media_search_alt_ai;
DROP TRIGGER IF EXISTS media_search_alt_au;
DROP TRIGGER IF EXISTS media_search_alt_ad;
DROP TRIGGER IF EXISTS media_search_override_ai;
DROP TRIGGER IF EXISTS media_search_override_au;
DROP TRIGGER IF EXISTS media_search_override_ad;
DROP TABLE IF EXISTS media_search;

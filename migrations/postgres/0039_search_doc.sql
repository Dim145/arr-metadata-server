-- The search index: one row for every name something is found by — each
-- title a work goes by, each person credited, each episode — with its text
-- as the server folds it (`norm`: decomposed, the combining accents of
-- U+0300 to U+036F taken off so that kana keep their marks, composed again,
-- in lower case, every mark of punctuation a space). The server writes it,
-- since neither engine folds text the way the other does, and ranks what it
-- finds itself; see `src/search`.
--
-- `id` is not a counter: it is worked out from what the row is, with its kind
-- in the top bits, so that a row written again keeps its number and a copy
-- from another database needs no sequence put right. `ref_id` is what the row
-- names — the work's id, the episode's, or `tmdb:<id>` and `name:<name>` for
-- a person, as a lock on a credit names one — and `work_id` the work a work's
-- or an episode's row belongs to, gone with it.
CREATE TABLE search_doc (
    id          BIGINT  PRIMARY KEY,
    kind        TEXT    NOT NULL CHECK (kind IN ('work', 'person', 'episode')),
    ref_id      TEXT    NOT NULL,
    work_id     TEXT    REFERENCES media_item (id) ON DELETE CASCADE,
    lang        TEXT,
    norm        TEXT    NOT NULL,
    text        TEXT    NOT NULL,
    popularity  DOUBLE PRECISION,
    year        INTEGER
);

-- A term shorter than a trigram is looked for at the start of a name, which
-- `text_pattern_ops` lets `LIKE 'term%'` read in order whatever the
-- database's collation; the other two are how a work's rows and a person's
-- are found again to be written — a person's by its key, which nothing reads
-- a work's or an episode's row by: over every row, that index weighed a
-- third of the table.
CREATE INDEX ix_search_doc_prefix ON search_doc (kind, norm text_pattern_ops);
CREATE INDEX ix_search_doc_work ON search_doc (work_id);
CREATE INDEX ix_search_doc_person_key ON search_doc (ref_id) WHERE kind = 'person';

-- Trigrams over the folded text, one index for each kind so that a search
-- of works reads none of the episodes' rows: `LIKE '%term%'` for a term found
-- inside a name, `word_similarity` for a misspelt one. Only where pg_trgm can
-- be had, as in 0022: without it a search reads every row of its kind, and
-- forgives no misspelling. The server says which it found when it starts.
DO $$
BEGIN
    BEGIN
        CREATE EXTENSION IF NOT EXISTS pg_trgm;
    EXCEPTION WHEN OTHERS THEN
        RAISE NOTICE 'pg_trgm is not available (%): searches read every row', SQLERRM;
    END;
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_trgm') THEN
        CREATE INDEX IF NOT EXISTS ix_search_doc_work_trgm
            ON search_doc USING gin (norm gin_trgm_ops) WHERE kind = 'work';
        CREATE INDEX IF NOT EXISTS ix_search_doc_person_trgm
            ON search_doc USING gin (norm gin_trgm_ops) WHERE kind = 'person';
        CREATE INDEX IF NOT EXISTS ix_search_doc_episode_trgm
            ON search_doc USING gin (norm gin_trgm_ops) WHERE kind = 'episode';
    END IF;
END $$;

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

-- The marks, one function for every table a work's names are read from: the
-- work of the row before and of the row after, whichever there is. A mark
-- already made is left alone, so a refresh writing a thousand episodes
-- writes the work's row once. The work is held for a key share first, as a
-- foreign key holds it for an insert: the server writing a work's rows holds
-- the works it reads for update, so a change made while it reads is either
-- waited for and read, or waits and marks the work again once it is done —
-- never read before it lands and marked as done after.
CREATE FUNCTION search_mark_stale() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM 1 FROM media_item WHERE id = OLD.media_id FOR KEY SHARE;
        UPDATE media_item SET search_stale = 1 WHERE id = OLD.media_id AND search_stale = 0;
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM 1 FROM media_item WHERE id = NEW.media_id FOR KEY SHARE;
        UPDATE media_item SET search_stale = 1 WHERE id = NEW.media_id AND search_stale = 0;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER search_stale_alt_ai AFTER INSERT ON media_alternative_title
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();
CREATE TRIGGER search_stale_alt_ad AFTER DELETE ON media_alternative_title
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();
CREATE TRIGGER search_stale_alt_au AFTER UPDATE ON media_alternative_title
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();

CREATE TRIGGER search_stale_translation_ai AFTER INSERT ON media_translation
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();
CREATE TRIGGER search_stale_translation_ad AFTER DELETE ON media_translation
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();
CREATE TRIGGER search_stale_translation_au AFTER UPDATE OF media_id, title ON media_translation
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();

CREATE TRIGGER search_stale_episode_ai AFTER INSERT ON media_episode
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();
CREATE TRIGGER search_stale_episode_ad AFTER DELETE ON media_episode
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();
CREATE TRIGGER search_stale_episode_au
    AFTER UPDATE OF media_id, season_number, episode_number, title, air_date ON media_episode
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();

CREATE TRIGGER search_stale_credit_ai AFTER INSERT ON media_credit
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();
CREATE TRIGGER search_stale_credit_ad AFTER DELETE ON media_credit
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();
CREATE TRIGGER search_stale_credit_au
    AFTER UPDATE OF media_id, person_name, tmdb_person_id ON media_credit
    FOR EACH ROW EXECUTE FUNCTION search_mark_stale();

-- The locks a work is found by: a title in any language, an episode's, and
-- an alternative title hidden.
CREATE TRIGGER search_stale_override_ai AFTER INSERT ON media_override
    FOR EACH ROW
    WHEN (NEW.field IN ('title', 'originalTitle', 'sortTitle') OR NEW.scope LIKE 'alternativeTitle:%')
    EXECUTE FUNCTION search_mark_stale();
CREATE TRIGGER search_stale_override_ad AFTER DELETE ON media_override
    FOR EACH ROW
    WHEN (OLD.field IN ('title', 'originalTitle', 'sortTitle') OR OLD.scope LIKE 'alternativeTitle:%')
    EXECUTE FUNCTION search_mark_stale();
CREATE TRIGGER search_stale_override_au AFTER UPDATE ON media_override
    FOR EACH ROW
    WHEN (NEW.field IN ('title', 'originalTitle', 'sortTitle') OR NEW.scope LIKE 'alternativeTitle:%'
          OR OLD.field IN ('title', 'originalTitle', 'sortTitle') OR OLD.scope LIKE 'alternativeTitle:%')
    EXECUTE FUNCTION search_mark_stale();

-- And the work's own names, year and popularity, wherever they are written:
-- marked on the row as it is written, which needs no second write of it.
CREATE FUNCTION search_mark_item_stale() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.search_stale := 1;
    RETURN NEW;
END;
$$;

CREATE TRIGGER search_stale_item_au
    BEFORE UPDATE OF kind, title, sort_title, original_title, original_language, year, popularity
    ON media_item
    FOR EACH ROW EXECUTE FUNCTION search_mark_item_stale();

-- The indexes of 0022, which the search no longer reads: the titles where
-- they are kept, matched with `LIKE` and unranked.
DROP INDEX IF EXISTS idx_media_item_title_trgm;
DROP INDEX IF EXISTS idx_media_item_sort_title_trgm;
DROP INDEX IF EXISTS idx_media_item_slug_trgm;
DROP INDEX IF EXISTS idx_media_alternative_title_trgm;
DROP INDEX IF EXISTS idx_media_override_value_trgm;

-- A lock in one language: the title or the overview a person wrote for the
-- readers of one language, beside the text every other one is given. The
-- language is the key translations are filed under — `fra`, `jpn` — and
-- empty for a lock in every language, which every lock held so far is.
--
-- Empty rather than NULL. A key over a column that may be NULL keeps any
-- number of rows that differ only by their NULLs, on SQLite as on PostgreSQL
-- before 15, so two locks of one field in every language could both be kept
-- and `ON CONFLICT` would never meet the first. An index over
-- `COALESCE(language, '')` would close that too, but every write would have
-- to name the expression to conflict on, and the column would say one thing
-- two ways. Empty is one value: the primary key holds it on both engines,
-- and a write conflicts on the four columns.
--
-- SQLite does not change a table's key in place, so the table is built
-- again, under its own name: its rows held aside, the table dropped and made
-- anew, the rows put back. Not built beside it and renamed: a rename reads
-- every trigger of the schema again, and the search index's triggers on the
-- works and their titles name this table, which is gone at that moment. Its
-- index and the search index's triggers on it go with the old table and are
-- put back as 0022 made them.
CREATE TEMP TABLE media_override_held AS SELECT * FROM media_override;

DROP TABLE media_override;

CREATE TABLE media_override (
    media_id    TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    scope       TEXT NOT NULL DEFAULT 'item',
    field       TEXT NOT NULL,
    language    TEXT NOT NULL DEFAULT '',
    value       TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    updated_by  TEXT,
    PRIMARY KEY (media_id, scope, field, language)
);

INSERT INTO media_override (media_id, scope, field, language, value, created_at, updated_at, updated_by)
SELECT media_id, scope, field, '', value, created_at, updated_at, updated_by FROM media_override_held;

DROP TABLE media_override_held;

CREATE INDEX ix_override_media ON media_override (media_id);

CREATE TRIGGER IF NOT EXISTS media_search_override_ai AFTER INSERT ON media_override
WHEN NEW.field IN ('title', 'sortTitle', 'originalTitle') BEGIN
    DELETE FROM media_search WHERE media_id = NEW.media_id;
    INSERT INTO media_search(media_id, text)
    SELECT i.id,
           i.title || ' ' || COALESCE(i.sort_title, '') || ' ' || i.slug || ' '
           || COALESCE((SELECT group_concat(t.title, ' ') FROM media_alternative_title t WHERE t.media_id = i.id), '')
           || ' '
           || COALESCE((SELECT group_concat(o.value, ' ') FROM media_override o
                        WHERE o.media_id = i.id AND o.field IN ('title', 'sortTitle', 'originalTitle')), '')
    FROM media_item i WHERE i.id = NEW.media_id;
END;

CREATE TRIGGER IF NOT EXISTS media_search_override_au AFTER UPDATE ON media_override
WHEN NEW.field IN ('title', 'sortTitle', 'originalTitle') OR OLD.field IN ('title', 'sortTitle', 'originalTitle') BEGIN
    DELETE FROM media_search WHERE media_id = NEW.media_id;
    INSERT INTO media_search(media_id, text)
    SELECT i.id,
           i.title || ' ' || COALESCE(i.sort_title, '') || ' ' || i.slug || ' '
           || COALESCE((SELECT group_concat(t.title, ' ') FROM media_alternative_title t WHERE t.media_id = i.id), '')
           || ' '
           || COALESCE((SELECT group_concat(o.value, ' ') FROM media_override o
                        WHERE o.media_id = i.id AND o.field IN ('title', 'sortTitle', 'originalTitle')), '')
    FROM media_item i WHERE i.id = NEW.media_id;
END;

CREATE TRIGGER IF NOT EXISTS media_search_override_ad AFTER DELETE ON media_override
WHEN OLD.field IN ('title', 'sortTitle', 'originalTitle') BEGIN
    DELETE FROM media_search WHERE media_id = OLD.media_id;
    INSERT INTO media_search(media_id, text)
    SELECT i.id,
           i.title || ' ' || COALESCE(i.sort_title, '') || ' ' || i.slug || ' '
           || COALESCE((SELECT group_concat(t.title, ' ') FROM media_alternative_title t WHERE t.media_id = i.id), '')
           || ' '
           || COALESCE((SELECT group_concat(o.value, ' ') FROM media_override o
                        WHERE o.media_id = i.id AND o.field IN ('title', 'sortTitle', 'originalTitle')), '')
    FROM media_item i WHERE i.id = OLD.media_id;
END;

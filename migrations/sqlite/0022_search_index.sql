-- A search index over every title a work goes by, so a title search reads an
-- index rather than every row. Trigrams, so it finds a term inside a word as
-- `LIKE '%term%'` did, folded of case and of accents, so `ete` finds `Été`.
--
-- Rows are found by the work's own id, never by rowid: media_item has a text
-- key, so its rowids are not stable across a VACUUM.
CREATE VIRTUAL TABLE IF NOT EXISTS media_search USING fts5(
    media_id UNINDEXED,
    text,
    tokenize = 'trigram remove_diacritics 1'
);

INSERT INTO media_search(media_id, text)
SELECT i.id,
       i.title || ' ' || COALESCE(i.sort_title, '') || ' ' || i.slug || ' '
       || COALESCE((SELECT group_concat(t.title, ' ') FROM media_alternative_title t WHERE t.media_id = i.id), '')
       || ' '
       || COALESCE((SELECT group_concat(o.value, ' ') FROM media_override o
                    WHERE o.media_id = i.id AND o.field IN ('title', 'sortTitle', 'originalTitle')), '')
FROM media_item i;

-- Kept current by the tables it reads: a work, an alternative title or an
-- override written is that work's text built again.
CREATE TRIGGER IF NOT EXISTS media_search_item_ai AFTER INSERT ON media_item BEGIN
    DELETE FROM media_search WHERE media_id = NEW.id;
    INSERT INTO media_search(media_id, text)
    SELECT i.id,
           i.title || ' ' || COALESCE(i.sort_title, '') || ' ' || i.slug || ' '
           || COALESCE((SELECT group_concat(t.title, ' ') FROM media_alternative_title t WHERE t.media_id = i.id), '')
           || ' '
           || COALESCE((SELECT group_concat(o.value, ' ') FROM media_override o
                        WHERE o.media_id = i.id AND o.field IN ('title', 'sortTitle', 'originalTitle')), '')
    FROM media_item i WHERE i.id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS media_search_item_au AFTER UPDATE OF title, sort_title, slug ON media_item BEGIN
    DELETE FROM media_search WHERE media_id = OLD.id;
    INSERT INTO media_search(media_id, text)
    SELECT i.id,
           i.title || ' ' || COALESCE(i.sort_title, '') || ' ' || i.slug || ' '
           || COALESCE((SELECT group_concat(t.title, ' ') FROM media_alternative_title t WHERE t.media_id = i.id), '')
           || ' '
           || COALESCE((SELECT group_concat(o.value, ' ') FROM media_override o
                        WHERE o.media_id = i.id AND o.field IN ('title', 'sortTitle', 'originalTitle')), '')
    FROM media_item i WHERE i.id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS media_search_item_ad AFTER DELETE ON media_item BEGIN
    DELETE FROM media_search WHERE media_id = OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS media_search_alt_ai AFTER INSERT ON media_alternative_title BEGIN
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

CREATE TRIGGER IF NOT EXISTS media_search_alt_au AFTER UPDATE ON media_alternative_title BEGIN
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

CREATE TRIGGER IF NOT EXISTS media_search_alt_ad AFTER DELETE ON media_alternative_title BEGIN
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

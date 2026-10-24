-- Two records of one work, folded into one.
--
-- Seeding from Sonarr and Radarr, imports by id and works entered by hand
-- leave the same production here twice: under two TMDB ids, as an entry
-- typed in and the import that followed it. A merge keeps one of them — the
-- survivor — and moves what the other held onto it; the other stays, as a
-- tombstone that names the survivor (`merged_into`), switched off and never
-- refreshed, hidden from every list, with what the merge did to both kept
-- beside it (`merge_record`, JSON) for as long as the merge can be undone.
--
-- `merged_into` is no foreign key: a copy of the table into another database
-- writes its rows in the order it reads them, and a tombstone read before its
-- survivor would be refused there.
ALTER TABLE media_item ADD COLUMN merged_into TEXT;
ALTER TABLE media_item ADD COLUMN merged_at TEXT;
ALTER TABLE media_item ADD COLUMN merge_record TEXT;

CREATE INDEX ix_media_item_merged ON media_item (merged_into) WHERE merged_into IS NOT NULL;

-- What a tombstone was known by, answered by its survivor: its own id
-- (`id`), its address (`slug`), and every identifier it had elsewhere, each
-- under the source `media_external_id` files it under. Clients keep the ids
-- they were handed — Sonarr TheTVDB's, Radarr TMDB's, Plex its rating key,
-- a reader's bookmark the site's address — so these are kept for ever: an
-- undo takes back those of its merge, and nothing else does. `merged_id` is
-- the tombstone the alias came from; the survivor going takes them with it.
CREATE TABLE media_alias (
    source      TEXT NOT NULL,
    value       TEXT NOT NULL,
    media_id    TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    merged_id   TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (source, value)
);

CREATE INDEX ix_media_alias_media ON media_alias (media_id);
CREATE INDEX ix_media_alias_merged ON media_alias (merged_id);

-- Two works that look like one, as the server last looked: under an
-- identifier both are filed under (`certain`), under one title within a year
-- (`probable`), or a film and a series under one IMDb id (`flag`, never
-- merged). Written again each time the duplicates are looked for, but for a
-- pair somebody said is not one: that row stays, so that it is not proposed
-- again. `first_id` sorts before `second_id`.
CREATE TABLE media_duplicate (
    first_id      TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    second_id     TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    confidence    TEXT NOT NULL,
    -- sharedId, title or imdbAcrossKinds.
    reason        TEXT NOT NULL,
    -- What they share: `imdb:tt0903747`, or the title as it was folded.
    detail        TEXT,
    found_at      TEXT NOT NULL,
    dismissed_at  TEXT,
    dismissed_by  TEXT,
    PRIMARY KEY (first_id, second_id)
);

CREATE INDEX ix_media_duplicate_second ON media_duplicate (second_id);

-- The identifiers each work was last said to have, read from its journal
-- when the duplicates are looked for: a refresh that takes an id from
-- another work leaves only the taker holding it, and its journal is where
-- the other's claim is still written.
CREATE INDEX ix_change_ids ON media_change (media_id, at) WHERE scope = 'item' AND field = 'externalIds';

-- The works a provider files beside one: AniList's relations, for now — what
-- an entry is the sequel, prequel, side story or spin-off of. Kept as AniList
-- names, dates and pictures them, so a related work is shown whether or not
-- the catalogue holds it; the one held is found again by its id when read.
CREATE TABLE media_relation (
    id             TEXT    PRIMARY KEY,
    media_id       TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    -- SEQUEL, PREQUEL, PARENT, SIDE_STORY, SPIN_OFF, ALTERNATIVE, SUMMARY,
    -- COMPILATION, CONTAINS, SOURCE, ADAPTATION, CHARACTER or OTHER.
    relation_type  TEXT    NOT NULL,
    -- Where the other work is filed, and its id there.
    source         TEXT    NOT NULL DEFAULT 'anilist',
    external_id    BIGINT  NOT NULL,
    mal_id         BIGINT,
    title          TEXT    NOT NULL,
    -- 'anime' or 'manga'.
    medium         TEXT    NOT NULL DEFAULT 'anime',
    -- TV, MOVIE, OVA, ONA, SPECIAL, MANGA, …
    format         TEXT,
    year           INTEGER,
    image          TEXT,
    -- As AniList flags the entry.
    is_adult       INTEGER NOT NULL DEFAULT 0,
    sort_order     INTEGER NOT NULL DEFAULT 0,
    is_manual      INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT    NOT NULL
);

CREATE INDEX ix_relation_media ON media_relation (media_id, sort_order);
CREATE INDEX ix_relation_entry ON media_relation (source, external_id);

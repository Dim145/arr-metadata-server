-- Curated lists: named selections of works, composed by hand in an order or
-- by a filter kept and evaluated as the list is read. Shown on the site as
-- the catalogue's own selections, and served to Sonarr and Radarr in the
-- shape their "custom list" import lists read — so a selection made here
-- becomes what those clients add on their own.
CREATE TABLE curated_list (
    id           TEXT    PRIMARY KEY,
    -- What the list is addressed by, derived from its name.
    slug         TEXT    NOT NULL,
    name         TEXT    NOT NULL,
    description  TEXT,
    -- 'series', 'movie' or 'mixed': what it holds, and so which client reads it.
    kind         TEXT    NOT NULL DEFAULT 'mixed',
    -- 'manual': the members below, in their order. 'filter': the query below.
    mode         TEXT    NOT NULL DEFAULT 'manual',
    -- The filter as JSON, in the native list query's own vocabulary.
    filter_json  TEXT,
    -- Whether a reader with no credential may see it, where public browsing
    -- is on at all; a private list is the maintainer's draft.
    is_public    INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT    NOT NULL,
    updated_at   TEXT    NOT NULL
);

CREATE UNIQUE INDEX ux_curated_list_slug ON curated_list (slug);

CREATE TABLE curated_list_item (
    list_id   TEXT    NOT NULL REFERENCES curated_list (id) ON DELETE CASCADE,
    media_id  TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    added_at  TEXT    NOT NULL,
    PRIMARY KEY (list_id, media_id)
);

CREATE INDEX ix_curated_list_item_media ON curated_list_item (media_id);

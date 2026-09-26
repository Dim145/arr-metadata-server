-- Fankai's productions, filed under an id of their own.
--
-- A Fan-Kai is an anime recut into films, and neither TheTVDB nor TMDB lists
-- one: the only id such a work has is Fankai's. The list of sources an
-- external id may come from is a CHECK on the table, and SQLite cannot change
-- a CHECK in place, so the table is rebuilt around the same rows, under the
-- same name — every other table that names it goes on naming it.
ALTER TABLE media_external_id RENAME TO media_external_id_old;
DROP INDEX IF EXISTS ix_media_external_id_media;

CREATE TABLE media_external_id (
    media_id    TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    source      TEXT NOT NULL CHECK (source IN (
                    'tmdb_movie', 'tmdb_tv', 'tvdb_series', 'tvdb_movie',
                    'imdb', 'tvmaze', 'tvrage', 'mal', 'anilist',
                    'trakt_show', 'trakt_movie', 'fankai'
                )),
    value       TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (source, value)
);

INSERT INTO media_external_id (media_id, source, value, created_at)
    SELECT media_id, source, value, created_at FROM media_external_id_old;

DROP TABLE media_external_id_old;

CREATE INDEX ix_media_external_id_media ON media_external_id (media_id);

-- Fankai's productions, filed under an id of their own.
--
-- A Fan-Kai is an anime recut into films, and neither TheTVDB nor TMDB lists
-- one: the only id such a work has is Fankai's. The list of sources an
-- external id may come from is a CHECK on the table; PostgreSQL swaps it for
-- one that knows the new source.
ALTER TABLE media_external_id DROP CONSTRAINT media_external_id_source_check;
ALTER TABLE media_external_id ADD CONSTRAINT media_external_id_source_check CHECK (source IN (
    'tmdb_movie', 'tmdb_tv', 'tvdb_series', 'tvdb_movie',
    'imdb', 'tvmaze', 'tvrage', 'mal', 'anilist',
    'trakt_show', 'trakt_movie', 'fankai'
));

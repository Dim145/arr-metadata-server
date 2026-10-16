-- What each country was given of a work, and what TMDB and AniList sent that
-- was dropped.
--
-- A work kept one release date of each kind and one rating, the American
-- block's wherever TMDB had one: a reader in France was given the day a film
-- came out in the United States, and Radarr the American rating whatever
-- country it was set to. Every country is kept now, a row a release — the
-- country, how the work came out there, its day, its rating, TMDB's note —
-- and a row for a rating given on its own, a series' or Radarr's, with
-- neither kind nor day. Wholly the providers': a refresh replaces them, as
-- it does the ratings.
--
-- `country` is ISO 3166-1 alpha-2 in upper case; `release_type` one of
-- premiere, limited, theatrical, digital, physical and tv, as TMDB tells its
-- releases apart; `release_date` a day, YYYY-MM-DD. Filled in as each work is
-- next refreshed.
CREATE TABLE media_release (
    id             TEXT    PRIMARY KEY,
    media_id       TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    country        TEXT    NOT NULL,
    release_type   TEXT,
    release_date   TEXT,
    certification  TEXT,
    note           TEXT,
    -- The provider it came from: tmdb, tvdb, radarr.
    source         TEXT,
    sort_order     INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX ix_release_media ON media_release (media_id, sort_order);
-- The schedule looks a country's days up.
CREATE INDEX ix_release_country_day ON media_release (country, release_date);

-- The line under the title, as TMDB gives it.
ALTER TABLE media_item ADD COLUMN tagline TEXT;
-- What AniList calls the entry: TV, TV_SHORT, MOVIE, SPECIAL, OVA, ONA, MUSIC.
ALTER TABLE media_item ADD COLUMN format TEXT;
-- Every country it was made in and every company that made it, as JSON
-- lists like the genres: the first of each was all that was kept.
ALTER TABLE media_item ADD COLUMN production_countries TEXT NOT NULL DEFAULT '[]';
ALTER TABLE media_item ADD COLUMN production_companies TEXT NOT NULL DEFAULT '[]';

-- Wikidata's item, which TMDB files beside its own ids. The list of sources
-- an external id may come from is a CHECK on the table; PostgreSQL swaps it
-- for one that knows the new source.
ALTER TABLE media_external_id DROP CONSTRAINT media_external_id_source_check;
ALTER TABLE media_external_id ADD CONSTRAINT media_external_id_source_check CHECK (source IN (
    'tmdb_movie', 'tmdb_tv', 'tvdb_series', 'tvdb_movie',
    'imdb', 'tvmaze', 'tvrage', 'mal', 'anilist',
    'trakt_show', 'trakt_movie', 'fankai', 'wikidata'
));

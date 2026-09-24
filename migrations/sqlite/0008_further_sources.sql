-- What the further sources need kept between requests.
--
-- None of these is fetched per request. Each is a list downloaded whole, on a
-- schedule, and asked locally — which is what makes the sources behind them
-- cheap to use and polite to their owners.

-- Which AniList and MyAnimeList entries a TheTVDB series or a TMDB film is.
--
-- From the Fribb anime-lists project, refreshed weekly. MyAnimeList and AniList
-- split a series into one entry per season or cour, so one TheTVDB id maps to
-- several rows here, each with the TheTVDB season it corresponds to and the
-- episode offset into it. That is what lets anime be looked up by identifier,
-- never guessed at by title — and what keeps a site that numbers by cour from
-- ever touching the numbering Sonarr is given.
CREATE TABLE anime_mapping (
    mal_id       INTEGER,
    anilist_id   INTEGER,
    tvdb_id      INTEGER,
    tvdb_season  INTEGER,
    tvdb_offset  INTEGER,
    tmdb_movie   INTEGER,
    kind         TEXT
);

CREATE INDEX ix_anime_mapping_tvdb ON anime_mapping (tvdb_id);
CREATE INDEX ix_anime_mapping_tmdb ON anime_mapping (tmdb_movie);
-- Sonarr's AniList and MyAnimeList import lists search by these.
CREATE INDEX ix_anime_mapping_mal ON anime_mapping (mal_id);
CREATE INDEX ix_anime_mapping_anilist ON anime_mapping (anilist_id);

-- IMDb's ratings, for the works this catalogue holds.
--
-- From IMDb's non-commercial datasets, refreshed daily. Filtered on import to
-- the works stored here rather than kept whole: the full list is a million and
-- a half rows for the few thousand that matter.
CREATE TABLE imdb_rating (
    tconst  TEXT PRIMARY KEY,
    rating  REAL NOT NULL,
    votes   INTEGER NOT NULL
);

-- TheTVDB's popularity figure, which was stored as a rating under `tvdb`: 3 776 757
-- for Breaking Bad, on no scale a rating is read on. It is no longer taken, and
-- it held the `tvdb` slot Skyhook's rating was filed under too, so that is gone
-- as well; the next refresh brings Skyhook's back, filed as the IMDb rating it
-- is.
DELETE FROM media_rating WHERE source = 'tvdb';

-- The IMDb catch-up asks, every minute, whether a work was added since its
-- last import.
CREATE INDEX ix_media_item_created ON media_item (created_at);

-- When each downloaded list was last brought in, so a restart does not fetch
-- them all again and the scheduler knows when one is due.
CREATE TABLE data_import (
    name         TEXT PRIMARY KEY,
    imported_at  TEXT NOT NULL,
    row_count    INTEGER NOT NULL
);

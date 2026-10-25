-- What two more of IMDb's non-commercial datasets give the works held here.
-- Each is off until switched on (`imdb.akas`, `imdb.episodes`), downloaded
-- weekly, read a line at a time and kept only for the works this catalogue
-- holds: IMDb lists twelve million titles for the few thousand that matter.

-- The titles IMDb gives a work in each region, from `title.akas`: only the
-- rows of the types `imdbDisplay`, `original` and `alternative`. Radarr is
-- given those of the regions `imdb.regions` names among a film's alternative
-- titles. They are kept apart from the work's own alternative titles so that
-- nothing else is given them: not Sonarr, for which each alias would cost a
-- search; not the search, the exports or the feeds.
CREATE TABLE imdb_title (
    tconst       TEXT    NOT NULL,
    -- IMDb's number for the row, unique within a title.
    ordering     INTEGER NOT NULL,
    title        TEXT    NOT NULL,
    -- ISO 3166-1, as IMDb writes it — or a code of IMDb's own, `XWW` for the
    -- world in English. None for the title the work was first released under.
    region       TEXT,
    language     TEXT,
    -- Which of `imdbDisplay`, `original` and `alternative` the row is, comma
    -- separated.
    types        TEXT    NOT NULL,
    is_original  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tconst, ordering)
);

-- The episodes IMDb lists of the series held here, from `title.episode`, by
-- the season and the number IMDb gives them; an episode IMDb has not
-- numbered is left out, since nothing could place it. An episode of the
-- catalogue takes the IMDb id of the one IMDb numbers as it is numbered in
-- the official order — which may be wrong where IMDb counts otherwise, and
-- is said to be less sure for it.
CREATE TABLE imdb_episode (
    tconst          TEXT    PRIMARY KEY,
    parent_tconst   TEXT    NOT NULL,
    season_number   INTEGER NOT NULL,
    episode_number  INTEGER NOT NULL
);

CREATE INDEX ix_imdb_episode_place ON imdb_episode (parent_tconst, season_number, episode_number);

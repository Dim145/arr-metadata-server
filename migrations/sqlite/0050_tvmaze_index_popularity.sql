-- TVmaze's index of every series it lists, kept by each instance once
-- `tvmaze.index` is on, and what the popularity of the works held here is
-- worked out from: TVmaze's weight, and how fast IMDb's list sees votes come.

-- The series TVmaze lists, as its index gives them: read whole the first time,
-- a page of two hundred and fifty ids at a time, then once a week the series
-- TVmaze says changed. What names a series and places it, and nothing more —
-- no synopsis, no picture: what a search answers with when the providers do
-- not answer, and the weight the popularity reads.
CREATE TABLE tvmaze_show (
    -- TVmaze's id.
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL,
    -- The name folded as the search index folds a name (`search::norm`): a
    -- term is looked for at its start, and at the start of its words.
    norm        TEXT    NOT NULL,
    -- TVmaze's `type`: `Scripted`, `Animation`, `Reality`, `Documentary`…
    kind        TEXT,
    -- Named in English, as TVmaze writes it: `English`, `Japanese`.
    language    TEXT,
    -- `Running`, `Ended`, `To Be Determined`, `In Development`.
    status      TEXT,
    -- When it first aired, and when it last did: `2008-01-20`.
    premiered   TEXT,
    ended       TEXT,
    -- Its network, else the web channel it streams on, and that one's
    -- country, ISO 3166-1 alpha-2; `web` says which of the two it is.
    network     TEXT,
    country     TEXT,
    web         INTEGER NOT NULL DEFAULT 0,
    -- Its ids elsewhere, as TVmaze files them.
    tvdb_id     INTEGER,
    imdb_id     TEXT,
    tvrage_id   INTEGER,
    -- How popular TVmaze says it is, from 0 to 100.
    weight      INTEGER,
    -- When TVmaze last changed it, in seconds since 1970: what TVmaze's list
    -- of the series it changed is compared with.
    updated     INTEGER
);

CREATE INDEX ix_tvmaze_show_norm ON tvmaze_show (norm);
CREATE INDEX ix_tvmaze_show_tvdb ON tvmaze_show (tvdb_id) WHERE tvdb_id IS NOT NULL;
CREATE INDEX ix_tvmaze_show_imdb ON tvmaze_show (imdb_id) WHERE imdb_id IS NOT NULL;

-- How many votes IMDb's list gave each work held here, by the day it was read:
-- the list replaces `imdb_rating` whole each day, and the votes a work gathers
-- a day are what its popularity is read from. A week and a day of them, for
-- the works held alone.
CREATE TABLE imdb_votes (
    tconst  TEXT    NOT NULL,
    -- The day the list was read, `2026-11-02`, in UTC.
    day     TEXT    NOT NULL,
    votes   INTEGER NOT NULL,
    PRIMARY KEY (tconst, day)
);

-- What each work's popularity was last worked out from: TVmaze's weight — the
-- index's, else the one in TVmaze's own answer for the series — and the votes
-- IMDb's list gathered a day, on average over the days kept.
CREATE TABLE media_popularity (
    media_id       TEXT    PRIMARY KEY REFERENCES media_item (id) ON DELETE CASCADE,
    -- The weight the figure was worked out from, and where it was read:
    -- 'index' or 'answer'.
    weight         INTEGER,
    weight_from    TEXT    CHECK (weight_from IN ('index', 'answer')),
    -- The weight TVmaze's own answer for the series gave, and when that answer
    -- was fetched: read again only once a refresh has fetched a newer one,
    -- since a long series' answer runs to megabytes.
    answer_weight  INTEGER,
    answer_at      TEXT,
    votes_per_day  REAL,
    worked_at      TEXT    NOT NULL
);

-- The popularity worked out here, on TMDB's scale. A work's own — TMDB's —
-- comes first wherever it has one; this one stands in where it has none, a
-- work entered by hand, or every work of a catalogue TMDB does not serve. The
-- lists are in the order of whichever the work has: their two indexes take
-- the place of `ix_media_item_browse` and `ix_media_item_popular`, which were
-- for TMDB's alone and which no list reads any more.
ALTER TABLE media_item ADD COLUMN local_popularity REAL;

DROP INDEX ix_media_item_browse;
DROP INDEX ix_media_item_popular;

CREATE INDEX ix_media_item_browse_known
    ON media_item (kind, is_enabled, COALESCE(popularity, local_popularity) DESC, title, is_adult);

CREATE INDEX ix_media_item_known
    ON media_item (COALESCE(popularity, local_popularity) DESC, title);

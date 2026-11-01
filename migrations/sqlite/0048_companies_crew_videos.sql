-- What the providers' answers carried all along and the model let go: the
-- companies, networks and keywords as things of their own, with their ids;
-- the crew whole, each by its department and its job; what TMDB says of its
-- pictures beyond their addresses; every video, not one trailer; the seasons'
-- titles and synopses in other languages; and somebody's biography in more
-- than one. Read from the answers a refresh already asks for — the season
-- documents and a person's record ask TMDB for their translations besides —
-- and kept as the rest is: what a refresh gives replaces what it gave before.

-- The department a credit's job is in and the job itself, as TMDB files them:
-- `Sound` and `Original Music Composer`. The job stays in `character_name`
-- too for every kind of credit, where its readers have always found it.
ALTER TABLE media_credit ADD COLUMN department TEXT;
ALTER TABLE media_credit ADD COLUMN job TEXT;

-- A picture's size in pixels and what TMDB's members made of it, where its
-- source says: TMDB ranks its pictures by these.
ALTER TABLE media_image ADD COLUMN width INTEGER;
ALTER TABLE media_image ADD COLUMN height INTEGER;
ALTER TABLE media_image ADD COLUMN vote_average REAL;
ALTER TABLE media_image ADD COLUMN vote_count INTEGER;

-- A network, or a company that makes or sells works, once for every work it
-- is credited on. TMDB numbers its networks and its companies apart — its
-- network 174 is AMC, its company 174 Warner Bros. Pictures — so a kind is
-- part of what an id names; TheTVDB numbers both alike, and its ids are kept
-- the same way. One of each source's ids at most for each kind, and the same
-- name of the same kind is one: TMDB's AMC and TheTVDB's are one network.
CREATE TABLE company (
    id          TEXT    PRIMARY KEY,
    kind        TEXT    NOT NULL CHECK (kind IN ('network', 'company')),
    name        TEXT    NOT NULL,
    -- The name folded as it is matched: trimmed, in lower case.
    name_key    TEXT    NOT NULL,
    tmdb_id     INTEGER,
    tvdb_id     INTEGER,
    -- Its logo, an address as its source gives it, like an image's.
    logo        TEXT,
    -- ISO 3166-1 alpha-2, upper case.
    country     TEXT,
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL
);

CREATE UNIQUE INDEX ux_company_tmdb ON company (kind, tmdb_id);
CREATE UNIQUE INDEX ux_company_tvdb ON company (kind, tvdb_id);
CREATE INDEX ix_company_name ON company (kind, name_key);

-- What a company did for a work — aired it, made it, animated it, sold it —
-- in the order its source lists them, and the source whose list it is.
CREATE TABLE media_company (
    media_id    TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    company_id  TEXT    NOT NULL REFERENCES company (id) ON DELETE CASCADE,
    role        TEXT    NOT NULL CHECK (role IN ('network', 'production', 'studio', 'distributor')),
    sort_order  INTEGER NOT NULL DEFAULT 0,
    source      TEXT,
    PRIMARY KEY (media_id, company_id, role)
);

CREATE INDEX ix_media_company_company ON media_company (company_id, role);

-- A keyword, once for every work it is given: TMDB's, by its id. The work's
-- own list of keywords, every source's texts, stays as it was.
CREATE TABLE keyword (
    id          TEXT    PRIMARY KEY,
    tmdb_id     INTEGER,
    name        TEXT    NOT NULL,
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL
);

CREATE UNIQUE INDEX ux_keyword_tmdb ON keyword (tmdb_id);

CREATE TABLE media_keyword (
    media_id    TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    keyword_id  TEXT    NOT NULL REFERENCES keyword (id) ON DELETE CASCADE,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    source      TEXT,
    PRIMARY KEY (media_id, keyword_id)
);

CREATE INDEX ix_media_keyword_keyword ON media_keyword (keyword_id);

-- Every video a source lists for a work: where it is (`YouTube`, `Vimeo`)
-- and its key there, what it is (`Trailer`, `Teaser`, `Clip`), its language
-- and country, whether the work's makers put it out. The work's trailer stays
-- the one chosen of them, in `media_item.trailer_youtube_id`.
CREATE TABLE media_video (
    id            TEXT    PRIMARY KEY,
    media_id      TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    site          TEXT    NOT NULL,
    video_key     TEXT    NOT NULL,
    name          TEXT,
    video_type    TEXT,
    -- ISO 639-3, as the rest of the model.
    language      TEXT,
    -- ISO 3166-1 alpha-2, upper case.
    country       TEXT,
    official      INTEGER NOT NULL DEFAULT 0,
    published_at  TEXT,
    -- Its height in lines: 1080, 2160.
    size          INTEGER,
    -- TMDB's id for the video itself.
    tmdb_id       TEXT,
    source        TEXT,
    sort_order    INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL
);

CREATE UNIQUE INDEX ux_media_video ON media_video (media_id, site, video_key);

-- A season's title and synopsis in a language, by the season's number, as
-- TMDB's season documents give them. ISO 639-3, as `media_translation`.
CREATE TABLE media_season_translation (
    media_id       TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    season_number  INTEGER NOT NULL,
    language       TEXT    NOT NULL,
    title          TEXT,
    overview       TEXT,
    PRIMARY KEY (media_id, season_number, language)
);

-- Somebody's name and biography in a language, as TMDB's record of them
-- gives them, read with it the first time their page is.
CREATE TABLE person_translation (
    person_id  TEXT NOT NULL REFERENCES person (id) ON DELETE CASCADE,
    language   TEXT NOT NULL,
    name       TEXT,
    biography  TEXT,
    PRIMARY KEY (person_id, language)
);

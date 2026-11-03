-- The characters of the works and the collections the films belong to, as
-- things of their own.
--
-- A character was the text a credit carried beside the person's name, and
-- TheTVDB's record of each part was read again from its answer whenever its
-- characters were asked for. A character is now a row, with the ids each
-- source files it under, its picture, what it is to each work it is in — a
-- lead, one beside them — who plays it there, and, for an anime, who voices it
-- in each dub. Read from the answers a refresh already asks for: TheTVDB's
-- series, and AniList's entry, which asks for its characters besides.
--
-- A collection was TMDB's record of it, asked again once a day by whichever
-- page or client wanted it. It is now kept: its name, its synopsis, its
-- pictures and its films, in the catalogue's language and in every other TMDB
-- has its texts in, asked for again when one of its films is refreshed and the
-- record kept is a week old.

-- A character, kept once for every work it is in: found again by the id a
-- source files it under — TheTVDB's record of the part, AniList's,
-- MyAnimeList's — and kept for its work alone where none gives one.
CREATE TABLE character (
    id           TEXT    PRIMARY KEY,
    name         TEXT    NOT NULL,
    -- Their name in the script of the work's own language: フリーレン.
    native_name  TEXT,
    -- Their picture, drawn or the actor in the part: an address as its
    -- source gives it, like an image's.
    image        TEXT,
    tvdb_id      INTEGER,
    anilist_id   INTEGER,
    mal_id       INTEGER,
    created_at   TEXT    NOT NULL,
    updated_at   TEXT    NOT NULL
);

CREATE UNIQUE INDEX ux_character_tvdb ON character (tvdb_id);
CREATE UNIQUE INDEX ux_character_anilist ON character (anilist_id);
CREATE UNIQUE INDEX ux_character_mal ON character (mal_id);

-- What a character is to a work — one of its leads, one beside them, one in
-- the background — where its sources list it, the source whose list it is,
-- and who plays it there: the person a credit of the work is filed under,
-- which the credit is found again by however often the credits are written.
CREATE TABLE media_character (
    media_id      TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    character_id  TEXT    NOT NULL REFERENCES character (id) ON DELETE CASCADE,
    role          TEXT    CHECK (role IN ('main', 'supporting', 'background')),
    sort_order    INTEGER NOT NULL DEFAULT 0,
    person_id     TEXT    REFERENCES person (id) ON DELETE SET NULL,
    source        TEXT,
    PRIMARY KEY (media_id, character_id)
);

CREATE INDEX ix_media_character_character ON media_character (character_id);
CREATE INDEX ix_media_character_person ON media_character (person_id);

-- Who voices a character on a work in a dub: the dub's language, and the
-- voice as the source names them, with the ids it files them under.
CREATE TABLE media_character_voice (
    media_id      TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    character_id  TEXT    NOT NULL REFERENCES character (id) ON DELETE CASCADE,
    -- ISO 639-3, as the rest of the model: jpn, eng, fra.
    language      TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    native_name   TEXT,
    image         TEXT,
    anilist_id    INTEGER,
    mal_id        INTEGER,
    sort_order    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (media_id, character_id, language, name)
);

-- A collection — a saga, a trilogy — as TMDB has it, by its id: the one a
-- film's `collection_tmdb_id` names. Its texts in the catalogue's language,
-- the one it was asked in; its pictures are addresses, like an image's.
CREATE TABLE collection (
    id            TEXT    PRIMARY KEY,
    tmdb_id       INTEGER NOT NULL,
    name          TEXT    NOT NULL,
    overview      TEXT,
    poster        TEXT,
    backdrop      TEXT,
    -- ISO 639-3: the language its name and its synopsis are in.
    language      TEXT,
    -- TMDB's answer itself, as it came — its pictures and its translations
    -- appended: what a document of TMDB's shape is written from where the
    -- columns keep nothing of it. JSON.
    document      TEXT,
    -- When TMDB was last asked about it.
    refreshed_at  TEXT    NOT NULL,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL
);

CREATE UNIQUE INDEX ux_collection_tmdb ON collection (tmdb_id);

-- Its name and synopsis in another language, as TMDB's translations give
-- them. ISO 639-3, as `media_translation`.
CREATE TABLE collection_translation (
    collection_id  TEXT NOT NULL REFERENCES collection (id) ON DELETE CASCADE,
    language       TEXT NOT NULL,
    name           TEXT,
    overview       TEXT,
    PRIMARY KEY (collection_id, language)
);

-- Its films, in TMDB's order, as much of each as a list shows — whether the
-- catalogue holds it or not.
CREATE TABLE collection_part (
    collection_id      TEXT    NOT NULL REFERENCES collection (id) ON DELETE CASCADE,
    tmdb_id            INTEGER NOT NULL,
    title              TEXT    NOT NULL,
    original_title     TEXT,
    overview           TEXT,
    -- As TMDB writes it: YYYY-MM-DD.
    release_date       TEXT,
    -- As TMDB writes it: ISO 639-1.
    original_language  TEXT,
    poster             TEXT,
    backdrop           TEXT,
    popularity         DOUBLE PRECISION,
    vote_average       DOUBLE PRECISION,
    vote_count         INTEGER,
    is_adult           INTEGER NOT NULL DEFAULT 0,
    sort_order         INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (collection_id, tmdb_id)
);

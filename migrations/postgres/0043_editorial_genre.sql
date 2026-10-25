-- Genres as things of their own, and the programmer's note.
--
-- A genre was the word a provider used for it, in the language the server
-- asked in: a catalogue fetched in English and then in French held "Drama"
-- and "Drame" side by side, two genres in every list for one. A genre is now
-- a row, with a slug that stays the same in every language — `drama`,
-- `science-fiction` — and its name in each language it is known in. The
-- works are listed under the slugs (`media_item.listed_genres`, which the
-- server writes again for every work once this is applied), so a filter, a
-- count and a genre's own page find "Drama" and "Drame" as one.
--
-- The rows are filled as works are listed: a genre the server knows is
-- written with its names in English and in French, any other with the name
-- a source gave it by, filed under `und` — undetermined — since nothing says
-- which language that is.
CREATE TABLE genre (
    id          TEXT PRIMARY KEY,
    -- What the pages address it by: `/genre/drama`.
    slug        TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE UNIQUE INDEX ux_genre_slug ON genre (slug);

-- A genre's name in one language: `en`, `fr`, or `und` for the name a source
-- gave it by where nothing says which language that is.
CREATE TABLE genre_name (
    genre_id    TEXT NOT NULL REFERENCES genre (id) ON DELETE CASCADE,
    language    TEXT NOT NULL,
    name        TEXT NOT NULL,
    PRIMARY KEY (genre_id, language)
);

-- The programmer's note: a few lines somebody wrote about a work featured
-- on the front page, a selection, a genre, a network, a year or a decade,
-- in one language. `scope` says which of those the note is about and `key`
-- which one — a work's or a selection's id, a genre's or a network's slug,
-- a year, the first year of a decade. The body is a restricted Markdown —
-- paragraphs, emphasis, links inside the site — checked as it is saved and
-- never read as HTML.
CREATE TABLE editorial (
    scope       TEXT NOT NULL,
    key         TEXT NOT NULL,
    language    TEXT NOT NULL,
    body        TEXT NOT NULL,
    -- Who wrote it last, as the journal names people: `editor:alice`.
    updated_by  TEXT,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (scope, key, language)
);

CREATE INDEX ix_editorial_updated ON editorial (updated_at);

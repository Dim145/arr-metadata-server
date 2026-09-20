-- Per-language episode titles and overviews.
--
-- Item-level translations already live in `media_translation`. Episodes need
-- their own table because there are two orders of magnitude more of them, and
-- because they are fetched separately: TMDB returns every language of a series'
-- own title in one call, but episode titles come one call per season per
-- language, so they are fetched only for languages somebody actually asks for.
--
-- `language` is ISO 639-2/T, matching `media_translation`.

CREATE TABLE media_episode_translation (
    media_id        TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    season_number   INTEGER NOT NULL,
    episode_number  INTEGER NOT NULL,
    language        TEXT    NOT NULL,
    title           TEXT,
    overview        TEXT,
    fetched_at      TEXT    NOT NULL,
    PRIMARY KEY (media_id, season_number, episode_number, language)
);

CREATE INDEX ix_episode_translation_lookup
    ON media_episode_translation (media_id, language);

-- Which languages have been fetched for a work, so a language with genuinely no
-- translations is not refetched on every request.
CREATE TABLE media_language_fetch (
    media_id    TEXT NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    language    TEXT NOT NULL,
    fetched_at  TEXT NOT NULL,
    PRIMARY KEY (media_id, language)
);

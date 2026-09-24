-- What a work is listed by: its values as the catalogue shows them — the
-- providers', with whatever somebody locked in their place — and the score its
-- card leads with, IMDb's daily list included. Filters, orders and the counts
-- beside them read these rather than the providers' own columns, so a work
-- locked to "Animation" is found under Animation, and one IMDb's list rates
-- 6.1 is not kept among the sevens by an older figure.
--
-- The server keeps them: each change to a work, one of its locks or IMDb's
-- list counts in `listed_change`, and the work is listed again until
-- `listed_seen` has caught up. See `service::listing`.
ALTER TABLE media_item ADD COLUMN listed_title TEXT;
ALTER TABLE media_item ADD COLUMN listed_year INTEGER;
ALTER TABLE media_item ADD COLUMN listed_genres TEXT;
ALTER TABLE media_item ADD COLUMN listed_keywords TEXT;
ALTER TABLE media_item ADD COLUMN listed_network TEXT;
ALTER TABLE media_item ADD COLUMN listed_studio TEXT;
ALTER TABLE media_item ADD COLUMN listed_status TEXT;
ALTER TABLE media_item ADD COLUMN listed_language TEXT;
ALTER TABLE media_item ADD COLUMN listed_release TEXT;
ALTER TABLE media_item ADD COLUMN listed_score DOUBLE PRECISION;
-- Whether the score took IMDb's list into account, which follows the switch.
ALTER TABLE media_item ADD COLUMN listed_imdb INTEGER NOT NULL DEFAULT 0;
-- Which way of listing wrote them, so a change to it lists every work again.
ALTER TABLE media_item ADD COLUMN listed_version INTEGER;
ALTER TABLE media_item ADD COLUMN listed_change INTEGER NOT NULL DEFAULT 1;
ALTER TABLE media_item ADD COLUMN listed_seen INTEGER;
ALTER TABLE media_item ADD COLUMN listed_at TEXT;

-- Until the server has listed every work itself, each is listed by what its
-- providers said — what the list read before this — so nothing drops out of a
-- filter in the meantime.
UPDATE media_item SET
    listed_title = LOWER(COALESCE(sort_title, title)),
    listed_year = year,
    listed_genres = genres,
    listed_keywords = keywords,
    listed_network = network,
    listed_studio = studio,
    listed_status = status,
    listed_language = original_language,
    listed_release = COALESCE(first_aired, in_cinemas, digital_release, physical_release, CAST(year AS TEXT)),
    listed_score = COALESCE(
        (SELECT r.value FROM media_rating r
          WHERE r.media_id = media_item.id AND r.source = 'imdb'
            AND r.value > 0 AND r.value <= 10),
        (SELECT r.value FROM media_rating r
          WHERE r.media_id = media_item.id AND r.value > 0 AND r.value <= 10
          ORDER BY COALESCE(r.votes, 0) DESC LIMIT 1));

-- "Top rated" is walked in order, as "most popular" is, rather than scored
-- with two lookups per work and sorted: a series or film tab by the first, like
-- `ix_media_item_browse`, and every kind at once by the second.
-- PostgreSQL puts NULLs first in a descending index unless told otherwise, and
-- the list asks for them last; see 0007.
CREATE INDEX ix_media_item_rated
    ON media_item (kind, is_enabled, listed_score DESC NULLS LAST, title, is_adult);

CREATE INDEX ix_media_item_listed_score
    ON media_item (listed_score DESC NULLS LAST, title);

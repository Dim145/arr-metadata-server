-- Wikidata's links between the sites' ids: which TMDB film or series,
-- TheTVDB series or film, IMDb title, AniList, MyAnimeList or AniDB entry
-- and TVmaze show each of its items is. Read from Wikidata's query service
-- while `wikidata.enabled` is on, weekly, one query a site, and kept whole:
-- an id of one site's leads to the item it is on, and the item to its other
-- ids, without asking anybody. TMDB's `/3/find`, TheTVDB's `search/remoteid`
-- and Sonarr's `imdb:` and `tmdb:` terms turn to it where the providers do
-- not answer, or where the work held here does not carry the id asked; it
-- only ever finds ids, never a work.
--
-- A row a link, an id on an item, all of it numbers: a million links on
-- 360 000 items in November 2026, which as text would take half as much
-- again.
--   property  the property Wikidata files the site's ids under: 4947 TMDB
--             film, 4983 TMDB series, 4835 TheTVDB series, 12196 TheTVDB
--             film, 345 IMDb, 8729 AniList, 4086 MyAnimeList, 5646 AniDB,
--             8600 TVmaze;
--   value     the id: IMDb's without its `tt`, written back with seven
--             digits at least, as IMDb writes it;
--   item      Wikidata's item, without its `Q`.
-- One id may be on several items — Wikidata has its duplicates — and one
-- item may carry several ids of a site. What a reading changes is written
-- once every query has been answered, so a reading that fails leaves the
-- links as they were; in one transaction, but for a change of more than
-- 25 000 links on SQLite — the first reading's million — which goes in
-- several, so as not to keep its other writers waiting (`db::repo::wikidata`).
--
-- Without a rowid on SQLite, the key being the row: 36 MB for the million
-- links of November 2026, where a rowid made it 56.
CREATE TABLE wikidata_link (
    property  INTEGER NOT NULL,
    value     INTEGER NOT NULL,
    item      INTEGER NOT NULL,
    PRIMARY KEY (property, value, item)
) WITHOUT ROWID;

-- From the item an id is on to the item's other ids.
CREATE INDEX ix_wikidata_link_item ON wikidata_link (item);

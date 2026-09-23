-- Indexes for the lists: the catalogue, a browse page, a count beside it.
--
-- Every list is ordered by popularity and then title, and nothing indexed that
-- order, so each one sorted every matching row to keep thirty-six of them. At
-- fifty thousand works that was 35–45 ms for a page and as much again for the
-- count shown next to it; with these it is a tenth of a millisecond and one.
--
-- `ix_media_item_browse` answers a list of one kind — which is what the series
-- and film tabs ask for — in order, and counts it without touching the table:
-- `is_adult` rides at the end so a count can read it from the index whether the
-- server shows adult titles or not. `ix_media_item_popular` is the same order
-- for a list of every kind, which is the administration catalogue.
--
-- SQLite sorts NULL below everything, so `popularity DESC` already puts works
-- with no score last — the order the queries ask for with `NULLS LAST`.
--
-- A search by text cannot use either: `LIKE '%term%'` has to read every row.
-- Left to itself the planner walks one of these in order hoping to meet enough
-- matches early, which for a term that matches nothing costs four times the
-- plain scan. `repo::item` tells it not to; see `from_clause` there.

CREATE INDEX ix_media_item_browse ON media_item (kind, is_enabled, popularity DESC, title, is_adult);

CREATE INDEX ix_media_item_popular ON media_item (popularity DESC, title);

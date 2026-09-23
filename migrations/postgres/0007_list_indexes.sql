-- Indexes for the lists: the catalogue, a browse page, a count beside it.
--
-- Every list is ordered by popularity and then title, and nothing indexed that
-- order, so each one sorted every matching row to keep thirty-six of them. At
-- fifty thousand works that was 35–45 ms for a page and as much again for the
-- count shown next to it; with these a page is under half a millisecond.
--
-- `ix_media_item_browse` answers a list of one kind — which is what the series
-- and film tabs ask for — in order, and counts it from the index alone:
-- `is_adult` rides at the end so a count can read it there whether the server
-- shows adult titles or not. `ix_media_item_popular` is the same order for a
-- list of every kind, which is the administration catalogue.
--
-- PostgreSQL sorts NULL first in a descending index unless told otherwise, and
-- the queries ask for `NULLS LAST`, so the index has to say so to be usable.
--
-- A search by text cannot use either: `LIKE '%term%'` has to read every row.
-- Left to itself the planner walks one of these in order hoping to meet enough
-- matches early, which for a term that matches nothing costs half as much again
-- as the plain scan. `repo::item` orders a text search by an expression neither
-- index can serve; see `from_clause` there.

CREATE INDEX ix_media_item_browse
    ON media_item (kind, is_enabled, popularity DESC NULLS LAST, title, is_adult);

CREATE INDEX ix_media_item_popular
    ON media_item (popularity DESC NULLS LAST, title);

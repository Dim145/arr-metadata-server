-- The picture a selection is shown with: on the page of the selections, and
-- at the head of its own. An address a browser can load — a provider's, or an
-- upload's `upload:` origin, which is served from the copy kept — and not the
-- bytes themselves, so a cover is chosen among the pictures the catalogue
-- already holds as often as it is typed. Empty for a list with none.
ALTER TABLE curated_list ADD COLUMN cover TEXT;

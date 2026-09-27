-- Trigram indexes over every title a work goes by, so a title search reads
-- an index rather than every row: the `LOWER(…) LIKE '%term%'` the search
-- runs is what pg_trgm indexes, as long as the expressions match.
--
-- Only where the extension can be had: creating it takes a privilege some
-- hosted databases withhold, and a search that reads every row is slow, not
-- wrong. The notice says which it was.
DO $$
BEGIN
    BEGIN
        CREATE EXTENSION IF NOT EXISTS pg_trgm;
    EXCEPTION WHEN OTHERS THEN
        RAISE NOTICE 'pg_trgm is not available (%): title searches read every row', SQLERRM;
    END;
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_trgm') THEN
        CREATE INDEX IF NOT EXISTS idx_media_item_title_trgm
            ON media_item USING gin (LOWER(title) gin_trgm_ops);
        CREATE INDEX IF NOT EXISTS idx_media_item_sort_title_trgm
            ON media_item USING gin (LOWER(COALESCE(sort_title, '')) gin_trgm_ops);
        CREATE INDEX IF NOT EXISTS idx_media_item_slug_trgm
            ON media_item USING gin (slug gin_trgm_ops);
        CREATE INDEX IF NOT EXISTS idx_media_alternative_title_trgm
            ON media_alternative_title USING gin (LOWER(title) gin_trgm_ops);
        CREATE INDEX IF NOT EXISTS idx_media_override_value_trgm
            ON media_override USING gin (LOWER(COALESCE(value, '')) gin_trgm_ops);
    END IF;
END $$;

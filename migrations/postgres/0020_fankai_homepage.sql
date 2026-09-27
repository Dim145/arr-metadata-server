-- Fankai's metadata service numbers its productions its own way, and its
-- website another: the homepage built from the one led to the other's
-- production of that number (Horimiya Kaï, series 33, opened Black Lagoon).
-- What refreshes wrote is cleared, and the source it was credited to with it;
-- the next refresh takes the production's page on the Fankai wiki instead.
UPDATE media_item
SET homepage = NULL,
    provenance = CASE
        WHEN provenance IS NULL THEN NULL
        ELSE (provenance::jsonb #- '{fields,homepage}')::text
    END
WHERE homepage LIKE 'https://fankai.fr/productions/%';

-- What TheTVDB adds to a series' name to tell it from another of the same
-- name: `2023` for "Rurouni Kenshin (2023)", `US` for "The Office (US)".
-- TMDB, whose name the title usually is, never adds one; Sonarr, which
-- Skyhook serves TheTVDB's names, has always had it. Kept so the Sonarr
-- surface can give it back. Filled in as each series is next refreshed.
ALTER TABLE media_item ADD COLUMN title_qualifier TEXT;

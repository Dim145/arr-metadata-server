-- TMDB's credit identifier, which is not the person's identifier.
--
-- A person has one TMDB id; each role they play carries its own `credit_id`.
-- Radarr stores that as `Credits.CreditTmdbId` with a NOT NULL constraint, so a
-- credit sent without one fails the entire movie refresh inside Radarr's own
-- database — not just that one credit.

ALTER TABLE media_credit ADD COLUMN credit_tmdb_id TEXT;

-- The tagline in each language a work is described in.
--
-- A work kept one tagline, the one TMDB gave in the language the catalogue
-- is stored in (0030), and a reader in another language was given it as it
-- was, under a title and an overview laid over in theirs — or, for Plex,
-- nothing at all. TMDB carries a tagline in each of its translations, beside
-- the title and the overview that were kept of them: it is kept here now,
-- and laid over the work as those two are, under the same lock in one
-- language. Filled in as each work is next refreshed.
ALTER TABLE media_translation ADD COLUMN tagline TEXT;

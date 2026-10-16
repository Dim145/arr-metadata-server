-- A lock in one language: the title or the overview a person wrote for the
-- readers of one language, beside the text every other one is given. The
-- language is the key translations are filed under — `fra`, `jpn` — and
-- empty for a lock in every language, which every lock held so far is.
--
-- Empty rather than NULL. A key over a column that may be NULL keeps any
-- number of rows that differ only by their NULLs — before 15, and after it
-- unless the index says `NULLS NOT DISTINCT`, which SQLite has no word for —
-- so two locks of one field in every language could both be kept and
-- `ON CONFLICT` would never meet the first. An index over
-- `COALESCE(language, '')` would close that too, but every write would have
-- to name the expression to conflict on, and the column would say one thing
-- two ways. Empty is one value: the primary key holds it on both engines,
-- and a write conflicts on the four columns.
ALTER TABLE media_override ADD COLUMN language TEXT NOT NULL DEFAULT '';

ALTER TABLE media_override DROP CONSTRAINT media_override_pkey;

ALTER TABLE media_override ADD PRIMARY KEY (media_id, scope, field, language);

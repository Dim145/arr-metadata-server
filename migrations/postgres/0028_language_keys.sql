-- Episode text, and the marks of a fetch, filed under a language that is not
-- one.
--
-- A language asked for was taken as it came and became the key both were
-- filed under: a path, a query, a string the size of a request, each a row
-- of its own. A language is now a tag, filed as two or three lower-case
-- letters, so nothing asks for such a key again and what was filed under
-- one is read by nobody.
DELETE FROM media_episode_translation WHERE language !~ '^[a-z]{2,3}$';

DELETE FROM media_language_fetch WHERE language !~ '^[a-z]{2,3}$';

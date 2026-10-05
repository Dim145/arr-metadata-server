-- Episode text, and the marks of a fetch, filed under a language that is not
-- one.
--
-- A language asked for was taken as it came and became the key both were
-- filed under: a path, a query, a string the size of a request, each a row
-- of its own. A language is now a tag, filed as two or three lower-case
-- letters, so nothing asks for such a key again and what was filed under
-- one is read by nobody.
DELETE FROM media_episode_translation
 WHERE length(language) NOT BETWEEN 2 AND 3 OR language GLOB '*[^a-z]*';

DELETE FROM media_language_fetch
 WHERE length(language) NOT BETWEEN 2 AND 3 OR language GLOB '*[^a-z]*';

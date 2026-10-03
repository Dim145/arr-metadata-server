-- When a language whose episode text could not be fetched in full for a
-- work is tried again: no provider answered, not every one asked did, or
-- none had anything to say in it. Such a language was marked fetched all the
-- same, and was not asked for again until the work's next refresh. Not
-- before this time either, or every request in the language would wait on
-- the providers again. NULL for a language fetched in full.
ALTER TABLE media_language_fetch ADD COLUMN retry_after TEXT;

-- What changed in each work, field by field: the journal its editor shows.
--
-- A refresh replaces the copy each source gave (`media_provider_snapshot`),
-- and the audit trail says that a lock was set, not what it held: neither
-- said what a source changed, nor what a field held before, nor how to go
-- back to it. One row a value that changed — of the work, of a season, of an
-- episode, or of a child a lock names — with what it held and what it took,
-- as JSON, why, and who: a person (`actor`), a source (`provider`), or both
-- when a person asked the sources again. A refresh writes its rows in one
-- statement, and one that changed nothing writes none. Kept as long as
-- `history.keepDays` says, by the sweep that prunes the audit trail; a work
-- deleted takes its journal with it.
--
-- `cause` is checked by the server rather than by a CHECK, as the audit
-- trail's `action` is: the set grows, and widening a CHECK means building the
-- table again on SQLite.
CREATE TABLE media_change (
    id            TEXT    PRIMARY KEY,
    media_id      TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    at            TEXT    NOT NULL,
    -- refresh, sync and fallback — TMDB numbering a series in TheTVDB's
    -- place, or TheTVDB taking it back — are a source's; lock, unlock,
    -- revert and edit, a person's; merge and import, another record's.
    cause         TEXT    NOT NULL,
    -- Who, as the audit trail names them; nothing for the schedule's doing.
    actor         TEXT,
    -- The source whose answer the new value is, where one is known.
    provider      TEXT,
    -- `item`, `season:2`, `episode:2x5`, or a child a lock names, as a lock
    -- names it: `credit:actor:tmdb:17419`.
    scope         TEXT    NOT NULL,
    field         TEXT    NOT NULL,
    -- The language a text was locked in alone, as translations are filed
    -- (`fra`); empty for every language, and for what is in none.
    language      TEXT    NOT NULL DEFAULT '',
    -- The values, as JSON; nothing where there was none.
    value_before  TEXT,
    value_after   TEXT,
    -- Whether a lock held the field as it changed: a source's change made
    -- under one reached the record and not what is served, and a person's
    -- change undone puts the lock back.
    was_locked    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX ix_change_media ON media_change (media_id, at);
CREATE INDEX ix_change_field ON media_change (media_id, scope, field);
CREATE INDEX ix_change_at ON media_change (at);

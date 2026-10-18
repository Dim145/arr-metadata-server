-- Whether a destination is told of failed refreshes together: the failures
-- wait a few minutes from the first, then go as one message, counted by
-- cause, rather than as a message per work — an outage of a source is one
-- message, not one per work it touched. On for the destinations people
-- read; off for the ones programs read, which want an event per work.
ALTER TABLE webhook_destination ADD COLUMN grouped INTEGER NOT NULL DEFAULT 0;
UPDATE webhook_destination SET grouped = 1
 WHERE kind IN ('discord', 'slack', 'ntfy', 'gotify', 'apprise');

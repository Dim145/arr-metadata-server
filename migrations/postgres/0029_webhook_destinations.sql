-- Where what happens here is told, and what was told. Until now one address
-- held in the settings, posted to once and forgotten; now as many
-- destinations as an operator names — a Standard Webhooks receiver, Discord,
-- Slack, ntfy, Gotify, Apprise, or the JSON the single address was sent —
-- and a line for every event owed to each, kept until it is delivered or
-- given up on, so that neither a receiver that is down nor a restart loses
-- one.
CREATE TABLE webhook_destination (
    id              TEXT    PRIMARY KEY,
    name            TEXT    NOT NULL,
    -- standard, json, discord, slack, ntfy, gotify or apprise: checked by the
    -- server rather than here, so that a kind added later needs no new table.
    kind            TEXT    NOT NULL,
    url             TEXT    NOT NULL,
    -- Its signing secret, made here (standard), or the token its service
    -- gave (ntfy, Gotify). Written, never read back by the API.
    secret          TEXT,
    -- The events it is told of: a JSON array of their names, `*` for every one.
    events          TEXT    NOT NULL DEFAULT '[]',
    enabled         INTEGER NOT NULL DEFAULT 1,
    -- Why it was switched off, when the server did it: an address that
    -- answered 410 Gone.
    disabled_reason TEXT,
    disabled_at     TEXT,
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL
);

CREATE TABLE webhook_delivery (
    id              TEXT    PRIMARY KEY,
    destination_id  TEXT    NOT NULL,
    -- The event's own id, the same for every destination told of it and for
    -- every try and replay: Standard Webhooks' `webhook-id`.
    message_id      TEXT    NOT NULL,
    event           TEXT    NOT NULL,
    -- What is told, as JSON: the event, who, to what, the detail, when.
    payload         TEXT    NOT NULL,
    status          TEXT    NOT NULL CHECK (status IN ('pending', 'delivered', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT,
    -- What the last try came to: the status answered, or why nothing was.
    last_status     INTEGER,
    last_error      TEXT,
    duration_ms     BIGINT,
    -- The worker sending it, and since when: a claim older than a few
    -- minutes is a worker that stopped mid-send, and the line is anybody's.
    claimed_by      TEXT,
    claimed_at      TEXT,
    -- The delivery this one sends again, when it was replayed.
    replay_of       TEXT,
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL,
    delivered_at    TEXT
);

-- What the worker asks for: the lines due.
CREATE INDEX ix_webhook_delivery_due ON webhook_delivery (status, next_attempt_at);
-- A destination's log, newest first, and what is kept of it.
CREATE INDEX ix_webhook_delivery_destination ON webhook_delivery (destination_id, created_at);

-- The reference lists the relays keep in the database: TMDB's
-- configuration, its genres, its certifications and the countries it lists
-- watch providers for; TheTVDB's languages, countries, genres, ratings, and
-- the types and statuses its records are filed under. A row a document, as
-- the service answered this server's own key, under what the relay files it
-- by — the address, the path, the query and the headers that change the
-- answer, never a credential — and written again at each answer. It is read
-- when the service fails and the memory holds nothing, not even a stale
-- copy: a client starting during an outage still gets its configuration.
--
-- Nothing of AniList's: its terms want nothing stored. Nothing older than
-- 180 days is served, and it is deleted: TMDB's terms allow six months, and
-- TheTVDB's lists keep to the same rule.
CREATE TABLE relay_document (
    -- Which relay: 'tmdb' or 'tvdb', and never another without a migration.
    relay         TEXT    NOT NULL CHECK (relay IN ('tmdb', 'tvdb')),
    -- What the relay files the document under, without the credential and
    -- without the generation the memory's keys carry.
    cache_key     TEXT    NOT NULL,
    content_type  TEXT    NOT NULL,
    -- The document as the service wrote it: JSON.
    body          TEXT    NOT NULL,
    -- When the service answered it.
    fetched_at    TEXT    NOT NULL,
    PRIMARY KEY (relay, cache_key)
);

CREATE INDEX ix_relay_document_fetched ON relay_document (fetched_at);

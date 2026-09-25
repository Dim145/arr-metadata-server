-- The other orders TheTVDB keeps a series' episodes in — DVD, absolute,
-- alternate, regional — each a placing of the same episodes, by their TVDB
-- id, in seasons and numbers of its own. Kept apart from the aired order:
-- what a client is served never changes, and a reader who knows a series by
-- its DVDs can still find their way through it.
CREATE TABLE media_episode_order (
    id               TEXT    PRIMARY KEY,
    media_id         TEXT    NOT NULL REFERENCES media_item (id) ON DELETE CASCADE,
    -- dvd, absolute, alternate, regional, altdvd: TheTVDB's season types.
    order_type       TEXT    NOT NULL,
    tvdb_episode_id  BIGINT  NOT NULL,
    season_number    INTEGER NOT NULL,
    episode_number   INTEGER NOT NULL,
    absolute_number  INTEGER,
    created_at       TEXT    NOT NULL
);

CREATE INDEX ix_episode_order_media
    ON media_episode_order (media_id, order_type, season_number, episode_number);

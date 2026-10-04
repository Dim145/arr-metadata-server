-- The TheTVDB relay answers /v4/episodes/{id} with the locks of the series
-- that holds the episode, found by TheTVDB's id of it — on every serve, the
-- cached ones included — so the lookup is indexed. Partial: an episode made
-- by hand has no such id.
CREATE INDEX ix_media_episode_tvdb ON media_episode (tvdb_id) WHERE tvdb_id IS NOT NULL;

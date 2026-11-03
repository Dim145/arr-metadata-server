-- The TheTVDB relay finds the work that holds a season, or an episode an
-- order keeps, by TheTVDB's id of it, as it finds an episode (0026): a number
-- nobody holds read every season and every episode of every order, on each
-- request. Partial, as there: what is made by hand has no such id.
CREATE INDEX ix_media_season_tvdb ON media_season (tvdb_id) WHERE tvdb_id IS NOT NULL;
CREATE INDEX ix_order_episode_tvdb ON media_order_episode (tvdb_episode_id) WHERE tvdb_episode_id IS NOT NULL;

-- Reads across works that the interface now makes.

-- The calendar: episodes by date where no provider knew the time. By the
-- moment they air is `ix_episode_air`, which 0001 made already.
CREATE INDEX ix_media_episode_air_date ON media_episode (air_date);

-- A person's page: every credit TMDB files under them.
CREATE INDEX ix_credit_person ON media_credit (tmdb_person_id);

-- A film's collection: the other films TMDB files with it.
CREATE INDEX ix_media_item_collection ON media_item (collection_tmdb_id);

-- An episode's moment, where a provider only ever gave its date. TheTVDB's
-- episodes used to be stored with midnight UTC of the air date as their
-- instant, which Sonarr needs but which is not a time anybody broadcast at:
-- shown to a person, it put every such episode at one or two in the morning.
-- The placeholder is now added on Sonarr's way out instead, so what is stored
-- here is only ever a time somebody knew. Episodes made by hand were stamped
-- the same way — an episode's author never gave this column a value; a time
-- they type is an override, kept elsewhere — so they are cleared with the rest.
UPDATE media_episode
   SET air_date_utc = NULL
 WHERE air_date_utc = air_date || 'T00:00:00Z';

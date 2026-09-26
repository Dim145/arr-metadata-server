-- A work's theme music, where a provider keeps one: Fankai does for every
-- Fan-Kai. An address, like an image's; the NFO export writes it beside the
-- documents as `theme.mp3`, which Plex, Jellyfin and Kodi play as the show's
-- theme, and a work's page plays it on demand.
ALTER TABLE media_item ADD COLUMN theme_music TEXT;

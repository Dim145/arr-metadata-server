-- Record which country a content rating came from.
--
-- Radarr matches certifications by country code and compares against its own
-- `CertificationCountry` setting, which is an ISO 3166-1 **alpha-2 uppercase**
-- code (`US` by default). Without the source country a certification either has
-- to be mislabelled or dropped, and Radarr silently shows none.
--
-- TMDB already gives the code; this is where it is kept.

ALTER TABLE media_item ADD COLUMN content_rating_country TEXT;

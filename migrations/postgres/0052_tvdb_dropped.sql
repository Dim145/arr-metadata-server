-- A series TheTVDB no longer knows: it deleted the series, merged it into
-- another, or answers that it has no such series. Its numbering as it was
-- then served is kept as an order, `tvdb-last`, frozen as the aired
-- numbering, and the episodes that air afterwards are placed after it: Sonarr
-- holds the series by TheTVDB's id and has matched its files by those
-- numbers.
--   gone         how many refreshes in a row, the most part of a day apart,
--                TheTVDB answered that it has no such series;
--   gone_at      when the last of them was counted;
--   dropped      since when the numbering is kept so: two such refreshes,
--                or TheTVDB's feed of updates saying it deleted the series;
--   dropped_why  missing, deleted or merged;
--   merged_into  the series TheTVDB merged it into, which it keeps: the
--                work takes that id, and answers to the one it had besides.
-- A row a work, as for TheTVDB's silence, and none while TheTVDB answers.
ALTER TABLE media_numbering ADD COLUMN gone INTEGER NOT NULL DEFAULT 0;
ALTER TABLE media_numbering ADD COLUMN gone_at TEXT;
ALTER TABLE media_numbering ADD COLUMN dropped TEXT;
ALTER TABLE media_numbering ADD COLUMN dropped_why TEXT;
ALTER TABLE media_numbering ADD COLUMN merged_into BIGINT;

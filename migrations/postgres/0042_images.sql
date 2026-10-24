-- What a picture kept is drawn as before it is there, and the copies made
-- of it for the interface.
--
-- `color` is the colour most of the picture is, `#rrggbb`, painted at once
-- behind its frame; `thumbhash` a sketch of it in two dozen bytes, base64,
-- drawn under it until it arrives. `prepared` says which version of the
-- server worked them out, with the first copy the picture's kind is asked
-- for: 0 for a picture stored before there were any, which the task
-- `media.variants` prepares.
ALTER TABLE media_asset ADD COLUMN color TEXT;
ALTER TABLE media_asset ADD COLUMN thumbhash TEXT;
ALTER TABLE media_asset ADD COLUMN prepared INTEGER NOT NULL DEFAULT 0;

-- The copies made of a picture: a WebP at each width the interface asks
-- for, an AVIF the night's task made, the square cut Plex is given. Filed
-- under the picture's hash beside it in the store, `<sha256>-480.webp`,
-- and made again whenever one is asked for and missing: a row is what lets
-- a page know an AVIF exists, the media page count what the copies weigh,
-- and the copies go with their picture.
CREATE TABLE media_variant (
    key        TEXT    PRIMARY KEY,
    -- The picture's hash: its key without its extension.
    stem       TEXT    NOT NULL,
    width      INTEGER NOT NULL,
    height     INTEGER NOT NULL,
    format     TEXT    NOT NULL CHECK (format IN ('webp', 'avif', 'jpeg')),
    bytes      BIGINT  NOT NULL,
    created_at TEXT    NOT NULL
);

CREATE INDEX ix_media_variant_stem ON media_variant (stem);

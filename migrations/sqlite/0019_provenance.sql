-- Who gave a work its values, as the last merge worked it out: field by
-- field the provider whose value was kept and those that said the same, the
-- images counted by source, the provider of the episode list. JSON; empty
-- for a work stored before, and for one made by hand.
ALTER TABLE media_item ADD COLUMN provenance TEXT;

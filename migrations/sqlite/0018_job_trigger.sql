-- Who started a run: nothing for the schedule, the person or key otherwise,
-- as the journal names them (`admin:dim145`). A task started by hand is one
-- somebody is waiting on, and the history says whose it was.
ALTER TABLE job_run ADD COLUMN triggered_by TEXT;

-- The runs recorded before there was a name to record: a work refreshed by
-- hand and an export were always somebody's, whose is not known.
UPDATE job_run SET triggered_by = 'unknown' WHERE kind IN ('refresh.item', 'export.nfo');

-- A task's latest run, which the tasks page reads for every task.
CREATE INDEX ix_job_run_kind_created ON job_run (kind, created_at);

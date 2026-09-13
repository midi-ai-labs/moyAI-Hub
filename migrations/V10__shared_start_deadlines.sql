ALTER TABLE shared_jobs ADD COLUMN start_before_ms INTEGER NOT NULL DEFAULT 9223372036854775807 CHECK(start_before_ms > 0);
UPDATE shared_jobs SET start_before_ms=MIN(created_at_ms,9223372036768375807)+86400000;
-- Existing descendants may be newer than the root; none may outlive its root's start window.
UPDATE shared_jobs SET start_before_ms=MIN(start_before_ms,(SELECT root.start_before_ms FROM shared_jobs root WHERE root.id=shared_jobs.root_id));
CREATE INDEX shared_jobs_start_deadline ON shared_jobs(state,start_before_ms,id);

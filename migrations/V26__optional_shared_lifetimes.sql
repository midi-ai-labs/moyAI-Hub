-- Absence is an unbounded job/service lifetime, not a distant sentinel timestamp.
-- Preserve explicit legacy queue deadlines and all previously issued process receipts.
CREATE TEMP TABLE shared_lifetime_migration AS
SELECT id, CASE
    WHEN state NOT IN ('succeeded','failed','cancelled') AND
      (COALESCE(json_extract(request_json,'$.start_before_ms'),json_extract(request_json,'$.request.start_before_ms')) IS NULL
       OR EXISTS(SELECT 1 FROM shared_attempts a WHERE a.job_id=shared_jobs.id AND a.state IN ('running','yielded','succeeded')))
    THEN NULL ELSE start_before_ms END AS start_before_ms
FROM shared_jobs;
DROP INDEX shared_jobs_start_deadline;
ALTER TABLE shared_jobs DROP COLUMN start_before_ms;
ALTER TABLE shared_jobs ADD COLUMN start_before_ms INTEGER CHECK(start_before_ms IS NULL OR start_before_ms > 0);
UPDATE shared_jobs SET start_before_ms=(SELECT old.start_before_ms FROM shared_lifetime_migration old WHERE old.id=shared_jobs.id);
DROP TABLE shared_lifetime_migration;
-- Older Runners still couple managed-process and approval lifetimes to clocks.
-- Fence their next assignment poll before they can consume the new contract;
-- exact stop/uncertain reports remain valid and keep their durable attempt owner.
UPDATE shared_jobs SET required_capability='multi_device_session_v2'
WHERE state NOT IN ('succeeded','failed','cancelled');
CREATE INDEX shared_jobs_start_deadline ON shared_jobs(state,start_before_ms,id);

CREATE TEMP TABLE shared_service_lifetime_migration AS SELECT service_id,expires_at_ms FROM shared_service_leases;
DROP INDEX shared_service_leases_active;
ALTER TABLE shared_service_leases DROP COLUMN expires_at_ms;
ALTER TABLE shared_service_leases ADD COLUMN expires_at_ms INTEGER CHECK(expires_at_ms IS NULL OR expires_at_ms > 0);
UPDATE shared_service_leases SET expires_at_ms=(SELECT old.expires_at_ms FROM shared_service_lifetime_migration old WHERE old.service_id=shared_service_leases.service_id);
DROP TABLE shared_service_lifetime_migration;
CREATE INDEX shared_service_leases_active ON shared_service_leases(state,expires_at_ms);

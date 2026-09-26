-- The opaque local session reference does not identify its owning PC. Bind new
-- delegated jobs to the authenticated controller device used at submission.
-- Historical rows remain NULL: there is no safe way to infer their origin PC.
ALTER TABLE shared_jobs ADD COLUMN origin_device_id TEXT;
CREATE INDEX shared_jobs_origin_device ON shared_jobs(origin_device_id,origin_session_ref,requestor_id,id)
    WHERE origin_device_id IS NOT NULL AND origin_session_ref IS NOT NULL;

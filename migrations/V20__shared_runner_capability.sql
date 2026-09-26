-- Existing conversations created through the new local-session route already
-- require a Runner that understands shared conversation/service semantics.
-- Ordinary project jobs remain compatible with older Runners.
ALTER TABLE shared_jobs ADD COLUMN required_capability TEXT;
UPDATE shared_jobs SET required_capability='multi_device_session_v1'
    WHERE origin_session_ref IS NOT NULL;

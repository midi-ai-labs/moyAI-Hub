-- A retained service continues to occupy its resource after its original
-- attempt yields or finishes. Only the owning Runner's stop report releases it.
CREATE TABLE shared_service_leases (
    service_id TEXT PRIMARY KEY NOT NULL,
    attempt_id TEXT NOT NULL REFERENCES shared_attempts(id),
    generation INTEGER NOT NULL CHECK (generation > 0),
    expires_at_ms INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active','uncertain','stopped')),
    uncertainty_reason TEXT,
    stop_requested_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX shared_service_leases_attempt ON shared_service_leases(attempt_id,state);
CREATE INDEX shared_service_leases_active ON shared_service_leases(state,expires_at_ms);

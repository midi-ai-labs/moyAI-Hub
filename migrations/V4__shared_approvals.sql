CREATE TABLE shared_approvals (
    id TEXT PRIMARY KEY NOT NULL,
    attempt_id TEXT NOT NULL REFERENCES shared_attempts(id),
    request_json TEXT NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    decision TEXT CHECK (decision IN ('approve','deny','stop')),
    decided_by TEXT REFERENCES identity_users(id),
    decided_at_ms INTEGER,
    consumed_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    CHECK ((decision IS NULL AND decided_by IS NULL AND decided_at_ms IS NULL)
        OR (decision IS NOT NULL AND decided_by IS NOT NULL AND decided_at_ms IS NOT NULL))
);
CREATE INDEX shared_approvals_attempt ON shared_approvals(attempt_id, created_at_ms, id);

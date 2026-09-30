-- A bounded display cache, never checkpoint or execution authority.
-- Canonical history remains on the Runner and in the existing settled archive.
CREATE TABLE shared_attempt_progress (
    attempt_id TEXT PRIMARY KEY NOT NULL REFERENCES shared_attempts(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK (revision > 0),
    payload_json TEXT NOT NULL,
    observed_at_ms INTEGER NOT NULL
);

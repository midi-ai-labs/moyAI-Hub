ALTER TABLE identity_users ADD COLUMN setup_pending INTEGER NOT NULL DEFAULT 0 CHECK(setup_pending IN (0,1));
CREATE TABLE identity_setup_codes (
    user_id TEXT PRIMARY KEY REFERENCES identity_users(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL UNIQUE,
    credential_generation INTEGER NOT NULL,
    issued_by TEXT NOT NULL,
    issued_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    consumed_at_ms INTEGER,
    CHECK(expires_at_ms > issued_at_ms)
);

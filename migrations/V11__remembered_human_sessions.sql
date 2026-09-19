-- A remembered human login is usable only with the registered device's mTLS.
-- Only hashes are stored. Access sessions retain their existing finite lifetime.
CREATE TABLE identity_refresh_sessions (
    token_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES identity_users(id),
    device_id TEXT NOT NULL,
    credential_generation INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    last_used_at_ms INTEGER NOT NULL,
    UNIQUE(user_id, device_id)
);
ALTER TABLE identity_sessions ADD COLUMN refresh_hash TEXT
    REFERENCES identity_refresh_sessions(token_hash) ON DELETE CASCADE;
CREATE INDEX identity_sessions_refresh ON identity_sessions(refresh_hash);
CREATE TRIGGER identity_refresh_credentials_changed
AFTER UPDATE OF credential_generation ON identity_users
WHEN NEW.credential_generation != OLD.credential_generation
BEGIN
    DELETE FROM identity_refresh_sessions WHERE user_id = NEW.id;
END;

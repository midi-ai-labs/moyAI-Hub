-- Earlier browser sessions originated from passwords and have no trusted
-- native/device provenance. Reopen through the native Host after upgrading.
-- Keep people, history and existing shared-work session ownership intact.
DELETE FROM identity_sessions WHERE audience = 'web_admin';
CREATE TABLE identity_local_admin (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    user_id TEXT NOT NULL UNIQUE REFERENCES identity_users(id)
);
CREATE TABLE identity_web_session_devices (
    token_hash TEXT PRIMARY KEY REFERENCES identity_sessions(token_hash) ON DELETE CASCADE,
    device_id TEXT NOT NULL,
    certificate_sha256 TEXT NOT NULL,
    binding_generation INTEGER NOT NULL CHECK (binding_generation > 0)
);

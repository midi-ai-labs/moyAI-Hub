CREATE TABLE identity_users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    administrator INTEGER NOT NULL CHECK (administrator IN (0, 1)),
    disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
    credential_generation INTEGER NOT NULL DEFAULT 1 CHECK (credential_generation > 0),
    password_algorithm TEXT NOT NULL CHECK (password_algorithm = 'pbkdf2_sha256'),
    password_iterations INTEGER NOT NULL CHECK (password_iterations BETWEEN 600000 AND 2000000),
    password_salt BLOB NOT NULL CHECK (length(password_salt) = 16),
    password_hash BLOB NOT NULL CHECK (length(password_hash) = 32),
    failed_logins INTEGER NOT NULL DEFAULT 0 CHECK (failed_logins >= 0),
    blocked_until_ms INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE identity_sessions (
    token_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES identity_users(id),
    audience TEXT NOT NULL CHECK (audience IN ('web_admin', 'shared_work')),
    device_id TEXT,
    credential_generation INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    CHECK ((audience = 'web_admin' AND device_id IS NULL)
        OR (audience = 'shared_work' AND device_id IS NOT NULL))
);
CREATE INDEX identity_sessions_user ON identity_sessions(user_id);
CREATE INDEX identity_sessions_expiry ON identity_sessions(expires_at_ms);

CREATE TABLE identity_project_memberships (
    project_id TEXT NOT NULL REFERENCES shared_projects(id),
    user_id TEXT NOT NULL REFERENCES identity_users(id),
    role TEXT NOT NULL CHECK (role IN ('reader', 'contributor', 'manager')),
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX identity_memberships_user ON identity_project_memberships(user_id, project_id);

CREATE TABLE shared_assets (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES shared_projects(id),
    job_id TEXT REFERENCES shared_jobs(id),
    attempt_id TEXT,
    generation INTEGER,
    kind TEXT NOT NULL CHECK(kind IN ('input','artifact','archive')),
    name TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    content BLOB NOT NULL,
    byte_length INTEGER NOT NULL CHECK(byte_length >= 0),
    base_sha256 TEXT,
    created_by TEXT NOT NULL,
    request_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    UNIQUE(created_by,request_id)
);
CREATE INDEX shared_assets_project ON shared_assets(project_id,created_at_ms,id);
CREATE INDEX shared_assets_job ON shared_assets(job_id,kind,created_at_ms,id);
CREATE TABLE shared_job_inputs (
    job_id TEXT NOT NULL REFERENCES shared_jobs(id),
    asset_id TEXT NOT NULL REFERENCES shared_assets(id),
    PRIMARY KEY(job_id,asset_id)
);
CREATE TABLE shared_continuations (
    job_id TEXT PRIMARY KEY REFERENCES shared_jobs(id),
    previous_job_id TEXT NOT NULL REFERENCES shared_jobs(id)
);

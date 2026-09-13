CREATE TABLE shared_projects (
    id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL
);
CREATE TABLE shared_resources (
    id TEXT PRIMARY KEY NOT NULL,
    capacity INTEGER NOT NULL CHECK (capacity BETWEEN 1 AND 128)
);
CREATE TABLE shared_environments (
    id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL,
    resource_id TEXT NOT NULL REFERENCES shared_resources(id),
    runner_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1))
);
CREATE TABLE shared_environment_projects (
    environment_id TEXT NOT NULL REFERENCES shared_environments(id),
    project_id TEXT NOT NULL REFERENCES shared_projects(id),
    PRIMARY KEY (environment_id, project_id)
);
CREATE TABLE shared_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    root_id TEXT NOT NULL REFERENCES shared_jobs(id),
    parent_id TEXT REFERENCES shared_jobs(id),
    project_id TEXT NOT NULL REFERENCES shared_projects(id),
    environment_id TEXT NOT NULL REFERENCES shared_environments(id),
    requestor_id TEXT NOT NULL,
    assignee_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    title TEXT NOT NULL,
    input_json TEXT NOT NULL,
    checkpoint_json TEXT,
    result_json TEXT,
    state TEXT NOT NULL CHECK (state IN ('queued','assigned','running','waiting_child','succeeded','failed','cancelling','cancelled')),
    awaiting_child_id TEXT REFERENCES shared_jobs(id),
    descendant_budget INTEGER NOT NULL CHECK (descendant_budget BETWEEN 0 AND 1000),
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE (requestor_id, request_id)
);
CREATE INDEX shared_jobs_root ON shared_jobs(root_id, created_at_ms, id);
CREATE INDEX shared_jobs_project ON shared_jobs(project_id, created_at_ms, id);
CREATE TABLE shared_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL REFERENCES shared_jobs(id),
    runner_id TEXT NOT NULL,
    resource_id TEXT NOT NULL REFERENCES shared_resources(id),
    generation INTEGER NOT NULL CHECK (generation > 0),
    state TEXT NOT NULL CHECK (state IN ('assigned','running','stop_requested','yielded','succeeded','failed','cancelled')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE (job_id, generation)
);
CREATE UNIQUE INDEX shared_attempts_one_active ON shared_attempts(job_id)
    WHERE state IN ('assigned','running','stop_requested');
CREATE INDEX shared_attempts_resource ON shared_attempts(resource_id, state);
CREATE TABLE shared_reports (
    attempt_id TEXT NOT NULL REFERENCES shared_attempts(id),
    event_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (attempt_id, event_id)
);

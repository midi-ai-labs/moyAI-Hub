-- A device that leaves and later rejoins must not inherit an old execution
-- environment or make a delayed request valid under its new participation.
ALTER TABLE shared_project_devices ADD COLUMN participation_generation INTEGER NOT NULL DEFAULT 1 CHECK(participation_generation > 0);
ALTER TABLE shared_jobs ADD COLUMN controller_device_id TEXT;
ALTER TABLE shared_jobs ADD COLUMN controller_participation_generation INTEGER;
-- A replacement turn retains the stopped original for audit without making it
-- part of the next execution context.
ALTER TABLE shared_jobs ADD COLUMN revises_job_id TEXT REFERENCES shared_jobs(id);

-- Referenced jobs, assets, approvals and settlement records stay durable after
-- a project is removed from ordinary use.
ALTER TABLE shared_projects ADD COLUMN deleted_at_ms INTEGER;
CREATE INDEX shared_jobs_controller_participation
  ON shared_jobs(project_id,controller_device_id,controller_participation_generation,state);

-- Submitted conversations use the first root job as their stable identity.
-- Metadata records only user edits and logical deletion, never a second copy
-- of the job history.
CREATE TABLE shared_conversation_details (
    project_id TEXT NOT NULL REFERENCES shared_projects(id),
    conversation_id TEXT NOT NULL REFERENCES shared_jobs(id),
    title TEXT,
    delete_requested_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
    PRIMARY KEY(project_id,conversation_id)
);

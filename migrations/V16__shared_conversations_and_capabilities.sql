-- A conversation is the user-visible identity of a root job and its confirmed
-- continuations. Child jobs retain the identity of their root.
ALTER TABLE shared_jobs ADD COLUMN conversation_id TEXT NOT NULL DEFAULT '';
WITH RECURSIVE roots(root_id, conversation_id) AS (
    SELECT j.id, j.id
    FROM shared_jobs j
    WHERE j.parent_id IS NULL
      AND NOT EXISTS (SELECT 1 FROM shared_continuations c WHERE c.job_id=j.id)
    UNION ALL
    SELECT next.id, roots.conversation_id
    FROM shared_continuations c
    JOIN shared_jobs previous ON previous.id=c.previous_job_id
    JOIN roots ON roots.root_id=previous.root_id
    JOIN shared_jobs next ON next.id=c.job_id AND next.parent_id IS NULL
)
UPDATE shared_jobs
SET conversation_id=COALESCE(
    (SELECT roots.conversation_id FROM roots WHERE roots.root_id=shared_jobs.root_id),
    shared_jobs.root_id
);
CREATE INDEX shared_jobs_conversation ON shared_jobs(project_id,conversation_id,id);

-- Capability names are administrator declarations, not proof of installation,
-- licence availability, or a successful run on the destination PC.
CREATE TABLE shared_environment_capabilities (
    environment_id TEXT NOT NULL REFERENCES shared_environments(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
    PRIMARY KEY (environment_id,name)
);

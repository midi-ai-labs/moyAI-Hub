ALTER TABLE shared_projects ADD COLUMN overview TEXT NOT NULL DEFAULT '';
-- A root owns the immutable descriptive context of one accepted request.
-- Descendants refer to that root; this is not an authorization snapshot.
ALTER TABLE shared_jobs ADD COLUMN project_context_json TEXT;

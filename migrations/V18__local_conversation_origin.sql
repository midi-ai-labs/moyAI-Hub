-- An opaque caller-owned reference identifies jobs delegated from a private
-- local conversation. It is metadata only and never authorizes access.
ALTER TABLE shared_jobs ADD COLUMN origin_session_ref TEXT;

-- A started attempt remains allocated when the Runner cannot establish its outcome.
-- This observation never serves as proof of process completion or resource release.
ALTER TABLE shared_attempts ADD COLUMN uncertainty_reason TEXT;

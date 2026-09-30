-- Keep transfer history and identities. Pending transfers no longer gate work.
UPDATE shared_handovers SET state='cancelled',
  completed_at_ms=COALESCE(completed_at_ms,requested_at_ms)
WHERE state='pending';

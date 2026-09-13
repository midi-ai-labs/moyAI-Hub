-- Rust migration first persists and verifies each retained legacy BLOB in the
-- adjacent managed asset directory. Empty content remains for schema compatibility.
CREATE INDEX shared_assets_retained_hash ON shared_assets(sha256) WHERE purged_at_ms IS NULL;

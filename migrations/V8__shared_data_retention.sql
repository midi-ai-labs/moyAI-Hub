ALTER TABLE shared_assets ADD COLUMN purged_at_ms INTEGER;
CREATE TABLE shared_data_retention_runs (
    id TEXT PRIMARY KEY,
    before_ms INTEGER NOT NULL,
    asset_count INTEGER NOT NULL,
    byte_count INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
);

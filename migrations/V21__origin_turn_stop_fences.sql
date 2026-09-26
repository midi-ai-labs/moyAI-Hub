-- A local user turn is admitted once before it may create remote work. A stop
-- fences its epoch; a later user turn in the same local conversation may use
-- a strictly newer epoch without reviving delayed work from the old turn.
ALTER TABLE shared_jobs ADD COLUMN origin_turn_ref TEXT;
ALTER TABLE shared_jobs ADD COLUMN origin_turn_epoch INTEGER;
ALTER TABLE shared_jobs ADD COLUMN origin_turn_revision INTEGER;

CREATE TABLE shared_origin_scopes (
    origin_device_id TEXT NOT NULL,
    origin_owner_id TEXT NOT NULL,
    origin_session_ref TEXT NOT NULL,
    current_epoch INTEGER NOT NULL DEFAULT 0,
    stop_floor INTEGER NOT NULL DEFAULT 0,
    stop_through_revision INTEGER,
    PRIMARY KEY (origin_device_id, origin_owner_id, origin_session_ref)
);
CREATE TABLE shared_origin_turns (
    origin_device_id TEXT NOT NULL,
    origin_owner_id TEXT NOT NULL,
    origin_session_ref TEXT NOT NULL,
    origin_turn_ref TEXT NOT NULL,
    turn_revision INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    stopped_at_ms INTEGER,
    PRIMARY KEY (origin_device_id, origin_owner_id, origin_session_ref, origin_turn_ref),
    FOREIGN KEY (origin_device_id, origin_owner_id, origin_session_ref)
        REFERENCES shared_origin_scopes(origin_device_id, origin_owner_id, origin_session_ref)
);
CREATE INDEX shared_jobs_origin_turn ON shared_jobs(origin_device_id, origin_session_ref, origin_turn_ref, origin_turn_epoch);

-- A project conversation stop advances a causal epoch. A late continuation
-- carrying the old epoch is rejected, while a later explicit user turn can
-- continue the same conversation with the new epoch. Its existing leases
-- remain occupied until the exact Runner stop confirmation.
CREATE TABLE shared_conversation_stop_fences (
    project_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    stopped_at_ms INTEGER NOT NULL,
    current_epoch INTEGER NOT NULL DEFAULT 1 CHECK(current_epoch>0),
    PRIMARY KEY (project_id, conversation_id)
);
CREATE TABLE shared_project_stop_receipts (
    actor_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    conversation_epoch INTEGER NOT NULL CHECK(conversation_epoch>0),
    PRIMARY KEY (actor_id, request_id),
    FOREIGN KEY (actor_id, request_id)
        REFERENCES shared_management_receipts(actor_id, request_id)
);
CREATE INDEX shared_jobs_conversation_history ON shared_jobs(project_id, conversation_id, parent_id);

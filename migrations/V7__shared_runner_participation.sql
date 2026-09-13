CREATE TABLE shared_external_attempts (
    attempt_id TEXT PRIMARY KEY REFERENCES shared_attempts(id),
    source TEXT NOT NULL CHECK(source IN ('local','desktop','cli','tui','legacy_mcp')),
    actor_device_id TEXT NOT NULL
);
CREATE TABLE shared_runner_templates (
    runner_id TEXT NOT NULL,
    template_id TEXT NOT NULL,
    label TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY(runner_id,template_id)
);
CREATE TABLE shared_provisioning (
    environment_id TEXT PRIMARY KEY REFERENCES shared_environments(id),
    runner_id TEXT NOT NULL,
    template_id TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 1 CHECK(generation > 0),
    state TEXT NOT NULL CHECK(state IN ('pending','ready','failed')),
    error TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE TRIGGER management_templates_insert AFTER INSERT ON shared_runner_templates BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_templates_delete AFTER DELETE ON shared_runner_templates BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_provision_insert AFTER INSERT ON shared_provisioning BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_provision_delete AFTER DELETE ON shared_provisioning BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_provision_update AFTER UPDATE OF state,error,generation ON shared_provisioning WHEN OLD.state IS NOT NEW.state OR OLD.error IS NOT NEW.error OR OLD.generation<>NEW.generation BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;

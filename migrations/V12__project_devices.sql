ALTER TABLE shared_projects ADD COLUMN legacy_devices_unrestricted INTEGER NOT NULL DEFAULT 1 CHECK(legacy_devices_unrestricted IN (0,1));
CREATE TABLE shared_project_devices (
    project_id TEXT NOT NULL REFERENCES shared_projects(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL,
    can_control INTEGER NOT NULL CHECK(can_control IN (0,1)),
    can_execute INTEGER NOT NULL CHECK(can_execute IN (0,1)),
    environment_id TEXT REFERENCES shared_environments(id),
    setup_error TEXT,
    PRIMARY KEY(project_id,device_id)
);
-- Preserve existing execution bindings. Ambiguous legacy mappings remain in their
-- original tables and need explicit resolution rather than silently choosing one.
INSERT INTO shared_project_devices(project_id,device_id,can_control,can_execute,environment_id)
SELECT ep.project_id,e.runner_id,0,1,CASE WHEN COUNT(*)=1 THEN MIN(e.id) ELSE NULL END
FROM shared_environment_projects ep JOIN shared_environments e ON e.id=ep.environment_id
GROUP BY ep.project_id,e.runner_id;
CREATE TRIGGER management_project_devices_insert AFTER INSERT ON shared_project_devices BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_project_devices_update AFTER UPDATE ON shared_project_devices WHEN OLD.can_control<>NEW.can_control OR OLD.can_execute<>NEW.can_execute OR OLD.environment_id IS NOT NEW.environment_id OR OLD.setup_error IS NOT NEW.setup_error BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_project_devices_delete AFTER DELETE ON shared_project_devices BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;

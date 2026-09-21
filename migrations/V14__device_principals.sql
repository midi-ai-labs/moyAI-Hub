-- Device enrollment supplies the shared-work actor. Windows names and PC labels
-- are descriptive only; a reviewed association is keyed by the registered ID.
ALTER TABLE identity_users ADD COLUMN credential_kind TEXT NOT NULL DEFAULT 'password'
    CHECK(credential_kind IN ('password','device','local_admin'));
CREATE TABLE identity_device_principals (
    device_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES identity_users(id),
    generation INTEGER NOT NULL DEFAULT 1 CHECK(generation > 0),
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX identity_device_principals_user ON identity_device_principals(user_id);
ALTER TABLE identity_sessions ADD COLUMN device_binding_generation INTEGER;

-- Retain explicit historical grants. An explicit role takes precedence over the
-- default contributor role derived from a project's controller PC selection.
DROP VIEW identity_effective_project_memberships;
CREATE VIEW identity_explicit_project_memberships AS
 SELECT user_id,project_id,CASE MAX(rank) WHEN 3 THEN 'manager' WHEN 2 THEN 'contributor' ELSE 'reader' END AS role FROM (
 SELECT m.user_id,m.project_id,CASE m.role WHEN 'manager' THEN 3 WHEN 'contributor' THEN 2 ELSE 1 END AS rank FROM identity_project_memberships m JOIN identity_users u ON u.id=m.user_id WHERE u.disabled=0
 UNION ALL SELECT m.user_id,p.project_id,CASE p.role WHEN 'manager' THEN 3 WHEN 'contributor' THEN 2 ELSE 1 END FROM identity_group_memberships m JOIN identity_groups g ON g.id=m.group_id JOIN identity_group_project_roles p ON p.group_id=g.id JOIN identity_users u ON u.id=m.user_id WHERE g.disabled=0 AND u.disabled=0
 ) GROUP BY user_id,project_id;
CREATE VIEW identity_effective_project_memberships AS
 SELECT user_id,project_id,role FROM identity_explicit_project_memberships
 UNION ALL
 SELECT DISTINCT b.user_id,d.project_id,'contributor' FROM identity_device_principals b
 JOIN identity_users u ON u.id=b.user_id AND u.disabled=0
 JOIN shared_project_devices d ON d.device_id=b.device_id AND d.can_control=1
 JOIN shared_projects p ON p.id=d.project_id AND p.legacy_devices_unrestricted=0
 WHERE NOT EXISTS(SELECT 1 FROM identity_explicit_project_memberships m WHERE m.user_id=b.user_id AND m.project_id=d.project_id);

CREATE TRIGGER identity_device_binding_insert AFTER INSERT ON identity_device_principals BEGIN
 UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id=NEW.user_id;
 UPDATE shared_management_state SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER identity_device_binding_update AFTER UPDATE ON identity_device_principals BEGIN
 DELETE FROM identity_sessions WHERE audience='shared_work' AND device_id=NEW.device_id;
 DELETE FROM identity_refresh_sessions WHERE device_id=NEW.device_id;
 UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id IN (OLD.user_id,NEW.user_id);
 UPDATE shared_management_state SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER identity_device_binding_delete AFTER DELETE ON identity_device_principals BEGIN
 DELETE FROM identity_sessions WHERE audience='shared_work' AND device_id=OLD.device_id;
 DELETE FROM identity_refresh_sessions WHERE device_id=OLD.device_id;
 UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id=OLD.user_id;
 UPDATE shared_management_state SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER identity_controller_insert AFTER INSERT ON shared_project_devices WHEN NEW.can_control=1 BEGIN
 UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id IN (SELECT user_id FROM identity_device_principals WHERE device_id=NEW.device_id);
END;
CREATE TRIGGER identity_controller_update AFTER UPDATE OF can_control ON shared_project_devices WHEN NEW.can_control<>OLD.can_control BEGIN
 UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id IN (SELECT user_id FROM identity_device_principals WHERE device_id=NEW.device_id);
END;
CREATE TRIGGER identity_controller_delete AFTER DELETE ON shared_project_devices WHEN OLD.can_control=1 BEGIN
 UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id IN (SELECT user_id FROM identity_device_principals WHERE device_id=OLD.device_id);
END;

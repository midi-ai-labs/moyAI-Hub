ALTER TABLE shared_projects ADD COLUMN archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1));
ALTER TABLE identity_users ADD COLUMN authorization_generation INTEGER NOT NULL DEFAULT 1 CHECK(authorization_generation>0);
CREATE TABLE identity_groups(id TEXT PRIMARY KEY,label TEXT NOT NULL,disabled INTEGER NOT NULL DEFAULT 0 CHECK(disabled IN (0,1)));
CREATE TABLE identity_group_memberships(group_id TEXT NOT NULL REFERENCES identity_groups(id) ON DELETE CASCADE,user_id TEXT NOT NULL REFERENCES identity_users(id) ON DELETE CASCADE,PRIMARY KEY(group_id,user_id));
CREATE TABLE identity_group_project_roles(group_id TEXT NOT NULL REFERENCES identity_groups(id) ON DELETE CASCADE,project_id TEXT NOT NULL REFERENCES shared_projects(id) ON DELETE CASCADE,role TEXT NOT NULL CHECK(role IN ('reader','contributor','manager')),PRIMARY KEY(group_id,project_id));
CREATE VIEW identity_effective_project_memberships AS
 SELECT user_id,project_id,CASE MAX(rank) WHEN 3 THEN 'manager' WHEN 2 THEN 'contributor' ELSE 'reader' END AS role FROM (
 SELECT m.user_id,m.project_id,CASE m.role WHEN 'manager' THEN 3 WHEN 'contributor' THEN 2 ELSE 1 END AS rank FROM identity_project_memberships m JOIN identity_users u ON u.id=m.user_id WHERE u.disabled=0
 UNION ALL SELECT m.user_id,p.project_id,CASE p.role WHEN 'manager' THEN 3 WHEN 'contributor' THEN 2 ELSE 1 END FROM identity_group_memberships m JOIN identity_groups g ON g.id=m.group_id JOIN identity_group_project_roles p ON p.group_id=g.id JOIN identity_users u ON u.id=m.user_id WHERE g.disabled=0 AND u.disabled=0
 ) GROUP BY user_id,project_id;

CREATE TABLE shared_management_state(id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL CHECK(revision>0));
INSERT INTO shared_management_state VALUES(1,1);
CREATE TABLE shared_management_receipts(request_id TEXT NOT NULL,actor_id TEXT NOT NULL REFERENCES identity_users(id),payload_hash TEXT NOT NULL,created_at_ms INTEGER NOT NULL,PRIMARY KEY(actor_id,request_id));
CREATE TABLE shared_management_audit(id TEXT PRIMARY KEY,actor_id TEXT NOT NULL REFERENCES identity_users(id),kind TEXT NOT NULL,target_id TEXT,reason TEXT,created_at_ms INTEGER NOT NULL);
ALTER TABLE shared_jobs ADD COLUMN authority_generation INTEGER NOT NULL DEFAULT 1 CHECK(authority_generation>0);
ALTER TABLE shared_jobs ADD COLUMN quota_owner_id TEXT;
UPDATE shared_jobs SET quota_owner_id=requestor_id;
ALTER TABLE shared_attempts ADD COLUMN authority_generation INTEGER NOT NULL DEFAULT 1 CHECK(authority_generation>0);
CREATE TABLE shared_job_environment_ceiling(root_id TEXT NOT NULL REFERENCES shared_jobs(id),environment_id TEXT NOT NULL REFERENCES shared_environments(id),PRIMARY KEY(root_id,environment_id));
INSERT INTO shared_job_environment_ceiling SELECT DISTINCT j.id,ep.environment_id FROM shared_jobs j JOIN shared_environment_projects ep ON ep.project_id=j.project_id WHERE j.parent_id IS NULL;
CREATE TABLE shared_handovers(id TEXT PRIMARY KEY,root_id TEXT NOT NULL REFERENCES shared_jobs(id),old_assignee_id TEXT NOT NULL REFERENCES identity_users(id),new_assignee_id TEXT NOT NULL REFERENCES identity_users(id),requested_by TEXT NOT NULL REFERENCES identity_users(id),state TEXT NOT NULL CHECK(state IN ('pending','completed','cancelled','rejected')),requested_at_ms INTEGER NOT NULL,completed_at_ms INTEGER);
CREATE UNIQUE INDEX shared_handover_pending ON shared_handovers(root_id) WHERE state='pending';
ALTER TABLE shared_approvals ADD COLUMN authority_generation INTEGER NOT NULL DEFAULT 1;
ALTER TABLE shared_approvals ADD COLUMN decider_credential_generation INTEGER;
ALTER TABLE shared_approvals ADD COLUMN decider_authorization_generation INTEGER;
ALTER TABLE shared_approvals ADD COLUMN invalidated_at_ms INTEGER;
-- An old approval has no evidence of its deciding credentials; require a fresh decision.
UPDATE shared_approvals SET invalidated_at_ms=created_at_ms;
CREATE TABLE shared_inbox_reads(user_id TEXT NOT NULL REFERENCES identity_users(id) ON DELETE CASCADE,item_id TEXT NOT NULL,read_at_ms INTEGER NOT NULL,PRIMARY KEY(user_id,item_id));

CREATE TRIGGER identity_direct_insert AFTER INSERT ON identity_project_memberships BEGIN UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id=NEW.user_id; END;
CREATE TRIGGER identity_direct_update AFTER UPDATE ON identity_project_memberships BEGIN UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id=NEW.user_id; END;
CREATE TRIGGER identity_direct_delete AFTER DELETE ON identity_project_memberships BEGIN UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id=OLD.user_id; END;
CREATE TRIGGER identity_group_member_insert AFTER INSERT ON identity_group_memberships BEGIN UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id=NEW.user_id; END;
CREATE TRIGGER identity_group_member_delete AFTER DELETE ON identity_group_memberships BEGIN UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id=OLD.user_id; END;
CREATE TRIGGER identity_group_update AFTER UPDATE OF disabled ON identity_groups BEGIN UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id IN (SELECT user_id FROM identity_group_memberships WHERE group_id=NEW.id); END;
CREATE TRIGGER identity_group_role_insert AFTER INSERT ON identity_group_project_roles BEGIN UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id IN (SELECT user_id FROM identity_group_memberships WHERE group_id=NEW.group_id); END;
CREATE TRIGGER identity_group_role_update AFTER UPDATE ON identity_group_project_roles BEGIN UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id IN (SELECT user_id FROM identity_group_memberships WHERE group_id=NEW.group_id); END;
CREATE TRIGGER identity_group_role_delete AFTER DELETE ON identity_group_project_roles BEGIN UPDATE identity_users SET authorization_generation=authorization_generation+1 WHERE id IN (SELECT user_id FROM identity_group_memberships WHERE group_id=OLD.group_id); END;
CREATE TRIGGER management_identity_users_INSERT AFTER INSERT ON identity_users BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_users_UPDATE AFTER UPDATE OF username,display_name,administrator,disabled,password_hash ON identity_users BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_users_DELETE AFTER DELETE ON identity_users BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_groups_INSERT AFTER INSERT ON identity_groups BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_groups_UPDATE AFTER UPDATE ON identity_groups BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_groups_DELETE AFTER DELETE ON identity_groups BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_group_memberships_INSERT AFTER INSERT ON identity_group_memberships BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_group_memberships_UPDATE AFTER UPDATE ON identity_group_memberships BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_group_memberships_DELETE AFTER DELETE ON identity_group_memberships BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_group_project_roles_INSERT AFTER INSERT ON identity_group_project_roles BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_group_project_roles_UPDATE AFTER UPDATE ON identity_group_project_roles BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_group_project_roles_DELETE AFTER DELETE ON identity_group_project_roles BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_project_memberships_INSERT AFTER INSERT ON identity_project_memberships BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_project_memberships_UPDATE AFTER UPDATE ON identity_project_memberships BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_identity_project_memberships_DELETE AFTER DELETE ON identity_project_memberships BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_projects_INSERT AFTER INSERT ON shared_projects BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_projects_UPDATE AFTER UPDATE ON shared_projects BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_projects_DELETE AFTER DELETE ON shared_projects BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_resources_INSERT AFTER INSERT ON shared_resources BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_resources_UPDATE AFTER UPDATE ON shared_resources BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_resources_DELETE AFTER DELETE ON shared_resources BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_environments_INSERT AFTER INSERT ON shared_environments BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_environments_UPDATE AFTER UPDATE ON shared_environments BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_environments_DELETE AFTER DELETE ON shared_environments BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_environment_projects_INSERT AFTER INSERT ON shared_environment_projects BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_environment_projects_UPDATE AFTER UPDATE ON shared_environment_projects BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER management_shared_environment_projects_DELETE AFTER DELETE ON shared_environment_projects BEGIN UPDATE shared_management_state SET revision=revision+1 WHERE id=1; END;

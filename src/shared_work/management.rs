//! Administrative configuration and explicit, audited recovery share the durable owner.
use super::*;
use crate::identity::{self, Principal, ProjectRole};
use base64::{Engine, engine::general_purpose::STANDARD};
use ring::{
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::num::NonZeroU32;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementCommand {
    pub request_id: String,
    pub expected_revision: String,
    pub request: ManagementRequest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagementRequest {
    PruneSharedData {
        before_ms: u64,
    },
    CreateUser {
        username: String,
        display_name: String,
        password: String,
        administrator: bool,
    },
    UpdateUser {
        user_id: String,
        display_name: String,
        administrator: bool,
        disabled: bool,
    },
    SetPassword {
        user_id: String,
        password: String,
    },
    DeleteUser {
        user_id: String,
    },
    CreateGroup {
        id: String,
        label: String,
    },
    UpdateGroup {
        id: String,
        label: String,
        disabled: bool,
    },
    DeleteGroup {
        id: String,
    },
    GroupMember {
        group_id: String,
        user_id: String,
        member: bool,
    },
    CreateProject {
        id: String,
        label: String,
    },
    UpdateProject {
        id: String,
        label: String,
        archived: bool,
    },
    DeleteProject {
        id: String,
    },
    ProjectMember {
        project_id: String,
        user_id: String,
        role: Option<ProjectRole>,
    },
    GroupProjectRole {
        project_id: String,
        group_id: String,
        role: Option<ProjectRole>,
    },
    CreateEnvironment {
        id: String,
        label: String,
        resource_id: String,
        runner_id: String,
        capacity: u32,
        project_ids: Vec<String>,
        #[serde(default)]
        template_id: Option<String>,
    },
    UpdateEnvironment {
        id: String,
        label: String,
        resource_id: String,
        runner_id: String,
        enabled: bool,
        project_ids: Vec<String>,
    },
    DeleteEnvironment {
        id: String,
    },
    RequestProvision {
        environment_id: String,
        template_id: String,
        generation: u64,
    },
    SetResource {
        id: String,
        capacity: u32,
    },
    DeleteResource {
        id: String,
    },
    SettleAttempt {
        attempt_id: String,
        generation: u64,
        acknowledge_stopped: bool,
        reason: String,
    },
}

fn rows(
    db: &Connection,
    sql: &str,
    read: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<Value>,
) -> Result<Vec<Value>> {
    Ok(db
        .prepare(sql)?
        .query_map([], read)?
        .collect::<std::result::Result<_, _>>()?)
}
fn maintenance(db: &Connection, before: Option<&str>, limit: u32) -> Result<Value> {
    if !(1..=128).contains(&limit) {
        return Err(WorkError::Invalid);
    }
    if let Some(before) = before {
        super::store::id(before)?;
    }
    let total:u64=db.query_row("SELECT COUNT(*) FROM shared_attempts WHERE state IN ('assigned','running','stop_requested')",[],|r|r.get(0))?;
    let mut items=db.prepare("SELECT id,job_id,runner_id,state,uncertainty_reason,updated_at_ms,generation FROM shared_attempts WHERE state IN ('assigned','running','stop_requested') AND (?1 IS NULL OR id<?1) ORDER BY id DESC LIMIT ?2")?.query_map(params![before,limit+1],|r|Ok(json!({"attempt_id":r.get::<_,String>(0)?,"job_id":r.get::<_,String>(1)?,"runner_id":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"uncertainty_reason":r.get::<_,Option<String>>(4)?,"updated_at_ms":r.get::<_,u64>(5)?,"generation":r.get::<_,u64>(6)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    let more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let next = if more {
        items.last().map(|v| v["attempt_id"].clone())
    } else {
        None
    };
    Ok(json!({"items":items,"next_before":next,"total_active":total}))
}
fn snapshot(db: &Connection, actor: &Principal) -> Result<Value> {
    identity::require_admin(db, actor)?;
    let revision: u64 = db.query_row(
        "SELECT revision FROM shared_management_state WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    let users = identity::list_users(db, actor)?;
    let groups = rows(
        db,
        "SELECT id,label,disabled FROM identity_groups ORDER BY id",
        |r| {
            Ok(
                json!({"id":r.get::<_,String>(0)?,"label":r.get::<_,String>(1)?,"disabled":r.get::<_,bool>(2)?}),
            )
        },
    )?;
    let group_memberships = rows(
        db,
        "SELECT group_id,user_id FROM identity_group_memberships ORDER BY group_id,user_id",
        |r| Ok(json!({"group_id":r.get::<_,String>(0)?,"user_id":r.get::<_,String>(1)?})),
    )?;
    let projects = rows(
        db,
        "SELECT id,label,archived FROM shared_projects ORDER BY id",
        |r| {
            Ok(
                json!({"id":r.get::<_,String>(0)?,"label":r.get::<_,String>(1)?,"archived":r.get::<_,bool>(2)?}),
            )
        },
    )?;
    let project_memberships = rows(
        db,
        "SELECT project_id,user_id,role FROM identity_project_memberships ORDER BY project_id,user_id",
        |r| {
            Ok(
                json!({"project_id":r.get::<_,String>(0)?,"user_id":r.get::<_,String>(1)?,"role":r.get::<_,String>(2)?}),
            )
        },
    )?;
    let group_project_roles = rows(
        db,
        "SELECT project_id,group_id,role FROM identity_group_project_roles ORDER BY project_id,group_id",
        |r| {
            Ok(
                json!({"project_id":r.get::<_,String>(0)?,"group_id":r.get::<_,String>(1)?,"role":r.get::<_,String>(2)?}),
            )
        },
    )?;
    let resources = rows(
        db,
        "SELECT id,capacity FROM shared_resources ORDER BY id",
        |r| Ok(json!({"id":r.get::<_,String>(0)?,"capacity":r.get::<_,u32>(1)?})),
    )?;
    let environments = rows(
        db,
        "SELECT e.id,e.label,e.resource_id,e.runner_id,e.enabled,(SELECT json_group_array(project_id) FROM (SELECT project_id FROM shared_environment_projects WHERE environment_id=e.id ORDER BY project_id)) FROM shared_environments e ORDER BY e.id",
        |r| {
            Ok(
                json!({"id":r.get::<_,String>(0)?,"label":r.get::<_,String>(1)?,"resource_id":r.get::<_,String>(2)?,"runner_id":r.get::<_,String>(3)?,"enabled":r.get::<_,bool>(4)?,"project_ids":super::store::decode::<Value>(r.get(5)?,5)?}),
            )
        },
    )?;
    let maintenance = maintenance(db, None, 128)?;
    let templates = rows(
        db,
        "SELECT runner_id,template_id,label FROM shared_runner_templates ORDER BY runner_id,template_id LIMIT 1000",
        |r| {
            Ok(
                json!({"runner_id":r.get::<_,String>(0)?,"id":r.get::<_,String>(1)?,"label":r.get::<_,String>(2)?}),
            )
        },
    )?;
    let provisioning = rows(
        db,
        "SELECT environment_id,runner_id,template_id,state,error,generation FROM shared_provisioning ORDER BY environment_id LIMIT 1000",
        |r| {
            Ok(
                json!({"environment_id":r.get::<_,String>(0)?,"runner_id":r.get::<_,String>(1)?,"template_id":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"error":r.get::<_,Option<String>>(4)?,"generation":r.get::<_,u64>(5)?}),
            )
        },
    )?;
    Ok(
        json!({"revision":revision.to_string(),"users":users,"groups":groups,"group_memberships":group_memberships,"projects":projects,"project_memberships":project_memberships,"group_project_roles":group_project_roles,"environments":environments,"resources":resources,"maintenance":maintenance["items"],"maintenance_next_before":maintenance["next_before"],"maintenance_total":maintenance["total_active"],"templates":templates,"provisioning":provisioning,"storage":super::data::storage_status(db)?}),
    )
}

fn changed(count: usize) -> Result<()> {
    if count == 0 {
        Err(WorkError::NotFound)
    } else {
        Ok(())
    }
}
fn receipt_fingerprint(command: &ManagementCommand, previous: Option<&str>) -> Result<String> {
    let payload = serde_json::to_vec(command)?;
    if !matches!(
        &command.request,
        ManagementRequest::CreateUser { .. } | ManagementRequest::SetPassword { .. }
    ) {
        return Ok(format!("{:x}", Sha256::digest(payload)));
    }
    // A fast hash of a password-bearing JSON command would be a second, weaker
    // offline password verifier. Protect exact-retry evidence at the same work factor.
    if let Some(previous) = previous {
        let (iterations, encoded) = previous
            .strip_prefix("pbkdf2_sha256:")
            .and_then(|v| v.split_once(':'))
            .ok_or(WorkError::Conflict)?;
        let iterations: u32 = iterations.parse().map_err(|_| WorkError::Conflict)?;
        if !(600_000..=2_000_000).contains(&iterations) {
            return Err(WorkError::Conflict);
        }
        let bytes = STANDARD.decode(encoded).map_err(|_| WorkError::Conflict)?;
        if bytes.len() != 48
            || pbkdf2::verify(
                pbkdf2::PBKDF2_HMAC_SHA256,
                NonZeroU32::new(iterations).ok_or(WorkError::Conflict)?,
                &bytes[..16],
                &payload,
                &bytes[16..],
            )
            .is_err()
        {
            return Err(WorkError::Conflict);
        }
        return Ok(previous.into());
    }
    let mut bytes = [0u8; 48];
    let (salt, hash) = bytes.split_at_mut(16);
    SystemRandom::new()
        .fill(salt)
        .map_err(|_| identity::IdentityError::Unavailable)?;
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(600_000).expect("positive work factor"),
        salt,
        &payload,
        hash,
    );
    Ok(format!("pbkdf2_sha256:600000:{}", STANDARD.encode(bytes)))
}
fn exists(db: &Connection, table: &str, id: &str) -> Result<()> {
    // Table names are private constants at every call site, never client input.
    if !db.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),
        [id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(WorkError::NotFound);
    }
    Ok(())
}
fn capacity(db: &Connection, table: &str, limit: u32) -> Result<()> {
    let count: u32 = db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
    if count >= limit {
        Err(WorkError::Capacity)
    } else {
        Ok(())
    }
}
fn no_reference(db: &Connection, sql: &str, id: &str) -> Result<()> {
    if db.query_row(sql, [id], |r| r.get::<_, bool>(0))? {
        Err(WorkError::Conflict)
    } else {
        Ok(())
    }
}
fn environment_projects(db: &Connection, environment: &str, projects: &[String]) -> Result<()> {
    if projects.is_empty() || projects.len() > 128 {
        return Err(WorkError::Invalid);
    }
    let mut seen = std::collections::BTreeSet::new();
    for project in projects {
        super::store::id(project)?;
        if !seen.insert(project) {
            return Err(WorkError::Invalid);
        }
        exists(db, "shared_projects", project)?;
    }
    db.execute(
        "DELETE FROM shared_environment_projects WHERE environment_id=?1",
        [environment],
    )?;
    for project in projects {
        db.execute(
            "INSERT INTO shared_environment_projects VALUES(?1,?2)",
            params![environment, project],
        )?;
    }
    Ok(())
}

impl SharedWork {
    pub fn management_maintenance(
        &self,
        actor: &Principal,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Value> {
        identity::require_admin(&self.db, actor)?;
        maintenance(&self.db, before, limit)
    }
    pub fn management_snapshot(&self, actor: &Principal) -> Result<Value> {
        snapshot(&self.db, actor)
    }
    pub fn manage(
        &mut self,
        actor: &Principal,
        command: &ManagementCommand,
        now: u64,
    ) -> Result<Value> {
        super::store::id(&command.request_id)?;
        super::store::timestamp(now)?;
        let expected: u64 = command
            .expected_revision
            .parse()
            .map_err(|_| WorkError::Invalid)?;
        if expected.to_string() != command.expected_revision {
            return Err(WorkError::Invalid);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        identity::require_admin(&tx, actor)?;
        let previous:Option<String>=tx.query_row("SELECT payload_hash FROM shared_management_receipts WHERE actor_id=?1 AND request_id=?2",params![actor.user_id,command.request_id],|r|r.get(0)).optional()?;
        if let Some(previous) = previous {
            if previous != receipt_fingerprint(command, Some(&previous))? {
                return Err(WorkError::Conflict);
            }
            return snapshot(&tx, actor);
        }
        let revision: u64 = tx.query_row(
            "SELECT revision FROM shared_management_state WHERE id=1",
            [],
            |r| r.get(0),
        )?;
        if revision != expected {
            return Err(WorkError::Conflict);
        }
        capacity(&tx, "shared_management_receipts", 100_000)?;
        let hash = receipt_fingerprint(command, None)?;
        use ManagementRequest::*;
        match &command.request {
            PruneSharedData { before_ms } => {
                super::data::prune(&tx, *before_ms, now)?;
            }
            CreateUser {
                username,
                display_name,
                password,
                administrator,
            } => {
                identity::create_user(
                    &tx,
                    actor,
                    username,
                    display_name,
                    password,
                    *administrator,
                    now,
                )?;
            }
            UpdateUser {
                user_id,
                display_name,
                administrator,
                disabled,
            } => {
                identity::update_user(
                    &tx,
                    actor,
                    user_id,
                    display_name,
                    *administrator,
                    *disabled,
                    now,
                )?;
            }
            SetPassword { user_id, password } => {
                identity::set_password(&tx, actor, user_id, password, now)?;
            }
            DeleteUser { user_id } => {
                // Disable first: preserves the last-administrator invariant even for unused accounts.
                let (display, admin): (String, bool) = tx
                    .query_row(
                        "SELECT display_name,administrator FROM identity_users WHERE id=?1",
                        [user_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?
                    .ok_or(WorkError::NotFound)?;
                no_reference(
                    &tx,
                    "SELECT EXISTS(SELECT 1 FROM shared_jobs WHERE requestor_id=?1 OR assignee_id=?1) OR EXISTS(SELECT 1 FROM shared_approvals WHERE decided_by=?1) OR EXISTS(SELECT 1 FROM shared_handovers WHERE requested_by=?1 OR old_assignee_id=?1 OR new_assignee_id=?1) OR EXISTS(SELECT 1 FROM shared_management_audit WHERE actor_id=?1) OR EXISTS(SELECT 1 FROM shared_management_receipts WHERE actor_id=?1) OR EXISTS(SELECT 1 FROM shared_assets WHERE created_by='user:'||?1)",
                    user_id,
                )?;
                identity::update_user(&tx, actor, user_id, &display, admin, true, now)?;
                tx.execute(
                    "DELETE FROM identity_project_memberships WHERE user_id=?1",
                    [user_id],
                )?;
                changed(tx.execute("DELETE FROM identity_users WHERE id=?1", [user_id])?)?;
            }
            CreateGroup { id, label } => {
                super::store::id(id)?;
                super::store::label(label)?;
                capacity(&tx, "identity_groups", 256)?;
                tx.execute(
                    "INSERT INTO identity_groups(id,label) VALUES(?1,?2)",
                    params![id, label],
                )?;
            }
            UpdateGroup {
                id,
                label,
                disabled,
            } => {
                super::store::label(label)?;
                changed(tx.execute(
                    "UPDATE identity_groups SET label=?2,disabled=?3 WHERE id=?1",
                    params![id, label, disabled],
                )?)?;
            }
            DeleteGroup { id } => {
                changed(tx.execute("DELETE FROM identity_groups WHERE id=?1", [id])?)?;
            }
            GroupMember {
                group_id,
                user_id,
                member,
            } => {
                exists(&tx, "identity_groups", group_id)?;
                exists(&tx, "identity_users", user_id)?;
                if *member {
                    capacity(&tx, "identity_group_memberships", 8192)?;
                    tx.execute(
                        "INSERT OR IGNORE INTO identity_group_memberships VALUES(?1,?2)",
                        params![group_id, user_id],
                    )?;
                } else {
                    tx.execute(
                        "DELETE FROM identity_group_memberships WHERE group_id=?1 AND user_id=?2",
                        params![group_id, user_id],
                    )?;
                }
            }
            CreateProject { id, label } => {
                super::store::id(id)?;
                super::store::label(label)?;
                capacity(&tx, "shared_projects", 1000)?;
                tx.execute(
                    "INSERT INTO shared_projects(id,label) VALUES(?1,?2)",
                    params![id, label],
                )?;
            }
            UpdateProject {
                id,
                label,
                archived,
            } => {
                super::store::label(label)?;
                changed(tx.execute(
                    "UPDATE shared_projects SET label=?2,archived=?3 WHERE id=?1",
                    params![id, label, archived],
                )?)?;
            }
            DeleteProject { id } => {
                no_reference(
                    &tx,
                    "SELECT EXISTS(SELECT 1 FROM shared_jobs WHERE project_id=?1)",
                    id,
                )?;
                tx.execute(
                    "DELETE FROM identity_project_memberships WHERE project_id=?1",
                    [id],
                )?;
                tx.execute(
                    "DELETE FROM shared_environment_projects WHERE project_id=?1",
                    [id],
                )?;
                changed(tx.execute("DELETE FROM shared_projects WHERE id=?1", [id])?)?;
            }
            ProjectMember {
                project_id,
                user_id,
                role,
            } => {
                identity::set_project_role(&tx, actor, project_id, user_id, *role, now)?;
            }
            GroupProjectRole {
                project_id,
                group_id,
                role,
            } => {
                exists(&tx, "shared_projects", project_id)?;
                exists(&tx, "identity_groups", group_id)?;
                if let Some(role) = role {
                    capacity(&tx, "identity_group_project_roles", 8192)?;
                    tx.execute("INSERT INTO identity_group_project_roles VALUES(?1,?2,?3) ON CONFLICT(group_id,project_id) DO UPDATE SET role=excluded.role",params![group_id,project_id,role.as_str()])?;
                } else {
                    tx.execute("DELETE FROM identity_group_project_roles WHERE group_id=?1 AND project_id=?2",params![group_id,project_id])?;
                }
            }
            CreateEnvironment {
                id,
                label,
                resource_id,
                runner_id,
                capacity: slots,
                project_ids,
                template_id,
            } => {
                super::store::id(id)?;
                super::store::id(resource_id)?;
                super::store::id(runner_id)?;
                super::store::label(label)?;
                if !(1..=128).contains(slots) {
                    return Err(WorkError::Invalid);
                }
                capacity(&tx, "shared_environments", 1000)?;
                let old: Option<u32> = tx
                    .query_row(
                        "SELECT capacity FROM shared_resources WHERE id=?1",
                        [resource_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if old.is_some_and(|old| old != *slots) {
                    return Err(WorkError::Conflict);
                }
                tx.execute(
                    "INSERT OR IGNORE INTO shared_resources VALUES(?1,?2)",
                    params![resource_id, slots],
                )?;
                tx.execute("INSERT INTO shared_environments(id,label,resource_id,runner_id) VALUES(?1,?2,?3,?4)",params![id,label,resource_id,runner_id])?;
                environment_projects(&tx, id, project_ids)?;
                if let Some(template_id) = template_id {
                    super::provisioning::enqueue(
                        &tx,
                        &ProvisionRequest {
                            environment_id: id.clone(),
                            template_id: template_id.clone(),
                            generation: 0,
                        },
                        now,
                    )?;
                }
            }
            UpdateEnvironment {
                id,
                label,
                resource_id,
                runner_id,
                enabled,
                project_ids,
            } => {
                super::store::id(resource_id)?;
                super::store::id(runner_id)?;
                super::store::label(label)?;
                exists(&tx, "shared_resources", resource_id)?;
                let (old_resource, old_runner): (String, String) = tx
                    .query_row(
                        "SELECT resource_id,runner_id FROM shared_environments WHERE id=?1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?
                    .ok_or(WorkError::NotFound)?;
                if old_resource != *resource_id || old_runner != *runner_id {
                    // A queued checkpoint and any active process retain their original environment binding.
                    no_reference(
                        &tx,
                        "SELECT EXISTS(SELECT 1 FROM shared_jobs WHERE environment_id=?1) OR EXISTS(SELECT 1 FROM shared_job_environment_ceiling WHERE environment_id=?1) OR EXISTS(SELECT 1 FROM shared_provisioning WHERE environment_id=?1)",
                        id,
                    )?;
                }
                changed(tx.execute("UPDATE shared_environments SET label=?2,resource_id=?3,runner_id=?4,enabled=?5 WHERE id=?1",params![id,label,resource_id,runner_id,enabled])?)?;
                environment_projects(&tx, id, project_ids)?;
            }
            DeleteEnvironment { id } => {
                no_reference(
                    &tx,
                    "SELECT EXISTS(SELECT 1 FROM shared_jobs WHERE environment_id=?1) OR EXISTS(SELECT 1 FROM shared_job_environment_ceiling WHERE environment_id=?1)",
                    id,
                )?;
                tx.execute(
                    "DELETE FROM shared_environment_projects WHERE environment_id=?1",
                    [id],
                )?;
                tx.execute(
                    "DELETE FROM shared_provisioning WHERE environment_id=?1",
                    [id],
                )?;
                changed(tx.execute("DELETE FROM shared_environments WHERE id=?1", [id])?)?;
            }
            RequestProvision {
                environment_id,
                template_id,
                generation,
            } => {
                super::provisioning::enqueue(
                    &tx,
                    &ProvisionRequest {
                        environment_id: environment_id.clone(),
                        template_id: template_id.clone(),
                        generation: *generation,
                    },
                    now,
                )?;
            }
            SetResource {
                id,
                capacity: slots,
            } => {
                super::store::id(id)?;
                if !(1..=128).contains(slots) {
                    return Err(WorkError::Invalid);
                }
                let occupied:u32=tx.query_row("SELECT COUNT(*) FROM shared_attempts WHERE resource_id=?1 AND state IN ('assigned','running','stop_requested')",[id],|r|r.get(0))?;
                if occupied > *slots {
                    return Err(WorkError::Conflict);
                }
                tx.execute("INSERT INTO shared_resources VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET capacity=excluded.capacity",params![id,slots])?;
            }
            DeleteResource { id } => {
                no_reference(
                    &tx,
                    "SELECT EXISTS(SELECT 1 FROM shared_environments WHERE resource_id=?1) OR EXISTS(SELECT 1 FROM shared_attempts WHERE resource_id=?1)",
                    id,
                )?;
                changed(tx.execute("DELETE FROM shared_resources WHERE id=?1", [id])?)?;
            }
            SettleAttempt {
                attempt_id,
                generation,
                acknowledge_stopped,
                reason,
            } => {
                super::store::label(reason)?;
                if !acknowledge_stopped {
                    return Err(WorkError::Invalid);
                }
                super::handover::settle_stopped(&tx, attempt_id, *generation, now)?;
                tx.execute(
                    "INSERT INTO shared_management_audit VALUES(?1,?2,'settle_attempt',?3,?4,?5)",
                    params![
                        ulid::Ulid::new().to_string(),
                        actor.user_id,
                        attempt_id,
                        reason,
                        now
                    ],
                )?;
            }
        }
        let request = serde_json::to_value(&command.request)?;
        tx.execute(
            "INSERT INTO shared_management_audit VALUES(?1,?2,?3,NULL,NULL,?4)",
            params![
                ulid::Ulid::new().to_string(),
                actor.user_id,
                request["kind"].as_str().ok_or(WorkError::Invalid)?,
                now
            ],
        )?;
        tx.execute(
            "INSERT INTO shared_management_receipts VALUES(?1,?2,?3,?4)",
            params![command.request_id, actor.user_id, hash, now],
        )?;
        tx.execute(
            "UPDATE shared_management_state SET revision=revision+1 WHERE id=1",
            [],
        )?;
        // Return configuration even when the command deliberately invalidates this login.
        // The caller must reauthenticate before another command.
        let result = if identity::require_admin(&tx, actor).is_ok() {
            snapshot(&tx, actor)?
        } else {
            json!({"reauthenticate":true})
        };
        tx.commit()?;
        if matches!(&command.request, ManagementRequest::PruneSharedData { .. }) {
            // The committed tombstones remain authoritative if filesystem cleanup fails.
            // Uploads retry bounded collection under the same SharedWork owner.
            let _ = super::blob_store::gc(&self.db);
        }
        Ok(result)
    }
}

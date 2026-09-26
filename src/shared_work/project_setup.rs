//! One administrative project edit owns people, PC purposes and managed provisioning.
use super::*;
use crate::identity::{self, Principal, ProjectRole};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const LOCAL_FOLDER_TEMPLATE: &str = "local-folder";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMember {
    pub user_id: String,
    pub role: ProjectRole,
}

pub(crate) fn require_project_controller(
    db: &Connection,
    project: &str,
    device: &str,
) -> Result<()> {
    if db.query_row("SELECT EXISTS(SELECT 1 FROM shared_projects p WHERE p.id=?1 AND p.deleted_at_ms IS NULL AND (p.legacy_devices_unrestricted=1 OR EXISTS(SELECT 1 FROM shared_project_devices d WHERE d.project_id=p.id AND d.device_id=?2 AND d.can_control=1)))", params![project,device], |r|r.get::<_,bool>(0))? { Ok(()) } else { Err(WorkError::Forbidden) }
}

pub(crate) fn controlled_projects(db: &Connection, device: &str) -> Result<Vec<String>> {
    Ok(db.prepare("SELECT p.id FROM shared_projects p WHERE p.deleted_at_ms IS NULL AND (p.legacy_devices_unrestricted=1 OR EXISTS(SELECT 1 FROM shared_project_devices d WHERE d.project_id=p.id AND d.device_id=?1 AND d.can_control=1)) ORDER BY p.id")?.query_map([device], |r|r.get(0))?.collect::<std::result::Result<_,_>>()?)
}

/// Explain only this authenticated person's readiness; disclose no unallowed project names.
pub(crate) fn project_access(db: &Connection, user: &str, device: &str) -> Result<&'static str> {
    let memberships: Vec<String> = db.prepare("SELECT m.project_id FROM identity_effective_project_memberships m JOIN shared_projects p ON p.id=m.project_id WHERE m.user_id=?1 AND p.archived=0 AND p.deleted_at_ms IS NULL")?
        .query_map([user], |r| r.get(0))?.collect::<std::result::Result<_,_>>()?;
    if memberships.is_empty() {
        return Ok("no_membership");
    }
    let allowed = controlled_projects(db, device)?;
    Ok(if memberships.iter().any(|id| allowed.contains(id)) {
        "ready"
    } else {
        "device_not_allowed"
    })
}

/// Legacy projects keep their old environment grants. Explicit PC projects
/// accept only the exact workspace binding chosen by the project owner.
pub(crate) fn environment_matches_project(
    db: &Connection,
    project: &str,
    environment: &str,
    runner: &str,
) -> Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM shared_projects p WHERE p.id=?1 AND p.archived=0 AND p.deleted_at_ms IS NULL AND (p.legacy_devices_unrestricted=1 OR EXISTS(SELECT 1 FROM shared_project_devices d WHERE d.project_id=p.id AND d.device_id=?3 AND d.environment_id=?2 AND d.can_execute=1)))",params![project,environment,runner],|r|r.get(0))?)
}
pub(super) fn validate_legacy_environment_edit(
    db: &Connection,
    environment: &str,
    projects: &[String],
) -> Result<()> {
    let runner: String = db.query_row(
        "SELECT runner_id FROM shared_environments WHERE id=?1",
        [environment],
        |r| r.get(0),
    )?;
    for project in projects {
        if !environment_matches_project(db, project, environment, &runner)? {
            return Err(WorkError::Conflict);
        }
    }
    let bound=db.prepare("SELECT d.project_id FROM shared_project_devices d JOIN shared_projects p ON p.id=d.project_id WHERE d.environment_id=?1 AND d.can_execute=1 AND p.legacy_devices_unrestricted=0 AND p.archived=0")?.query_map([environment],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
    if bound.iter().any(|project| !projects.contains(project)) {
        return Err(WorkError::Conflict);
    }
    Ok(())
}

fn selections(ids: &[String]) -> Result<BTreeSet<&str>> {
    if ids.len() > 128 {
        return Err(WorkError::Capacity);
    }
    let mut seen = BTreeSet::new();
    for id in ids {
        super::store::id(id)?;
        if !seen.insert(id.as_str()) {
            return Err(WorkError::Invalid);
        }
    }
    Ok(seen)
}

/// Revocation is durable before any new assignment can observe the changed
/// device purpose. Existing attempts and retained apps remain visible until
/// the runner reports that they actually stopped.
pub(super) fn revoke_device_usage(
    db: &Connection,
    project: &str,
    device: &str,
    old_control: bool,
    old_execute: bool,
    next_control: bool,
    next_execute: bool,
    now: u64,
) -> Result<()> {
    let lost_control = old_control && !next_control;
    let lost_execute = old_execute && !next_execute;
    if !lost_control && !lost_execute {
        return Ok(());
    }
    let environment: Option<String> = db.query_row(
        "SELECT environment_id FROM shared_project_devices WHERE project_id=?1 AND device_id=?2",
        params![project, device], |row| row.get(0),
    ).optional()?.flatten();
    let roots = db.prepare(
        "SELECT DISTINCT root.id FROM shared_jobs root JOIN shared_jobs member ON member.root_id=root.id
         WHERE root.project_id=?1 AND root.id=root.root_id AND root.state NOT IN ('succeeded','failed','cancelled')
           AND ((?3 AND (root.controller_device_id=?2 OR root.origin_device_id=?2))
                OR (?4 AND member.environment_id=?5)) ORDER BY root.id",
    )?.query_map(params![project, device, lost_control, lost_execute, environment],
        |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for root in roots {
        super::store::cancel_root_in_transaction(db, &root, now)?;
    }
    db.execute(
        "UPDATE shared_service_leases SET stop_requested_at_ms=COALESCE(stop_requested_at_ms,?6),updated_at_ms=?6
         WHERE state IN ('active','uncertain') AND service_id IN (
           SELECT s.service_id FROM shared_service_leases s
           JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id
           JOIN shared_jobs root ON root.id=j.root_id
           WHERE j.project_id=?1 AND ((?3 AND (root.controller_device_id=?2 OR root.origin_device_id=?2))
                                    OR (?4 AND j.environment_id=?5)))",
        params![project, device, lost_control, lost_execute, environment, now],
    )?;
    if lost_execute {
        if let Some(environment) = environment {
            db.execute(
                "DELETE FROM shared_environment_projects WHERE project_id=?1 AND environment_id=?2",
                params![project, environment],
            )?;
            db.execute("UPDATE shared_environments SET enabled=0 WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM shared_environment_projects WHERE environment_id=?1)",[&environment])?;
        }
    }
    Ok(())
}

pub(super) fn revoke_project_usage(db: &Connection, project: &str, now: u64) -> Result<()> {
    let roots = db
        .prepare(
            "SELECT id FROM shared_jobs WHERE project_id=?1 AND id=root_id AND parent_id IS NULL
         AND state NOT IN ('succeeded','failed','cancelled') ORDER BY id",
        )?
        .query_map([project], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for root in roots {
        super::store::cancel_root_in_transaction(db, &root, now)?;
    }
    db.execute("UPDATE shared_service_leases SET stop_requested_at_ms=COALESCE(stop_requested_at_ms,?2),updated_at_ms=?2 WHERE state IN ('active','uncertain') AND service_id IN (SELECT s.service_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id WHERE j.project_id=?1)",params![project,now])?;
    db.execute("UPDATE shared_environments SET enabled=0 WHERE id IN (SELECT environment_id FROM shared_environment_projects WHERE project_id=?1) AND NOT EXISTS(SELECT 1 FROM shared_environment_projects WHERE environment_id=shared_environments.id AND project_id<>?1)",[project])?;
    db.execute(
        "DELETE FROM shared_environment_projects WHERE project_id=?1",
        [project],
    )?;
    db.execute("UPDATE shared_project_devices SET can_control=0,can_execute=0,environment_id=NULL,setup_error=NULL WHERE project_id=?1",[project])?;
    Ok(())
}

pub(super) fn save(
    db: &Connection,
    actor: &Principal,
    id: &str,
    label: &str,
    archived: bool,
    members: Option<&[ProjectMember]>,
    controllers: &[String],
    runners: &[String],
    now: u64,
) -> Result<()> {
    super::store::id(id)?;
    super::store::label(label)?;
    if members.is_some_and(|members| members.len() > 512) {
        return Err(WorkError::Capacity);
    }
    let controls = selections(controllers)?;
    let executes = selections(runners)?;
    let mut people = BTreeSet::new();
    for member in members.unwrap_or_default() {
        super::store::id(&member.user_id)?;
        if !people.insert(&member.user_id) {
            return Err(WorkError::Invalid);
        }
    }
    let count: u32 = db.query_row("SELECT COUNT(*) FROM shared_projects", [], |r| r.get(0))?;
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_projects WHERE id=?1)",
        [id],
        |r| r.get(0),
    )?;
    let previously_archived: bool = db
        .query_row(
            "SELECT archived FROM shared_projects WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false);
    if !exists && count >= 1000 {
        return Err(WorkError::Capacity);
    }
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_projects WHERE id=?1 AND deleted_at_ms IS NOT NULL)",
        [id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(WorkError::Conflict);
    }
    db.execute("INSERT INTO shared_projects(id,label,archived,legacy_devices_unrestricted) VALUES(?1,?2,?3,0) ON CONFLICT(id) DO UPDATE SET label=excluded.label,archived=excluded.archived,legacy_devices_unrestricted=0",params![id,label,archived])?;
    if let Some(members) = members {
        // The legacy editor starts from explicit memberships. Saving that explicit list
        // removes hidden group inheritance for this project without changing other projects.
        db.execute(
            "DELETE FROM identity_group_project_roles WHERE project_id=?1",
            [id],
        )?;
        let old = db
            .prepare("SELECT user_id FROM identity_project_memberships WHERE project_id=?1")?
            .query_map([id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for user in old {
            if !people.contains(&user) {
                identity::set_project_role(db, actor, id, &user, None, now)?;
            }
        }
        for member in members {
            let previous: Option<String> = db
            .query_row(
                "SELECT role FROM identity_project_memberships WHERE project_id=?1 AND user_id=?2",
                params![id, member.user_id],
                |r| r.get(0),
            )
            .optional()?;
            if previous.as_deref() != Some(member.role.as_str()) {
                identity::set_project_role(db, actor, id, &member.user_id, Some(member.role), now)?;
            }
        }
    }
    let old_devices = db
        .prepare("SELECT device_id,can_control,can_execute FROM shared_project_devices WHERE project_id=?1")?
        .query_map([id], |r| Ok((r.get::<_, String>(0)?,r.get::<_,bool>(1)?,r.get::<_,bool>(2)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (device, old_control, old_execute) in old_devices {
        let control = controls.contains(device.as_str());
        let execute = executes.contains(device.as_str());
        revoke_device_usage(
            db,
            id,
            &device,
            old_control,
            old_execute,
            control,
            execute,
            now,
        )?;
        if !control && !execute {
            db.execute("UPDATE shared_project_devices SET can_control=0,can_execute=0,environment_id=NULL,setup_error=NULL WHERE project_id=?1 AND device_id=?2",params![id,device])?;
        }
    }
    for device in controls.union(&executes) {
        db.execute("INSERT INTO shared_project_devices(project_id,device_id,can_control,can_execute) VALUES(?1,?2,?3,?4) ON CONFLICT(project_id,device_id) DO UPDATE SET participation_generation=CASE WHEN shared_project_devices.can_control=0 AND shared_project_devices.can_execute=0 THEN shared_project_devices.participation_generation+1 ELSE shared_project_devices.participation_generation END,can_control=excluded.can_control,can_execute=excluded.can_execute,environment_id=CASE WHEN excluded.can_execute=0 THEN NULL ELSE shared_project_devices.environment_id END,setup_error=CASE WHEN excluded.can_execute=0 THEN NULL ELSE shared_project_devices.setup_error END",params![id,device,controls.contains(device),executes.contains(device)])?;
    }
    // Old APIs may have created a project after the migration. Adopt its one
    // existing binding; do not create a second workspace for the same selection.
    db.execute("UPDATE shared_project_devices AS d SET environment_id=(SELECT MIN(e.id) FROM shared_environments e JOIN shared_environment_projects ep ON ep.environment_id=e.id WHERE ep.project_id=d.project_id AND e.runner_id=d.device_id) WHERE d.project_id=?1 AND d.can_execute=1 AND d.environment_id IS NULL AND (SELECT COUNT(*) FROM shared_environments e JOIN shared_environment_projects ep ON ep.environment_id=e.id WHERE ep.project_id=d.project_id AND e.runner_id=d.device_id)=1",[id])?;
    // Remove only this project's grants; other projects and historical work keep
    // their original environment/resource identities.
    let removed = db.prepare("SELECT e.id FROM shared_environments e JOIN shared_environment_projects ep ON ep.environment_id=e.id WHERE ep.project_id=?1 AND (?2 OR NOT EXISTS(SELECT 1 FROM shared_project_devices d WHERE d.project_id=?1 AND d.environment_id=e.id AND d.can_execute=1))")?.query_map(params![id,archived], |r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
    for environment in removed {
        db.execute(
            "DELETE FROM shared_environment_projects WHERE environment_id=?1 AND project_id=?2",
            params![environment, id],
        )?;
        db.execute("UPDATE shared_environments SET enabled=0 WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM shared_environment_projects WHERE environment_id=?1)",[&environment])?;
    }
    for runner in executes {
        synchronize(db, Some(runner), now)?;
    }
    db.execute("UPDATE shared_environments SET label=?2 WHERE id IN (SELECT environment_id FROM shared_project_devices WHERE project_id=?1 AND environment_id LIKE 'project-%') AND label<>?2",params![id,label])?;
    if previously_archived && !archived {
        db.execute("UPDATE shared_environments SET enabled=1 WHERE id IN (SELECT environment_id FROM shared_project_devices WHERE project_id=?1 AND can_execute=1) AND NOT EXISTS(SELECT 1 FROM shared_provisioning s WHERE s.environment_id=shared_environments.id AND s.state<>'ready')",[id])?;
    }
    Ok(())
}

fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let bytes = serde_json::to_vec(parts).expect("strings serialize");
    format!("{prefix}-{:x}", Sha256::digest(bytes))
}

/// Add one reviewed PC without replacing people, other PCs or inherited roles.
pub(super) fn assign_device(
    db: &Connection,
    project: &str,
    device: &str,
    control: bool,
    execute: bool,
    now: u64,
) -> Result<()> {
    super::store::id(project)?;
    super::store::id(device)?;
    if !control && !execute {
        return Err(WorkError::Invalid);
    }
    let available: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM shared_projects WHERE id=?1 AND archived=0 AND deleted_at_ms IS NULL AND legacy_devices_unrestricted=0)",[project],|r|r.get(0))?;
    if !available {
        return Err(WorkError::Conflict);
    }
    let count: u32 = db.query_row("SELECT COUNT(*) FROM shared_project_devices WHERE project_id=?1 AND (can_control=1 OR can_execute=1) AND device_id<>?2",params![project,device],|r|r.get(0))?;
    if count >= 128 {
        return Err(WorkError::Capacity);
    }
    // This approval operation adds purposes. Removing a purpose remains an
    // explicit project edit, so a repeated join cannot revoke existing usage.
    db.execute("INSERT INTO shared_project_devices(project_id,device_id,can_control,can_execute) VALUES(?1,?2,?3,?4) ON CONFLICT(project_id,device_id) DO UPDATE SET participation_generation=CASE WHEN shared_project_devices.can_control=0 AND shared_project_devices.can_execute=0 THEN shared_project_devices.participation_generation+1 ELSE shared_project_devices.participation_generation END,can_control=MAX(shared_project_devices.can_control,excluded.can_control),can_execute=MAX(shared_project_devices.can_execute,excluded.can_execute)",params![project,device,control,execute])?;
    if execute {
        synchronize(db, Some(device), now)?;
    }
    Ok(())
}

pub(super) fn synchronize(db: &Connection, runner: Option<&str>, now: u64) -> Result<()> {
    let rows = db.prepare("SELECT d.project_id,d.device_id,d.environment_id,p.label FROM shared_project_devices d JOIN shared_projects p ON p.id=d.project_id WHERE d.can_execute=1 AND p.archived=0 AND p.deleted_at_ms IS NULL AND (?1 IS NULL OR d.device_id=?1) ORDER BY d.project_id,d.device_id")?.query_map([runner],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
    for (project, device, existing, label) in rows {
        if let Some(environment) = existing {
            let owner: String = db.query_row(
                "SELECT runner_id FROM shared_environments WHERE id=?1",
                [&environment],
                |r| r.get(0),
            )?;
            if owner != device {
                return Err(WorkError::Conflict);
            }
            db.execute("UPDATE shared_project_devices SET setup_error=NULL WHERE project_id=?1 AND device_id=?2",params![project,device])?;
            db.execute(
                "INSERT OR IGNORE INTO shared_environment_projects VALUES(?1,?2)",
                params![environment, project],
            )?;
            continue;
        }
        let legacy_count:u32=db.query_row("SELECT COUNT(*) FROM shared_environments e JOIN shared_environment_projects p ON p.environment_id=e.id WHERE p.project_id=?1 AND e.runner_id=?2",params![project,device],|r|r.get(0))?;
        if legacy_count > 1 {
            db.execute("UPDATE shared_project_devices SET setup_error=?3 WHERE project_id=?1 AND device_id=?2",params![project,device,"このPCには既存の実行場所が複数あります。管理者が既存設定を整理するまで、新しい場所は作成しません。"])?;
            continue;
        }
        let resources = db.prepare("SELECT DISTINCT e.resource_id,r.capacity FROM shared_environments e JOIN shared_resources r ON r.id=e.resource_id WHERE e.runner_id=?1")?.query_map([&device],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u32>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        if resources.len() > 1
            || resources
                .first()
                .is_some_and(|(_, capacity)| *capacity != 1)
        {
            db.execute("UPDATE shared_project_devices SET setup_error=?3 WHERE project_id=?1 AND device_id=?2",params![project,device,"このPCの既存の実行設定が複数の共有枠を使用しています。端末の実行設定を確認してください。"])?;
            continue;
        }
        // This workspace belongs to this execution participation. A revoked
        // and later re-enabled purpose must get a fresh local folder binding.
        let environment = format!("project-{}", ulid::Ulid::new());
        let resource = resources
            .first()
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| stable_id("device", &[&device]));
        let capacity: Option<u32> = db
            .query_row(
                "SELECT capacity FROM shared_resources WHERE id=?1",
                [&resource],
                |r| r.get(0),
            )
            .optional()?;
        if capacity.is_some_and(|capacity| capacity != 1) {
            return Err(WorkError::Conflict);
        }
        let count: u32 =
            db.query_row("SELECT COUNT(*) FROM shared_environments", [], |r| r.get(0))?;
        let pc_count: u32 = db.query_row(
            "SELECT COUNT(*) FROM shared_environments WHERE runner_id=?1",
            [&device],
            |r| r.get(0),
        )?;
        if pc_count >= super::provisioning::MAX_RUNNER_ENVIRONMENTS || count >= 1000 {
            let reason = if pc_count >= super::provisioning::MAX_RUNNER_ENVIRONMENTS {
                "このPCの実行場所が上限に達しています。不要な既存設定を整理してから再試行してください。"
            } else {
                "Hubの実行場所が上限に達しています。不要な既存設定を整理してから再試行してください。"
            };
            db.execute("UPDATE shared_project_devices SET setup_error=?3 WHERE project_id=?1 AND device_id=?2",params![project,device,reason])?;
            continue;
        }
        db.execute(
            "INSERT OR IGNORE INTO shared_resources(id,capacity) VALUES(?1,1)",
            [&resource],
        )?;
        db.execute("INSERT INTO shared_environments(id,label,resource_id,runner_id,enabled) VALUES(?1,?2,?3,?4,0)",params![environment,label,resource,device])?;
        db.execute(
            "INSERT INTO shared_environment_projects VALUES(?1,?2)",
            params![environment, project],
        )?;
        db.execute("UPDATE shared_project_devices SET environment_id=?3,setup_error=NULL WHERE project_id=?1 AND device_id=?2",params![project,device,environment])?;
        super::provisioning::enqueue(
            db,
            &ProvisionRequest {
                environment_id: environment,
                template_id: LOCAL_FOLDER_TEMPLATE.into(),
                generation: 0,
                state: None,
            },
            now,
        )?;
    }
    Ok(())
}

fn preparation_state(
    executor: bool,
    environment: Option<&str>,
    setup_error: Option<&str>,
    state: Option<&str>,
    template: Option<&str>,
) -> String {
    if !executor {
        "not_selected"
    } else if setup_error.is_some() {
        "failed"
    } else if environment.is_none() {
        "waiting_setup"
    } else if state == Some("pending") && template == Some(LOCAL_FOLDER_TEMPLATE) {
        "waiting_local_folder"
    } else {
        state.unwrap_or("ready")
    }
    .into()
}

pub(super) fn execution_devices(
    db: &Connection,
    project: &str,
) -> Result<Vec<ExecutionDeviceStatus>> {
    Ok(db.prepare("SELECT d.device_id,d.environment_id,d.setup_error,s.state,s.error,s.template_id FROM shared_project_devices d JOIN shared_projects p ON p.id=d.project_id LEFT JOIN shared_provisioning s ON s.environment_id=d.environment_id WHERE d.project_id=?1 AND d.can_execute=1 AND p.archived=0 AND p.deleted_at_ms IS NULL ORDER BY d.device_id LIMIT 128")?.query_map([project], |r| {
        let environment: Option<String> = r.get(1)?;
        let setup_error: Option<String> = r.get(2)?;
        let state: Option<String> = r.get(3)?;
        let error: Option<String> = r.get(4)?;
        let template: Option<String> = r.get(5)?;
        Ok(ExecutionDeviceStatus {
            device_id: r.get(0)?,
            device_label: None,
            preparation_state: preparation_state(true, environment.as_deref(), setup_error.as_deref(), state.as_deref(), template.as_deref()),
            environment_id: environment,
            error: setup_error.or(error),
        })
    })?.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub(super) fn device_rows(db: &Connection, device: Option<&str>) -> Result<Vec<Value>> {
    let mut result = db.prepare("SELECT d.project_id,p.label,d.device_id,d.can_control,d.can_execute,d.environment_id,d.setup_error,s.state,s.error,d.participation_generation,EXISTS(SELECT 1 FROM identity_device_principals b JOIN identity_effective_project_memberships m ON m.user_id=b.user_id AND m.project_id=d.project_id WHERE b.device_id=d.device_id AND m.role IN ('contributor','manager')),s.template_id FROM shared_project_devices d JOIN shared_projects p ON p.id=d.project_id LEFT JOIN shared_provisioning s ON s.environment_id=d.environment_id WHERE p.deleted_at_ms IS NULL AND (?1 IS NULL OR d.device_id=?1) AND (?1 IS NULL OR (p.archived=0 AND (d.can_control=1 OR d.can_execute=1))) ORDER BY d.project_id,d.device_id")?.query_map([device],|r|{
        let controller:bool=r.get(3)?;
        let executor:bool=r.get(4)?;
        let environment:Option<String>=r.get(5)?;
        let setup_error:Option<String>=r.get(6)?;
        let state:Option<String>=r.get(7)?;
        let error:Option<String>=r.get(8)?;
        let participation_generation:u64=r.get(9)?;
        let contributor:bool=r.get(10)?;
        let template:Option<String>=r.get(11)?;
        let state=preparation_state(executor,environment.as_deref(),setup_error.as_deref(),state.as_deref(),template.as_deref());
        Ok(json!({"id":r.get::<_,String>(0)?,"project_id":r.get::<_,String>(0)?,"label":r.get::<_,String>(1)?,"device_id":r.get::<_,String>(2)?,"can_control":controller,"can_submit":controller && contributor,"can_execute":executor,"participation_generation":participation_generation,"environment_id":environment,"preparation_state":state,"error":setup_error.or(error)}))
    })?.collect::<std::result::Result<Vec<_>,_>>()?;
    if let Some(device) = device {
        // Legacy unrestricted control remains visible until an administrator
        // explicitly saves that project's PC list.
        let legacy=db.prepare("SELECT id,label,EXISTS(SELECT 1 FROM identity_device_principals b JOIN identity_effective_project_memberships m ON m.user_id=b.user_id AND m.project_id=shared_projects.id WHERE b.device_id=?1 AND m.role IN ('contributor','manager')) FROM shared_projects WHERE archived=0 AND deleted_at_ms IS NULL AND legacy_devices_unrestricted=1")?.query_map([device],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        for (id, label, can_submit) in legacy {
            if let Some(row) = result.iter_mut().find(|row| row["id"] == id) {
                row["can_control"] = json!(true);
                row["can_submit"] = json!(can_submit);
            } else {
                result.push(json!({"id":id,"project_id":id,"label":label,"device_id":device,"can_control":true,"can_submit":can_submit,"can_execute":false,"participation_generation":1,"environment_id":null,"preparation_state":"not_selected","error":null}));
            }
        }
    }
    Ok(result)
}

impl SharedWork {
    pub fn leave_project(
        &mut self,
        project: &str,
        device: &str,
        expected_participation_generation: u64,
        now: u64,
    ) -> Result<Value> {
        super::store::id(project)?;
        super::store::id(device)?;
        super::store::timestamp(now)?;
        if expected_participation_generation == 0 {
            return Err(WorkError::Invalid);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<(bool,bool,u64)> = tx.query_row(
            "SELECT d.can_control,d.can_execute,d.participation_generation FROM shared_project_devices d JOIN shared_projects p ON p.id=d.project_id WHERE d.project_id=?1 AND d.device_id=?2 AND p.legacy_devices_unrestricted=0",
            params![project,device],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).optional()?;
        let (control, execute, generation) = previous.ok_or(WorkError::NotFound)?;
        if generation != expected_participation_generation {
            return Err(WorkError::Conflict);
        }
        if control || execute {
            revoke_device_usage(&tx, project, device, control, execute, false, false, now)?;
            tx.execute("UPDATE shared_project_devices SET can_control=0,can_execute=0,environment_id=NULL,setup_error=NULL WHERE project_id=?1 AND device_id=?2",params![project,device])?;
        }
        tx.commit()?;
        Ok(json!({"left":true,"project_id":project,"participation_generation":generation}))
    }

    pub fn require_project_controller(&self, project: &str, device: &str) -> Result<()> {
        require_project_controller(&self.db, project, device)
    }
    pub fn controlled_projects(&self, device: &str) -> Result<Vec<String>> {
        controlled_projects(&self.db, device)
    }
    pub fn device_projects(&self, device: &str) -> Result<Value> {
        super::store::id(device)?;
        let revision: u64 = self.db.query_row(
            "SELECT revision FROM shared_management_state WHERE id=1",
            [],
            |r| r.get(0),
        )?;
        Ok(json!({"revision":revision.to_string(),"projects":device_rows(&self.db,Some(device))?}))
    }
}

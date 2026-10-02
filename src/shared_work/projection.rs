//! Read projections of the job/resource owner, filtered by current human membership.
use super::*;
use crate::identity::{self, Principal, ProjectRole};
use rusqlite::{OptionalExtension, params};

const ENVIRONMENT_PAGE: usize = 20;
const OCCUPANT_PREVIEW: usize = 5;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkPerson {
    pub user_id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: String,
    pub conversation_id: String,
    pub conversation_epoch: u64,
    pub origin_session_ref: Option<String>,
    pub root_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub state: JobState,
    pub environment_id: String,
    pub environment_label: String,
    pub device_label: Option<String>,
    pub requestor: WorkPerson,
    pub assignee: WorkPerson,
    pub wait_reason: Option<String>,
    pub uncertainty_reason: Option<String>,
    pub can_cancel: bool,
    pub can_continue: bool,
    pub can_revise: bool,
    pub revises_job_id: Option<String>,
    pub revision: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub start_before_ms: Option<u64>,
    pub runner_contact: RunnerContact,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OriginJobView {
    pub project_id: String,
    pub job: JobSummary,
    pub artifacts: Vec<super::Asset>,
    pub more_artifacts: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OriginServiceView {
    pub project_id: String,
    pub service: super::ServiceLeaseView,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OriginWorkView {
    pub origin_session_ref: String,
    pub jobs: Vec<OriginJobView>,
    pub retained_services: Vec<OriginServiceView>,
    pub hidden_active_work: bool,
    pub next_job_before: Option<String>,
    pub next_service_before: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationHistoryTurn {
    pub job: JobSummary,
    pub input: Value,
    pub result: Option<Value>,
    pub artifacts: Vec<super::Asset>,
    pub more_artifacts: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationHistoryPage {
    pub project_id: String,
    pub conversation_id: String,
    pub conversation_epoch: u64,
    pub snapshot: u64,
    pub jobs: Vec<ConversationHistoryTurn>,
    pub next_before: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceOccupant {
    pub job_id: String,
    pub title: String,
    pub project_id: String,
    pub project_label: String,
    pub user: WorkPerson,
    pub state: JobState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvironmentStatus {
    pub id: String,
    pub label: String,
    pub resource_id: String,
    pub runner_id: String,
    pub device_label: Option<String>,
    pub enabled: bool,
    pub capacity: u32,
    pub occupied: u32,
    pub occupants: Vec<ResourceOccupant>,
    pub other_occupants: u32,
    pub additional_visible_occupants: u32,
    pub runner_contact: RunnerContact,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionDeviceStatus {
    pub device_id: String,
    pub device_label: Option<String>,
    pub environment_id: Option<String>,
    pub preparation_state: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectStatus {
    pub project_id: String,
    pub jobs: Vec<JobSummary>,
    pub environments: Vec<EnvironmentStatus>,
    pub execution_devices: Vec<ExecutionDeviceStatus>,
    pub next_before: Option<String>,
    pub next_environment_before: Option<String>,
}

fn person(db: &Connection, user: &str) -> Result<WorkPerson> {
    Ok(db.query_row(
        "SELECT id,display_name FROM identity_users WHERE id=?1",
        [user],
        |r| {
            Ok(WorkPerson {
                user_id: r.get(0)?,
                display_name: r.get(1)?,
            })
        },
    )?)
}

impl SharedWork {
    pub fn conversation_history(
        &self,
        actor: &Principal,
        project: &str,
        conversation: &str,
        snapshot: Option<u64>,
        before: Option<u64>,
        limit: u32,
        now: u64,
    ) -> Result<ConversationHistoryPage> {
        store::id(project)?;
        store::id(conversation)?;
        store::timestamp(now)?;
        if !(1..=50).contains(&limit) || before.is_some() && snapshot.is_none() {
            return Err(WorkError::Invalid);
        }
        let role =
            identity::require_project_role(&self.db, &actor.user_id, project, ProjectRole::Reader)?;
        if !super::conversation_catalog::anchor_exists(&self.db, project, conversation)? {
            return Err(WorkError::NotFound);
        }
        if !super::conversation_catalog::visible(&self.db, project, conversation)? {
            return Err(WorkError::NotFound);
        }
        let snapshot=snapshot.unwrap_or(self.db.query_row(
            "SELECT COALESCE(MAX(rowid),0) FROM shared_jobs WHERE project_id=?1 AND conversation_id=?2 AND id=root_id AND parent_id IS NULL",
            params![project,conversation],|r|r.get(0),
        )?);
        if snapshot == 0
            || snapshot > i64::MAX as u64
            || before.is_some_and(|v| v == 0 || v > snapshot)
        {
            return Err(WorkError::Invalid);
        }
        let mut statement = self.db.prepare(
            "SELECT rowid,id FROM shared_jobs WHERE project_id=?1 AND conversation_id=?2
             AND id=root_id AND parent_id IS NULL AND rowid<=?3 AND (?4 IS NULL OR rowid<?4)
             ORDER BY rowid DESC LIMIT ?5",
        )?;
        let mut selected = statement
            .query_map(
                params![project, conversation, snapshot, before, limit + 1],
                |r| Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?)),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let has_more = selected.len() > limit as usize;
        selected.truncate(limit as usize);
        let next_before = if has_more {
            selected.last().map(|(rowid, _)| *rowid)
        } else {
            None
        };
        let jobs = selected
            .into_iter()
            .map(|(_, id)| {
                let job = store::load(&self.db, &id)?;
                let (artifacts, more_artifacts) = self.job_artifact_preview(&id)?;
                Ok(ConversationHistoryTurn {
                    job: self.summary(actor, role, &job, now)?,
                    input: job.input,
                    result: job.result,
                    artifacts,
                    more_artifacts,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ConversationHistoryPage {
            project_id: project.into(),
            conversation_id: conversation.into(),
            conversation_epoch: super::stop_scope::project_conversation_epoch(
                &self.db,
                project,
                conversation,
            )?,
            snapshot,
            jobs,
            next_before,
        })
    }
    pub fn origin_work_for_device(
        &self,
        actor: &Principal,
        device: &str,
        origin: &str,
        job_before: Option<&str>,
        service_before: Option<&str>,
        now: u64,
    ) -> Result<OriginWorkView> {
        self.origin_work_page_for_device(
            actor,
            device,
            origin,
            job_before,
            service_before,
            true,
            true,
            now,
        )
    }

    /// Private conversation projection: the opaque ref alone never grants
    /// access. Every returned row must still belong to this exact controller
    /// device, original requestor and a currently readable project.
    pub fn origin_work_page_for_device(
        &self,
        actor: &Principal,
        device: &str,
        origin: &str,
        job_before: Option<&str>,
        service_before: Option<&str>,
        include_jobs: bool,
        include_services: bool,
        now: u64,
    ) -> Result<OriginWorkView> {
        super::store::id(device)?;
        super::store::id(origin)?;
        if let Some(before) = job_before {
            super::store::id(before)?;
        }
        if let Some(before) = service_before {
            super::store::id(before)?;
        }
        super::store::timestamp(now)?;
        let mut selected = if include_jobs {
            let mut statement = self.db.prepare("SELECT j.id,j.project_id FROM shared_jobs j JOIN shared_jobs origin ON origin.id=j.conversation_id AND origin.id=origin.root_id AND origin.parent_id IS NULL AND origin.conversation_id=origin.id AND origin.project_id=j.project_id AND origin.origin_session_ref=j.origin_session_ref AND origin.origin_device_id=j.origin_device_id JOIN shared_projects p ON p.id=j.project_id AND p.archived=0 JOIN identity_effective_project_memberships m ON m.project_id=j.project_id AND m.user_id=?2 WHERE j.origin_session_ref=?1 AND j.origin_device_id=?3 AND origin.requestor_id=?2 AND (?4 IS NULL OR j.id<?4) AND (p.legacy_devices_unrestricted=1 OR EXISTS(SELECT 1 FROM shared_project_devices d WHERE d.project_id=j.project_id AND d.device_id=?3 AND d.can_control=1)) ORDER BY j.id DESC LIMIT 51")?;
            statement
                .query_map(params![origin, actor.user_id, device, job_before], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        let next_job_before = if selected.len() > 50 {
            selected.truncate(50);
            selected.last().map(|(id, _)| id.clone())
        } else {
            None
        };
        let jobs = selected
            .into_iter()
            .map(|(id, project_id)| {
                let job = super::store::load(&self.db, &id)?;
                let role = identity::require_project_role(
                    &self.db,
                    &actor.user_id,
                    &project_id,
                    ProjectRole::Reader,
                )?;
                let (artifacts, more_artifacts) = self.job_artifact_preview(&id)?;
                Ok(OriginJobView {
                    project_id,
                    job: self.summary(actor, role, &job, now)?,
                    artifacts,
                    more_artifacts,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let (retained_services, next_service_before) = if include_services {
            self.origin_services(actor, device, origin, service_before, now)?
        } else {
            (Vec::new(), None)
        };
        // A revoked grant must not make an active remote job or lease look like
        // an empty local session to its original owner. Only expose presence:
        // no project, job, participant or service identity crosses the grant.
        let hidden_active_work: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_jobs root JOIN shared_jobs origin
               ON origin.id=root.conversation_id AND origin.id=origin.root_id
              AND origin.parent_id IS NULL AND origin.conversation_id=origin.id
              AND origin.project_id=root.project_id
              AND origin.origin_session_ref=root.origin_session_ref
              AND origin.origin_device_id=root.origin_device_id
             WHERE root.id=root.root_id AND root.parent_id IS NULL
               AND root.origin_session_ref=?1 AND root.origin_device_id=?2
               AND origin.requestor_id=?3
               AND (root.state NOT IN ('succeeded','failed','cancelled') OR EXISTS(
                    SELECT 1 FROM shared_service_leases s
                    JOIN shared_attempts a ON a.id=s.attempt_id
                    JOIN shared_jobs j ON j.id=a.job_id
                    WHERE j.root_id=root.id AND s.state IN ('active','uncertain')))
               AND NOT EXISTS(SELECT 1 FROM shared_projects p
                    JOIN identity_effective_project_memberships m
                      ON m.project_id=p.id AND m.user_id=?3
                    WHERE p.id=root.project_id AND p.archived=0
                      AND (p.legacy_devices_unrestricted=1 OR EXISTS(
                        SELECT 1 FROM shared_project_devices d
                        WHERE d.project_id=p.id AND d.device_id=?2 AND d.can_control=1))))",
            params![origin, device, actor.user_id],
            |row| row.get(0),
        )?;
        Ok(OriginWorkView {
            origin_session_ref: origin.into(),
            jobs,
            retained_services,
            hidden_active_work,
            next_job_before,
            next_service_before,
        })
    }

    pub fn job_detail(&self, actor: &Principal, id: &str) -> Result<Value> {
        self.job_detail_at(actor, id, crate::service::now_ms())
    }
    pub(super) fn job_detail_at(&self, actor: &Principal, id: &str, now: u64) -> Result<Value> {
        let job = self.job(actor, id)?;
        let role = identity::require_project_role(
            &self.db,
            &actor.user_id,
            &job.project_id,
            ProjectRole::Reader,
        )?;
        let summary = self.summary(actor, role, &job, now)?;
        let mut value = serde_json::to_value(&job)?;
        value["can_cancel"] = Value::Bool(summary.can_cancel);
        value["can_continue"] = Value::Bool(summary.can_continue);
        value["can_revise"] = Value::Bool(summary.can_revise);
        value["conversation_epoch"] = serde_json::to_value(summary.conversation_epoch)?;
        value["wait_reason"] = serde_json::to_value(summary.wait_reason)?;
        value["uncertainty_reason"] = serde_json::to_value(summary.uncertainty_reason)?;
        value["runner_contact"] = serde_json::to_value(summary.runner_contact)?;
        value["retained_services"] = serde_json::to_value(self.project_services(
            actor,
            &job.project_id,
            Some(&job.conversation_id),
            now,
        )?)?;
        Ok(value)
    }
    pub fn status(
        &self,
        actor: &Principal,
        project: &str,
        before: Option<&str>,
        limit: u32,
        environment_before: Option<&str>,
    ) -> Result<ProjectStatus> {
        self.status_at(
            actor,
            project,
            before,
            limit,
            environment_before,
            crate::service::now_ms(),
        )
    }
    pub(super) fn status_at(
        &self,
        actor: &Principal,
        project: &str,
        before: Option<&str>,
        limit: u32,
        environment_before: Option<&str>,
        now: u64,
    ) -> Result<ProjectStatus> {
        self.status_at_for_device(actor, project, before, limit, environment_before, now, None)
    }
    pub fn status_for_device(
        &self,
        actor: &Principal,
        project: &str,
        before: Option<&str>,
        limit: u32,
        environment_before: Option<&str>,
        device: Option<&str>,
    ) -> Result<ProjectStatus> {
        self.status_at_for_device(
            actor,
            project,
            before,
            limit,
            environment_before,
            crate::service::now_ms(),
            device,
        )
    }
    fn status_at_for_device(
        &self,
        actor: &Principal,
        project: &str,
        before: Option<&str>,
        limit: u32,
        environment_before: Option<&str>,
        now: u64,
        device: Option<&str>,
    ) -> Result<ProjectStatus> {
        if let Some(device) = device {
            self.require_project_controller(project, device)?;
        }
        let controlled = device
            .map(|id| self.controlled_projects(id))
            .transpose()?
            .map(|ids| serde_json::to_string(&ids))
            .transpose()?;
        let role =
            identity::require_project_role(&self.db, &actor.user_id, project, ProjectRole::Reader)?;
        let jobs = self.list(actor, project, before, limit)?;
        let next_before = if let Some(last) = jobs.last() {
            let more: bool = self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM shared_jobs WHERE project_id=?1 AND id<?2)",
                params![project, last.id],
                |r| r.get(0),
            )?;
            more.then(|| last.id.clone())
        } else {
            None
        };
        let summaries = jobs
            .iter()
            .map(|job| self.summary(actor, role, job, now))
            .collect::<Result<Vec<_>>>()?;
        let mut environments = {
            let mut statement = self.db.prepare("SELECT e.id,e.label,e.resource_id,e.runner_id,e.enabled,r.capacity FROM shared_environments e JOIN shared_resources r ON r.id=e.resource_id JOIN shared_environment_projects ep ON ep.environment_id=e.id WHERE ep.project_id=?1 AND (?2 IS NULL OR e.id<?2) ORDER BY e.id DESC LIMIT ?3")?;
            statement
                .query_map(
                    params![project, environment_before, ENVIRONMENT_PAGE + 1],
                    |r| {
                        Ok(EnvironmentStatus {
                            id: r.get(0)?,
                            label: r.get(1)?,
                            resource_id: r.get(2)?,
                            runner_id: r.get(3)?,
                            device_label: None,
                            enabled: r.get(4)?,
                            capacity: r.get(5)?,
                            occupied: 0,
                            occupants: vec![],
                            other_occupants: 0,
                            additional_visible_occupants: 0,
                            runner_contact: self.environment_contact(
                                &r.get::<_, String>(3)?,
                                &r.get::<_, String>(0)?,
                                now,
                            ),
                        })
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let next_environment_before = if environments.len() > ENVIRONMENT_PAGE {
            environments.truncate(ENVIRONMENT_PAGE);
            environments.last().map(|env| env.id.clone())
        } else {
            None
        };
        for environment in &mut environments {
            environment.occupied =
                super::store::resource_occupancy(&self.db, &environment.resource_id)?;
            let visible: u32 = self.db.query_row("SELECT COUNT(*) FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id JOIN identity_effective_project_memberships m ON m.project_id=j.project_id AND m.user_id=?2 WHERE a.resource_id=?1 AND a.state IN ('assigned','running','stop_requested') AND (?3 IS NULL OR j.project_id IN (SELECT value FROM json_each(?3)))", params![environment.resource_id, actor.user_id, controlled], |r|r.get(0))?;
            let mut statement = self.db.prepare("SELECT j.id,j.title,j.project_id,p.label,u.id,u.display_name,j.state FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id JOIN shared_projects p ON p.id=j.project_id JOIN identity_users u ON u.id=j.assignee_id JOIN identity_effective_project_memberships m ON m.project_id=j.project_id AND m.user_id=?2 WHERE a.resource_id=?1 AND a.state IN ('assigned','running','stop_requested') AND (?3 IS NULL OR j.project_id IN (SELECT value FROM json_each(?3))) ORDER BY a.created_at_ms,a.id LIMIT ?4")?;
            environment.occupants = statement
                .query_map(
                    params![
                        environment.resource_id,
                        actor.user_id,
                        controlled,
                        OCCUPANT_PREVIEW
                    ],
                    |r| {
                        Ok(ResourceOccupant {
                            job_id: r.get(0)?,
                            title: r.get(1)?,
                            project_id: r.get(2)?,
                            project_label: r.get(3)?,
                            user: WorkPerson {
                                user_id: r.get(4)?,
                                display_name: r.get(5)?,
                            },
                            state: super::store::decode(
                                format!("\"{}\"", r.get::<_, String>(6)?),
                                6,
                            )?,
                        })
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            environment.other_occupants = environment.occupied.saturating_sub(visible);
            environment.additional_visible_occupants =
                visible.saturating_sub(environment.occupants.len() as u32);
        }
        Ok(ProjectStatus {
            project_id: project.into(),
            jobs: summaries,
            environments,
            execution_devices: super::project_setup::execution_devices(&self.db, project)?,
            next_before,
            next_environment_before,
        })
    }

    fn summary(
        &self,
        actor: &Principal,
        role: ProjectRole,
        job: &Job,
        now: u64,
    ) -> Result<JobSummary> {
        let (environment_label, enabled, runner_id): (String, bool, String) = self.db.query_row(
            "SELECT label,enabled,runner_id FROM shared_environments WHERE id=?1",
            [&job.environment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let uncertainty_reason:Option<String> = self.db.query_row("SELECT uncertainty_reason FROM shared_attempts WHERE job_id=?1 AND state IN ('assigned','running','stop_requested')", [&job.id], |r|r.get(0)).optional()?.flatten();
        let runner_contact = self.job_contact(job, now)?;
        let wait_reason = if super::deadlines::is_expired_result(job) {
            Some(if job.state.terminal() {
                super::deadlines::EXPIRED_MESSAGE.into()
            } else {
                "開始期限を過ぎました。実行状態が不明なため、停止が確認されるまで利用枠を保持します。".into()
            })
        } else if uncertainty_reason.is_some() {
            Some("実行結果の照合待ちです。停止が確認されるまで利用枠を保持します。".into())
        } else if matches!(
            job.state,
            JobState::Assigned | JobState::Running | JobState::Cancelling
        ) && runner_contact.state != RunnerContactState::Recent
        {
            Some("実行端末との共有制御通信の応答を確認できません。仕事の結果は未確定で、利用枠を保持します。".into())
        } else {
            let approval_pending:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM shared_approvals p JOIN shared_attempts a ON a.id=p.attempt_id WHERE a.job_id=?1 AND a.state='running' AND p.consumed_at_ms IS NULL AND p.expires_at_ms>?2)",params![job.id,now],|r|r.get(0))?;
            match job.state {
                JobState::Running if approval_pending => {
                    Some("操作の承認と実行端末への回答を待っています。".into())
                }
                JobState::Queued if !enabled => Some("この環境の新規受付は停止しています。".into()),
                JobState::Queued
                    if super::handover::may_continue(&self.db, job, &job.environment_id)
                        .is_err() =>
                {
                    Some("担当者の現在の利用権限を確認する必要があります。".into())
                }
                JobState::Queued => {
                    match super::store::check_job_capacity(&self.db, &runner_id, job) {
                        Ok(_) => Some("実行端末の受付を待っています。".into()),
                        Err(WorkError::Capacity) => {
                            Some("ほかの仕事が使っている共通利用枠が空くのを待っています。".into())
                        }
                        Err(error) => return Err(error),
                    }
                }
                JobState::Assigned => Some("実行端末が開始を確認しています。".into()),
                JobState::WaitingChild => Some("子の仕事の結果を待っています。".into()),
                JobState::Cancelling => Some("実行端末の停止確認を待っています。".into()),
                _ => None,
            }
        };
        Ok(JobSummary {
            runner_contact,
            start_before_ms: job.start_before_ms,
            id: job.id.clone(),
            conversation_id: job.conversation_id.clone(),
            conversation_epoch: super::stop_scope::project_conversation_epoch(&self.db,&job.project_id,&job.conversation_id)?,
            origin_session_ref: job.origin_session_ref.clone(),
            root_id: job.root_id.clone(),
            parent_id: job.parent_id.clone(),
            title: job.title.clone(),
            state: job.state.clone(),
            environment_id: job.environment_id.clone(),
            environment_label,
            device_label: None,
            requestor: person(&self.db, &job.requestor_id)?,
            assignee: person(&self.db, &job.assignee_id)?,
            wait_reason,
            uncertainty_reason,
            can_cancel: job.id == job.root_id
                && !job.state.terminal()
                && job.state != JobState::Cancelling
                && role >= ProjectRole::Contributor
                && actor.user_id == job.requestor_id,
            can_revise: actor.user_id == job.requestor_id && role >= ProjectRole::Contributor
                && super::conversation_catalog::can_revise(&self.db,job)?,
            revises_job_id: job.revises_job_id.clone(),
            can_continue: job.state.terminal()
                && super::conversation_catalog::is_latest_root(&self.db,job)?
                && !super::conversation_catalog::execution_pending(&self.db,&job.project_id,&job.conversation_id)?
                && !super::conversation_catalog::deletion_requested(&self.db,&job.project_id,&job.conversation_id)?
                && super::stop_scope::validate_continuation(&self.db,job).is_ok()
                && role >= ProjectRole::Contributor
                && actor.user_id == job.requestor_id
                && super::store::may_use(
                    &self.db,
                    &actor.user_id,
                    &job.project_id,
                    &job.environment_id,
                )
                .is_ok()
                && self.db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM shared_assets WHERE job_id=?1 AND kind='archive' AND purged_at_ms IS NULL)",
                    [&job.id],
                    |r| r.get::<_, bool>(0),
                )?,
            revision: job.revision,
            created_at_ms: job.created_at_ms,
            updated_at_ms: job.updated_at_ms,
        })
    }
}

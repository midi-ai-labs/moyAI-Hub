//! A handover changes continuation authority only at a proven execution boundary.
use super::*;
use crate::identity::{self, Principal, ProjectRole};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoverRequest {
    pub expected_revision: u64,
    pub new_assignee_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoverView {
    pub candidates: Vec<WorkPerson>,
    pub pending: Option<Value>,
    pub can_handover: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinueJob {
    pub request_id: String,
    pub expected_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_participation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_epoch: Option<u64>,
    pub prompt: String,
    #[serde(default)]
    pub input_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_before_ms: Option<u64>,
}

pub(super) fn held(db: &Connection, root: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_handovers WHERE root_id=?1 AND state='pending')",
        [root],
        |r| r.get(0),
    )?)
}
pub(super) fn may_continue(db: &Connection, job: &Job, environment: &str) -> Result<()> {
    super::store::may_use(db, &job.assignee_id, &job.project_id, environment)?;
    if !db.query_row("SELECT EXISTS(SELECT 1 FROM shared_job_environment_ceiling WHERE root_id=?1 AND environment_id=?2)",params![job.root_id,environment],|r|r.get::<_,bool>(0))? {return Err(WorkError::Forbidden)}
    Ok(())
}
fn may_handover(db: &Connection, actor: &Principal, job: &Job) -> Result<()> {
    let role = identity::require_project_role(
        db,
        &actor.user_id,
        &job.project_id,
        ProjectRole::Contributor,
    )?;
    if actor.user_id != job.assignee_id && role != ProjectRole::Manager {
        return Err(WorkError::Forbidden);
    }
    Ok(())
}
pub(super) fn settle(db: &Connection, root_id: &str, now: u64) -> Result<()> {
    let pending: Option<(String, String)> = db
        .query_row(
            "SELECT id,new_assignee_id FROM shared_handovers WHERE root_id=?1 AND state='pending'",
            [root_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((id, target)) = pending else {
        return Ok(());
    };
    let root = super::store::load(db, root_id)?;
    if root.state.terminal() || root.state == JobState::Cancelling {
        db.execute(
            "UPDATE shared_handovers SET state='cancelled',completed_at_ms=?2 WHERE id=?1",
            params![id, now],
        )?;
        return Ok(());
    }
    let active:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id WHERE j.root_id=?1 AND a.state IN ('assigned','running','stop_requested'))",[root_id],|r|r.get(0))?;
    if active {
        return Ok(());
    }
    if identity::require_project_role(db, &target, &root.project_id, ProjectRole::Contributor)
        .is_err()
    {
        db.execute(
            "UPDATE shared_handovers SET state='rejected',completed_at_ms=?2 WHERE id=?1",
            params![id, now],
        )?;
        return Ok(());
    }
    // Retain only the existing ceiling intersected with the new principal's current
    // project/environment grant. Future configuration changes cannot widen this root.
    db.execute("DELETE FROM shared_job_environment_ceiling WHERE root_id=?1 AND environment_id NOT IN (SELECT e.id FROM shared_environments e JOIN shared_environment_projects ep ON ep.environment_id=e.id JOIN shared_projects p ON p.id=ep.project_id WHERE ep.project_id=?2 AND e.enabled=1 AND p.archived=0)",params![root_id,root.project_id])?;
    db.execute("UPDATE shared_jobs SET assignee_id=?2,authority_generation=authority_generation+1,revision=revision+1,updated_at_ms=?3 WHERE root_id=?1 AND state NOT IN ('succeeded','failed','cancelled')",params![root_id,target,now])?;
    db.execute("UPDATE shared_approvals SET invalidated_at_ms=COALESCE(invalidated_at_ms,?2) WHERE attempt_id IN (SELECT a.id FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id WHERE j.root_id=?1)",params![root_id,now])?;
    db.execute(
        "UPDATE shared_handovers SET state='completed',completed_at_ms=?2 WHERE id=?1",
        params![id, now],
    )?;
    Ok(())
}

/// Explicit operator attestation is the only administrative capacity-release path.
/// It never retries an attempt or pretends a missing process succeeded.
pub(super) fn settle_stopped(
    db: &Connection,
    attempt_id: &str,
    generation: u64,
    now: u64,
) -> Result<()> {
    let (job_id, actual, state): (String, u64, String) = db
        .query_row(
            "SELECT job_id,generation,state FROM shared_attempts WHERE id=?1",
            [attempt_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or(WorkError::NotFound)?;
    if actual != generation || !matches!(state.as_str(), "assigned" | "running" | "stop_requested")
    {
        return Err(WorkError::Conflict);
    }
    let job = super::store::load(db, &job_id)?;
    let root = super::store::load(db, &job.root_id)?;
    let terminal = if matches!(root.state, JobState::Cancelling | JobState::Cancelled) {
        "cancelled"
    } else {
        "failed"
    };
    db.execute(
        "UPDATE shared_attempts SET state=?2,updated_at_ms=?3 WHERE id=?1",
        params![attempt_id, terminal, now],
    )?;
    db.execute("UPDATE shared_jobs SET state=?2,result_json=?4,revision=revision+1,updated_at_ms=?3 WHERE id=?1",params![job_id,terminal,now,serde_json::json!({"error":"operator_confirmed_stopped"}).to_string()])?;
    if terminal == "cancelled" {
        super::store::settle_cancel(db, &job.root_id, now)?;
    } else if let Some(parent) = &job.parent_id {
        db.execute("UPDATE shared_jobs SET state='queued',revision=revision+1,updated_at_ms=?3 WHERE id=?1 AND awaiting_child_id=?2 AND state='waiting_child'",params![parent,job.id,now])?;
        super::deadlines::expire_job(db, parent, now)?;
    }
    settle(db, &job.root_id, now)
}

impl SharedWork {
    pub fn continue_job(
        &mut self,
        actor: &Principal,
        id: &str,
        request: &ContinueJob,
        now: u64,
    ) -> Result<Job> {
        self.continue_job_for_device(actor, None, id, request, now)
    }

    pub fn continue_job_for_device(
        &mut self,
        actor: &Principal,
        controller_device: Option<&str>,
        id: &str,
        request: &ContinueJob,
        now: u64,
    ) -> Result<Job> {
        super::store::id(&request.request_id)?;
        super::store::timestamp(now)?;
        if request.prompt.trim().is_empty() || request.prompt.len() > 32 * 1024 {
            return Err(WorkError::Invalid);
        }
        let input = serde_json::json!({"version":2,"prompt":request.prompt,"input_refs":request.input_refs});
        let encoded = super::store::json(&input)?;
        if encoded.len() > 48 * 1024 {
            return Err(WorkError::Invalid);
        }
        let payload = serde_json::json!({"continue_from":id,"request":request}).to_string();
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = super::store::load(&tx, id)?;
        may_handover(&tx, actor, &previous)?;
        if !super::conversation_catalog::visible(
            &tx,
            &previous.project_id,
            &previous.conversation_id,
        )? {
            return Err(WorkError::NotFound);
        }
        if let Some(device) = controller_device {
            let generation:Option<u64>=tx.query_row(
                "SELECT d.participation_generation FROM shared_project_devices d JOIN shared_projects p ON p.id=d.project_id WHERE d.project_id=?1 AND d.device_id=?2 AND d.can_control=1 AND p.archived=0 AND p.deleted_at_ms IS NULL",
                params![previous.project_id,device],|row|row.get(0),
            ).optional()?;
            let generation=match generation {
                Some(value)=>value,
                None if tx.query_row("SELECT EXISTS(SELECT 1 FROM shared_projects WHERE id=?1 AND archived=0 AND deleted_at_ms IS NULL AND legacy_devices_unrestricted=1)",[&previous.project_id],|row|row.get::<_,bool>(0))?=>1,
                None=>return Err(WorkError::Forbidden),
            };
            if request.project_participation.unwrap_or(1) != generation {
                return Err(WorkError::Conflict);
            }
        }
        super::store::may_use(
            &tx,
            &actor.user_id,
            &previous.project_id,
            &previous.environment_id,
        )?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT id,request_json FROM shared_jobs WHERE requestor_id=?1 AND request_id=?2",
                params![actor.user_id, request.request_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, old)) = existing {
            if old != payload {
                return Err(WorkError::Conflict);
            }
            return super::store::load(&tx, &id);
        }
        if super::conversation_catalog::deletion_requested(
            &tx,
            &previous.project_id,
            &previous.conversation_id,
        )? {
            return Err(WorkError::Conflict);
        }
        if !super::conversation_catalog::is_latest_root(&tx, &previous)?
            || super::conversation_catalog::execution_pending(
                &tx,
                &previous.project_id,
                &previous.conversation_id,
            )?
        {
            return Err(WorkError::Conflict);
        }
        super::stop_scope::validate_continuation(&tx, &previous)?;
        if request.conversation_epoch.unwrap_or(0)
            != super::stop_scope::project_conversation_epoch(
                &tx,
                &previous.project_id,
                &previous.conversation_id,
            )?
        {
            return Err(WorkError::Conflict);
        }
        if !previous.state.terminal() || previous.revision != request.expected_revision {
            return Err(WorkError::Conflict);
        }
        let start_before_ms = super::deadlines::resolve(request.start_before_ms, now)?;
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_assets WHERE job_id=?1 AND kind='archive' AND purged_at_ms IS NULL)",
            [id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(WorkError::Conflict);
        }
        if !tx.query_row("SELECT EXISTS(SELECT 1 FROM shared_job_environment_ceiling WHERE root_id=?1 AND environment_id=?2)",params![previous.root_id,previous.environment_id],|r|r.get::<_,bool>(0))?{return Err(WorkError::Forbidden)}
        super::data::validate_inputs(&tx, &previous.project_id, &input)?;
        let budget: u32 = tx.query_row(
            "SELECT descendant_budget FROM shared_jobs WHERE id=?1",
            [&previous.root_id],
            |r| r.get(0),
        )?;
        super::store::root_capacity(&tx, budget)?;
        let next = ulid::Ulid::new().to_string();
        tx.execute("INSERT INTO shared_jobs(id,root_id,conversation_id,origin_session_ref,origin_device_id,origin_turn_revision,required_capability,project_id,environment_id,requestor_id,assignee_id,request_id,request_json,title,input_json,state,descendant_budget,revision,created_at_ms,updated_at_ms,quota_owner_id) VALUES(?1,?1,?2,?3,(SELECT origin_device_id FROM shared_jobs WHERE id=?14),(SELECT origin_turn_revision FROM shared_jobs WHERE id=?14),(SELECT required_capability FROM shared_jobs WHERE id=?14),?4,?5,?6,?6,?7,?8,?9,?10,'queued',?11,1,?12,?12,?13)",params![next,previous.conversation_id,previous.origin_session_ref,previous.project_id,previous.environment_id,actor.user_id,request.request_id,payload,previous.title,encoded,budget,now,previous.quota_owner_id,previous.id])?;
        super::conversation_catalog::appended_root(
            &tx,
            &previous.project_id,
            &previous.conversation_id,
            now,
        )?;
        tx.execute("UPDATE shared_jobs SET controller_device_id=?2,controller_participation_generation=?3 WHERE id=?1",params![next,controller_device,request.project_participation])?;
        tx.execute(
            "UPDATE shared_jobs SET start_before_ms=?2 WHERE id=?1",
            params![next, start_before_ms],
        )?;
        tx.execute("INSERT INTO shared_job_environment_ceiling SELECT ?1,c.environment_id FROM shared_job_environment_ceiling c JOIN shared_environments e ON e.id=c.environment_id JOIN shared_environment_projects ep ON ep.environment_id=e.id WHERE c.root_id=?2 AND e.enabled=1 AND ep.project_id=?3",params![next,previous.root_id,previous.project_id])?;
        tx.execute(
            "INSERT INTO shared_continuations VALUES(?1,?2)",
            params![next, id],
        )?;
        let result = super::store::load(&tx, &next)?;
        super::data::bind_inputs(&tx, &result)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn handover_view(&self, actor: &Principal, id: &str) -> Result<HandoverView> {
        let job = self.job(actor, id)?;
        let can_handover = job.id == job.root_id
            && !job.state.terminal()
            && job.state != JobState::Cancelling
            && may_handover(&self.db, actor, &job).is_ok();
        let candidates = if can_handover {
            self.db.prepare("SELECT u.id,u.display_name FROM identity_users u JOIN identity_effective_project_memberships m ON m.user_id=u.id WHERE m.project_id=?1 AND m.role IN ('contributor','manager') AND u.id<>?2 ORDER BY u.display_name,u.id LIMIT 4096")?.query_map(params![job.project_id,job.assignee_id],|r|Ok(WorkPerson{user_id:r.get(0)?,display_name:r.get(1)?}))?.collect::<std::result::Result<Vec<_>,_>>()?
        } else {
            Vec::new()
        };
        let pending=self.db.query_row("SELECT new_assignee_id,requested_by,requested_at_ms FROM shared_handovers WHERE root_id=?1 AND state='pending'",[&job.root_id],|r|Ok(serde_json::json!({"new_assignee_id":r.get::<_,String>(0)?,"requested_by":r.get::<_,String>(1)?,"requested_at_ms":r.get::<_,u64>(2)?}))).optional()?;
        Ok(HandoverView {
            candidates,
            pending,
            can_handover,
        })
    }
    pub fn handover(
        &mut self,
        actor: &Principal,
        id: &str,
        request: &HandoverRequest,
        now: u64,
    ) -> Result<Job> {
        super::store::timestamp(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let job = super::store::load(&tx, id)?;
        may_handover(&tx, actor, &job)?;
        if job.id != job.root_id || job.state.terminal() || job.state == JobState::Cancelling {
            return Err(WorkError::Conflict);
        }
        if job.revision != request.expected_revision
            || job.assignee_id == request.new_assignee_id
            || held(&tx, &job.root_id)?
        {
            return Err(WorkError::Conflict);
        }
        identity::require_project_role(
            &tx,
            &request.new_assignee_id,
            &job.project_id,
            ProjectRole::Contributor,
        )?;
        tx.execute(
            "INSERT INTO shared_handovers VALUES(?1,?2,?3,?4,?5,'pending',?6,NULL)",
            params![
                ulid::Ulid::new().to_string(),
                job.id,
                job.assignee_id,
                request.new_assignee_id,
                actor.user_id,
                now
            ],
        )?;
        // The database serializes Started with this fence. Assigned has no issued
        // execution authorization; running/uncertain work is never freed here.
        tx.execute("UPDATE shared_jobs SET state='queued',revision=revision+1,updated_at_ms=?2 WHERE root_id=?1 AND id IN (SELECT job_id FROM shared_attempts WHERE state='assigned' AND uncertainty_reason IS NULL)",params![job.id,now])?;
        tx.execute("UPDATE shared_attempts SET state=CASE WHEN uncertainty_reason IS NULL THEN 'cancelled' ELSE 'stop_requested' END,updated_at_ms=?2 WHERE job_id IN (SELECT id FROM shared_jobs WHERE root_id=?1) AND state='assigned'",params![job.id,now])?;
        tx.execute("UPDATE shared_approvals SET invalidated_at_ms=COALESCE(invalidated_at_ms,?2) WHERE attempt_id IN (SELECT a.id FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id WHERE j.root_id=?1)",params![job.id,now])?;
        tx.execute(
            "UPDATE shared_jobs SET revision=revision+1,updated_at_ms=?2 WHERE id=?1",
            params![job.id, now],
        )?;
        settle(&tx, &job.root_id, now)?;
        let result = super::store::load(&tx, id)?;
        tx.commit()?;
        Ok(result)
    }
}

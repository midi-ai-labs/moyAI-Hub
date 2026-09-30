//! Continuation and explicit administrative stop settlement.
use super::*;
use crate::identity::Principal;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoverRequest {
    pub expected_revision: u64,
    pub new_assignee_id: String,
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

pub(super) fn may_continue(db: &Connection, job: &Job, environment: &str) -> Result<()> {
    super::store::may_use(db, &job.assignee_id, &job.project_id, environment)?;
    if !db.query_row("SELECT EXISTS(SELECT 1 FROM shared_job_environment_ceiling WHERE root_id=?1 AND environment_id=?2)",params![job.root_id,environment],|r|r.get::<_,bool>(0))? {return Err(WorkError::Forbidden)}
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
    Ok(())
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
        super::origin_control::require_origin(&tx, &actor.user_id, &previous, controller_device)?;
        if !super::conversation_catalog::visible(
            &tx,
            &previous.project_id,
            &previous.conversation_id,
        )? {
            return Err(WorkError::NotFound);
        }
        let controller_participation_generation = if let Some(device) = controller_device {
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
            Some(generation)
        } else {
            None
        };
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
        tx.execute("INSERT INTO shared_jobs(id,root_id,conversation_id,origin_session_ref,origin_device_id,origin_turn_revision,required_capability,project_id,environment_id,requestor_id,assignee_id,request_id,request_json,title,input_json,state,descendant_budget,revision,created_at_ms,updated_at_ms,quota_owner_id) VALUES(?1,?1,?2,?3,(SELECT origin_device_id FROM shared_jobs WHERE id=?14),(SELECT origin_turn_revision FROM shared_jobs WHERE id=?14),'multi_device_session_v2',?4,?5,?6,?6,?7,?8,?9,?10,'queued',?11,1,?12,?12,?13)",params![next,previous.conversation_id,previous.origin_session_ref,previous.project_id,previous.environment_id,actor.user_id,request.request_id,payload,previous.title,encoded,budget,now,previous.quota_owner_id,previous.id])?;
        super::conversation_catalog::appended_root(
            &tx,
            &previous.project_id,
            &previous.conversation_id,
            now,
        )?;
        tx.execute("UPDATE shared_jobs SET controller_device_id=?2,controller_participation_generation=?3 WHERE id=?1",params![next,controller_device,controller_participation_generation])?;
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
        super::project_context::capture(
            &tx,
            &next,
            &previous.project_id,
            result.input["prompt"].as_str().unwrap_or(&result.title),
            controller_device,
        )?;
        super::data::bind_inputs(&tx, &result)?;
        tx.commit()?;
        Ok(result)
    }
    /// Compatibility entry point: transfer requests are inert after retirement.
    pub fn handover(
        &mut self,
        _actor: &Principal,
        _id: &str,
        _request: &HandoverRequest,
        _now: u64,
    ) -> Result<Job> {
        Err(WorkError::Retired)
    }
}

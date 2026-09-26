use super::*;
use crate::identity::{self, Principal, ProjectRole};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

const MAX_APPROVAL_LIFETIME_MS: u64 = 15 * 60 * 1000;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Deny,
    Stop,
}
impl ApprovalDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
            Self::Stop => "stop",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalView {
    pub id: String,
    pub attempt_id: String,
    pub request: Value,
    pub status: String,
    pub decision: Option<ApprovalDecision>,
    pub expires_at_ms: u64,
    pub can_decide: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalAnswer {
    pub approval_id: String,
    pub decision: ApprovalDecision,
}

/// An invalidated answer is never delivered as a human decision. A still-paused
/// Runner may replace that exact request with a new approval ID instead.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ApprovalConsumeResult {
    Answer(ApprovalAnswer),
    ReconfirmationRequired {
        approval_id: String,
        reconfirmation_required: bool,
    },
}

struct Record {
    id: String,
    attempt_id: String,
    request: Value,
    expires: u64,
    decision: Option<ApprovalDecision>,
    decided_by: Option<String>,
    consumed_at: Option<u64>,
    authority_generation: u64,
    decider_credential_generation: Option<u64>,
    decider_authorization_generation: Option<u64>,
    invalidated_at: Option<u64>,
}
fn load(db: &Connection, id: &str) -> Result<Record> {
    Ok(db.query_row("SELECT id,attempt_id,request_json,expires_at_ms,decision,decided_by,consumed_at_ms,authority_generation,decider_credential_generation,decider_authorization_generation,invalidated_at_ms FROM shared_approvals WHERE id=?1",[id],|r|Ok(Record {
        id:r.get(0)?,attempt_id:r.get(1)?,request:super::store::decode(r.get(2)?,2)?,expires:r.get(3)?,
        decision:r.get::<_,Option<String>>(4)?.map(|s|super::store::decode(format!("\"{s}\""),4)).transpose()?,
        decided_by:r.get(5)?,consumed_at:r.get(6)?,
        authority_generation:r.get(7)?,decider_credential_generation:r.get(8)?,decider_authorization_generation:r.get(9)?,invalidated_at:r.get(10)?,
    })).optional()?.ok_or(WorkError::NotFound)?)
}

fn may_decide(db: &Connection, actor: &str, job: &Job) -> Result<()> {
    let role =
        identity::require_project_role(db, actor, &job.project_id, ProjectRole::Contributor)?;
    if actor != job.assignee_id && role != ProjectRole::Manager {
        return Err(WorkError::Forbidden);
    }
    Ok(())
}

fn running_job(db: &Connection, record: &Record) -> Result<Job> {
    let (job_id, state, uncertainty, authority): (String, String, Option<String>, u64) = db.query_row(
        "SELECT job_id,state,uncertainty_reason,authority_generation FROM shared_attempts WHERE id=?1",
        [&record.attempt_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let job = super::store::load(db, &job_id)?;
    let root = super::store::load(db, &job.root_id)?;
    if state != "running"
        || uncertainty.is_some()
        || job.state != JobState::Running
        || root.state.terminal()
        || root.state == JobState::Cancelling
        || record.authority_generation != authority
        || record.authority_generation != job.authority_generation
    {
        return Err(WorkError::Conflict);
    }
    Ok(job)
}

fn live(db: &Connection, record: &Record, now: u64) -> Result<Job> {
    if record.expires <= now || record.invalidated_at.is_some() {
        return Err(WorkError::Conflict);
    }
    let job = running_job(db, record)?;
    if let Some(decider) = &record.decided_by {
        let current:Option<(u64,u64)>=db.query_row("SELECT credential_generation,authorization_generation FROM identity_users WHERE id=?1 AND disabled=0",[decider],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if current
            != record
                .decider_credential_generation
                .zip(record.decider_authorization_generation)
        {
            return Err(WorkError::Conflict);
        }
    }
    Ok(job)
}

fn view(db: &Connection, actor: &Principal, record: Record, now: u64) -> Result<ApprovalView> {
    let status = if record.invalidated_at.is_some() {
        "cancelled"
    } else if record.consumed_at.is_some() {
        "consumed"
    } else if record.expires <= now {
        "expired"
    } else if live(db, &record, now).is_err() {
        "cancelled"
    } else if record.decision.is_some() {
        "decided"
    } else {
        "pending"
    };
    let can_decide = record.decision.is_none()
        && live(db, &record, now).is_ok_and(|job| {
            may_decide(db, &actor.user_id, &job).is_ok()
                && super::store::may_use(db, &job.assignee_id, &job.project_id, &job.environment_id)
                    .is_ok()
        });
    Ok(ApprovalView {
        id: record.id,
        attempt_id: record.attempt_id,
        request: record.request,
        status: status.into(),
        decision: record.decision,
        expires_at_ms: record.expires,
        can_decide,
    })
}

pub(super) fn view_by_id(
    db: &Connection,
    actor: &Principal,
    id: &str,
    now: u64,
) -> Result<ApprovalView> {
    view(db, actor, load(db, id)?, now)
}

pub(super) fn record_request(
    db: &Connection,
    attempt: &str,
    id: &str,
    request: &Value,
    expires: u64,
    now: u64,
) -> Result<()> {
    super::store::id(id)?;
    super::store::timestamp(expires)?;
    if expires <= now || expires > now.saturating_add(MAX_APPROVAL_LIFETIME_MS) {
        return Err(WorkError::Invalid);
    }
    let request = super::store::json(request)?;
    let (exists,pending,count):(bool,bool,u32)=db.query_row("SELECT EXISTS(SELECT 1 FROM shared_approvals WHERE id=?1),EXISTS(SELECT 1 FROM shared_approvals WHERE attempt_id=?2 AND consumed_at_ms IS NULL AND invalidated_at_ms IS NULL AND expires_at_ms>?3),(SELECT COUNT(*) FROM shared_approvals WHERE attempt_id=?2)",params![id,attempt,now],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if exists || pending {
        return Err(WorkError::Conflict);
    }
    if count >= 1000 {
        return Err(WorkError::Capacity);
    }
    db.execute("INSERT INTO shared_approvals(id,attempt_id,request_json,expires_at_ms,created_at_ms) VALUES(?1,?2,?3,?4,?5)",params![id,attempt,request,expires,now])?;
    db.execute("UPDATE shared_approvals SET authority_generation=(SELECT authority_generation FROM shared_attempts WHERE id=?2) WHERE id=?1",params![id,attempt])?;
    Ok(())
}

impl SharedWork {
    pub fn authorize_effect(
        &self,
        runner: &str,
        attempt_id: &str,
        generation: u64,
        authority_generation: u64,
        approval_id: Option<&str>,
        now: u64,
    ) -> Result<()> {
        let attempt = self.attempt(runner, attempt_id)?;
        let job = &attempt.assignment.job;
        let root = super::store::load(&self.db, &job.root_id)?;
        if attempt.assignment.generation != generation
            || attempt.assignment.authority_generation != authority_generation
            || job.authority_generation != authority_generation
            || attempt.state != "running"
            || attempt.uncertainty_reason.is_some()
            || job.state != JobState::Running
            || root.state.terminal()
            || root.state == JobState::Cancelling
        {
            return Err(WorkError::Conflict);
        }
        super::handover::may_continue(&self.db, job, &job.environment_id)?;
        if let Some(id) = approval_id {
            let record = load(&self.db, id)?;
            if record.attempt_id != attempt_id
                || record.decision != Some(ApprovalDecision::Approve)
                || record.consumed_at.is_none()
            {
                return Err(WorkError::Forbidden);
            }
            let approved = live(&self.db, &record, now)?;
            may_decide(
                &self.db,
                record.decided_by.as_deref().ok_or(WorkError::Forbidden)?,
                &approved,
            )?;
        }
        Ok(())
    }
    pub fn approval(
        &self,
        actor: &Principal,
        job_id: &str,
        now: u64,
    ) -> Result<Option<ApprovalView>> {
        self.job(actor, job_id)?;
        let id:Option<String>=self.db.query_row("SELECT p.id FROM shared_approvals p JOIN shared_attempts a ON a.id=p.attempt_id WHERE a.job_id=?1 ORDER BY p.created_at_ms DESC,p.id DESC LIMIT 1",[job_id],|r|r.get(0)).optional()?;
        id.map(|id| view_by_id(&self.db, actor, &id, now))
            .transpose()
    }

    pub fn decide_approval(
        &mut self,
        actor: &Principal,
        job_id: &str,
        id: &str,
        decision: ApprovalDecision,
        now: u64,
    ) -> Result<ApprovalView> {
        super::store::timestamp(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load(&tx, id)?;
        let job = live(&tx, &record, now)?;
        if job.id != job_id {
            return Err(WorkError::Conflict);
        }
        may_decide(&tx, &actor.user_id, &job)?;
        // Human authorization never widens the execution environment's current grant.
        super::store::may_use(&tx, &job.assignee_id, &job.project_id, &job.environment_id)?;
        if let Some(previous) = record.decision {
            if previous != decision || record.decided_by.as_deref() != Some(&actor.user_id) {
                return Err(WorkError::Conflict);
            }
        } else {
            tx.execute("UPDATE shared_approvals SET decision=?2,decided_by=?3,decided_at_ms=?4,decider_credential_generation=(SELECT credential_generation FROM identity_users WHERE id=?3),decider_authorization_generation=(SELECT authorization_generation FROM identity_users WHERE id=?3) WHERE id=?1",params![id,decision.as_str(),actor.user_id,now])?;
            tx.execute(
                "UPDATE shared_jobs SET revision=revision+1,updated_at_ms=?2 WHERE id=?1",
                params![job.id, now],
            )?;
        }
        let result = view(&tx, actor, load(&tx, id)?, now)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn consume_approval(
        &mut self,
        runner: &str,
        id: &str,
        attempt_id: &str,
        generation: u64,
        now: u64,
    ) -> Result<Option<ApprovalConsumeResult>> {
        super::store::timestamp(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load(&tx, id)?;
        if record.attempt_id != attempt_id {
            return Err(WorkError::Conflict);
        }
        let (owner, actual_generation): (String, u64) = tx.query_row(
            "SELECT runner_id,generation FROM shared_attempts WHERE id=?1",
            [attempt_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if owner != runner {
            return Err(WorkError::Forbidden);
        }
        if actual_generation != generation {
            return Err(WorkError::Conflict);
        }
        if record.invalidated_at.is_some() {
            if record.expires <= now {
                return Err(WorkError::Conflict);
            }
            let job = running_job(&tx, &record)?;
            super::handover::may_continue(&tx, &job, &job.environment_id)?;
            // This read changes neither the invalidated record nor execution
            // ownership/capacity. It is not permission to replay any effect.
            return Ok(Some(ApprovalConsumeResult::ReconfirmationRequired {
                approval_id: id.into(),
                reconfirmation_required: true,
            }));
        }
        let job = live(&tx, &record, now)?;
        let Some(decision) = record.decision else {
            return Ok(None);
        };
        may_decide(
            &tx,
            record.decided_by.as_deref().ok_or(WorkError::Conflict)?,
            &job,
        )?;
        super::store::may_use(&tx, &job.assignee_id, &job.project_id, &job.environment_id)?;
        if record.consumed_at.is_none() {
            tx.execute(
                "UPDATE shared_approvals SET consumed_at_ms=?2 WHERE id=?1",
                params![id, now],
            )?;
            tx.execute(
                "UPDATE shared_jobs SET revision=revision+1,updated_at_ms=?2 WHERE id=?1",
                params![job.id, now],
            )?;
        }
        tx.commit()?;
        Ok(Some(ApprovalConsumeResult::Answer(ApprovalAnswer {
            approval_id: id.into(),
            decision,
        })))
    }
}

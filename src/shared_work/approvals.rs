use super::*;
use crate::identity::Principal;
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
    pub can_reconfirm: bool,
    pub context: ApprovalContext,
}

/// The exact operation owner stays separate from the conversation being shown.
/// Child execution never moves human approval to the receiving Runner PC.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalContext {
    pub job_id: String,
    pub project_id: String,
    pub root_id: String,
    pub conversation_id: String,
    pub job_title: String,
    pub controller_device_id: Option<String>,
    pub controller_device_label: Option<String>,
    pub execution_device_id: String,
    pub execution_device_label: Option<String>,
}
impl ApprovalContext {
    pub(super) fn label_devices(&mut self, network: &crate::network::DeviceNetwork) {
        self.controller_device_label = self
            .controller_device_id
            .as_deref()
            .and_then(|id| network.device_label(id))
            .map(str::to_owned);
        self.execution_device_label = network
            .device_label(&self.execution_device_id)
            .map(str::to_owned);
    }
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
    let controller: Option<String> = db.query_row(
        "SELECT COALESCE(controller_device_id,origin_device_id) FROM shared_jobs WHERE id=?1",
        [&job.root_id],
        |row| row.get(0),
    )?;
    super::origin_control::require_origin(db, actor, job, controller.as_deref())
}

fn may_decide_from_device(
    db: &Connection,
    actor: &str,
    job: &Job,
    device: Option<&str>,
) -> Result<()> {
    super::origin_control::require_origin(db, actor, job, device)
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

fn current_record(db: &Connection, record: &Record, now: u64) -> Result<()> {
    if record.expires <= now || record.invalidated_at.is_some() {
        return Err(WorkError::Conflict);
    }
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
    Ok(())
}

fn live(db: &Connection, record: &Record, now: u64) -> Result<Job> {
    current_record(db, record, now)?;
    running_job(db, record)
}

/// A human refusal cancels this root turn immediately. Its exact negative
/// answer still needs durable delivery/retry while the Runner stops. This is
/// never used by effect admission or by a new approval decision.
fn answer_job(db: &Connection, record: &Record, now: u64) -> Result<Job> {
    if !matches!(
        record.decision,
        Some(ApprovalDecision::Deny | ApprovalDecision::Stop)
    ) {
        return live(db, record, now);
    }
    current_record(db, record, now)?;
    // Retain compatibility with negative answers saved before root cancellation
    // became part of the decision transaction. Consumption cancels them below.
    if let Ok(job) = running_job(db, record) {
        return Ok(job);
    }
    let (job_id, state, authority): (String, String, u64) = db.query_row(
        "SELECT job_id,state,authority_generation FROM shared_attempts WHERE id=?1",
        [&record.attempt_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let job = super::store::load(db, &job_id)?;
    let root = super::store::load(db, &job.root_id)?;
    if record.decided_by.is_none()
        || !matches!(state.as_str(), "stop_requested" | "cancelled")
        || !matches!(job.state, JobState::Cancelling | JobState::Cancelled)
        || !matches!(root.state, JobState::Cancelling | JobState::Cancelled)
        || record.authority_generation != authority
        || record.authority_generation != job.authority_generation
    {
        return Err(WorkError::Conflict);
    }
    Ok(job)
}

fn cancel_refused_turn(db: &Connection, job: &Job, now: u64) -> Result<()> {
    if !matches!(
        super::store::load(db, &job.root_id)?.state,
        JobState::Cancelling | JobState::Cancelled
    ) {
        super::store::cancel_root_in_transaction(db, &job.root_id, now)?;
    }
    Ok(())
}

fn view(
    db: &Connection,
    actor: &Principal,
    record: Record,
    now: u64,
    device: Option<&str>,
) -> Result<ApprovalView> {
    let status = if record.invalidated_at.is_some() {
        "cancelled"
    } else if record.consumed_at.is_some() {
        "consumed"
    } else if record.expires <= now {
        "expired"
    } else if answer_job(db, &record, now).is_err() {
        "cancelled"
    } else if record.decision.is_some() {
        "decided"
    } else {
        "pending"
    };
    let can_decide = record.decision.is_none()
        && live(db, &record, now).is_ok_and(|job| {
            may_decide_from_device(db, &actor.user_id, &job, device).is_ok()
                && super::store::may_use(db, &job.assignee_id, &job.project_id, &job.environment_id)
                    .is_ok()
        });
    let can_reconfirm = record.expires <= now
        && record.invalidated_at.is_none()
        && record.consumed_at.is_none()
        && !matches!(
            record.decision,
            Some(ApprovalDecision::Deny | ApprovalDecision::Stop)
        )
        && running_job(db, &record).is_ok_and(|job| {
            may_decide_from_device(db, &actor.user_id, &job, device).is_ok()
                && super::store::may_use(db, &job.assignee_id, &job.project_id, &job.environment_id)
                    .is_ok()
        });
    let context = db.query_row(
        "SELECT j.id,j.project_id,j.root_id,j.conversation_id,j.title,
           COALESCE(j.controller_device_id,j.origin_device_id),a.runner_id
         FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id WHERE a.id=?1",
        [&record.attempt_id],
        |row| {
            Ok(ApprovalContext {
                job_id: row.get(0)?,
                project_id: row.get(1)?,
                root_id: row.get(2)?,
                conversation_id: row.get(3)?,
                job_title: row.get(4)?,
                controller_device_id: row.get(5)?,
                controller_device_label: None,
                execution_device_id: row.get(6)?,
                execution_device_label: None,
            })
        },
    )?;
    Ok(ApprovalView {
        id: record.id,
        attempt_id: record.attempt_id,
        request: record.request,
        status: status.into(),
        decision: record.decision,
        expires_at_ms: record.expires,
        can_decide,
        can_reconfirm,
        context,
    })
}

pub(super) fn view_by_id(
    db: &Connection,
    actor: &Principal,
    id: &str,
    now: u64,
    device: Option<&str>,
) -> Result<ApprovalView> {
    view(db, actor, load(db, id)?, now, device)
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
    let (exists,pending,count):(bool,bool,u32)=db.query_row("SELECT EXISTS(SELECT 1 FROM shared_approvals WHERE id=?1),EXISTS(SELECT 1 FROM shared_approvals WHERE attempt_id=?2 AND consumed_at_ms IS NULL AND invalidated_at_ms IS NULL),(SELECT COUNT(*) FROM shared_approvals WHERE attempt_id=?2)",params![id,attempt],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
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
    /// Expiry is not refusal. Only the original controller can reopen the exact
    /// still-suspended operation; the Runner supplies its replacement identity.
    pub fn reconfirm_expired_approval_for_device(
        &mut self,
        actor: &Principal,
        job_id: &str,
        approval_id: &str,
        now: u64,
        device: Option<&str>,
    ) -> Result<ApprovalView> {
        super::store::timestamp(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load(&tx, approval_id)?;
        let job = running_job(&tx, &record)?;
        if job.id != job_id
            || record.expires > now
            || matches!(
                record.decision,
                Some(ApprovalDecision::Deny | ApprovalDecision::Stop)
            )
            || record.consumed_at.is_some()
        {
            return Err(WorkError::Conflict);
        }
        may_decide_from_device(&tx, &actor.user_id, &job, device)?;
        super::store::may_use(&tx, &job.assignee_id, &job.project_id, &job.environment_id)?;
        tx.execute("UPDATE shared_approvals SET invalidated_at_ms=COALESCE(invalidated_at_ms,?2) WHERE id=?1", params![approval_id, now])?;
        let result = view_by_id(&tx, actor, approval_id, now, device)?;
        tx.commit()?;
        Ok(result)
    }

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
        self.approval_exact_for_device(actor, job_id, now, None)
    }

    pub fn approval_for_device(
        &self,
        actor: &Principal,
        job_id: &str,
        now: u64,
        device: Option<&str>,
    ) -> Result<Option<ApprovalView>> {
        let job = self.job(actor, job_id)?;
        // Keep the normal conversation selected while a descendant waits. An old
        // consumed root request must never hide a current child confirmation.
        let ids = self
            .db
            .prepare(
                "SELECT p.id FROM shared_approvals p JOIN shared_attempts a ON a.id=p.attempt_id
             JOIN shared_jobs j ON j.id=a.job_id
             WHERE j.root_id=?1 AND (p.decision IS NULL OR p.decision='approve') AND p.consumed_at_ms IS NULL
               AND p.invalidated_at_ms IS NULL AND a.state='running' AND j.state='running'
             ORDER BY p.created_at_ms,p.id LIMIT 1001",
            )?
            .query_map(params![job.root_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for id in ids {
            let candidate = view_by_id(&self.db, actor, &id, now, device)?;
            if candidate.status == "pending" || candidate.status == "expired" {
                return Ok(Some(candidate));
            }
        }
        // Keep the child refusal visible on its originating conversation after
        // it has stopped the whole turn, even if the parent had an older answer.
        let root = super::store::load(&self.db, &job.root_id)?;
        if matches!(root.state, JobState::Cancelling | JobState::Cancelled) {
            let refusal: Option<String> = self.db.query_row(
                "SELECT p.id FROM shared_approvals p JOIN shared_attempts a ON a.id=p.attempt_id
                 JOIN shared_jobs j ON j.id=a.job_id WHERE j.root_id=?1
                 AND p.decision IN ('deny','stop') AND p.invalidated_at_ms IS NULL
                 ORDER BY p.decided_at_ms DESC,p.id DESC LIMIT 1",
                [&job.root_id], |row| row.get(0),
            ).optional()?;
            if let Some(id) = refusal {
                return view_by_id(&self.db, actor, &id, now, device).map(Some);
            }
        }
        self.approval_exact_for_device(actor, job_id, now, device)
    }

    pub(crate) fn approval_exact_for_device(
        &self,
        actor: &Principal,
        job_id: &str,
        now: u64,
        device: Option<&str>,
    ) -> Result<Option<ApprovalView>> {
        self.job(actor, job_id)?;
        let id:Option<String>=self.db.query_row("SELECT p.id FROM shared_approvals p JOIN shared_attempts a ON a.id=p.attempt_id WHERE a.job_id=?1 ORDER BY p.created_at_ms DESC,p.id DESC LIMIT 1",[job_id],|r|r.get(0)).optional()?;
        id.map(|id| view_by_id(&self.db, actor, &id, now, device))
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
        self.decide_approval_for_device(actor, job_id, id, decision, now, None)
    }

    pub fn decide_approval_for_device(
        &mut self,
        actor: &Principal,
        job_id: &str,
        id: &str,
        decision: ApprovalDecision,
        now: u64,
        device: Option<&str>,
    ) -> Result<ApprovalView> {
        super::store::timestamp(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load(&tx, id)?;
        let job = answer_job(&tx, &record, now)?;
        if job.id != job_id {
            return Err(WorkError::Conflict);
        }
        may_decide_from_device(&tx, &actor.user_id, &job, device)?;
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
        if matches!(decision, ApprovalDecision::Deny | ApprovalDecision::Stop) {
            // One cancellation owner fences descendants, retained services,
            // later grants and parent wake-up in this same transaction.
            cancel_refused_turn(&tx, &job, now)?;
        }
        let result = view(&tx, actor, load(&tx, id)?, now, device)?;
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
            let job = running_job(&tx, &record)?;
            super::handover::may_continue(&tx, &job, &job.environment_id)?;
            // This read changes neither the invalidated record nor execution
            // ownership/capacity. It is not permission to replay any effect.
            return Ok(Some(ApprovalConsumeResult::ReconfirmationRequired {
                approval_id: id.into(),
                reconfirmation_required: true,
            }));
        }
        if record.expires <= now
            && !matches!(
                record.decision,
                Some(ApprovalDecision::Deny | ApprovalDecision::Stop)
            )
            && record.consumed_at.is_none()
        {
            let job = running_job(&tx, &record)?;
            super::store::may_use(&tx, &job.assignee_id, &job.project_id, &job.environment_id)?;
            // The exact tool remains paused; only its origin can request a new review.
            return Ok(None);
        }
        let job = answer_job(&tx, &record, now)?;
        let Some(decision) = record.decision else {
            return Ok(None);
        };
        may_decide(
            &tx,
            record.decided_by.as_deref().ok_or(WorkError::Conflict)?,
            &job,
        )?;
        super::store::may_use(&tx, &job.assignee_id, &job.project_id, &job.environment_id)?;
        if matches!(decision, ApprovalDecision::Deny | ApprovalDecision::Stop) {
            cancel_refused_turn(&tx, &job, now)?;
        }
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

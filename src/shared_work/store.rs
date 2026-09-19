use super::*;
use crate::identity::{self, Principal, ProjectRole};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use std::path::Path;

const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_RETAINED_JOBS: u64 = 100_000;
const MAX_RESERVED_DESCENDANTS: u64 = 32_000;
const MAX_ROOTS: u64 = 1_000;
const MAX_RUNNER_ATTEMPTS: u32 = 128;
const JOB_COLUMNS: &str = "id,root_id,parent_id,project_id,environment_id,requestor_id,assignee_id,title,input_json,checkpoint_json,result_json,state,awaiting_child_id,revision,created_at_ms,updated_at_ms,COALESCE(quota_owner_id,requestor_id),authority_generation,(SELECT previous_job_id FROM shared_continuations WHERE job_id=shared_jobs.id),start_before_ms";

pub(super) fn id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
    {
        return Err(WorkError::Invalid);
    }
    Ok(())
}
pub(super) fn label(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 1024 || value.contains('\0') {
        return Err(WorkError::Invalid);
    }
    Ok(())
}
pub(super) fn json(value: &Value) -> Result<String> {
    let value = serde_json::to_string(value)?;
    if value.len() > MAX_INPUT_BYTES {
        return Err(WorkError::Invalid);
    }
    Ok(value)
}
pub(super) fn timestamp(now: u64) -> Result<()> {
    if now > i64::MAX as u64 {
        return Err(WorkError::Invalid);
    }
    Ok(())
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(
    value: String,
    index: usize,
) -> rusqlite::Result<T> {
    serde_json::from_str(&value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}
fn row_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: row.get(0)?,
        root_id: row.get(1)?,
        parent_id: row.get(2)?,
        project_id: row.get(3)?,
        environment_id: row.get(4)?,
        requestor_id: row.get(5)?,
        assignee_id: row.get(6)?,
        title: row.get(7)?,
        input: decode(row.get(8)?, 8)?,
        checkpoint: row
            .get::<_, Option<String>>(9)?
            .map(|v| decode(v, 9))
            .transpose()?,
        result: row
            .get::<_, Option<String>>(10)?
            .map(|v| decode(v, 10))
            .transpose()?,
        state: decode(format!("\"{}\"", row.get::<_, String>(11)?), 11)?,
        awaiting_child_id: row.get(12)?,
        revision: row.get(13)?,
        created_at_ms: row.get(14)?,
        updated_at_ms: row.get(15)?,
        quota_owner_id: row.get(16)?,
        authority_generation: row.get(17)?,
        continued_from: row.get(18)?,
        start_before_ms: row.get(19)?,
    })
}
pub(super) fn load(db: &Connection, job_id: &str) -> Result<Job> {
    db.query_row(
        &format!("SELECT {JOB_COLUMNS} FROM shared_jobs WHERE id=?1"),
        [job_id],
        row_job,
    )
    .optional()?
    .ok_or(WorkError::NotFound)
}
pub(super) fn may_use(db: &Connection, user: &str, project: &str, environment: &str) -> Result<()> {
    identity::require_project_role(db, user, project, ProjectRole::Contributor)?;
    let enabled: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_environments e JOIN shared_environment_projects p ON p.environment_id=e.id JOIN shared_projects project ON project.id=p.project_id WHERE e.id=?1 AND e.enabled=1 AND p.project_id=?2 AND project.archived=0 AND NOT EXISTS(SELECT 1 FROM shared_provisioning provision WHERE provision.environment_id=e.id AND provision.state<>'ready'))",
        params![environment, project], |r| r.get(0))?;
    if !enabled {
        return Err(WorkError::Forbidden);
    }
    let runner: String = db.query_row(
        "SELECT runner_id FROM shared_environments WHERE id=?1",
        [environment],
        |r| r.get(0),
    )?;
    if !super::project_setup::environment_matches_project(db, project, environment, &runner)? {
        return Err(WorkError::Forbidden);
    }
    Ok(())
}
fn open_root(db: &Connection, root_id: &str) -> Result<Job> {
    let root = load(db, root_id)?;
    if root.state.terminal() || root.state == JobState::Cancelling {
        return Err(WorkError::Conflict);
    }
    Ok(root)
}
fn set_state(db: &Connection, job_id: &str, state: &str, now: u64) -> Result<()> {
    db.execute(
        "UPDATE shared_jobs SET state=?2,revision=revision+1,updated_at_ms=?3 WHERE id=?1",
        params![job_id, state, now],
    )?;
    Ok(())
}
pub(super) fn settle_cancel(db: &Connection, root: &str, now: u64) -> Result<()> {
    // A cancellation request is not evidence that any issued attempt has stopped.
    let active: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id WHERE j.root_id=?1 AND a.state IN ('assigned','running','stop_requested'))", [root], |r| r.get(0))?;
    if !active {
        db.execute("UPDATE shared_jobs SET state='cancelled',revision=revision+1,updated_at_ms=?2 WHERE root_id=?1 AND state='cancelling'", params![root,now])?;
    }
    Ok(())
}
pub(super) fn root_capacity(db: &Connection, budget: u32) -> Result<()> {
    if budget > 1000 {
        return Err(WorkError::Invalid);
    }
    let (roots,reserved,remaining):(u64,u64,u64)=db.query_row("SELECT COUNT(*),COALESCE(SUM(descendant_budget),0),COALESCE(SUM(MAX(0,descendant_budget-(SELECT COUNT(*) FROM shared_jobs child WHERE child.root_id=root.id AND child.parent_id IS NOT NULL))),0) FROM shared_jobs root WHERE parent_id IS NULL AND state NOT IN ('succeeded','failed','cancelled')",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let retained: u64 = db.query_row("SELECT COUNT(*) FROM shared_jobs", [], |r| r.get(0))?;
    if roots >= MAX_ROOTS
        || reserved + budget as u64 > MAX_RESERVED_DESCENDANTS
        || retained + remaining + budget as u64 + 1 > MAX_RETAINED_JOBS
    {
        return Err(WorkError::Capacity);
    }
    Ok(())
}

pub(super) fn submit_in_transaction(
    db: &Connection,
    actor: &Principal,
    request: &SubmitJob,
    now: u64,
) -> Result<Job> {
    id(&request.request_id)?;
    label(&request.title)?;
    timestamp(now)?;
    let input = json(&request.input)?;
    if request.descendant_budget > 1000 {
        return Err(WorkError::Invalid);
    }
    let request_json = serde_json::to_string(request)?;
    may_use(
        &db,
        &actor.user_id,
        &request.project_id,
        &request.environment_id,
    )?;
    super::data::validate_inputs(&db, &request.project_id, &request.input)?;
    let existing: Option<(String, String)> = db
        .query_row(
            "SELECT id,request_json FROM shared_jobs WHERE requestor_id=?1 AND request_id=?2",
            params![actor.user_id, request.request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((job_id, old)) = existing {
        if old != request_json {
            return Err(WorkError::Conflict);
        }
        return load(&db, &job_id);
    }
    root_capacity(&db, request.descendant_budget)?;
    let start_before_ms = super::deadlines::resolve(request.start_before_ms, now)?;
    let job_id = ulid::Ulid::new().to_string();
    db.execute("INSERT INTO shared_jobs(id,root_id,project_id,environment_id,requestor_id,assignee_id,request_id,request_json,title,input_json,state,descendant_budget,revision,created_at_ms,updated_at_ms) VALUES(?1,?1,?2,?3,?4,?4,?5,?6,?7,?8,'queued',?9,1,?10,?10)", params![job_id,request.project_id,request.environment_id,actor.user_id,request.request_id,request_json,request.title,input,request.descendant_budget,now])?;
    db.execute(
        "UPDATE shared_jobs SET quota_owner_id=?2,start_before_ms=?3 WHERE id=?1",
        params![job_id, actor.user_id, start_before_ms],
    )?;
    db.execute("INSERT INTO shared_job_environment_ceiling SELECT ?1,e.id FROM shared_environments e JOIN shared_environment_projects ep ON ep.environment_id=e.id WHERE ep.project_id=?2 AND e.enabled=1",params![job_id,request.project_id])?;
    let job = load(&db, &job_id)?;
    super::data::bind_inputs(&db, &job)?;

    Ok(job)
}

pub(super) fn reserve_job(
    db: &Connection,
    runner: &str,
    job: &Job,
    now: u64,
) -> Result<Assignment> {
    id(runner)?;
    timestamp(now)?;
    if now >= job.start_before_ms {
        return Err(WorkError::StartDeadline);
    }
    if job.state != JobState::Queued
        || open_root(db, &job.root_id).is_err()
        || super::handover::held(db, &job.root_id)?
    {
        return Err(WorkError::Conflict);
    }
    super::handover::may_continue(db, job, &job.environment_id)?;
    let (resource,capacity,owner):(String,u32,String)=db.query_row("SELECT e.resource_id,r.capacity,e.runner_id FROM shared_environments e JOIN shared_resources r ON r.id=e.resource_id WHERE e.id=?1",[&job.environment_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if owner != runner {
        return Err(WorkError::Forbidden);
    }
    let (occupied,runner_active):(u32,u32)=db.query_row("SELECT (SELECT COUNT(*) FROM shared_attempts WHERE resource_id=?1 AND state IN ('assigned','running','stop_requested')),(SELECT COUNT(*) FROM shared_attempts WHERE runner_id=?2 AND state IN ('assigned','running','stop_requested'))",params![resource,runner],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if occupied >= capacity || runner_active >= MAX_RUNNER_ATTEMPTS {
        return Err(WorkError::Capacity);
    }
    let generation: u64 = db.query_row(
        "SELECT COALESCE(MAX(generation),0)+1 FROM shared_attempts WHERE job_id=?1",
        [&job.id],
        |r| r.get(0),
    )?;
    let attempt_id = ulid::Ulid::new().to_string();
    db.execute("INSERT INTO shared_attempts(id,job_id,runner_id,resource_id,generation,state,created_at_ms,updated_at_ms,authority_generation) VALUES(?1,?2,?3,?4,?5,'assigned',?6,?6,?7)",params![attempt_id,job.id,runner,resource,generation,now,job.authority_generation])?;
    set_state(db, &job.id, "assigned", now)?;
    assignment(db, &attempt_id)
}

impl SharedWork {
    pub fn open(path: &Path) -> Result<Self> {
        let mut db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.pragma_update(None, "foreign_keys", true)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        let version: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 12 {
            return Err(WorkError::Conflict);
        }
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if version < 1 {
            tx.execute_batch(include_str!("../../migrations/V1__shared_work.sql"))?;
        }
        if version < 2 {
            identity::migrate(&tx)?;
        }
        if version < 3 {
            tx.execute_batch(include_str!(
                "../../migrations/V3__shared_attempt_observation.sql"
            ))?;
        }
        if version < 4 {
            tx.execute_batch(include_str!("../../migrations/V4__shared_approvals.sql"))?;
        }
        if version < 5 {
            tx.execute_batch(include_str!("../../migrations/V5__shared_management.sql"))?;
        }
        if version < 6 {
            tx.execute_batch(include_str!("../../migrations/V6__shared_data.sql"))?;
        }
        if version < 7 {
            tx.execute_batch(include_str!(
                "../../migrations/V7__shared_runner_participation.sql"
            ))?;
        }
        if version < 8 {
            tx.execute_batch(include_str!(
                "../../migrations/V8__shared_data_retention.sql"
            ))?;
        }
        if version < 9 {
            tx.execute_batch(include_str!(
                "../../migrations/V9__shared_managed_files.sql"
            ))?;
            super::blob_store::migrate(&tx)?;
        }
        if version < 10 {
            tx.execute_batch(include_str!(
                "../../migrations/V10__shared_start_deadlines.sql"
            ))?;
        }
        if version < 11 {
            tx.execute_batch(include_str!(
                "../../migrations/V11__remembered_human_sessions.sql"
            ))?;
        }
        if version < 12 {
            tx.execute_batch(include_str!("../../migrations/V12__project_devices.sql"))?;
        }
        tx.pragma_update(None, "user_version", 12)?;
        tx.commit()?;
        let check: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(WorkError::Conflict);
        }
        Ok(Self {
            db,
            runner_contacts: Default::default(),
        })
    }

    pub fn identity_connection(&self) -> &Connection {
        &self.db
    }

    pub fn create_project(
        &mut self,
        actor: &Principal,
        project_id: &str,
        name: &str,
    ) -> Result<()> {
        id(project_id)?;
        label(name)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        identity::require_admin(&tx, actor)?;
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_projects WHERE id=?1)",
            [project_id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(WorkError::Conflict);
        }
        tx.execute(
            "INSERT INTO shared_projects(id,label) VALUES(?1,?2)",
            params![project_id, name],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn register_environment(&mut self, actor: &Principal, env: &Environment) -> Result<()> {
        id(&env.id)?;
        id(&env.resource_id)?;
        id(&env.runner_id)?;
        label(&env.label)?;
        if !(1..=128).contains(&env.capacity)
            || env.project_ids.is_empty()
            || env.project_ids.len() > 128
        {
            return Err(WorkError::Invalid);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        identity::require_admin(&tx, actor)?;
        let capacity: Option<u32> = tx
            .query_row(
                "SELECT capacity FROM shared_resources WHERE id=?1",
                [&env.resource_id],
                |r| r.get(0),
            )
            .optional()?;
        if capacity.is_some_and(|c| c != env.capacity) {
            return Err(WorkError::Conflict);
        }
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_environments WHERE id=?1)",
            [&env.id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(WorkError::Conflict);
        }
        for project in &env.project_ids {
            id(project)?;
            if !super::project_setup::environment_matches_project(
                &tx,
                project,
                &env.id,
                &env.runner_id,
            )? {
                return Err(WorkError::Conflict);
            }
        }
        tx.execute(
            "INSERT OR IGNORE INTO shared_resources(id,capacity) VALUES(?1,?2)",
            params![env.resource_id, env.capacity],
        )?;
        tx.execute(
            "INSERT INTO shared_environments(id,label,resource_id,runner_id) VALUES(?1,?2,?3,?4)",
            params![env.id, env.label, env.resource_id, env.runner_id],
        )?;
        for project in &env.project_ids {
            id(project)?;
            tx.execute(
                "INSERT INTO shared_environment_projects VALUES(?1,?2)",
                params![env.id, project],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn submit(&mut self, actor: &Principal, request: &SubmitJob, now: u64) -> Result<Job> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let job = submit_in_transaction(&tx, actor, request, now)?;
        tx.commit()?;
        Ok(job)
    }

    pub fn job(&self, actor: &Principal, job_id: &str) -> Result<Job> {
        let job = load(&self.db, job_id)?;
        identity::require_project_role(
            &self.db,
            &actor.user_id,
            &job.project_id,
            ProjectRole::Reader,
        )?;
        Ok(job)
    }

    pub fn list(
        &self,
        actor: &Principal,
        project: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Job>> {
        identity::require_project_role(&self.db, &actor.user_id, project, ProjectRole::Reader)?;
        if !(1..=100).contains(&limit) {
            return Err(WorkError::Invalid);
        }
        let mut stmt = self.db.prepare(&format!("SELECT {JOB_COLUMNS} FROM shared_jobs WHERE project_id=?1 AND (?2 IS NULL OR id<?2) ORDER BY id DESC LIMIT ?3"))?;
        let rows = stmt
            .query_map(params![project, before, limit], row_job)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Redelivery returns the existing assigned attempt. No timeout creates a replacement.
    pub fn claim(&mut self, runner: &str, now: u64) -> Result<Option<Assignment>> {
        self.claim_for(runner, None, now)
    }

    pub fn claim_for(
        &mut self,
        runner: &str,
        environments: Option<&[String]>,
        now: u64,
    ) -> Result<Option<Assignment>> {
        id(runner)?;
        timestamp(now)?;
        if let Some(environments) = environments {
            if environments.is_empty() || environments.len() > 128 {
                return Err(WorkError::Invalid);
            }
            let mut seen = std::collections::BTreeSet::new();
            for environment in environments {
                id(environment)?;
                if !seen.insert(environment) {
                    return Err(WorkError::Invalid);
                }
            }
        }
        let filter = environments.map(serde_json::to_string).transpose()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::deadlines::expire_due(&tx, now)?;
        let existing: Option<String> = tx.query_row("SELECT a.id FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id WHERE a.runner_id=?1 AND a.state IN ('assigned','stop_requested') AND NOT EXISTS(SELECT 1 FROM shared_external_attempts x WHERE x.attempt_id=a.id) AND (?2 IS NULL OR j.environment_id IN (SELECT value FROM json_each(?2))) ORDER BY a.created_at_ms,a.id LIMIT 1", params![runner, filter], |r|r.get(0)).optional()?;
        if let Some(attempt) = existing {
            let prior = assignment(&tx, &attempt)?;
            super::deadlines::expire_job(&tx, &prior.job.id, now)?;
            let result = assignment(&tx, &attempt)?;
            if !result.job.state.terminal() {
                tx.commit()?;
                return Ok(Some(result));
            }
        }
        let active:u32=tx.query_row("SELECT COUNT(*) FROM shared_attempts WHERE runner_id=?1 AND state IN ('assigned','running','stop_requested')",[runner],|r|r.get(0))?;
        if active >= MAX_RUNNER_ATTEMPTS {
            tx.commit()?;
            return Ok(None);
        }
        // Oldest-served person first; a blocked resource never blocks another resource.
        let candidates: Vec<String> = {
            let mut stmt = tx.prepare("SELECT j.id FROM shared_jobs j JOIN shared_environments e ON e.id=j.environment_id WHERE j.state='queued' AND e.runner_id=?1 AND e.enabled=1 AND NOT EXISTS(SELECT 1 FROM shared_external_attempts x JOIN shared_attempts external ON external.id=x.attempt_id WHERE external.job_id=j.id) AND (?2 IS NULL OR e.id IN (SELECT value FROM json_each(?2))) ORDER BY COALESCE((SELECT MAX(a.created_at_ms) FROM shared_attempts a JOIN shared_jobs served ON served.id=a.job_id WHERE COALESCE(served.quota_owner_id,served.requestor_id)=COALESCE(j.quota_owner_id,j.requestor_id)),0),j.created_at_ms,j.id LIMIT 1000")?;
            stmt.query_map(params![runner, filter], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for job_id in candidates {
            let job = load(&tx, &job_id)?;
            if now >= job.start_before_ms {
                super::deadlines::expire_job(&tx, &job.id, now)?;
                continue;
            }
            if open_root(&tx, &job.root_id).is_err() || super::handover::held(&tx, &job.root_id)? {
                continue;
            }
            match super::handover::may_continue(&tx, &job, &job.environment_id) {
                Ok(()) => {}
                Err(WorkError::Forbidden | WorkError::Identity(_)) => continue,
                Err(e) => return Err(e),
            }
            let result = match reserve_job(&tx, runner, &job, now) {
                Ok(value) => value,
                Err(WorkError::Capacity) => continue,
                Err(error) => return Err(error),
            };
            tx.commit()?;
            return Ok(Some(result));
        }
        tx.commit()?;
        Ok(None)
    }

    pub fn assignments(&self, runner: &str) -> Result<Vec<Assignment>> {
        id(runner)?;
        let mut stmt=self.db.prepare("SELECT id FROM shared_attempts WHERE runner_id=?1 AND state IN ('assigned','running','stop_requested') ORDER BY created_at_ms,id LIMIT ?2")?;
        let ids = stmt
            .query_map(params![runner, MAX_RUNNER_ATTEMPTS], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.iter().map(|id| assignment(&self.db, id)).collect()
    }

    pub fn attempt(&self, runner: &str, attempt_id: &str) -> Result<AttemptStatus> {
        id(runner)?;
        id(attempt_id)?;
        let (owner, state, uncertainty_reason): (String, String, Option<String>) = self
            .db
            .query_row(
                "SELECT runner_id,state,uncertainty_reason FROM shared_attempts WHERE id=?1",
                [attempt_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or(WorkError::NotFound)?;
        if owner != runner {
            return Err(WorkError::Forbidden);
        }
        Ok(AttemptStatus {
            assignment: assignment(&self.db, attempt_id)?,
            state,
            uncertainty_reason,
        })
    }

    pub fn report(&mut self, runner: &str, report: &Report, now: u64) -> Result<Job> {
        id(&report.event_id)?;
        timestamp(now)?;
        let payload = serde_json::to_string(report)?;
        if payload.len() > MAX_INPUT_BYTES * 3 {
            return Err(WorkError::Invalid);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (job_id, owner, generation, state, uncertainty): (String, String, u64, String, Option<String>) = tx
            .query_row(
                "SELECT job_id,runner_id,generation,state,uncertainty_reason FROM shared_attempts WHERE id=?1",
                [&report.attempt_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?
            .ok_or(WorkError::NotFound)?;
        if owner != runner {
            return Err(WorkError::Forbidden);
        }
        if generation != report.generation {
            return Err(WorkError::Conflict);
        }
        let previous: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM shared_reports WHERE attempt_id=?1 AND event_id=?2",
                params![report.attempt_id, report.event_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            if previous != payload {
                return Err(WorkError::Conflict);
            }
            return load(&tx, &job_id);
        }
        super::data::require_checkpoint_archive(&tx, report)?;
        let job = load(&tx, &job_id)?;
        let root = load(&tx, &job.root_id)?;
        let cancelling = matches!(root.state, JobState::Cancelling | JobState::Cancelled);
        if matches!(report.outcome, ReportOutcome::Started)
            && state == "assigned"
            && !cancelling
            && now >= job.start_before_ms
        {
            super::deadlines::expire_job(&tx, &job.id, now)?;
            tx.commit()?;
            return Err(WorkError::StartDeadline);
        }
        if !matches!(state.as_str(), "assigned" | "running" | "stop_requested") {
            return Err(WorkError::Conflict);
        }
        match &report.outcome {
            ReportOutcome::ApprovalRequested {
                approval_id,
                request,
                expires_at_ms,
            } => {
                if cancelling || state != "running" || uncertainty.is_some() {
                    return Err(WorkError::Conflict);
                }
                super::handover::may_continue(&tx, &job, &job.environment_id)?;
                super::approvals::record_request(
                    &tx,
                    &report.attempt_id,
                    approval_id,
                    request,
                    *expires_at_ms,
                    now,
                )?;
                tx.execute(
                    "UPDATE shared_jobs SET revision=revision+1,updated_at_ms=?2 WHERE id=?1",
                    params![job_id, now],
                )?;
            }
            ReportOutcome::Started => {
                if cancelling
                    || state != "assigned"
                    || uncertainty.is_some()
                    || super::handover::held(&tx, &job.root_id)?
                {
                    return Err(WorkError::Conflict);
                }
                super::handover::may_continue(&tx, &job, &job.environment_id)?;
                tx.execute(
                    "UPDATE shared_attempts SET state='running',updated_at_ms=?2 WHERE id=?1",
                    params![report.attempt_id, now],
                )?;
                set_state(&tx, &job_id, "running", now)?;
            }
            ReportOutcome::Uncertain { reason } => {
                label(reason)?;
                if uncertainty.is_some() {
                    return Err(WorkError::Conflict);
                }
                tx.execute(
                    "UPDATE shared_attempts SET uncertainty_reason=?2,updated_at_ms=?3 WHERE id=?1",
                    params![report.attempt_id, reason, now],
                )?;
                tx.execute(
                    "UPDATE shared_jobs SET revision=revision+1,updated_at_ms=?2 WHERE id=?1",
                    params![job_id, now],
                )?;
            }
            ReportOutcome::YieldToChild {
                checkpoint,
                child_environment_id,
                child_title,
                child_input,
                resources_released,
            } => {
                if cancelling || state != "running" || !resources_released || uncertainty.is_some()
                {
                    return Err(WorkError::Conflict);
                }
                label(child_title)?;
                super::handover::may_continue(&tx, &job, child_environment_id)?;
                super::data::validate_inputs(&tx, &job.project_id, child_input)?;
                let checkpoint = json(checkpoint)?;
                let input = json(child_input)?;
                let (budget,count): (u64,u64) = tx.query_row("SELECT descendant_budget,(SELECT COUNT(*)-1 FROM shared_jobs WHERE root_id=?1) FROM shared_jobs WHERE id=?1", [&job.root_id], |r|Ok((r.get(0)?,r.get(1)?)))?;
                if count >= budget {
                    return Err(WorkError::Capacity);
                }
                let child = ulid::Ulid::new().to_string();
                let request_id = format!("child:{}", child);
                tx.execute("INSERT INTO shared_jobs(id,root_id,parent_id,project_id,environment_id,requestor_id,assignee_id,request_id,request_json,title,input_json,state,descendant_budget,revision,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'queued',0,1,?12,?12)", params![child,job.root_id,job.id,job.project_id,child_environment_id,job.requestor_id,job.assignee_id,request_id,payload,child_title,input,now])?;
                tx.execute(
                    "UPDATE shared_jobs SET quota_owner_id=?2,authority_generation=?3,start_before_ms=?4 WHERE id=?1",
                    params![child, job.quota_owner_id, job.authority_generation,job.start_before_ms],
                )?;
                super::data::bind_inputs(&tx, &load(&tx, &child)?)?;
                tx.execute(
                    "UPDATE shared_jobs SET checkpoint_json=?2,awaiting_child_id=?3 WHERE id=?1",
                    params![job.id, checkpoint, child],
                )?;
                set_state(&tx, &job.id, "waiting_child", now)?;
                tx.execute(
                    "UPDATE shared_attempts SET state='yielded',updated_at_ms=?2 WHERE id=?1",
                    params![report.attempt_id, now],
                )?;
            }
            ReportOutcome::Finished {
                success,
                result,
                resources_released,
            } => {
                if !resources_released {
                    return Err(WorkError::Conflict);
                }
                // Assigned is a preparation reservation. Successful work requires the
                // durable Started authorization before effects. Cancellation settlement
                // may still report an outcome from an already-issued in-flight attempt.
                if *success && state != "running" && !cancelling {
                    return Err(WorkError::Conflict);
                }
                let result = if !cancelling && super::deadlines::is_expired_result(&job) {
                    // Retain the deadline reason after unknown execution is
                    // finally confirmed stopped. The exact reported result is
                    // retained by shared_reports without expanding its size limit.
                    json(result)?;
                    json(&super::deadlines::expired_result())?
                } else {
                    json(result)?
                };
                let terminal = if cancelling {
                    "cancelled"
                } else if *success {
                    "succeeded"
                } else {
                    "failed"
                };
                tx.execute(
                    "UPDATE shared_attempts SET state=?2,updated_at_ms=?3 WHERE id=?1",
                    params![report.attempt_id, terminal, now],
                )?;
                tx.execute(
                    "UPDATE shared_jobs SET result_json=?2 WHERE id=?1",
                    params![job.id, result],
                )?;
                set_state(&tx, &job.id, terminal, now)?;
                if cancelling {
                    settle_cancel(&tx, &job.root_id, now)?;
                } else if let Some(parent) = &job.parent_id {
                    // This single transition owns the parent wake, including report replay.
                    tx.execute("UPDATE shared_jobs SET state='queued',revision=revision+1,updated_at_ms=?3 WHERE id=?1 AND awaiting_child_id=?2 AND state='waiting_child'", params![parent,job.id,now])?;
                }
            }
        }
        tx.execute(
            "INSERT INTO shared_reports(attempt_id,event_id,payload_json) VALUES(?1,?2,?3)",
            params![report.attempt_id, report.event_id, payload],
        )?;
        // A newly yielded child or a parent woken by Finished is a new start.
        // Reconcile it in this transaction even when a bounded queue sweep is full.
        if let ReportOutcome::YieldToChild { .. } = report.outcome {
            if let Some(child) = load(&tx, &job.id)?.awaiting_child_id {
                super::deadlines::expire_job(&tx, &child, now)?;
            }
        }
        if let Some(parent) = &job.parent_id {
            super::deadlines::expire_job(&tx, parent, now)?;
        }
        super::deadlines::expire_due(&tx, now)?;
        super::handover::settle(&tx, &job.root_id, now)?;
        let updated = load(&tx, &job.id)?;
        tx.commit()?;
        Ok(updated)
    }

    pub fn cancel(&mut self, actor: &Principal, job_id: &str, now: u64) -> Result<Job> {
        timestamp(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let job = load(&tx, job_id)?;
        if job.id != job.root_id {
            return Err(WorkError::Invalid);
        }
        identity::require_project_role(
            &tx,
            &actor.user_id,
            &job.project_id,
            ProjectRole::Contributor,
        )?;
        if actor.user_id != job.assignee_id {
            identity::require_project_role(
                &tx,
                &actor.user_id,
                &job.project_id,
                ProjectRole::Manager,
            )?;
        }
        if job.state.terminal() {
            return Ok(job);
        }
        tx.execute("UPDATE shared_jobs SET state=CASE WHEN state='queued' THEN 'cancelled' ELSE 'cancelling' END,revision=revision+1,updated_at_ms=?2 WHERE root_id=?1 AND state NOT IN ('succeeded','failed','cancelled')", params![job.root_id,now])?;
        tx.execute("UPDATE shared_attempts SET state='stop_requested',updated_at_ms=?2 WHERE job_id IN (SELECT id FROM shared_jobs WHERE root_id=?1) AND state IN ('assigned','running')", params![job.root_id,now])?;
        // Keep the root nonterminal while an issued descendant is unconfirmed.
        set_state(&tx, &job.id, "cancelling", now)?;
        settle_cancel(&tx, &job.root_id, now)?;
        super::handover::settle(&tx, &job.root_id, now)?;
        let result = load(&tx, &job.id)?;
        tx.commit()?;
        Ok(result)
    }
}

pub(super) fn assignment(db: &Connection, attempt: &str) -> Result<Assignment> {
    let (job_id, runner_id, generation, state, authority_generation): (String, String, u64, String,u64) = db.query_row(
        "SELECT job_id,runner_id,generation,state,authority_generation FROM shared_attempts WHERE id=?1",
        [attempt],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?,r.get(4)?)),
    )?;
    let job = load(db, &job_id)?;
    let child_result = job
        .awaiting_child_id
        .as_deref()
        .map(|id| load(db, id).map(Box::new))
        .transpose()?;
    let allowed_child_environments = {
        let mut statement=db.prepare("SELECT e.id FROM shared_environments e JOIN shared_environment_projects ep ON ep.environment_id=e.id JOIN identity_effective_project_memberships m ON m.project_id=ep.project_id JOIN shared_job_environment_ceiling c ON c.environment_id=e.id AND c.root_id=?3 JOIN shared_projects p ON p.id=ep.project_id WHERE ep.project_id=?1 AND m.user_id=?2 AND m.role IN ('contributor','manager') AND e.enabled=1 AND p.archived=0 ORDER BY e.id LIMIT 1000")?;
        statement
            .query_map(params![job.project_id, job.assignee_id, job.root_id], |r| {
                r.get(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    Ok(Assignment {
        attempt_id: attempt.into(),
        generation,
        authority_generation,
        runner_id,
        stop_requested: state == "stop_requested",
        job,
        child_result,
        allowed_child_environments,
    })
}

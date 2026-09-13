//! A start deadline gates new attempts, never the lifetime of an already authorized effect.
use super::*;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

pub const DEFAULT_START_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
pub(super) const EXPIRED_MESSAGE: &str =
    "新しい実行の開始期限を過ぎました。必要な場合は新しい仕事として依頼してください。";

pub(super) fn expired_result() -> Value {
    serde_json::json!({"error":"start_deadline_exceeded","message":EXPIRED_MESSAGE})
}

pub(super) fn is_expired_result(job: &Job) -> bool {
    job.result
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(Value::as_str)
        == Some("start_deadline_exceeded")
}

pub(super) fn resolve(requested: Option<u64>, now: u64) -> Result<u64> {
    let deadline = requested.unwrap_or_else(|| now.saturating_add(DEFAULT_START_WINDOW_MS));
    store::timestamp(deadline)?;
    if deadline <= now {
        return Err(WorkError::StartDeadline);
    }
    Ok(deadline)
}

/// The caller holds the shared-work transaction. A missing Started receipt is
/// authoritative only while the attempt is still known, never after uncertainty.
pub(super) fn expire_job(db: &Connection, job_id: &str, now: u64) -> Result<()> {
    let root_id = store::load(db, job_id)?.root_id;
    let mut next = Some(job_id.to_owned());
    while let Some(id) = next.take() {
        let job = store::load(db, &id)?;
        if now < job.start_before_ms || !matches!(job.state, JobState::Queued | JobState::Assigned)
        {
            break;
        }
        let active: Option<(String, String, Option<String>)> = db.query_row(
            "SELECT id,state,uncertainty_reason FROM shared_attempts WHERE job_id=?1 AND state IN ('assigned','running','stop_requested')",
            [&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).optional()?;
        if let Some((attempt, state, uncertainty)) = active {
            if state != "assigned" {
                break;
            }
            if uncertainty.is_some() {
                db.execute("UPDATE shared_attempts SET state='stop_requested',updated_at_ms=?2 WHERE id=?1", params![attempt, now])?;
                db.execute("UPDATE shared_jobs SET result_json=?2,revision=revision+1,updated_at_ms=?3 WHERE id=?1", params![id, expired_result().to_string(), now])?;
                break;
            }
            // A delayed Started must re-enter this same owner; it cannot gain
            // authority after this terminal transition or release a second slot.
            db.execute(
                "UPDATE shared_attempts SET state='failed',updated_at_ms=?2 WHERE id=?1",
                params![attempt, now],
            )?;
        }
        db.execute("UPDATE shared_jobs SET state='failed',result_json=?2,revision=revision+1,updated_at_ms=?3 WHERE id=?1", params![id, expired_result().to_string(), now])?;
        if let Some(parent) = job.parent_id {
            let woke = db.execute("UPDATE shared_jobs SET state='queued',revision=revision+1,updated_at_ms=?3 WHERE id=?1 AND awaiting_child_id=?2 AND state='waiting_child'", params![parent, id, now])?;
            if woke != 0 {
                next = Some(parent);
            }
        }
    }
    super::handover::settle(db, &root_id, now)?;
    Ok(())
}

/// Bounded reconciliation on normal API activity also makes an offline Runner's
/// expired queue visible. Exact allocation/Started checks do not depend on this sweep.
pub(super) fn expire_due(db: &Connection, now: u64) -> Result<()> {
    store::timestamp(now)?;
    let ids = {
        let mut statement = db.prepare("SELECT j.id FROM shared_jobs j WHERE j.start_before_ms<=?1 AND (j.state='queued' OR (j.state='assigned' AND EXISTS(SELECT 1 FROM shared_attempts a WHERE a.job_id=j.id AND a.state='assigned'))) ORDER BY j.start_before_ms,j.id LIMIT 128")?;
        statement
            .query_map([now], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for id in ids {
        expire_job(db, &id, now)?;
    }
    Ok(())
}

impl SharedWork {
    pub(crate) fn expire_start_deadlines(&mut self, now: u64) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        expire_due(&tx, now)?;
        tx.commit()?;
        Ok(())
    }
}

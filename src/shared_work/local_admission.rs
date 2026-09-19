//! Local and legacy moyAI execution reserves the ordinary Hub capacity before any effect.
use super::*;
use crate::identity::{self, Principal, SessionAudience};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalAdmission {
    pub request_id: String,
    pub environment_id: String,
    pub project_id: String,
    pub actor_device_id: String,
    pub principal_session: String,
    pub source: String,
    pub title: String,
}

impl SharedWork {
    pub fn admit_local(
        &mut self,
        runner: &str,
        request: &LocalAdmission,
        now: u64,
    ) -> Result<Assignment> {
        for value in [
            runner,
            &request.request_id,
            &request.environment_id,
            &request.project_id,
            &request.actor_device_id,
        ] {
            store::id(value)?;
        }
        store::label(&request.title)?;
        if !matches!(
            request.source.as_str(),
            "local" | "desktop" | "cli" | "tui" | "legacy_mcp"
        ) {
            return Err(WorkError::Invalid);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::project_setup::require_project_controller(
            &tx,
            &request.project_id,
            &request.actor_device_id,
        )?;
        // The forwarding Runner cannot invent a human identity. The opaque session is verified
        // against its original device and is never retained in a work receipt or projection.
        let actor: Principal = identity::authenticate(
            &tx,
            &request.principal_session,
            SessionAudience::SharedWork,
            Some(&request.actor_device_id),
            now,
        )?;
        let submit = SubmitJob {
            request_id: format!("local:{}", request.request_id),
            project_id: request.project_id.clone(),
            environment_id: request.environment_id.clone(),
            title: request.title.clone(),
            input: serde_json::json!({"version":1,"prompt":request.title,"source":request.source,"actor_device_id":request.actor_device_id}),
            descendant_budget: 0,
            start_before_ms: None,
        };
        let job = store::submit_in_transaction(&tx, &actor, &submit, now)?;
        let existing: Option<String> = tx.query_row("SELECT a.id FROM shared_external_attempts x JOIN shared_attempts a ON a.id=x.attempt_id WHERE a.job_id=?1 AND a.runner_id=?2",params![job.id,runner],|row|row.get(0)).optional()?;
        if let Some(attempt) = existing {
            return store::assignment(&tx, &attempt);
        }
        let assignment = store::reserve_job(&tx, runner, &job, now)?;
        tx.execute("INSERT INTO shared_external_attempts(attempt_id,source,actor_device_id) VALUES(?1,?2,?3)",params![assignment.attempt_id,request.source,request.actor_device_id])?;
        // This is the exact start acknowledgement. A lost response may have allowed execution;
        // no lease timeout or later claimant can recycle it without an explicit final report.
        tx.execute("UPDATE shared_attempts SET state='running',updated_at_ms=?2 WHERE id=?1 AND state='assigned'",params![assignment.attempt_id,now])?;
        tx.execute("UPDATE shared_jobs SET state='running',revision=revision+1,updated_at_ms=?2 WHERE id=?1 AND state='assigned'",params![job.id,now])?;
        let result = store::assignment(&tx, &assignment.attempt_id)?;
        tx.commit()?;
        Ok(result)
    }
}

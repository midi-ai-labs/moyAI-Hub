//! The lease row is the capacity owner for a process retained after a job yields.
use super::*;
use crate::identity::{self, Principal, ProjectRole};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceLeaseView {
    pub service_id: String,
    pub attempt_id: String,
    pub generation: u64,
    pub conversation_id: String,
    pub environment_id: String,
    pub expires_at_ms: u64,
    pub stop_requested: bool,
    pub uncertain: bool,
    pub uncertainty_reason: Option<String>,
    pub can_stop: bool,
}

impl SharedWork {
    fn service_view(
        &self,
        row: &rusqlite::Row<'_>,
        now: u64,
        stop_actor: Option<(&str, ProjectRole)>,
    ) -> rusqlite::Result<ServiceLeaseView> {
        let environment_id: String = row.get(4)?;
        let root_state: String = row.get(7)?;
        let runner: String = row.get(10)?;
        let assignee: String = row.get(11)?;
        let project: String = row.get(12)?;
        let authority_revoked =
            store::may_use(&self.db, &assignee, &project, &environment_id).is_err();
        Ok(ServiceLeaseView {
            service_id: row.get(0)?,
            attempt_id: row.get(1)?,
            generation: row.get(2)?,
            conversation_id: row.get(3)?,
            environment_id: environment_id.clone(),
            expires_at_ms: row.get(5)?,
            stop_requested: row.get::<_, Option<u64>>(6)?.is_some()
                || matches!(root_state.as_str(), "cancelling" | "cancelled" | "failed")
                || authority_revoked
                || now >= row.get::<_, u64>(5)?,
            uncertain: row.get::<_, String>(8)? == "uncertain"
                || self
                    .environment_contact(&runner, &environment_id, now)
                    .state
                    != RunnerContactState::Recent,
            uncertainty_reason: row.get(9)?,
            can_stop: stop_actor.is_none_or(|(actor, role)| {
                role >= ProjectRole::Contributor
                    && (role == ProjectRole::Manager || actor == assignee)
            }),
        })
    }

    fn service_rows(
        &self,
        sql: &str,
        first: &str,
        conversation: Option<&str>,
        now: u64,
        stop_actor: Option<(&str, ProjectRole)>,
    ) -> Result<Vec<ServiceLeaseView>> {
        store::timestamp(now)?;
        let mut statement = self.db.prepare(sql)?;
        Ok(statement
            .query_map(params![first, conversation], |row| {
                self.service_view(row, now, stop_actor)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(super) fn origin_services(
        &self,
        actor: &Principal,
        device: &str,
        origin: &str,
        before: Option<&str>,
        now: u64,
    ) -> Result<(Vec<super::projection::OriginServiceView>, Option<String>)> {
        store::timestamp(now)?;
        let mut statement = self.db.prepare("SELECT s.service_id,s.attempt_id,s.generation,j.conversation_id,j.environment_id,s.expires_at_ms,s.stop_requested_at_ms,root.state,s.state,s.uncertainty_reason,a.runner_id,j.assignee_id,j.project_id,m.role FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id JOIN shared_jobs origin ON origin.id=j.conversation_id AND origin.id=origin.root_id AND origin.parent_id IS NULL AND origin.conversation_id=origin.id AND origin.project_id=j.project_id AND origin.origin_session_ref=j.origin_session_ref AND origin.origin_device_id=j.origin_device_id JOIN shared_jobs root ON root.id=j.root_id JOIN shared_projects p ON p.id=j.project_id AND p.archived=0 JOIN identity_effective_project_memberships m ON m.project_id=j.project_id AND m.user_id=?2 WHERE j.origin_session_ref=?1 AND j.origin_device_id=?3 AND origin.requestor_id=?2 AND s.state IN ('active','uncertain') AND (?4 IS NULL OR s.service_id<?4) AND (p.legacy_devices_unrestricted=1 OR EXISTS(SELECT 1 FROM shared_project_devices d WHERE d.project_id=j.project_id AND d.device_id=?3 AND d.can_control=1)) ORDER BY s.service_id DESC LIMIT 129")?;
        let mut services = statement
            .query_map(params![origin, actor.user_id, device, before], |row| {
                let project_id: String = row.get(12)?;
                let role: String = row.get(13)?;
                let mut service = self.service_view(row, now, None)?;
                service.can_stop = role == "manager"
                    || (role == "contributor" && row.get::<_, String>(11)? == actor.user_id);
                Ok(super::projection::OriginServiceView {
                    project_id,
                    service,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let next_service_before = if services.len() > 128 {
            services.truncate(128);
            services.last().map(|item| item.service.service_id.clone())
        } else {
            None
        };
        Ok((services, next_service_before))
    }

    pub fn runner_services(&self, runner: &str, now: u64) -> Result<Vec<ServiceLeaseView>> {
        store::id(runner)?;
        self.service_rows("SELECT s.service_id,s.attempt_id,s.generation,j.conversation_id,j.environment_id,s.expires_at_ms,s.stop_requested_at_ms,root.state,s.state,s.uncertainty_reason,a.runner_id,j.assignee_id,j.project_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id JOIN shared_jobs root ON root.id=j.root_id WHERE a.runner_id=?1 AND (?2 IS NULL OR j.conversation_id=?2) AND s.state IN ('active','uncertain') ORDER BY s.service_id LIMIT 128",runner,None,now,None)
    }

    pub fn conversation_services_for_attempt(
        &self,
        runner: &str,
        attempt_id: &str,
        generation: u64,
        now: u64,
    ) -> Result<Vec<ServiceLeaseView>> {
        store::id(runner)?;
        store::id(attempt_id)?;
        store::timestamp(now)?;
        let scope: Option<(String, String)> = self
            .db
            .query_row(
                "SELECT j.project_id,j.conversation_id FROM shared_attempts a \
                 JOIN shared_jobs j ON j.id=a.job_id \
                 WHERE a.id=?1 AND a.runner_id=?2 AND a.generation=?3 \
                   AND a.state IN ('running','stop_requested')",
                params![attempt_id, runner, generation],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (project, conversation) = scope.ok_or(WorkError::Forbidden)?;
        self.service_rows("SELECT s.service_id,s.attempt_id,s.generation,j.conversation_id,j.environment_id,s.expires_at_ms,s.stop_requested_at_ms,root.state,s.state,s.uncertainty_reason,a.runner_id,j.assignee_id,j.project_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id JOIN shared_jobs root ON root.id=j.root_id WHERE j.project_id=?1 AND j.conversation_id=?2 AND s.state IN ('active','uncertain') ORDER BY s.service_id LIMIT 128",&project,Some(&conversation),now,None)
    }

    pub fn project_services(
        &self,
        actor: &Principal,
        project: &str,
        conversation: Option<&str>,
        now: u64,
    ) -> Result<Vec<ServiceLeaseView>> {
        store::id(project)?;
        if let Some(conversation) = conversation {
            store::id(conversation)?;
        }
        let role =
            identity::require_project_role(&self.db, &actor.user_id, project, ProjectRole::Reader)?;
        self.service_rows("SELECT s.service_id,s.attempt_id,s.generation,j.conversation_id,j.environment_id,s.expires_at_ms,s.stop_requested_at_ms,root.state,s.state,s.uncertainty_reason,a.runner_id,j.assignee_id,j.project_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id JOIN shared_jobs root ON root.id=j.root_id WHERE j.project_id=?1 AND (?2 IS NULL OR j.conversation_id=?2) AND s.state IN ('active','uncertain') ORDER BY s.service_id LIMIT 128",project,conversation,now,Some((&actor.user_id,role)))
    }

    pub fn request_service_stop(
        &mut self,
        actor: &Principal,
        service_id: &str,
        now: u64,
    ) -> Result<ServiceLeaseView> {
        store::id(service_id)?;
        store::timestamp(now)?;
        let (project,assignee):(String,String)=self.db.query_row("SELECT j.project_id,j.assignee_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id WHERE s.service_id=?1 AND s.state IN ('active','uncertain')",[service_id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?.ok_or(WorkError::NotFound)?;
        let role = identity::require_project_role(
            &self.db,
            &actor.user_id,
            &project,
            ProjectRole::Contributor,
        )?;
        if actor.user_id != assignee && role != ProjectRole::Manager {
            return Err(WorkError::Forbidden);
        }
        self.db.execute("UPDATE shared_service_leases SET stop_requested_at_ms=COALESCE(stop_requested_at_ms,?2),updated_at_ms=?2 WHERE service_id=?1 AND state IN ('active','uncertain')",params![service_id,now])?;
        let runner:String=self.db.query_row("SELECT a.runner_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id WHERE s.service_id=?1",[service_id],|row|row.get(0))?;
        self.runner_services(&runner, now)?
            .into_iter()
            .find(|service| service.service_id == service_id)
            .ok_or(WorkError::NotFound)
    }

    /// One durable request stops the services that exist at its first admission.
    /// The existing management receipt key prevents a retry from touching a
    /// service started later in this conversation; only ServiceStopped releases
    /// the individual resource lease.
    pub fn request_conversation_services_stop(
        &mut self,
        actor: &Principal,
        project_id: &str,
        conversation_id: &str,
        request_id: &str,
        now: u64,
    ) -> Result<Value> {
        store::id(project_id)?;
        store::id(conversation_id)?;
        store::id(request_id)?;
        store::timestamp(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let role = identity::require_project_role(
            &tx,
            &actor.user_id,
            project_id,
            ProjectRole::Contributor,
        )?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_jobs WHERE project_id=?1 AND conversation_id=?2)",
            params![project_id, conversation_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(WorkError::NotFound);
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&serde_json::json!({
                "operation":"stop_conversation_services",
                "project_id":project_id,
                "conversation_id":conversation_id
            }))?)
        );
        let previous: Option<String> = tx
            .query_row(
                "SELECT payload_hash FROM shared_management_receipts WHERE actor_id=?1 AND request_id=?2",
                params![actor.user_id, request_id],
                |row| row.get(0),
            )
            .optional()?;
        let receipt = serde_json::json!({
            "project_id":project_id,
            "conversation_id":conversation_id,
            "request_id":request_id,
            "accepted":true
        });
        if let Some(previous) = previous {
            return if previous == fingerprint {
                Ok(receipt)
            } else {
                Err(WorkError::Conflict)
            };
        }
        if role != ProjectRole::Manager {
            let foreign: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id WHERE j.project_id=?1 AND j.conversation_id=?2 AND s.state IN ('active','uncertain') AND j.assignee_id<>?3)",
                params![project_id, conversation_id, actor.user_id],
                |row| row.get(0),
            )?;
            if foreign {
                return Err(WorkError::Forbidden);
            }
        }
        management::capacity(&tx, "shared_management_receipts", 100_000)?;
        tx.execute(
            "UPDATE shared_service_leases SET stop_requested_at_ms=COALESCE(stop_requested_at_ms,?3),updated_at_ms=?3 WHERE state IN ('active','uncertain') AND service_id IN (SELECT s.service_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id WHERE j.project_id=?1 AND j.conversation_id=?2 AND s.state IN ('active','uncertain'))",
            params![project_id, conversation_id, now],
        )?;
        tx.execute(
            "INSERT INTO shared_management_receipts(request_id,actor_id,payload_hash,created_at_ms) VALUES(?1,?2,?3,?4)",
            params![request_id, actor.user_id, fingerprint, now],
        )?;
        tx.commit()?;
        Ok(receipt)
    }

    pub fn request_local_service_stop(
        &mut self,
        runner: &str,
        service_id: &str,
        now: u64,
    ) -> Result<ServiceLeaseView> {
        store::id(service_id)?;
        store::timestamp(now)?;
        let owner:Option<String>=self.db.query_row("SELECT a.runner_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id WHERE s.service_id=?1 AND s.state IN ('active','uncertain')",[service_id],|row|row.get(0)).optional()?;
        if owner.as_deref() != Some(runner) {
            return Err(WorkError::Forbidden);
        }
        self.db.execute("UPDATE shared_service_leases SET stop_requested_at_ms=COALESCE(stop_requested_at_ms,?2),updated_at_ms=?2 WHERE service_id=?1 AND state IN ('active','uncertain')",params![service_id,now])?;
        self.runner_services(runner, now)?
            .into_iter()
            .find(|service| service.service_id == service_id)
            .ok_or(WorkError::NotFound)
    }

    pub fn request_conversation_service_stop(
        &mut self,
        runner: &str,
        attempt_id: &str,
        generation: u64,
        service_id: &str,
        now: u64,
    ) -> Result<ServiceLeaseView> {
        store::id(runner)?;
        store::id(attempt_id)?;
        store::id(service_id)?;
        store::timestamp(now)?;
        let owner: Option<String> = self.db.query_row(
            "SELECT retained_attempt.runner_id FROM shared_attempts current_attempt \
             JOIN shared_jobs current_job ON current_job.id=current_attempt.job_id \
             JOIN shared_service_leases service ON service.service_id=?3 AND service.state IN ('active','uncertain') \
             JOIN shared_attempts retained_attempt ON retained_attempt.id=service.attempt_id \
             JOIN shared_jobs retained_job ON retained_job.id=retained_attempt.job_id \
             WHERE current_attempt.id=?1 AND current_attempt.runner_id=?2 \
               AND current_attempt.generation=?4 AND current_attempt.state IN ('running','stop_requested') \
               AND current_job.project_id=retained_job.project_id \
               AND current_job.conversation_id=retained_job.conversation_id",
            params![attempt_id,runner,service_id,generation],
            |row| row.get(0),
        ).optional()?;
        let owner = owner.ok_or(WorkError::Forbidden)?;
        self.db.execute("UPDATE shared_service_leases SET stop_requested_at_ms=COALESCE(stop_requested_at_ms,?2),updated_at_ms=?2 WHERE service_id=?1 AND state IN ('active','uncertain')",params![service_id,now])?;
        self.runner_services(&owner, now)?
            .into_iter()
            .find(|service| service.service_id == service_id)
            .ok_or(WorkError::NotFound)
    }
}

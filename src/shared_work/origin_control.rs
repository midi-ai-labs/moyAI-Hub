//! Normal conversation operations remain with the PC that submitted the work.
use super::*;
use crate::identity::{self, ProjectRole};
use rusqlite::{OptionalExtension, params};

pub(super) fn require_origin(
    db: &Connection,
    actor: &str,
    job: &Job,
    device: Option<&str>,
) -> Result<()> {
    identity::require_project_role(db, actor, &job.project_id, ProjectRole::Contributor)?;
    let (requestor, controller): (String, Option<String>) = db.query_row(
        "SELECT requestor_id,COALESCE(controller_device_id,origin_device_id) FROM shared_jobs WHERE id=?1",
        [&job.root_id], |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if requestor != actor || controller.as_deref() != device {
        return Err(WorkError::Forbidden);
    }
    if let Some(device) = device {
        let current: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_jobs j JOIN shared_projects p ON p.id=j.project_id
             WHERE j.id=?1 AND p.archived=0 AND p.deleted_at_ms IS NULL
               AND ((p.legacy_devices_unrestricted=1 AND j.controller_participation_generation=1)
                 OR EXISTS(SELECT 1 FROM shared_project_devices d WHERE d.project_id=j.project_id
                   AND d.device_id=?2 AND d.can_control=1
                   AND d.participation_generation=j.controller_participation_generation)))",
            params![job.root_id, device],
            |row| row.get(0),
        )?;
        if !current {
            return Err(WorkError::Forbidden);
        }
    }
    // Completed transfers are kept as history, never as authority to resume.
    let transferred: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_handovers h JOIN shared_jobs j ON j.id=h.root_id
         WHERE j.project_id=?1 AND j.conversation_id=?2 AND h.state='completed')",
        params![job.project_id, job.conversation_id],
        |row| row.get(0),
    )?;
    if transferred {
        return Err(WorkError::Forbidden);
    }
    Ok(())
}

impl SharedWork {
    pub(super) fn require_service_origin(
        &self,
        actor: &crate::identity::Principal,
        service: &str,
        device: &str,
    ) -> Result<()> {
        let job: String = self.db.query_row("SELECT a.job_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id WHERE s.service_id=?1", [service], |row| row.get(0))?;
        self.require_job_origin(actor, &job, device)
    }

    pub(super) fn restrict_summary_to_origin(
        &self,
        actor: &crate::identity::Principal,
        device: &str,
        summary: &mut JobSummary,
    ) -> Result<()> {
        let allowed = self.require_job_origin(actor, &summary.id, device).is_ok();
        summary.can_cancel &= allowed;
        summary.can_continue &= allowed;
        summary.can_revise &= allowed;
        Ok(())
    }

    pub(super) fn restrict_detail_to_origin(
        &self,
        actor: &crate::identity::Principal,
        device: &str,
        detail: &mut Value,
    ) -> Result<()> {
        let id = detail["id"].as_str().ok_or(WorkError::Invalid)?;
        if self.require_job_origin(actor, id, device).is_err() {
            for key in ["can_cancel", "can_continue", "can_revise"] {
                detail[key] = Value::Bool(false);
            }
        }
        if let Some(services) = detail["retained_services"].as_array_mut() {
            for service in services {
                if self
                    .require_service_origin(
                        actor,
                        service["service_id"].as_str().ok_or(WorkError::Invalid)?,
                        device,
                    )
                    .is_err()
                {
                    service["can_stop"] = Value::Bool(false);
                }
            }
        }
        Ok(())
    }

    pub(super) fn restrict_conversations_to_origin(
        &self,
        actor: &crate::identity::Principal,
        project: &str,
        device: &str,
        listing: &mut Value,
    ) -> Result<()> {
        for conversation in listing["conversations"]
            .as_array_mut()
            .ok_or(WorkError::Invalid)?
        {
            let id = conversation["id"].as_str().ok_or(WorkError::Invalid)?;
            let allowed = self
                .require_conversation_origin(actor, project, id, device)
                .is_ok();
            let pending = super::conversation_catalog::deletion_pending(&self.db, project, id)?;
            if !allowed {
                conversation["can_revise"] = Value::Bool(false);
                if pending {
                    conversation["can_delete"] = Value::Bool(false);
                }
            }
        }
        Ok(())
    }
    pub(super) fn require_job_origin(
        &self,
        actor: &crate::identity::Principal,
        id: &str,
        device: &str,
    ) -> Result<()> {
        let job = super::store::load(&self.db, id)?;
        require_origin(&self.db, &actor.user_id, &job, Some(device))
    }

    pub(super) fn require_conversation_origin(
        &self,
        actor: &crate::identity::Principal,
        project: &str,
        conversation: &str,
        device: &str,
    ) -> Result<()> {
        let mut stmt = self.db.prepare(
            "SELECT id FROM shared_jobs WHERE project_id=?1 AND conversation_id=?2 AND id=root_id",
        )?;
        let ids = stmt
            .query_map(params![project, conversation], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if ids.is_empty() {
            return Err(WorkError::NotFound);
        }
        for id in ids {
            self.require_job_origin(actor, &id, device)?;
        }
        Ok(())
    }
}

/// Run inside the schema transaction, before any persistent migration changes.
pub(super) fn retirement_preflight(db: &Connection) -> Result<()> {
    let affected: Option<String> = db.query_row(
        "SELECT h.root_id FROM shared_handovers h WHERE h.state='completed' AND (
          EXISTS(SELECT 1 FROM shared_jobs j WHERE j.root_id=h.root_id AND j.state NOT IN ('succeeded','failed','cancelled'))
          OR EXISTS(SELECT 1 FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id WHERE j.root_id=h.root_id AND a.state IN ('assigned','running','stop_requested'))
          OR EXISTS(SELECT 1 FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id WHERE j.root_id=h.root_id AND s.state IN ('active','uncertain'))
        ) ORDER BY h.root_id LIMIT 1", [], |row| row.get(0),
    ).optional()?;
    if let Some(root) = affected {
        return Err(WorkError::Management(format!(
            "旧担当交代の仕事 {root} が未清算です。既存版で完了または管理取消と実停止確認を済ませてから更新してください。"
        )));
    }
    Ok(())
}

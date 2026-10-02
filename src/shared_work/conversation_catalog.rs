//! The first submitted root job is a shared conversation's stable identity.
//! Edits and deletion are small metadata over that canonical job history.
//! External attempts use the same job ledger for capacity, but their conversations
//! remain owned by the local Desktop/CLI/TUI session.
use super::*;
use crate::identity::{self, Principal, ProjectRole};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::json;

pub(super) fn anchor_exists(db: &Connection, project: &str, conversation: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_jobs j JOIN shared_projects p ON p.id=j.project_id
         WHERE j.id=?2 AND j.project_id=?1 AND j.id=j.root_id AND j.parent_id IS NULL
           AND j.conversation_id=j.id AND p.archived=0 AND p.deleted_at_ms IS NULL
           AND NOT EXISTS(SELECT 1 FROM shared_attempts a JOIN shared_external_attempts x
                          ON x.attempt_id=a.id WHERE a.job_id=j.id))",
        params![project, conversation],
        |row| row.get(0),
    )?)
}

pub(super) fn deletion_pending(db: &Connection, project: &str, conversation: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_jobs j WHERE j.project_id=?1 AND j.conversation_id=?2
                 AND j.state NOT IN ('succeeded','failed','cancelled'))
           OR EXISTS(SELECT 1 FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id
                     WHERE j.project_id=?1 AND j.conversation_id=?2
                       AND a.state IN ('assigned','running','stop_requested'))
           OR EXISTS(SELECT 1 FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id
                     JOIN shared_jobs j ON j.id=a.job_id
                     WHERE j.project_id=?1 AND j.conversation_id=?2
                       AND s.state IN ('active','uncertain'))",
        params![project, conversation], |row| row.get(0),
    )?)
}

pub(super) fn deletion_requested(
    db: &Connection,
    project: &str,
    conversation: &str,
) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_conversation_details WHERE project_id=?1 AND conversation_id=?2 AND delete_requested_at_ms IS NOT NULL)",
        params![project, conversation], |row| row.get(0),
    )?)
}

pub(super) fn is_latest_root(db: &Connection, job: &Job) -> Result<bool> {
    if job.id != job.root_id || job.parent_id.is_some() {
        return Ok(false);
    }
    let latest: Option<String> = db
        .query_row(
            "SELECT id FROM shared_jobs WHERE project_id=?1 AND conversation_id=?2
         AND parent_id IS NULL AND id=root_id ORDER BY rowid DESC LIMIT 1",
            params![job.project_id, job.conversation_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(latest.as_deref() == Some(job.id.as_str()))
}

pub(super) fn execution_pending(
    db: &Connection,
    project: &str,
    conversation: &str,
) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_jobs j WHERE j.project_id=?1 AND j.conversation_id=?2
                 AND j.state NOT IN ('succeeded','failed','cancelled'))
           OR EXISTS(SELECT 1 FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id
                     WHERE j.project_id=?1 AND j.conversation_id=?2
                       AND a.state IN ('assigned','running','stop_requested'))",
        params![project, conversation],
        |row| row.get(0),
    )?)
}

pub(super) fn can_revise(db: &Connection, job: &Job) -> Result<bool> {
    if job.id != job.root_id
        || job.parent_id.is_some()
        || !job.state.terminal()
        || !anchor_exists(db, &job.project_id, &job.conversation_id)?
        || deletion_pending(db, &job.project_id, &job.conversation_id)?
    {
        return Ok(false);
    }
    if deletion_requested(db, &job.project_id, &job.conversation_id)? {
        return Ok(false);
    }
    is_latest_root(db, job)
}

pub(super) fn revision_predecessor(
    db: &Connection,
    actor: &str,
    job: &Job,
    revision: u64,
) -> Result<Option<String>> {
    identity::require_project_role(db, actor, &job.project_id, ProjectRole::Contributor)?;
    if job.revision != revision || !can_revise(db, job)? {
        return Err(WorkError::Conflict);
    }
    let prior: Option<String> = db.query_row(
        "SELECT j.id FROM shared_jobs j WHERE j.project_id=?1 AND j.conversation_id=?2
         AND j.parent_id IS NULL AND j.id=j.root_id AND j.state='succeeded'
         AND j.rowid<(SELECT rowid FROM shared_jobs WHERE id=?3)
         AND EXISTS(SELECT 1 FROM shared_assets a WHERE a.job_id=j.id AND a.kind='archive' AND a.purged_at_ms IS NULL)
         ORDER BY j.rowid DESC LIMIT 1",
        params![job.project_id,job.conversation_id,job.id], |row| row.get(0),
    ).optional()?;
    Ok(prior)
}

fn current_revision(db: &Connection, project: &str, conversation: &str) -> Result<u64> {
    Ok(db.query_row("SELECT COALESCE((SELECT revision FROM shared_conversation_details WHERE project_id=?1 AND conversation_id=?2),1)",params![project,conversation],|row|row.get(0))?)
}

pub(super) fn appended_root(
    db: &Connection,
    project: &str,
    conversation: &str,
    now: u64,
) -> Result<()> {
    db.execute("INSERT INTO shared_conversation_details(project_id,conversation_id,updated_at_ms,revision) VALUES(?1,?2,?3,2) ON CONFLICT(project_id,conversation_id) DO UPDATE SET updated_at_ms=excluded.updated_at_ms,revision=revision+1",params![project,conversation,now])?;
    Ok(())
}

pub(super) fn visible(db: &Connection, project: &str, conversation: &str) -> Result<bool> {
    let requested: Option<Option<u64>> = db
        .query_row(
            "SELECT delete_requested_at_ms FROM shared_conversation_details
         WHERE project_id=?1 AND conversation_id=?2",
            params![project, conversation],
            |row| row.get(0),
        )
        .optional()?;
    Ok(match requested.flatten() {
        None => true,
        Some(_) => deletion_pending(db, project, conversation)?,
    })
}

impl SharedWork {
    pub fn list_project_conversations(&self, actor: &Principal, project: &str) -> Result<Value> {
        store::id(project)?;
        identity::require_project_role(&self.db, &actor.user_id, project, ProjectRole::Reader)?;
        let available:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM shared_projects WHERE id=?1 AND archived=0 AND deleted_at_ms IS NULL)",[project],|row|row.get(0))?;
        if !available {
            return Err(WorkError::NotFound);
        }
        let anchors=self.db.prepare(
            "SELECT anchor.id,COALESCE(details.title,anchor.title),
               (SELECT j.id FROM shared_jobs j WHERE j.project_id=anchor.project_id AND j.conversation_id=anchor.id AND j.parent_id IS NULL AND j.id=j.root_id ORDER BY j.rowid DESC LIMIT 1),
               MAX(COALESCE(details.updated_at_ms,0),(SELECT MAX(j.updated_at_ms) FROM shared_jobs j WHERE j.project_id=anchor.project_id AND j.conversation_id=anchor.id)),
               details.delete_requested_at_ms,COALESCE(details.revision,1)
             FROM shared_jobs anchor LEFT JOIN shared_conversation_details details
               ON details.project_id=anchor.project_id AND details.conversation_id=anchor.id
             WHERE anchor.project_id=?1 AND anchor.id=anchor.root_id AND anchor.parent_id IS NULL
               AND anchor.conversation_id=anchor.id
               AND NOT EXISTS(SELECT 1 FROM shared_attempts a JOIN shared_external_attempts x
                              ON x.attempt_id=a.id WHERE a.job_id=anchor.id)
             ORDER BY anchor.created_at_ms DESC,anchor.id DESC LIMIT 1000"
        )?.query_map([project],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Option<String>>(2)?,row.get::<_,u64>(3)?,row.get::<_,Option<u64>>(4)?,row.get::<_,u64>(5)?)))?
          .collect::<std::result::Result<Vec<_>,_>>()?;
        let role =
            identity::require_project_role(&self.db, &actor.user_id, project, ProjectRole::Reader)?;
        let can_change = matches!(role, ProjectRole::Contributor | ProjectRole::Manager);
        let mut conversations = Vec::new();
        for (id, title, latest_job_id, updated_at_ms, delete_requested_at_ms, revision) in anchors {
            let pending =
                delete_requested_at_ms.is_some() && deletion_pending(&self.db, project, &id)?;
            if delete_requested_at_ms.is_some() && !pending {
                continue;
            }
            let can_revise = if can_change && !pending {
                latest_job_id
                    .as_deref()
                    .map(|latest| store::load(&self.db, latest))
                    .transpose()?
                    .map(|latest| {
                        Ok::<bool, WorkError>(
                            latest.requestor_id == actor.user_id && can_revise(&self.db, &latest)?,
                        )
                    })
                    .transpose()?
                    .unwrap_or(false)
            } else {
                false
            };
            conversations.push(json!({"id":id,"title":title,"latest_job_id":latest_job_id,
                "updated_at_ms":updated_at_ms,"revision":revision,"delete_pending":pending,
                "can_rename":can_change && !pending,"can_delete":can_change && !pending,
                "can_revise":can_revise}));
        }
        let revision: u64 = self.db.query_row(
            "SELECT revision FROM shared_management_state WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        Ok(json!({"revision":revision.to_string(),"conversations":conversations}))
    }

    pub fn rename_project_conversation(
        &mut self,
        actor: &Principal,
        project: &str,
        conversation: &str,
        title: &str,
        expected_revision: u64,
        now: u64,
    ) -> Result<Value> {
        for value in [project, conversation] {
            store::id(value)?;
        }
        store::label(title)?;
        store::timestamp(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        identity::require_project_role(&tx, &actor.user_id, project, ProjectRole::Contributor)?;
        if !anchor_exists(&tx, project, conversation)? {
            return Err(WorkError::NotFound);
        }
        if !visible(&tx, project, conversation)? {
            return Err(WorkError::NotFound);
        }
        if current_revision(&tx, project, conversation)? != expected_revision {
            return Err(WorkError::Conflict);
        }
        let deleting:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM shared_conversation_details WHERE project_id=?1 AND conversation_id=?2 AND delete_requested_at_ms IS NOT NULL)",params![project,conversation],|row|row.get(0))?;
        if deleting {
            return Err(WorkError::Conflict);
        }
        tx.execute("INSERT INTO shared_conversation_details(project_id,conversation_id,title,updated_at_ms,revision) VALUES(?1,?2,?3,?4,2) ON CONFLICT(project_id,conversation_id) DO UPDATE SET title=excluded.title,updated_at_ms=excluded.updated_at_ms,revision=revision+1",params![project,conversation,title,now])?;
        let revision:u64=tx.query_row("SELECT revision FROM shared_conversation_details WHERE project_id=?1 AND conversation_id=?2",params![project,conversation],|row|row.get(0))?;
        tx.commit()?;
        Ok(json!({"id":conversation,"project_id":project,"title":title,"revision":revision}))
    }

    pub fn delete_project_conversation(
        &mut self,
        actor: &Principal,
        project: &str,
        conversation: &str,
        expected_revision: u64,
        now: u64,
    ) -> Result<Value> {
        for value in [project, conversation] {
            store::id(value)?;
        }
        store::timestamp(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        identity::require_project_role(&tx, &actor.user_id, project, ProjectRole::Contributor)?;
        if !anchor_exists(&tx, project, conversation)? {
            return Err(WorkError::NotFound);
        }
        let already:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM shared_conversation_details WHERE project_id=?1 AND conversation_id=?2 AND delete_requested_at_ms IS NOT NULL)",params![project,conversation],|row|row.get(0))?;
        if !already {
            if current_revision(&tx, project, conversation)? != expected_revision {
                return Err(WorkError::Conflict);
            }
            tx.execute("INSERT INTO shared_conversation_details(project_id,conversation_id,delete_requested_at_ms,updated_at_ms,revision) VALUES(?1,?2,?3,?3,2) ON CONFLICT(project_id,conversation_id) DO UPDATE SET delete_requested_at_ms=excluded.delete_requested_at_ms,updated_at_ms=excluded.updated_at_ms,revision=revision+1",params![project,conversation,now])?;
            let roots=tx.prepare("SELECT id FROM shared_jobs WHERE project_id=?1 AND conversation_id=?2 AND id=root_id AND parent_id IS NULL AND state NOT IN ('succeeded','failed','cancelled') ORDER BY id")?.query_map(params![project,conversation],|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
            for root in roots {
                store::cancel_root_in_transaction(&tx, &root, now)?;
            }
            tx.execute("UPDATE shared_service_leases SET stop_requested_at_ms=COALESCE(stop_requested_at_ms,?3),updated_at_ms=?3 WHERE state IN ('active','uncertain') AND service_id IN (SELECT s.service_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id WHERE j.project_id=?1 AND j.conversation_id=?2)",params![project,conversation,now])?;
        }
        let pending = deletion_pending(&tx, project, conversation)?;
        tx.commit()?;
        Ok(
            json!({"id":conversation,"project_id":project,"delete_pending":pending,"deleted":!pending}),
        )
    }
}

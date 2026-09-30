use super::{Job, ProjectContext, Result, WorkError};
use rusqlite::{Connection, params};

pub(super) const MAX_OVERVIEW_BYTES: usize = 8 * 1024;

pub(super) fn normalize_overview(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.len() > MAX_OVERVIEW_BYTES || value.contains('\0') {
        return Err(WorkError::Management(
            "プロジェクト概要はUTF-8で8 KiB以内にしてください。".into(),
        ));
    }
    Ok(value)
}

pub(super) fn capture(
    db: &Connection,
    job_id: &str,
    project_id: &str,
    root_prompt: &str,
    origin_device_id: Option<&str>,
) -> Result<()> {
    let (label, overview, revision): (String, String, u64) = db.query_row(
        "SELECT label,overview,(SELECT revision FROM shared_management_state WHERE id=1) FROM shared_projects WHERE id=?1 AND deleted_at_ms IS NULL",
        [project_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let context = ProjectContext {
        project_id: project_id.into(),
        label,
        overview,
        revision: revision.to_string(),
        root_prompt: root_prompt.into(),
        origin_device_id: origin_device_id.map(str::to_string),
    };
    let updated = db.execute(
        "UPDATE shared_jobs SET project_context_json=?2 WHERE id=?1 AND id=root_id AND project_context_json IS NULL",
        params![job_id, serde_json::to_string(&context)?],
    )?;
    if updated != 1 {
        return Err(WorkError::Conflict);
    }
    Ok(())
}

pub(super) fn for_job(db: &Connection, job: &Job) -> Result<Option<ProjectContext>> {
    let context: Option<String> = db.query_row(
        "SELECT project_context_json FROM shared_jobs WHERE id=?1 AND project_id=?2",
        params![job.root_id, job.project_id],
        |row| row.get(0),
    )?;
    context
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}

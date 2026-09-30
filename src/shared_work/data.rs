//! Business-authorized data references point to durable files in Hub's managed directory.
use super::*;
use crate::identity::{self, Principal, ProjectRole};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

const MAX_FILE: usize = 8 * 1024 * 1024;
const MAX_ARCHIVE: usize = 64 * 1024 * 1024;
const MAX_STORE: u64 = 2 * 1024 * 1024 * 1024;
const COLUMNS: &str = "id,project_id,job_id,kind,name,sha256,byte_length,base_sha256,created_at_ms,version,purged_at_ms";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Asset {
    pub id: String,
    pub project_id: String,
    pub job_id: Option<String>,
    pub kind: String,
    pub name: String,
    pub sha256: String,
    pub byte_length: u64,
    pub base_sha256: Option<String>,
    pub created_at_ms: u64,
    pub version: u64,
    pub purged_at_ms: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetUpload {
    pub request_id: String,
    pub name: String,
    pub content_base64: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetDownload {
    pub asset: Asset,
    pub content_base64: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerAssetUpload {
    pub generation: u64,
    pub kind: String,
    pub base_sha256: Option<String>,
    pub upload: AssetUpload,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TranscriptItem {
    pub position: u64,
    pub kind: String,
    pub payload: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptPage {
    pub items: Vec<TranscriptItem>,
    pub next_after: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<TranscriptActivity>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptActivity {
    pub job_id: String,
    pub attempt_id: String,
    pub generation: u64,
    pub environment_id: String,
    pub runner_id: String,
    pub revision: u64,
    pub observed_at_ms: u64,
    pub items: Vec<TranscriptItem>,
    pub truncated: bool,
}

fn progress_job(db: &Connection, runner: &str, attempt: &str, generation: u64) -> Result<Job> {
    let (job_id, owner, actual, authority, state, uncertainty):
        (String, String, u64, u64, String, Option<String>) = db.query_row(
        "SELECT job_id,runner_id,generation,authority_generation,state,uncertainty_reason FROM shared_attempts WHERE id=?1",
        [attempt], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
    ).optional()?.ok_or(WorkError::NotFound)?;
    if owner != runner {
        return Err(WorkError::Forbidden);
    }
    let job = store::load(db, &job_id)?;
    let root = store::load(db, &job.root_id)?;
    if actual != generation
        || authority != job.authority_generation
        || authority != root.authority_generation
        || state != "running"
        || uncertainty.is_some()
        || job.state != JobState::Running
        || root.state.terminal()
        || root.state == JobState::Cancelling
    {
        return Err(WorkError::Conflict);
    }
    let current: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_environments e WHERE e.id=?1 AND e.runner_id=?2)",
        params![job.environment_id, runner],
        |r| r.get(0),
    )?;
    if !current {
        return Err(WorkError::Forbidden);
    }
    super::handover::may_continue(db, &job, &job.environment_id)?;
    Ok(job)
}

fn public_progress_payload(item: &TranscriptItem) -> bool {
    let Some(value) = item.payload.as_object() else {
        return false;
    };
    let fields = |keys: &[&str]| {
        value.len() == keys.len() && keys.iter().all(|key| value.contains_key(*key))
    };
    let strings = |keys: &[&str]| keys.iter().all(|key| item.payload[*key].is_string());
    match item.kind.as_str() {
        "assistant_message" => {
            fields(&["content"])
                && item.payload["content"].as_array().is_some_and(|parts| {
                    !parts.is_empty()
                        && parts.iter().all(|part| {
                            part.as_object().is_some_and(|object| object.len() == 2)
                                && part["kind"] == "text"
                                && part["text"].is_string()
                        })
                })
        }
        "tool_call" => {
            fields(&["call_id", "tool_name", "arguments_json"])
                && strings(&["call_id", "tool_name", "arguments_json"])
        }
        "tool_output" => {
            fields(&["call_id", "status", "title", "output_text", "success"])
                && strings(&["call_id", "status", "title", "output_text"])
                && matches!(
                    item.payload["status"].as_str(),
                    Some("pending" | "running" | "completed" | "declined" | "cancelled" | "failed")
                )
                && (item.payload["success"].is_null() || item.payload["success"].is_boolean())
        }
        "error" => fields(&["message"]) && strings(&["message"]),
        "file_change" => fields(&["summary"]) && strings(&["summary"]),
        _ => false,
    }
}

pub(super) fn record_progress(
    db: &Connection,
    runner: &str,
    report: &Report,
    payload: &str,
    now: u64,
) -> Result<Job> {
    let ReportOutcome::Progress {
        revision, items, ..
    } = &report.outcome
    else {
        return Err(WorkError::Invalid);
    };
    if *revision == 0
        || *revision > i64::MAX as u64
        || items.is_empty()
        || items.len() > 12
        || serde_json::to_vec(&report.outcome)?.len() > 64 * 1024
    {
        return Err(WorkError::Invalid);
    }
    let mut previous = 0;
    for item in items {
        if item.position <= previous
            || item.position > *revision
            || !public_progress_payload(item)
            || serde_json::to_vec(item)?.len() > 16 * 1024
        {
            return Err(WorkError::Invalid);
        }
        previous = item.position;
    }
    if previous != *revision {
        return Err(WorkError::Invalid);
    }
    // Recheck authority even for a duplicate; a stale display report must not
    // acknowledge authority after stop, leave, reassignment, or terminal.
    let job = progress_job(db, runner, &report.attempt_id, report.generation)?;
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_reports WHERE attempt_id=?1 AND event_id=?2)",
        params![report.attempt_id, report.event_id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(WorkError::Conflict);
    }
    let saved: Option<(u64, String)> = db
        .query_row(
            "SELECT revision,payload_json FROM shared_attempt_progress WHERE attempt_id=?1",
            [&report.attempt_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((saved_revision, saved_payload)) = saved {
        if *revision < saved_revision || (*revision == saved_revision && payload != saved_payload) {
            return Err(WorkError::Conflict);
        }
        if *revision == saved_revision {
            return Ok(job);
        }
    }
    db.execute("INSERT INTO shared_attempt_progress(attempt_id,revision,payload_json,observed_at_ms) VALUES(?1,?2,?3,?4) ON CONFLICT(attempt_id) DO UPDATE SET revision=excluded.revision,payload_json=excluded.payload_json,observed_at_ms=excluded.observed_at_ms",
        params![report.attempt_id,revision,payload,now])?;
    Ok(job)
}

fn transcript_activity(db: &Connection, selected: &Job) -> Result<Option<TranscriptActivity>> {
    let mut job = selected.clone();
    // The existing descendant budget bounds the chain. Follow only the current
    // awaited child, never a previously completed sibling or another root.
    for _ in 0..=1000 {
        if job.state == JobState::WaitingChild {
            let Some(child_id) = &job.awaiting_child_id else {
                return Ok(None);
            };
            let child = store::load(db, child_id)?;
            if child.parent_id.as_deref() != Some(job.id.as_str())
                || child.root_id != selected.root_id
            {
                return Err(WorkError::Conflict);
            }
            job = child;
            continue;
        }
        if job.state != JobState::Running {
            return Ok(None);
        }
        let saved: Option<(String, String, u64, String, u64)> = db.query_row(
            "SELECT a.id,a.runner_id,a.generation,p.payload_json,p.observed_at_ms FROM shared_attempts a JOIN shared_attempt_progress p ON p.attempt_id=a.id WHERE a.job_id=?1 AND a.state='running' AND NOT EXISTS(SELECT 1 FROM shared_assets archive WHERE archive.attempt_id=a.id AND archive.generation=a.generation AND archive.kind='archive' AND archive.purged_at_ms IS NULL)",
            [&job.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
        ).optional()?;
        let Some((attempt_id, runner_id, generation, payload, observed_at_ms)) = saved else {
            return Ok(None);
        };
        match progress_job(db, &runner_id, &attempt_id, generation) {
            Ok(_) => {}
            Err(
                WorkError::Forbidden
                | WorkError::Conflict
                | WorkError::NotFound
                | WorkError::Identity(identity::IdentityError::Forbidden),
            ) => return Ok(None),
            Err(error) => return Err(error),
        }
        let report: Report = serde_json::from_str(&payload)?;
        let ReportOutcome::Progress {
            revision,
            items,
            truncated,
        } = report.outcome
        else {
            return Err(WorkError::Conflict);
        };
        return Ok(Some(TranscriptActivity {
            job_id: job.id,
            attempt_id,
            generation,
            environment_id: job.environment_id,
            runner_id,
            revision,
            observed_at_ms,
            items,
            truncated,
        }));
    }
    Err(WorkError::Conflict)
}
fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Asset> {
    Ok(Asset {
        id: row.get(0)?,
        project_id: row.get(1)?,
        job_id: row.get(2)?,
        kind: row.get(3)?,
        name: row.get(4)?,
        sha256: row.get(5)?,
        byte_length: row.get(6)?,
        base_sha256: row.get(7)?,
        created_at_ms: row.get(8)?,
        version: row.get(9)?,
        purged_at_ms: row.get(10)?,
    })
}
fn get(db: &Connection, id: &str) -> Result<Asset> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM shared_assets WHERE id=?1"),
        [id],
        row,
    )
    .optional()?
    .ok_or(WorkError::NotFound)
}
fn hash_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn file_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 512
        || value.contains(['\\', ':'])
        || value.starts_with('/')
    {
        return Err(WorkError::Invalid);
    }
    for part in value.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with([' ', '.'])
            || part
                .chars()
                .any(|c| c.is_control() || "<>\"|?*".contains(c))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(WorkError::Invalid);
        }
    }
    Ok(())
}
fn bytes(upload: &AssetUpload, max: usize) -> Result<Vec<u8>> {
    store::id(&upload.request_id)?;
    file_name(&upload.name)?;
    if !hash_valid(&upload.sha256) || upload.content_base64.len() > max.div_ceil(3) * 4 {
        return Err(WorkError::Invalid);
    }
    let bytes = STANDARD
        .decode(&upload.content_base64)
        .map_err(|_| WorkError::Invalid)?;
    if bytes.len() > max || format!("{:x}", Sha256::digest(&bytes)) != upload.sha256 {
        return Err(WorkError::Invalid);
    }
    Ok(bytes)
}
#[allow(clippy::too_many_arguments)]
fn insert(
    db: &Connection,
    owner: &str,
    project: &str,
    job: Option<&str>,
    attempt: Option<(&str, u64)>,
    kind: &str,
    base: Option<&str>,
    upload: &AssetUpload,
    bytes: &[u8],
    now: u64,
) -> Result<Asset> {
    store::timestamp(now)?;
    if base.is_some_and(|hash| !hash_valid(hash)) {
        return Err(WorkError::Invalid);
    }
    let existing: Option<String> = db
        .query_row(
            "SELECT id FROM shared_assets WHERE created_by=?1 AND request_id=?2",
            params![owner, upload.request_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        let asset = get(db, &id)?;
        let identity: (Option<String>, Option<u64>) = db.query_row(
            "SELECT attempt_id,generation FROM shared_assets WHERE id=?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if asset.project_id != project
            || asset.job_id.as_deref() != job
            || asset.kind != kind
            || asset.name != upload.name
            || asset.sha256 != upload.sha256
            || asset.base_sha256.as_deref() != base
            || identity != (attempt.map(|a| a.0.to_owned()), attempt.map(|a| a.1))
        {
            return Err(WorkError::Conflict);
        }
        return Ok(asset);
    }
    let (size,count): (u64,u64) = db.query_row("SELECT COALESCE(SUM(CASE WHEN purged_at_ms IS NULL THEN byte_length ELSE 0 END),0),COUNT(*) FROM shared_assets",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if size.saturating_add(bytes.len() as u64) > MAX_STORE || count >= 100_000 {
        return Err(WorkError::Capacity);
    }
    let version: u64 = db.query_row("SELECT COALESCE(MAX(version),0)+1 FROM shared_assets WHERE project_id=?1 AND job_id IS ?2 AND name=?3 AND kind=?4",params![project,job,upload.name,kind],|r|r.get(0))?;
    let id = ulid::Ulid::new().to_string();
    super::blob_store::persist(db, &upload.sha256, bytes)?;
    db.execute("INSERT INTO shared_assets(id,project_id,job_id,attempt_id,generation,kind,name,sha256,content,byte_length,base_sha256,created_by,request_id,created_at_ms,version) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        params![id,project,job,attempt.map(|a|a.0),attempt.map(|a|a.1),kind,upload.name,upload.sha256,&[] as &[u8],bytes.len() as u64,base,owner,upload.request_id,now,version])?;
    get(db, &id)
}
fn download(db: &Connection, asset: Asset) -> Result<AssetDownload> {
    if db.query_row(
        "SELECT purged_at_ms IS NOT NULL FROM shared_assets WHERE id=?1",
        [&asset.id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(WorkError::NotFound);
    }
    let bytes = super::blob_store::read(db, &asset.sha256, asset.byte_length)?;
    if bytes.len() as u64 != asset.byte_length
        || format!("{:x}", Sha256::digest(&bytes)) != asset.sha256
    {
        return Err(WorkError::Conflict);
    }
    Ok(AssetDownload {
        asset,
        content_base64: STANDARD.encode(bytes),
    })
}

pub(super) fn storage_status(db: &Connection) -> Result<Value> {
    let (bytes,count,purged):(u64,u64,u64)=db.query_row("SELECT COALESCE(SUM(CASE WHEN purged_at_ms IS NULL THEN byte_length ELSE 0 END),0),COUNT(*),COALESCE(SUM(purged_at_ms IS NOT NULL),0) FROM shared_assets",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    Ok(
        serde_json::json!({"retained_bytes":bytes,"asset_count":count,"purged_asset_count":purged,"limit_bytes":MAX_STORE}),
    )
}
const PRUNABLE:&str="a.purged_at_ms IS NULL AND a.created_at_ms < ?1
 AND (a.job_id IS NULL OR EXISTS(SELECT 1 FROM shared_jobs j WHERE j.id=a.job_id AND j.state IN ('succeeded','failed','cancelled') AND j.updated_at_ms < ?1
   AND NOT EXISTS(SELECT 1 FROM shared_jobs member WHERE member.root_id=j.root_id AND member.state NOT IN ('succeeded','failed','cancelled'))))
 AND NOT EXISTS(SELECT 1 FROM shared_job_inputs i JOIN shared_jobs j ON j.id=i.job_id WHERE i.asset_id=a.id AND (j.state NOT IN ('succeeded','failed','cancelled') OR j.updated_at_ms >= ?1))
 AND NOT EXISTS(SELECT 1 FROM shared_continuations c JOIN shared_jobs next ON next.id=c.job_id WHERE c.previous_job_id=a.job_id AND (next.state NOT IN ('succeeded','failed','cancelled') OR next.updated_at_ms >= ?1))";
pub(super) fn retention_preview(db: &Connection, before: u64) -> Result<Value> {
    store::timestamp(before)?;
    let (count, bytes): (u64, u64) = db.query_row(
        &format!(
            "SELECT COUNT(*),COALESCE(SUM(byte_length),0) FROM shared_assets a WHERE {PRUNABLE}"
        ),
        [before],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(serde_json::json!({"before_ms":before,"asset_count":count,"byte_count":bytes}))
}
pub(super) fn prune(db: &Connection, before: u64, now: u64) -> Result<()> {
    if before == 0 || before > now {
        return Err(WorkError::Invalid);
    }
    let preview = retention_preview(db, before)?;
    db.execute(&format!("UPDATE shared_assets SET content=x'',purged_at_ms=?2 WHERE id IN (SELECT a.id FROM shared_assets a WHERE {PRUNABLE})"),params![before,now])?;
    db.execute("INSERT INTO shared_data_retention_runs(id,before_ms,asset_count,byte_count,created_at_ms) VALUES(?1,?2,?3,?4,?5)",params![ulid::Ulid::new().to_string(),before,preview["asset_count"].as_u64().unwrap_or(0),preview["byte_count"].as_u64().unwrap_or(0),now])?;
    Ok(())
}

/// Called inside the job owner's creation transaction. References never grant another project's data.
pub(super) fn validate_inputs(db: &Connection, project: &str, input: &Value) -> Result<()> {
    let Some(refs) = input.get("input_refs") else {
        return Ok(());
    };
    let refs = refs.as_array().ok_or(WorkError::Invalid)?;
    if input["version"] != 2 || refs.len() > 32 {
        return Err(WorkError::Invalid);
    }
    let mut names = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeSet::new();
    for id in refs {
        let id = id.as_str().ok_or(WorkError::Invalid)?;
        let asset = get(db, id)?;
        if asset.project_id != project || asset.kind == "archive" {
            return Err(WorkError::Forbidden);
        }
        if db.query_row(
            "SELECT purged_at_ms IS NOT NULL FROM shared_assets WHERE id=?1",
            [id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(WorkError::NotFound);
        }
        let name = asset.name.to_lowercase();
        if !ids.insert(id)
            || !names.insert(name.clone())
            || names.iter().any(|other| {
                other != &name
                    && (other.starts_with(&(name.clone() + "/"))
                        || name.starts_with(&(other.clone() + "/")))
            })
        {
            return Err(WorkError::Invalid);
        }
    }
    Ok(())
}
pub(super) fn bind_inputs(db: &Connection, job: &Job) -> Result<()> {
    validate_inputs(db, &job.project_id, &job.input)?;
    if let Some(refs) = job.input["input_refs"].as_array() {
        for id in refs {
            db.execute(
                "INSERT OR IGNORE INTO shared_job_inputs(job_id,asset_id) VALUES(?1,?2)",
                params![job.id, id.as_str().ok_or(WorkError::Invalid)?],
            )?;
        }
    }
    Ok(())
}

pub(super) fn require_checkpoint_archive(db: &Connection, report: &Report) -> Result<()> {
    let ReportOutcome::YieldToChild { checkpoint, .. } = &report.outcome else {
        return Ok(());
    };
    // Old receipts predate portable history. New receipts cannot release the worker slot until
    // their exact canonical archive is durable under the same attempt and generation.
    if checkpoint["version"] != 2 {
        return Ok(());
    }
    let asset_id:String=db.query_row("SELECT id FROM shared_assets WHERE attempt_id=?1 AND generation=?2 AND kind='archive' AND purged_at_ms IS NULL ORDER BY created_at_ms DESC,id DESC LIMIT 1",params![report.attempt_id,report.generation],|r|r.get(0)).optional()?.ok_or(WorkError::Conflict)?;
    let asset = get(db, &asset_id)?;
    let bytes = super::blob_store::read(db, &asset.sha256, asset.byte_length)?;
    let archive: Value = serde_json::from_slice(&bytes)?;
    let saved = archive["tables"]["shared_run_checkpoints"]
        .as_array()
        .filter(|rows| rows.len() == 1)
        .ok_or(WorkError::Conflict)?;
    let saved: Value = serde_json::from_str(
        saved[0]["checkpoint_json"]
            .as_str()
            .ok_or(WorkError::Conflict)?,
    )?;
    if &saved != checkpoint {
        return Err(WorkError::Conflict);
    }
    Ok(())
}

impl SharedWork {
    pub(crate) fn fence_restored_backup(&mut self, now: u64) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("UPDATE shared_environments SET enabled=0", [])?;
        tx.execute("UPDATE shared_attempts SET state='stop_requested',uncertainty_reason='Restored backup: verify all previous effects and process drain before settlement',updated_at_ms=?1 WHERE state IN ('assigned','running','stop_requested')",[now])?;
        // Even a queued snapshot may have executed after the backup was taken.
        // Never turn old persisted work back into admission after restoring it.
        tx.execute("UPDATE shared_jobs SET state='cancelling',revision=revision+1,updated_at_ms=?1 WHERE state NOT IN ('succeeded','failed','cancelled')",[now])?;
        let roots = {
            let mut statement =
                tx.prepare("SELECT DISTINCT root_id FROM shared_jobs WHERE state='cancelling'")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for root in roots {
            super::store::settle_cancel(&tx, &root, now)?;
        }
        tx.execute("UPDATE shared_handovers SET state='cancelled',completed_at_ms=?1 WHERE state='pending'", [now])?;
        tx.execute("UPDATE shared_approvals SET invalidated_at_ms=?1", [now])?;
        tx.execute("DELETE FROM identity_sessions", [])?;
        tx.execute("DELETE FROM identity_refresh_sessions", [])?;
        tx.commit()?;
        Ok(())
    }
    pub fn preview_retention(&self, actor: &Principal, before: u64) -> Result<Value> {
        identity::require_admin(&self.db, actor)?;
        retention_preview(&self.db, before)
    }
    pub fn upload_input(
        &mut self,
        actor: &Principal,
        project: &str,
        upload: &AssetUpload,
        now: u64,
    ) -> Result<Asset> {
        let bytes = bytes(upload, MAX_FILE)?;
        let tx = self.db.transaction()?;
        identity::require_project_role(&tx, &actor.user_id, project, ProjectRole::Contributor)?;
        let asset = insert(
            &tx,
            &format!("user:{}", actor.user_id),
            project,
            None,
            None,
            "input",
            None,
            upload,
            &bytes,
            now,
        )?;
        tx.commit()?;
        Ok(asset)
    }
    pub fn job_assets(&self, actor: &Principal, job_id: &str) -> Result<Vec<Asset>> {
        self.job(actor, job_id)?;
        let mut stmt=self.db.prepare(&format!("SELECT {COLUMNS} FROM shared_assets WHERE kind!='archive' AND (job_id=?1 OR id IN (SELECT asset_id FROM shared_job_inputs WHERE job_id=?1)) ORDER BY created_at_ms,id LIMIT 1000"))?;
        Ok(stmt
            .query_map([job_id], row)?
            .collect::<std::result::Result<_, _>>()?)
    }
    /// The caller first selects a device- and grant-authorized job. Return
    /// only small artifact metadata for its chat card, never file content.
    pub(super) fn job_artifact_preview(&self, job_id: &str) -> Result<(Vec<Asset>, bool)> {
        let mut stmt = self.db.prepare(&format!("SELECT {COLUMNS} FROM shared_assets WHERE job_id=?1 AND kind='artifact' AND purged_at_ms IS NULL ORDER BY created_at_ms,id LIMIT 21"))?;
        let mut artifacts = stmt
            .query_map([job_id], row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let more = artifacts.len() > 20;
        artifacts.truncate(20);
        Ok((artifacts, more))
    }
    pub fn asset(&self, actor: &Principal, id: &str) -> Result<AssetDownload> {
        let asset = get(&self.db, id)?;
        identity::require_project_role(
            &self.db,
            &actor.user_id,
            &asset.project_id,
            ProjectRole::Reader,
        )?;
        // Internal restore archives include execution metadata; normal clients use the typed transcript.
        if asset.kind == "archive" {
            return Err(WorkError::Forbidden);
        }
        download(&self.db, asset)
    }
    pub fn runner_upload_asset(
        &mut self,
        runner: &str,
        attempt: &str,
        input: &RunnerAssetUpload,
        now: u64,
    ) -> Result<Asset> {
        if !matches!(input.kind.as_str(), "artifact" | "archive" | "input") {
            return Err(WorkError::Invalid);
        }
        if input.kind == "input" && input.base_sha256.is_some() {
            return Err(WorkError::Invalid);
        }
        let bytes = bytes(
            &input.upload,
            if input.kind == "archive" {
                MAX_ARCHIVE
            } else {
                MAX_FILE
            },
        )?;
        let status = self.attempt(runner, attempt)?;
        if status.assignment.generation != input.generation {
            return Err(WorkError::Conflict);
        }
        let job = &status.assignment.job;
        let received: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_assets WHERE created_by=?1 AND request_id=?2)",
            params![format!("runner:{runner}"), input.upload.request_id],
            |r| r.get(0),
        )?;
        if input.kind == "input" && !received {
            if status.state != "running" {
                return Err(WorkError::Conflict);
            }
            runner_may_read(&self.db, &status)?;
        }
        if !received && !matches!(status.state.as_str(), "running" | "stop_requested") {
            return Err(WorkError::Conflict);
        }
        if input.kind == "archive" {
            let archive: Value = serde_json::from_slice(&bytes)?;
            if archive["version"] != 1
                || archive["job_id"] != job.id
                || archive["project_id"] != job.project_id
                || archive["environment_id"] != job.environment_id
            {
                return Err(WorkError::Invalid);
            }
        }
        let tx = self.db.transaction()?;
        let asset = insert(
            &tx,
            &format!("runner:{runner}"),
            &job.project_id,
            Some(&job.id),
            Some((attempt, input.generation)),
            &input.kind,
            input.base_sha256.as_deref(),
            &input.upload,
            &bytes,
            now,
        )?;
        tx.commit()?;
        Ok(asset)
    }
    pub fn runner_asset(&self, runner: &str, attempt: &str, id: &str) -> Result<AssetDownload> {
        let status = self.attempt(runner, attempt)?;
        if !matches!(
            status.state.as_str(),
            "assigned" | "running" | "stop_requested"
        ) {
            return Err(WorkError::Conflict);
        }
        let asset = get(&self.db, id)?;
        let job = &status.assignment.job;
        runner_may_read(&self.db, &status)?;
        let referenced: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_job_inputs WHERE job_id=?1 AND asset_id=?2)",
            params![job.id, id],
            |r| r.get(0),
        )?;
        if asset.project_id != job.project_id
            || !(referenced
                || (asset.kind == "archive" && asset.job_id.as_deref() == Some(&job.id)))
        {
            return Err(WorkError::Forbidden);
        }
        download(&self.db, asset)
    }
    fn child_artifact_scope(
        &self,
        runner: &str,
        attempt: &str,
        generation: u64,
        child_id: &str,
    ) -> Result<Job> {
        let status = self.attempt(runner, attempt)?;
        if status.assignment.generation != generation
            || !matches!(status.state.as_str(), "assigned" | "running")
        {
            return Err(WorkError::Conflict);
        }
        runner_may_read(&self.db, &status)?;
        let parent = &status.assignment.job;
        let child = store::load(&self.db, child_id)?;
        if child.parent_id.as_deref() != Some(parent.id.as_str())
            || child.root_id != parent.root_id
            || child.project_id != parent.project_id
            || child.conversation_id != parent.conversation_id
        {
            return Err(WorkError::Forbidden);
        }
        if !child.state.terminal() {
            return Err(WorkError::Conflict);
        }
        Ok(child)
    }
    pub fn runner_child_artifacts(
        &self,
        runner: &str,
        attempt: &str,
        generation: u64,
        child_id: &str,
    ) -> Result<Vec<Asset>> {
        let child = self.child_artifact_scope(runner, attempt, generation, child_id)?;
        let mut statement = self.db.prepare(&format!(
            "SELECT {COLUMNS} FROM shared_assets WHERE project_id=?1 AND job_id=?2 AND kind='artifact' AND purged_at_ms IS NULL ORDER BY created_at_ms,id LIMIT 1001"
        ))?;
        let artifacts = statement
            .query_map(params![child.project_id, child.id], row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if artifacts.len() > 1000 {
            return Err(WorkError::Capacity);
        }
        Ok(artifacts)
    }
    pub fn runner_child_artifact(
        &self,
        runner: &str,
        attempt: &str,
        generation: u64,
        child_id: &str,
        asset_id: &str,
    ) -> Result<AssetDownload> {
        let child = self.child_artifact_scope(runner, attempt, generation, child_id)?;
        let asset = get(&self.db, asset_id)?;
        if asset.project_id != child.project_id
            || asset.job_id.as_deref() != Some(child.id.as_str())
            || asset.kind != "artifact"
        {
            return Err(WorkError::Forbidden);
        }
        download(&self.db, asset)
    }
    pub fn runner_archive(&self, runner: &str, attempt: &str) -> Result<Option<AssetDownload>> {
        let status = self.attempt(runner, attempt)?;
        if !matches!(
            status.state.as_str(),
            "assigned" | "running" | "stop_requested"
        ) {
            return Err(WorkError::Conflict);
        }
        runner_may_read(&self.db, &status)?;
        let job = &status.assignment.job;
        let previous: Option<String> = self
            .db
            .query_row(
                "SELECT previous_job_id FROM shared_continuations WHERE job_id=?1",
                [&job.id],
                |r| r.get(0),
            )
            .optional()?;
        let source = if job.checkpoint.is_none() {
            previous.as_deref().unwrap_or(&job.id)
        } else {
            &job.id
        };
        let id: Option<String>=self.db.query_row("SELECT id FROM shared_assets WHERE job_id=?1 AND kind='archive' AND purged_at_ms IS NULL ORDER BY created_at_ms DESC,id DESC LIMIT 1",[source],|r|r.get(0)).optional()?;
        id.map(|id| download(&self.db, get(&self.db, &id)?))
            .transpose()
    }
    pub fn transcript(
        &self,
        actor: &Principal,
        job: &str,
        after: u64,
        limit: u32,
    ) -> Result<TranscriptPage> {
        let selected = self.job(actor, job)?;
        if !(1..=100).contains(&limit) {
            return Err(WorkError::Invalid);
        }
        let activity = transcript_activity(&self.db, &selected)?;
        let asset_id: Option<String>=self.db.query_row("SELECT id FROM shared_assets WHERE job_id=?1 AND kind='archive' AND purged_at_ms IS NULL ORDER BY created_at_ms DESC,id DESC LIMIT 1",[job],|r|r.get(0)).optional()?;
        let Some(asset_id) = asset_id else {
            return Ok(TranscriptPage {
                items: vec![],
                next_after: None,
                activity,
            });
        };
        let asset = get(&self.db, &asset_id)?;
        let content = super::blob_store::read(&self.db, &asset.sha256, asset.byte_length)?;
        let archive: Value = serde_json::from_slice(&content)?;
        let source = archive["transcript"]
            .as_array()
            .ok_or(WorkError::Conflict)?;
        let mut items = Vec::new();
        let mut page_bytes = 0usize;
        for (index, item) in source.iter().enumerate() {
            let position = index as u64 + 1;
            if position <= after {
                continue;
            }
            if items.len() >= limit as usize {
                break;
            }
            let size = serde_json::to_vec(item)?.len();
            if !items.is_empty() && page_bytes.saturating_add(size) > 3 * 1024 * 1024 {
                break;
            }
            page_bytes = page_bytes.saturating_add(size);
            items.push(TranscriptItem {
                position,
                kind: item["kind"].as_str().unwrap_or("history").into(),
                payload: item["payload"].clone(),
            });
        }
        let next_after = items
            .last()
            .map(|item| item.position)
            .filter(|position| *position < source.len() as u64);
        Ok(TranscriptPage {
            items,
            next_after,
            activity,
        })
    }
}

fn runner_may_read(db: &Connection, status: &AttemptStatus) -> Result<()> {
    let job = &status.assignment.job;
    let root = store::load(db, &job.root_id)?;
    if status.assignment.stop_requested
        || root.state.terminal()
        || root.state == JobState::Cancelling
    {
        return Err(WorkError::Forbidden);
    }
    let current_runner: String = db.query_row(
        "SELECT runner_id FROM shared_environments WHERE id=?1",
        [&job.environment_id],
        |row| row.get(0),
    )?;
    if current_runner != status.assignment.runner_id {
        return Err(WorkError::Forbidden);
    }
    super::handover::may_continue(db, job, &job.environment_id)
}

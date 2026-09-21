//! Durable shared-work ownership. Device transport and OS execution have separate owners.
pub mod api;
mod approvals;
pub(crate) mod blob_store;
mod data;
mod deadlines;
mod handover;
mod inbox;
mod local_admission;
mod provisioning;
pub use local_admission::LocalAdmission;
pub use provisioning::{ProvisionReceipt, ProvisionRequest, RunnerTemplate};
mod management;
pub(crate) mod project_setup;
pub use project_setup::ProjectMember;
mod projection;
mod runner_contact;
pub use data::{Asset, AssetDownload, AssetUpload, RunnerAssetUpload, TranscriptPage};
pub use deadlines::DEFAULT_START_WINDOW_MS;
pub use handover::{ContinueJob, HandoverRequest, HandoverView};
pub use management::{ManagementCommand, ManagementRequest};
pub use runner_contact::{RUNNER_CONTACT_STALE_MS, RunnerContact, RunnerContactState};

pub use approvals::{ApprovalAnswer, ApprovalConsumeResult, ApprovalDecision, ApprovalView};
mod store;

pub use projection::{
    EnvironmentStatus, ExecutionDeviceStatus, JobSummary, ProjectStatus, ResourceOccupant,
    WorkPerson,
};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub type SharedStore = Arc<Mutex<SharedWork>>;
pub type Result<T> = std::result::Result<T, WorkError>;

#[derive(Debug, thiserror::Error)]
pub enum WorkError {
    #[error("要求の内容を確認してください。")]
    Invalid,
    #[error("{0}")]
    Management(String),
    #[error("この仕事を操作する権限がありません。")]
    Forbidden,
    #[error("対象の仕事または環境がありません。")]
    NotFound,
    #[error("仕事の状態または同じ要求の内容が変わっています。")]
    Conflict,
    #[error("仕事の受付または子孫生成の上限に達しています。")]
    Capacity,
    #[error("新しい実行の開始期限を過ぎました。必要な場合は新しい仕事として依頼してください。")]
    StartDeadline,
    #[error("共有仕事の保存状態を確認できません。")]
    Storage(#[from] rusqlite::Error),
    #[error("共有仕事の記録を読み取れません。")]
    Encoding(#[from] serde_json::Error),
    #[error("共有ファイルの保存状態を確認できません。")]
    FileStorage(#[from] std::io::Error),
    #[error(transparent)]
    Identity(#[from] crate::identity::IdentityError),
}

pub struct SharedWork {
    pub(crate) db: Connection,
    runner_contacts: runner_contact::Observations,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Assigned,
    Running,
    WaitingChild,
    Succeeded,
    Failed,
    Cancelling,
    Cancelled,
}
impl JobState {
    pub fn terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub root_id: String,
    pub parent_id: Option<String>,
    pub project_id: String,
    pub environment_id: String,
    pub requestor_id: String,
    pub assignee_id: String,
    pub quota_owner_id: String,
    pub authority_generation: u64,
    pub continued_from: Option<String>,
    pub title: String,
    pub input: Value,
    pub checkpoint: Option<Value>,
    pub result: Option<Value>,
    pub state: JobState,
    pub awaiting_child_id: Option<String>,
    pub revision: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub start_before_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SubmitJob {
    pub request_id: String,
    pub project_id: String,
    pub environment_id: String,
    pub title: String,
    pub input: Value,
    pub descendant_budget: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_before_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub id: String,
    pub label: String,
    pub resource_id: String,
    pub runner_id: String,
    pub capacity: u32,
    pub project_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Assignment {
    pub attempt_id: String,
    pub generation: u64,
    pub authority_generation: u64,
    pub runner_id: String,
    pub stop_requested: bool,
    pub job: Job,
    /// Results to consume while resuming the saved parent checkpoint.
    pub child_result: Option<Box<Job>>,
    /// Current project/environment allowlist; Runner-local policy can only narrow it.
    pub allowed_child_environments: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttemptStatus {
    pub assignment: Assignment,
    pub state: String,
    pub uncertainty_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub event_id: String,
    pub attempt_id: String,
    pub generation: u64,
    pub outcome: ReportOutcome,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReportOutcome {
    Started,
    Uncertain {
        reason: String,
    },
    ApprovalRequested {
        approval_id: String,
        request: Value,
        expires_at_ms: u64,
    },
    YieldToChild {
        checkpoint: Value,
        child_environment_id: String,
        child_title: String,
        child_input: Value,
        resources_released: bool,
    },
    Finished {
        success: bool,
        result: Value,
        resources_released: bool,
    },
}

#[cfg(test)]
mod tests;

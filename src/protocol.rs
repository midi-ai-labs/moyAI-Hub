use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequestPurpose {
    #[default]
    UserTurn,
    Preparation,
    Delegated,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Unknown,
    Offline,
    ModelMissing,
    /// Provider-reported loaded instances, not a guarantee of generation readiness.
    Ready,
    /// Legacy wire name for provider-reported unloaded state. Routing does not wait on it.
    JitPending,
    Maintenance,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientActivity {
    Idle,
    Waiting,
    Running,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientConnection {
    Connected,
    Stale,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaitPolicy {
    /// Keep the preferred model (or first selected model) until available.
    WaitForPreferred,
    /// An explicitly selected alternative may be used at a turn boundary.
    AnyAllowed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Selection {
    pub allowed_model_ids: Vec<String>,
    pub preferred_model_id: Option<String>,
    pub required_capabilities: BTreeSet<String>,
    pub wait_policy: WaitPolicy,
    pub affinity_turns: u32,
    pub reviewed_revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    Busy,
    GlobalCapacity,
    TargetUnavailable,
    Maintenance,
    /// Retained for wire compatibility with older Hubs; current routing never emits this.
    JitPending,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum HubError {
    #[error("invalid catalog: {message}")]
    InvalidCatalog { message: String },
    #[error("catalog revision exhausted")]
    RevisionExhausted,
    #[error("catalog review required")]
    ReviewRequired { current_revision: u64 },
    #[error("unknown client")]
    UnknownClient,
    #[error("client heartbeat is stale")]
    StaleClient,
    #[error("model selection is required")]
    SelectionRequired,
    #[error("invalid selection: {message}")]
    InvalidSelection { message: String },
    #[error("selected model no longer exists")]
    ModelRemoved { model_id: String },
    #[error("provider no longer has the registered model")]
    DeploymentModelMissing { deployment_id: String },
    #[error("no deployment is configured for the selected models")]
    DeploymentRequired,
    #[error("no selected model meets required capabilities")]
    CapabilityMismatch,
    #[error("unknown deployment")]
    UnknownDeployment,
    #[error("another user turn is active")]
    ActiveTurn,
    #[error("user turn was already completed or canceled")]
    TurnClosed,
    #[error("route lease expired or is unknown")]
    LeaseExpired,
    #[error("execution permit expired or was released")]
    PermitExpired,
    #[error("target does not belong to this client")]
    WrongOwner,
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("runtime registry limit reached")]
    RegistryFull,
}

/// Metadata only. This is not a provider authorization token or generation target.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteLease {
    pub id: String,
    pub client_id: String,
    pub context_id: String,
    pub logical_model_id: String,
    pub deployment_id: String,
    pub catalog_revision: u64,
    pub expires_at_ms: u64,
    pub turns_remaining: u32,
    pub advisory_only: bool,
}

/// Capacity bookkeeping for cooperating clients; providers do not validate this token yet.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionPermit {
    pub id: String,
    pub client_id: String,
    pub context_id: String,
    pub lease_id: String,
    pub request_id: String,
    pub turn_id: String,
    pub catalog_revision: u64,
    pub logical_model_id: String,
    pub deployment_id: String,
    pub provider_profile: String,
    pub expires_at_ms: u64,
    pub advisory_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RouteDecision {
    Assigned {
        lease: RouteLease,
    },
    Waiting {
        reason: WaitReason,
        retry_after_ms: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PermitDecision {
    Granted {
        permit: ExecutionPermit,
    },
    Waiting {
        reason: WaitReason,
        retry_after_ms: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientRow {
    pub id: String,
    pub label: String,
    /// Transport-observed address for display only; never authentication or identity.
    pub observed_ip: String,
    pub last_seen_ms: u64,
    pub connection: ClientConnection,
    pub activity: ClientActivity,
    pub contexts: Vec<RouteContextRow>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteContextRow {
    pub context_id: String,
    pub reviewed_revision: u64,
    pub review_required: bool,
    pub active_model_id: Option<String>,
}

//! Shared-work routes are installed only inside the device TLS guard.
use super::*;
use crate::identity::{self, IdentityError, Principal, ProjectRole, SessionAudience};
use crate::network::tls::DeviceIdentity;
use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use rusqlite::params;
use serde_json::json;
mod data;
mod local_admission;
pub(crate) mod management;
mod provisioning;

#[derive(Clone)]
struct ApiState {
    store: SharedStore,
    network: crate::network::SharedNetwork,
    slots: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    login_slots: Arc<tokio::sync::Semaphore>,
    web_access: Option<Arc<crate::web_admin::WebAccess>>,
}
type ApiResult<T> = std::result::Result<Json<T>, (StatusCode, Json<Value>)>;

/// This router must stay behind the real device-certificate middleware.
#[cfg(test)]
pub(crate) fn router(store: SharedStore, network: crate::network::SharedNetwork) -> Router {
    router_with_web_access(store, network, None)
}
pub(crate) fn router_with_web_access(
    store: SharedStore,
    network: crate::network::SharedNetwork,
    web_access: Option<Arc<crate::web_admin::WebAccess>>,
) -> Router {
    Router::new()
        .route("/v1/shared/device-session", post(device_session))
        .route("/v1/shared/admin/web-access", post(web_access_ticket))
        .route("/v1/shared/login", post(retired_authentication))
        .route("/v1/shared/setup-password", post(retired_authentication))
        .route("/v1/shared/refresh", post(retired_authentication))
        .route("/v1/shared/session", get(session))
        .route("/v1/shared/logout", post(logout))
        .route("/v1/shared/projects", get(projects))
        .route("/v1/shared/projects/{id}/leave", post(leave_project))
        .route(
            "/v1/shared/projects/{id}/conversations",
            get(project_conversations),
        )
        .route("/v1/shared/device-projects", get(device_projects))
        .route("/v1/shared/environments", get(environments))
        .route("/v1/shared/jobs", get(list).post(submit))
        .route(
            "/v1/shared/origins/{origin_session_ref}/work",
            get(origin_work),
        )
        .route(
            "/v1/shared/origins/{origin_session_ref}/turns/{origin_turn_ref}/admit",
            post(origin_turn_admit),
        )
        .route(
            "/v1/shared/origins/{origin_session_ref}/turns/{origin_turn_ref}/stop",
            post(origin_turn_stop),
        )
        .route(
            "/v1/shared/origins/{origin_session_ref}/stop",
            post(origin_stop),
        )
        .route("/v1/shared/status", get(status))
        .route("/v1/shared/jobs/{id}", get(detail))
        .route("/v1/shared/jobs/{id}/cancel", post(cancel))
        .route(
            "/v1/shared/conversations/{id}/history",
            get(conversation_history),
        )
        .route(
            "/v1/shared/conversations/{id}/rename",
            post(rename_conversation),
        )
        .route(
            "/v1/shared/conversations/{id}/delete",
            post(delete_conversation),
        )
        .route(
            "/v1/shared/conversations/{id}/stop",
            post(stop_project_conversation),
        )
        .route("/v1/shared/jobs/{id}/approval", get(approval))
        .route(
            "/v1/shared/jobs/{job_id}/approvals/{approval_id}/decision",
            post(decide_approval),
        )
        .route("/v1/shared/runner/claim", post(claim))
        .route("/v1/shared/runner/assignments", get(assignments))
        .route("/v1/shared/runner/attempts/{id}", get(attempt))
        .route("/v1/shared/runner/report", post(report))
        .route("/v1/shared/runner/services", get(runner_services))
        .route(
            "/v1/shared/runner/conversation-services",
            get(conversation_services_for_attempt),
        )
        .route(
            "/v1/shared/runner/services/{id}/stop",
            post(local_service_stop),
        )
        .route(
            "/v1/shared/runner/services/{id}/conversation-stop",
            post(conversation_service_stop),
        )
        .route("/v1/shared/services", get(project_services))
        .route("/v1/shared/services/{id}/stop", post(service_stop))
        .route(
            "/v1/shared/conversations/{id}/services/stop",
            post(stop_conversation_services),
        )
        .route(
            "/v1/shared/runner/approvals/{id}/consume",
            post(consume_approval),
        )
        .route("/v1/shared/admin/users", get(users).post(create_user))
        .route("/v1/shared/admin/projects", post(create_project))
        .route("/v1/shared/admin/memberships", post(membership))
        .route("/v1/shared/admin/environments", post(register_environment))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .merge(data::routes())
        .merge(management::routes())
        .merge(local_admission::routes())
        .merge(provisioning::routes())
        .with_state(ApiState {
            store,
            network: network.clone(),
            slots: Arc::new(tokio::sync::Semaphore::new(8)),
            #[cfg(test)]
            login_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            web_access,
        })
        .layer(axum::middleware::from_fn_with_state(
            network,
            crate::network::tls::guard,
        ))
}

async fn web_access_ticket(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<Value>,
) -> ApiResult<Value> {
    if !input.as_object().is_some_and(|v| v.is_empty()) {
        return Err(error(WorkError::Invalid));
    }
    let access = state
        .web_access
        .clone()
        .ok_or_else(|| error(WorkError::NotFound))?;
    let proof_device = device.clone();
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::NewUsage,
        move |store, actor| {
            let db = store.identity_connection();
            identity::require_admin(db, &actor)?;
            let proof = identity::web_access::device_proof(
                db,
                &actor.user_id,
                &proof_device.device_id,
                &proof_device.certificate_sha256,
            )?;
            let url = access
                .remote_ticket(actor.user_id, proof)
                .map_err(|_| WorkError::Invalid)?;
            Ok(json!({"url":url}))
        },
    )
    .await
}

async fn retired_authentication() -> (StatusCode, Json<Value>) {
    (
        StatusCode::GONE,
        Json(
            json!({"code":"password_login_removed","error":"Hubで許可されたPCから、そのまま利用できます。Desktopを更新してください。"}),
        ),
    )
}
enum DeviceAccess {
    Current,
    NewUsage,
    Settlement,
    Environment(String),
    ForwardedActor(String),
    ProjectDevices(Vec<String>),
    Controller {
        target: ControllerTarget,
        new_usage: bool,
    },
}
enum ControllerTarget {
    Project(String),
    Job(String),
    Asset(String),
    Service(String),
}
impl DeviceAccess {
    fn verify(
        &self,
        network: &crate::network::DeviceNetwork,
        device: &DeviceIdentity,
    ) -> Result<()> {
        let now = crate::service::now_ms();
        let current = if matches!(self, Self::Settlement) {
            network.authenticate_settlement(&device.certificate_sha256, now)
        } else {
            network.authenticate(&device.certificate_sha256, now)
        }
        .map_err(|_| WorkError::Forbidden)?;
        if current != device.device_id {
            return Err(WorkError::Forbidden);
        }
        if let Self::ProjectDevices(devices) = self {
            for id in devices {
                network
                    .self_projection(id, now)
                    .map_err(|_| WorkError::Forbidden)?;
            }
        }
        if let Self::ForwardedActor(actor_device) = self {
            network
                .admission_revision(&device.device_id, now)
                .map_err(|_| WorkError::Forbidden)?;
            network
                .admission_revision(actor_device, now)
                .map_err(|_| WorkError::Forbidden)?;
        }
        let target = match self {
            Self::NewUsage => Some(device.device_id.as_str()),
            Self::Controller {
                new_usage: true, ..
            } => Some(device.device_id.as_str()),
            Self::Environment(runner) => Some(runner.as_str()),
            _ => None,
        };
        if let Some(target) = target {
            network
                .admission_revision(target, now)
                .map_err(|_| WorkError::Forbidden)?;
        }
        Ok(())
    }
}
pub(crate) fn error(e: WorkError) -> (StatusCode, Json<Value>) {
    let status = match &e {
        WorkError::Forbidden
        | WorkError::Identity(IdentityError::Forbidden | IdentityError::DeviceUnbound) => {
            StatusCode::FORBIDDEN
        }
        WorkError::Identity(IdentityError::Unauthorized) => StatusCode::UNAUTHORIZED,
        WorkError::Invalid
        | WorkError::Management(_)
        | WorkError::Identity(IdentityError::Invalid(_)) => StatusCode::BAD_REQUEST,
        WorkError::Conflict
        | WorkError::StartDeadline
        | WorkError::Identity(IdentityError::Conflict) => StatusCode::CONFLICT,
        WorkError::Storage(rusqlite::Error::SqliteFailure(code, _))
            if code.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            StatusCode::CONFLICT
        }
        WorkError::NotFound => StatusCode::NOT_FOUND,
        WorkError::Capacity | WorkError::Identity(IdentityError::Limit) => {
            StatusCode::TOO_MANY_REQUESTS
        }
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    let code = if matches!(&e, WorkError::Identity(IdentityError::DeviceUnbound)) {
        "device_identity_required"
    } else {
        match status {
            StatusCode::FORBIDDEN => "forbidden",
            StatusCode::UNAUTHORIZED => "unauthorized",
            StatusCode::BAD_REQUEST => "invalid",
            StatusCode::CONFLICT => "conflict",
            StatusCode::NOT_FOUND => "not_found",
            StatusCode::TOO_MANY_REQUESTS => "capacity",
            _ => "unavailable",
        }
    };
    (status, Json(json!({"error":e.to_string(),"code":code})))
}
fn token(headers: &HeaderMap) -> Result<String> {
    if headers.get_all("authorization").iter().count() != 1 {
        return Err(IdentityError::Unauthorized.into());
    }
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned)
        .ok_or_else(|| IdentityError::Unauthorized.into())
}
async fn operation<T: Serialize + Send + 'static>(
    state: ApiState,
    device: DeviceIdentity,
    access: DeviceAccess,
    execute: impl FnOnce(&mut SharedWork) -> Result<T> + Send + 'static,
) -> ApiResult<T> {
    operation_with_network(state, device, access, move |store, _| execute(store)).await
}
async fn operation_with_network<T: Serialize + Send + 'static>(
    state: ApiState,
    device: DeviceIdentity,
    access: DeviceAccess,
    execute: impl FnOnce(&mut SharedWork, &crate::network::DeviceNetwork) -> Result<T> + Send + 'static,
) -> ApiResult<T> {
    let permit = state
        .slots
        .try_acquire_owned()
        .map_err(|_| error(WorkError::Capacity))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut store = state
            .store
            .lock()
            .map_err(|_| WorkError::Identity(IdentityError::Unavailable))?;
        // Lock order is shared store -> device network. Recheck after the store
        // wait, and retain the network owner through the durable mutation so a
        // completed device stop/revocation cannot be followed by stale admission.
        // No async wait occurs while either owner is held.
        let network = state
            .network
            .lock()
            .map_err(|_| IdentityError::Unavailable)?;
        access.verify(&network, &device)?;
        if let DeviceAccess::Controller { target, .. } = &access {
            use rusqlite::OptionalExtension;
            let project = match target {
                ControllerTarget::Project(id) => id.clone(),
                ControllerTarget::Job(id) => store
                    .db
                    .query_row(
                        "SELECT project_id FROM shared_jobs WHERE id=?1",
                        [id],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?
                    .ok_or(WorkError::Forbidden)?,
                ControllerTarget::Asset(id) => store
                    .db
                    .query_row(
                        "SELECT project_id FROM shared_assets WHERE id=?1",
                        [id],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?
                    .ok_or(WorkError::Forbidden)?,
                ControllerTarget::Service(id) => store
                    .db
                    .query_row("SELECT j.project_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id WHERE s.service_id=?1",[id],|r|r.get::<_,String>(0))
                    .optional()?
                    .ok_or(WorkError::Forbidden)?,
            };
            store.require_project_controller(&project, &device.device_id)?;
        }
        store.expire_start_deadlines(crate::service::now_ms())?;
        let result = execute(&mut store, &network);
        drop(network);
        result
    })
    .await
    .map_err(|_| error(IdentityError::Unavailable.into()))?
    .map(Json)
    .map_err(error)
}
async fn authenticated<T: Serialize + Send + 'static>(
    state: ApiState,
    headers: HeaderMap,
    device: DeviceIdentity,
    access: DeviceAccess,
    execute: impl FnOnce(&mut SharedWork, Principal) -> Result<T> + Send + 'static,
) -> ApiResult<T> {
    authenticated_with_network(state, headers, device, access, move |store, actor, _| {
        execute(store, actor)
    })
    .await
}
async fn authenticated_with_network<T: Serialize + Send + 'static>(
    state: ApiState,
    headers: HeaderMap,
    device: DeviceIdentity,
    access: DeviceAccess,
    execute: impl FnOnce(&mut SharedWork, Principal, &crate::network::DeviceNetwork) -> Result<T>
    + Send
    + 'static,
) -> ApiResult<T> {
    let token = token(&headers).map_err(error)?;
    operation_with_network(state, device.clone(), access, move |store, network| {
        let actor = identity::authenticate(
            store.identity_connection(),
            &token,
            SessionAudience::SharedWork,
            Some(&device.device_id),
            crate::service::now_ms(),
        )?;
        execute(store, actor, network)
    })
    .await
}
#[cfg(test)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    username: String,
    password: String,
}
async fn device_session(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
) -> ApiResult<identity::LoginSession> {
    operation(state, device.clone(), DeviceAccess::Current, move |store| {
        Ok(identity::device::issue_session(
            store.identity_connection(),
            &device.device_id,
            crate::service::now_ms(),
        )?)
    })
    .await
}
#[cfg(test)]
async fn login(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<Login>,
) -> ApiResult<identity::RememberedSession> {
    let permit = state
        .login_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| error(WorkError::Capacity))?;
    operation(state, device.clone(), DeviceAccess::Current, move |store| {
        let _permit = permit;
        Ok(identity::login_remembered(
            store.identity_connection(),
            &input.username,
            &input.password,
            &device.device_id,
            crate::service::now_ms(),
        )?)
    })
    .await
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Logout {
    #[serde(default)]
    refresh_token: Option<String>,
}
async fn logout(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    body: Option<Json<Logout>>,
) -> ApiResult<Value> {
    let refresh = body.and_then(|Json(body)| body.refresh_token);
    let token = if headers.contains_key("authorization") {
        Some(token(&headers).map_err(error)?)
    } else if refresh.is_some() {
        None
    } else {
        return Err(error(IdentityError::Unauthorized.into()));
    };
    operation(state, device.clone(), DeviceAccess::Current, move |store| {
        identity::logout_remembered(
            store.identity_connection(),
            token.as_deref(),
            refresh.as_deref(),
            &device.device_id,
        )?;
        Ok(json!({"logged_out":true}))
    })
    .await
}
async fn session(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
) -> ApiResult<Value> {
    let token = token(&headers).map_err(error)?;
    operation(state, device.clone(), DeviceAccess::Current, move |store| {
        let session = identity::session_status(
            &store.db,
            &token,
            SessionAudience::SharedWork,
            Some(&device.device_id),
            crate::service::now_ms(),
        )?;
        let access = super::project_setup::project_access(&store.db, &session.principal.user_id, &device.device_id)?;
        Ok(json!({"principal":session.principal,"expires_at_ms":session.expires_at_ms,"project_access":access}))
    })
    .await
}
async fn projects(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
) -> ApiResult<Vec<Value>> {
    let caller = device.device_id.clone();
    authenticated(state,headers,device,DeviceAccess::Current,move|store,actor| {
        let mut stmt=store.db.prepare("SELECT p.id,p.label,m.role,COALESCE(d.participation_generation,1),COALESCE(d.can_execute,0),d.environment_id FROM shared_projects p JOIN identity_effective_project_memberships m ON m.project_id=p.id LEFT JOIN shared_project_devices d ON d.project_id=p.id AND d.device_id=?2 WHERE m.user_id=?1 AND p.archived=0 AND p.deleted_at_ms IS NULL AND (p.legacy_devices_unrestricted=1 OR d.can_control=1) ORDER BY p.label,p.id LIMIT 1000")?;
        Ok(stmt.query_map(params![actor.user_id,caller],|r|{
            let role:String=r.get(2)?;
            Ok(json!({"id":r.get::<_,String>(0)?,"label":r.get::<_,String>(1)?,"role":role,"can_submit":role=="contributor" || role=="manager","participation_generation":r.get::<_,u64>(3)?,"can_execute":r.get::<_,bool>(4)?,"environment_id":r.get::<_,Option<String>>(5)?}))
        })?.collect::<std::result::Result<Vec<_>,_>>()?)
    }).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaveProject {
    expected_participation_generation: u64,
}
async fn leave_project(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path(project): Path<String>,
    Json(input): Json<LeaveProject>,
) -> ApiResult<Value> {
    operation(state, device.clone(), DeviceAccess::Current, move |store| {
        store.leave_project(
            &project,
            &device.device_id,
            input.expected_participation_generation,
            crate::service::now_ms(),
        )
    })
    .await
}

async fn project_conversations(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(project): Path<String>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(project.clone()),
            new_usage: false,
        },
        move |store, actor| store.list_project_conversations(&actor, &project),
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameConversation {
    project_id: String,
    title: String,
    expected_revision: u64,
}
async fn rename_conversation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(conversation): Path<String>,
    Json(input): Json<RenameConversation>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(input.project_id.clone()),
            new_usage: false,
        },
        move |store, actor| {
            store.rename_project_conversation(
                &actor,
                &input.project_id,
                &conversation,
                &input.title,
                input.expected_revision,
                crate::service::now_ms(),
            )
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteConversation {
    project_id: String,
    request_id: String,
    expected_revision: u64,
}
async fn delete_conversation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(conversation): Path<String>,
    Json(input): Json<DeleteConversation>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(input.project_id.clone()),
            new_usage: false,
        },
        move |store, actor| {
            store::id(&input.request_id)?;
            store.delete_project_conversation(
                &actor,
                &input.project_id,
                &conversation,
                input.expected_revision,
                crate::service::now_ms(),
            )
        },
    )
    .await
}
async fn device_projects(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
) -> ApiResult<Value> {
    operation(state, device.clone(), DeviceAccess::Current, move |store| {
        store.device_projects(&device.device_id)
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectQuery {
    project_id: String,
}
async fn environments(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Query(input): Query<ProjectQuery>,
) -> ApiResult<Vec<Value>> {
    authenticated_with_network(state,headers,device,DeviceAccess::Controller{target:ControllerTarget::Project(input.project_id.clone()),new_usage:false},move|store,actor,network| {
        identity::require_project_role(&store.db,&actor.user_id,&input.project_id,ProjectRole::Reader)?;
        let mut stmt=store.db.prepare("SELECT e.id,e.label,e.runner_id,e.enabled,r.capacity,e.resource_id FROM shared_environments e JOIN shared_resources r ON r.id=e.resource_id JOIN shared_environment_projects ep ON ep.environment_id=e.id WHERE ep.project_id=?1 ORDER BY e.label,e.id LIMIT 1000")?;
        let now=crate::service::now_ms();
        Ok(stmt.query_map([&input.project_id],|r| {
            let id=r.get::<_,String>(0)?;
            let runner=r.get::<_,String>(2)?;
            let capabilities=super::management::declared_capabilities(&store.db,&id)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            let occupied=super::store::resource_occupancy(&store.db,&r.get::<_,String>(5)?)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            let can_submit=match super::store::may_use(&store.db,&actor.user_id,&input.project_id,&id) {
                Ok(())=>true,
                Err(WorkError::Forbidden | WorkError::Identity(IdentityError::Forbidden))=>false,
                Err(error)=>return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error))),
            };
            Ok(json!({"runner_contact":store.environment_contact(&runner,&id,now),"id":id,"label":r.get::<_,String>(1)?,"runner_id":runner,"device_id":runner,"device_label":network.device_label(&runner),"capabilities":capabilities,"enabled":r.get::<_,bool>(3)?,"can_submit":can_submit,"capacity":r.get::<_,u32>(4)?,"occupied":occupied}))
        })?.collect::<std::result::Result<Vec<_>,_>>()?)
    }).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    project_id: String,
    before: Option<String>,
    limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusQuery {
    project_id: String,
    before: Option<String>,
    limit: Option<u32>,
    environment_before: Option<String>,
}
async fn status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Query(input): Query<StatusQuery>,
) -> ApiResult<ProjectStatus> {
    let caller = device.device_id.clone();
    authenticated_with_network(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(input.project_id.clone()),
            new_usage: false,
        },
        move |store, actor, network| {
            let mut status = store.status_for_device(
                &actor,
                &input.project_id,
                input.before.as_deref(),
                input.limit.unwrap_or(50),
                input.environment_before.as_deref(),
                Some(&caller),
            )?;
            // The project/controller checks above select exactly which IDs may
            // be resolved. Names remain owned by device registration, so edits
            // are visible without rewriting project or environment records.
            for device in &mut status.execution_devices {
                device.device_label = network.device_label(&device.device_id).map(str::to_owned);
            }
            for environment in &mut status.environments {
                environment.device_label = network
                    .device_label(&environment.runner_id)
                    .map(str::to_owned);
            }
            for job in &mut status.jobs {
                let runner: String = store.db.query_row(
                    "SELECT runner_id FROM shared_environments WHERE id=?1",
                    [&job.environment_id],
                    |r| r.get(0),
                )?;
                job.device_label = network.device_label(&runner).map(str::to_owned);
            }
            Ok(status)
        },
    )
    .await
}
async fn list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Query(input): Query<List>,
) -> ApiResult<Vec<Job>> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(input.project_id.clone()),
            new_usage: false,
        },
        move |store, actor| {
            store.list(
                &actor,
                &input.project_id,
                input.before.as_deref(),
                input.limit.unwrap_or(50),
            )
        },
    )
    .await
}
async fn detail(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Job(id.clone()),
            new_usage: false,
        },
        move |store, actor| store.job_detail(&actor, &id),
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginWorkQuery {
    job_before: Option<String>,
    service_before: Option<String>,
    include_jobs: Option<bool>,
    include_services: Option<bool>,
}
async fn origin_work(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(origin_session_ref): Path<String>,
    Query(input): Query<OriginWorkQuery>,
) -> ApiResult<OriginWorkView> {
    let caller = device.device_id.clone();
    authenticated_with_network(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor, network| {
            let mut view = store.origin_work_page_for_device(
                &actor,
                &caller,
                &origin_session_ref,
                input.job_before.as_deref(),
                input.service_before.as_deref(),
                input.include_jobs.unwrap_or(true),
                input.include_services.unwrap_or(true),
                crate::service::now_ms(),
            )?;
            for entry in &mut view.jobs {
                let runner: String = store.db.query_row(
                    "SELECT runner_id FROM shared_environments WHERE id=?1",
                    [&entry.job.environment_id],
                    |row| row.get(0),
                )?;
                entry.job.device_label = network.device_label(&runner).map(str::to_owned);
            }
            Ok(view)
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginTurnAdmit {
    request_id: String,
    #[serde(default)]
    create: bool,
    origin_turn_revision: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginTurnStop {
    request_id: String,
    origin_turn_revision: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginStop {
    request_id: String,
    through_revision: u64,
}
async fn origin_turn_admit(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path((origin, turn)): Path<(String, String)>,
    Json(input): Json<OriginTurnAdmit>,
) -> ApiResult<Value> {
    let caller = device.device_id.clone();
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| {
            store.admit_origin_turn(
                &actor,
                &caller,
                &origin,
                &turn,
                &input.request_id,
                input.create,
                input.origin_turn_revision,
                crate::service::now_ms(),
            )
        },
    )
    .await
}
async fn origin_turn_stop(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path((origin, turn)): Path<(String, String)>,
    Json(input): Json<OriginTurnStop>,
) -> ApiResult<Value> {
    let caller = device.device_id.clone();
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| {
            store.stop_origin_turn(
                &actor,
                &caller,
                &origin,
                &turn,
                input.origin_turn_revision,
                &input.request_id,
                crate::service::now_ms(),
            )
        },
    )
    .await
}
async fn origin_stop(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(origin): Path<String>,
    Json(input): Json<OriginStop>,
) -> ApiResult<Value> {
    let caller = device.device_id.clone();
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| {
            store.stop_origin(
                &actor,
                &caller,
                &origin,
                input.through_revision,
                &input.request_id,
                crate::service::now_ms(),
            )
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationHistoryQuery {
    project_id: String,
    snapshot: Option<u64>,
    before: Option<u64>,
    limit: Option<u32>,
}
async fn conversation_history(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(conversation): Path<String>,
    Query(input): Query<ConversationHistoryQuery>,
) -> ApiResult<ConversationHistoryPage> {
    authenticated_with_network(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(input.project_id.clone()),
            new_usage: false,
        },
        move |store, actor, network| {
            let mut page = store.conversation_history(
                &actor,
                &input.project_id,
                &conversation,
                input.snapshot,
                input.before,
                input.limit.unwrap_or(20),
                crate::service::now_ms(),
            )?;
            for turn in &mut page.jobs {
                let runner: String = store.db.query_row(
                    "SELECT runner_id FROM shared_environments WHERE id=?1",
                    [&turn.job.environment_id],
                    |r| r.get(0),
                )?;
                turn.job.device_label = network.device_label(&runner).map(str::to_owned);
            }
            Ok(page)
        },
    )
    .await
}
async fn submit(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<SubmitJob>,
) -> ApiResult<Job> {
    let controller_device = device.device_id.clone();
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(input.project_id.clone()),
            new_usage: true,
        },
        move |store, actor| {
            store.submit_for_device(
                &actor,
                &input,
                crate::service::now_ms(),
                Some(&controller_device),
            )
        },
    )
    .await
}
async fn cancel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
) -> ApiResult<Job> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Job(id.clone()),
            new_usage: false,
        },
        move |store, actor| store.cancel(&actor, &id, crate::service::now_ms()),
    )
    .await
}

async fn approval(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
) -> ApiResult<Option<ApprovalView>> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Job(id.clone()),
            new_usage: false,
        },
        move |store, actor| store.approval(&actor, &id, crate::service::now_ms()),
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideApproval {
    decision: ApprovalDecision,
}
async fn decide_approval(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path((job_id, approval_id)): Path<(String, String)>,
    Json(input): Json<DecideApproval>,
) -> ApiResult<ApprovalView> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Job(job_id.clone()),
            new_usage: true,
        },
        move |store, actor| {
            store.decide_approval(
                &actor,
                &job_id,
                &approval_id,
                input.decision,
                crate::service::now_ms(),
            )
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsumeApproval {
    attempt_id: String,
    generation: u64,
}
async fn consume_approval(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
    Json(input): Json<ConsumeApproval>,
) -> ApiResult<Option<ApprovalConsumeResult>> {
    operation(
        state,
        device.clone(),
        DeviceAccess::NewUsage,
        move |store| {
            store.consume_approval(
                &device.device_id,
                &id,
                &input.attempt_id,
                input.generation,
                crate::service::now_ms(),
            )
        },
    )
    .await
}
// Workers possess a transport-verified device identity, not a caller-supplied person.
// They receive only attempts assigned to that exact device by the durable scheduler.
fn runner_capability(headers: &HeaderMap) -> Result<Option<&'static str>> {
    let values = headers.get_all("x-moyai-runner-capabilities");
    if values.iter().count() > 1 {
        return Err(WorkError::Invalid);
    }
    let Some(value) = values.iter().next() else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| WorkError::Invalid)?;
    if value.len() > 1024 {
        return Err(WorkError::Invalid);
    }
    Ok(value
        .split(',')
        .any(|name| name.trim() == store::MULTI_DEVICE_SESSION_CAPABILITY)
        .then_some(store::MULTI_DEVICE_SESSION_CAPABILITY))
}
fn label_candidates(assignment: &mut Assignment, network: &crate::network::DeviceNetwork) {
    for candidate in &mut assignment.allowed_child_candidates {
        if let Some(label) = network.device_label(&candidate.device_id) {
            candidate.device_label = label.chars().take(128).collect();
        }
    }
}
async fn claim(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    input: Option<Json<ClaimRequest>>,
) -> ApiResult<Option<Assignment>> {
    let capability = runner_capability(&headers).map_err(error)?;
    operation_with_network(
        state,
        device.clone(),
        DeviceAccess::NewUsage,
        move |store, network| {
            let mut assignment = store.claim_observed_for_capability(
                &device.device_id,
                input.as_ref().map(|i| i.environment_ids.as_slice()),
                capability,
                crate::service::now_ms(),
            )?;
            if let Some(assignment) = &mut assignment {
                label_candidates(assignment, network)
            }
            Ok(assignment)
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimRequest {
    environment_ids: Vec<String>,
}
async fn attempt(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
) -> ApiResult<AttemptStatus> {
    let capability = runner_capability(&headers).map_err(error)?;
    operation_with_network(
        state,
        device.clone(),
        DeviceAccess::Current,
        move |store, network| {
            let mut result = store.attempt(&device.device_id, &id)?;
            if result
                .assignment
                .required_runner_capabilities
                .iter()
                .any(|required| Some(required.as_str()) != capability)
            {
                return Err(WorkError::NotFound);
            }
            store.observe_attempt(&device.device_id, &id, crate::service::now_ms())?;
            label_candidates(&mut result.assignment, network);
            Ok(result)
        },
    )
    .await
}
async fn report(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<Report>,
) -> ApiResult<Job> {
    let access = if matches!(
        input.outcome,
        ReportOutcome::Started
            | ReportOutcome::YieldToChild { .. }
            | ReportOutcome::ApprovalRequested { .. }
            | ReportOutcome::RetainService { .. }
    ) {
        DeviceAccess::NewUsage
    } else {
        DeviceAccess::Settlement
    };
    operation(state, device.clone(), access, move |store| {
        let result = store.report(&device.device_id, &input, crate::service::now_ms())?;
        store.observe_attempt(
            &device.device_id,
            &input.attempt_id,
            crate::service::now_ms(),
        )?;
        Ok(result)
    })
    .await
}
async fn runner_services(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
) -> ApiResult<Vec<ServiceLeaseView>> {
    operation(
        state,
        device.clone(),
        DeviceAccess::Settlement,
        move |store| store.runner_services(&device.device_id, crate::service::now_ms()),
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationServicesQuery {
    attempt_id: String,
    generation: u64,
}
async fn conversation_services_for_attempt(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Query(query): Query<ConversationServicesQuery>,
) -> ApiResult<Vec<ServiceLeaseView>> {
    operation(
        state,
        device.clone(),
        DeviceAccess::Settlement,
        move |store| {
            store.conversation_services_for_attempt(
                &device.device_id,
                &query.attempt_id,
                query.generation,
                crate::service::now_ms(),
            )
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceQuery {
    project_id: String,
    conversation_id: Option<String>,
}
async fn project_services(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Query(query): Query<ServiceQuery>,
) -> ApiResult<Vec<ServiceLeaseView>> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(query.project_id.clone()),
            new_usage: false,
        },
        move |store, actor| {
            store.project_services(
                &actor,
                &query.project_id,
                query.conversation_id.as_deref(),
                crate::service::now_ms(),
            )
        },
    )
    .await
}
async fn local_service_stop(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
) -> ApiResult<ServiceLeaseView> {
    operation(
        state,
        device.clone(),
        DeviceAccess::Settlement,
        move |store| {
            store.request_local_service_stop(&device.device_id, &id, crate::service::now_ms())
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationServiceStop {
    attempt_id: String,
    generation: u64,
}
async fn conversation_service_stop(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
    Json(input): Json<ConversationServiceStop>,
) -> ApiResult<ServiceLeaseView> {
    operation(
        state,
        device.clone(),
        DeviceAccess::Settlement,
        move |store| {
            store.request_conversation_service_stop(
                &device.device_id,
                &input.attempt_id,
                input.generation,
                &id,
                crate::service::now_ms(),
            )
        },
    )
    .await
}
async fn service_stop(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
) -> ApiResult<ServiceLeaseView> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Service(id.clone()),
            new_usage: false,
        },
        move |store, actor| store.request_service_stop(&actor, &id, crate::service::now_ms()),
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopConversationServices {
    project_id: String,
    request_id: String,
}
async fn stop_conversation_services(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(conversation_id): Path<String>,
    Json(input): Json<StopConversationServices>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(input.project_id.clone()),
            new_usage: false,
        },
        move |store, actor| {
            store.request_conversation_services_stop(
                &actor,
                &input.project_id,
                &conversation_id,
                &input.request_id,
                crate::service::now_ms(),
            )
        },
    )
    .await
}
async fn stop_project_conversation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(conversation): Path<String>,
    Json(input): Json<StopConversationServices>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(input.project_id.clone()),
            new_usage: false,
        },
        move |store, actor| {
            store.stop_project_conversation(
                &actor,
                &input.project_id,
                &conversation,
                &input.request_id,
                crate::service::now_ms(),
            )
        },
    )
    .await
}
async fn users(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
) -> ApiResult<Vec<identity::UserRecord>> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        |store, actor| Ok(identity::list_users(&store.db, &actor)?),
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateUser {
    username: String,
    display_name: String,
    password: String,
    administrator: bool,
}
async fn create_user(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<CreateUser>,
) -> ApiResult<Principal> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| {
            Ok(identity::create_user(
                &store.db,
                &actor,
                &input.username,
                &input.display_name,
                &input.password,
                input.administrator,
                crate::service::now_ms(),
            )?)
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateProject {
    id: String,
    label: String,
}
async fn create_project(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<CreateProject>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| {
            store.create_project(&actor, &input.id, &input.label)?;
            Ok(json!({"id":input.id}))
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Membership {
    project_id: String,
    user_id: String,
    role: Option<ProjectRole>,
}
async fn membership(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<Membership>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| {
            identity::set_project_role(
                &store.db,
                &actor,
                &input.project_id,
                &input.user_id,
                input.role,
                crate::service::now_ms(),
            )?;
            Ok(json!({"updated":true}))
        },
    )
    .await
}
async fn register_environment(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<Environment>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Environment(input.runner_id.clone()),
        move |store, actor| {
            store.register_environment(&actor, &input)?;
            Ok(json!({"id":input.id}))
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssignmentScope {
    environment_ids: Option<String>,
}
impl AssignmentScope {
    fn parse(self) -> Result<Option<Vec<String>>> {
        self.environment_ids
            .map(|value| {
                if value.len() > 128 * 129 {
                    return Err(WorkError::Invalid);
                }
                if value.is_empty() {
                    return Ok(vec![]);
                }
                let ids: Vec<String> = value.split(',').map(str::to_owned).collect();
                if ids.len() > 128 {
                    return Err(WorkError::Invalid);
                }
                Ok(ids)
            })
            .transpose()
    }
}
async fn assignments(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Query(scope): Query<AssignmentScope>,
) -> ApiResult<Vec<Assignment>> {
    let scope = scope.parse().map_err(error)?;
    let capability = runner_capability(&headers).map_err(error)?;
    operation_with_network(
        state,
        device.clone(),
        DeviceAccess::Current,
        move |store, network| {
            let mut assignments = store.assignments_observed_for_capability(
                &device.device_id,
                scope.as_deref(),
                capability,
                crate::service::now_ms(),
            )?;
            for assignment in &mut assignments {
                label_candidates(assignment, network)
            }
            Ok(assignments)
        },
    )
    .await
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;

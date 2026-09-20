use super::*;
pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/v1/shared/admin/snapshot", get(snapshot))
        .route("/v1/shared/admin/command", post(command))
        .route("/v1/shared/admin/maintenance", get(maintenance))
        .route(
            "/v1/shared/jobs/{id}/handover",
            get(handover_view).post(handover),
        )
        .route("/v1/shared/jobs/{id}/continue", post(continue_job))
        .route("/v1/shared/inbox", get(inbox))
        .route("/v1/shared/inbox/{id}/read", post(read_inbox))
        .route("/v1/shared/runner/attempts/{id}/authorize", post(authorize))
        .layer(DefaultBodyLimit::max(256 * 1024))
}
async fn snapshot(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| store.management_snapshot(&actor),
    )
    .await
}
pub(crate) fn environment_target(command: &ManagementCommand) -> Option<String> {
    match &command.request {
        ManagementRequest::CreateEnvironment { runner_id, .. }
        | ManagementRequest::UpdateEnvironment { runner_id, .. } => Some(runner_id.clone()),
        _ => None,
    }
}
pub(crate) fn project_device_targets(command: &ManagementCommand) -> Option<Vec<String>> {
    match &command.request {
        ManagementRequest::SaveProject {
            controller_device_ids,
            runner_device_ids,
            ..
        } => Some(
            controller_device_ids
                .iter()
                .chain(runner_device_ids)
                .cloned()
                .collect(),
        ),
        ManagementRequest::RetryProjectDevice { device_id, .. }
        | ManagementRequest::AssignProjectDevice { device_id, .. } => Some(vec![device_id.clone()]),
        _ => None,
    }
}
async fn command(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<ManagementCommand>,
) -> ApiResult<Value> {
    let access = project_device_targets(&input)
        .map(DeviceAccess::ProjectDevices)
        .unwrap_or_else(|| {
            environment_target(&input)
                .map(DeviceAccess::Environment)
                .unwrap_or(DeviceAccess::Current)
        });
    authenticated(state, headers, device, access, move |store, actor| {
        store.manage(&actor, &input, crate::service::now_ms())
    })
    .await
}
async fn handover_view(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
) -> ApiResult<HandoverView> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Job(id.clone()),
            new_usage: false,
        },
        move |store, actor| store.handover_view(&actor, &id),
    )
    .await
}
async fn handover(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
    Json(input): Json<HandoverRequest>,
) -> ApiResult<Job> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Job(id.clone()),
            new_usage: false,
        },
        move |store, actor| store.handover(&actor, &id, &input, crate::service::now_ms()),
    )
    .await
}
async fn continue_job(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
    Json(input): Json<ContinueJob>,
) -> ApiResult<Job> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Job(id.clone()),
            new_usage: true,
        },
        move |store, actor| store.continue_job(&actor, &id, &input, crate::service::now_ms()),
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    before: Option<String>,
    limit: Option<u32>,
}
async fn maintenance(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Query(page): Query<Page>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| {
            store.management_maintenance(&actor, page.before.as_deref(), page.limit.unwrap_or(128))
        },
    )
    .await
}
async fn inbox(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Query(page): Query<Page>,
) -> ApiResult<Value> {
    let caller = device.device_id.clone();
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| {
            store.inbox_for_device(
                &actor,
                page.before.as_deref(),
                page.limit.unwrap_or(50),
                crate::service::now_ms(),
                Some(&caller),
            )
        },
    )
    .await
}
async fn read_inbox(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    let caller = device.device_id.clone();
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| {
            store.read_inbox_for_device(&actor, &id, crate::service::now_ms(), Some(&caller))
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Authorize {
    generation: u64,
    authority_generation: u64,
    approval_id: Option<String>,
}
async fn authorize(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
    Json(input): Json<Authorize>,
) -> ApiResult<Value> {
    operation(
        state,
        device.clone(),
        DeviceAccess::NewUsage,
        move |store| {
            store.authorize_effect(
                &device.device_id,
                &id,
                input.generation,
                input.authority_generation,
                input.approval_id.as_deref(),
                crate::service::now_ms(),
            )?;
            store.observe_attempt(&device.device_id, &id, crate::service::now_ms())?;
            Ok(json!({"authorized":true}))
        },
    )
    .await
}

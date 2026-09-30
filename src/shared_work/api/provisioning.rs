use super::super::provisioning::{ProvisionReceipt, ProvisionRequest, RunnerTemplate};
use super::*;

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/v1/shared/runner/templates", post(publish))
        .route("/v1/shared/runner/environments", get(environments))
        .route("/v1/shared/runner/provisioning", get(pending).post(receipt))
        .route("/v1/shared/admin/provisioning", get(catalog).post(request))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Templates {
    templates: Vec<RunnerTemplate>,
}
async fn publish(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<Templates>,
) -> ApiResult<Value> {
    operation(state, device.clone(), DeviceAccess::Current, move |store| {
        store.publish_templates(
            &device.device_id,
            &input.templates,
            crate::service::now_ms(),
        )?;
        Ok(json!({"saved":true}))
    })
    .await
}
async fn pending(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
) -> ApiResult<Vec<ProvisionRequest>> {
    operation(
        state,
        device.clone(),
        DeviceAccess::NewUsage,
        move |store| store.pending_provision(&device.device_id),
    )
    .await
}
#[derive(Default, Deserialize)]
struct EnvironmentQuery {
    #[serde(default)]
    current_only: bool,
}

async fn environments(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Query(query): Query<EnvironmentQuery>,
) -> ApiResult<Vec<super::super::provisioning::RunnerEnvironment>> {
    operation(state, device.clone(), DeviceAccess::Current, move |store| {
        if query.current_only {
            store.current_runner_environments(&device.device_id)
        } else {
            store.runner_environments(&device.device_id)
        }
    })
    .await
}
async fn receipt(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<ProvisionReceipt>,
) -> ApiResult<Value> {
    operation(
        state,
        device.clone(),
        DeviceAccess::NewUsage,
        move |store| {
            store.provision_receipt(&device.device_id, &input, crate::service::now_ms())?;
            Ok(json!({"saved":true}))
        },
    )
    .await
}
async fn catalog(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| store.provision_catalog(&actor),
    )
    .await
}
async fn request(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<ProvisionRequest>,
) -> ApiResult<Value> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| {
            store.request_provision(&actor, &input, crate::service::now_ms())?;
            Ok(json!({"queued":true}))
        },
    )
    .await
}

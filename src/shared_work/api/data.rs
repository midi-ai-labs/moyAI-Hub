use super::*;
use crate::shared_work::data::{
    Asset, AssetDownload, AssetUpload, RunnerAssetUpload, TranscriptPage,
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/v1/shared/projects/{id}/assets", post(upload_input))
        .route("/v1/shared/jobs/{id}/assets", get(job_assets))
        .route("/v1/shared/jobs/{id}/transcript", get(transcript))
        .route("/v1/shared/assets/{id}", get(asset))
        .route(
            "/v1/shared/runner/attempts/{id}/assets",
            post(runner_upload),
        )
        .route(
            "/v1/shared/runner/attempts/{attempt}/assets/{id}",
            get(runner_asset),
        )
        .route(
            "/v1/shared/runner/attempts/{attempt}/children/{child}/artifacts",
            get(runner_child_artifacts),
        )
        .route(
            "/v1/shared/runner/attempts/{attempt}/children/{child}/artifacts/{id}",
            get(runner_child_artifact),
        )
        .route(
            "/v1/shared/runner/attempts/{id}/archive",
            get(runner_archive),
        )
        // One bounded data operation at a time, separately from control messages.
        // Reads can return backpressure too; callers retry the same read without restarting work.
        .layer(DefaultBodyLimit::max(90 * 1024 * 1024))
        .layer(axum::middleware::from_fn(data_slot))
}
static UPLOAD: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
#[cfg(test)]
pub(super) async fn hold_data_slot() -> tokio::sync::SemaphorePermit<'static> {
    UPLOAD.acquire().await.unwrap()
}
async fn data_slot(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let Ok(_permit) = UPLOAD.try_acquire() else {
        return error(WorkError::Capacity).into_response();
    };
    next.run(request).await
}
async fn upload_input(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(upload): Json<AssetUpload>,
) -> ApiResult<Asset> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Project(id.clone()),
            new_usage: true,
        },
        move |store, actor| store.upload_input(&actor, &id, &upload, crate::service::now_ms()),
    )
    .await
}
async fn job_assets(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Vec<Asset>> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Job(id.clone()),
            new_usage: false,
        },
        move |store, actor| store.job_assets(&actor, &id),
    )
    .await
}
async fn asset(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<AssetDownload> {
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Asset(id.clone()),
            new_usage: false,
        },
        move |store, actor| store.asset(&actor, &id),
    )
    .await
}
#[derive(Deserialize)]
pub(super) struct Page {
    #[serde(default)]
    after: u64,
    #[serde(default = "page_size")]
    limit: u32,
}
fn page_size() -> u32 {
    100
}
async fn transcript(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<TranscriptPage> {
    authenticated_with_network(
        state,
        headers,
        device,
        DeviceAccess::Controller {
            target: ControllerTarget::Job(id.clone()),
            new_usage: false,
        },
        move |store, actor, network| {
            let mut result = store.transcript(&actor, &id, page.after, page.limit)?;
            if let Some(activity) = &result.activity {
                let controller: Option<String> = store.db.query_row(
                    "SELECT COALESCE(controller_device_id,origin_device_id) FROM shared_jobs WHERE id=?1",
                    [&activity.job_id], |row| row.get(0),
                )?;
                let now = crate::service::now_ms();
                if network.admission_revision(&activity.runner_id, now).is_err()
                    || controller.as_ref().is_some_and(|controller| network.admission_revision(controller, now).is_err()) {
                    result.activity = None;
                }
            }
            Ok(result)
        },
    )
    .await
}
async fn runner_upload(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
    Json(upload): Json<RunnerAssetUpload>,
) -> ApiResult<Asset> {
    let access = if upload.kind == "input" {
        DeviceAccess::NewUsage
    } else {
        DeviceAccess::Settlement
    };
    operation(state, device.clone(), access, move |store| {
        let result =
            store.runner_upload_asset(&device.device_id, &id, &upload, crate::service::now_ms())?;
        store.observe_attempt(&device.device_id, &id, crate::service::now_ms())?;
        Ok(result)
    })
    .await
}
async fn runner_asset(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path((attempt, id)): Path<(String, String)>,
) -> ApiResult<AssetDownload> {
    operation(
        state,
        device.clone(),
        DeviceAccess::NewUsage,
        move |store| {
            let result = store.runner_asset(&device.device_id, &attempt, &id)?;
            store.observe_attempt(&device.device_id, &attempt, crate::service::now_ms())?;
            Ok(result)
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildArtifactsQuery {
    generation: u64,
}
async fn runner_child_artifacts(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path((attempt, child)): Path<(String, String)>,
    Query(input): Query<ChildArtifactsQuery>,
) -> ApiResult<Value> {
    operation(
        state,
        device.clone(),
        DeviceAccess::NewUsage,
        move |store| {
            let items = store.runner_child_artifacts(
                &device.device_id,
                &attempt,
                input.generation,
                &child,
            )?;
            store.observe_attempt(&device.device_id, &attempt, crate::service::now_ms())?;
            Ok(json!({"items":items}))
        },
    )
    .await
}
async fn runner_child_artifact(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path((attempt, child, id)): Path<(String, String, String)>,
    Query(input): Query<ChildArtifactsQuery>,
) -> ApiResult<AssetDownload> {
    operation(
        state,
        device.clone(),
        DeviceAccess::NewUsage,
        move |store| {
            let result = store.runner_child_artifact(
                &device.device_id,
                &attempt,
                input.generation,
                &child,
                &id,
            )?;
            store.observe_attempt(&device.device_id, &attempt, crate::service::now_ms())?;
            Ok(result)
        },
    )
    .await
}
async fn runner_archive(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
) -> ApiResult<Option<AssetDownload>> {
    operation(
        state,
        device.clone(),
        DeviceAccess::NewUsage,
        move |store| {
            let result = store.runner_archive(&device.device_id, &id)?;
            store.observe_attempt(&device.device_id, &id, crate::service::now_ms())?;
            Ok(result)
        },
    )
    .await
}

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
        DeviceAccess::NewUsage,
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
        DeviceAccess::Current,
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
        DeviceAccess::Current,
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
    authenticated(
        state,
        headers,
        device,
        DeviceAccess::Current,
        move |store, actor| store.transcript(&actor, &id, page.after, page.limit),
    )
    .await
}
async fn runner_upload(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Path(id): Path<String>,
    Json(upload): Json<RunnerAssetUpload>,
) -> ApiResult<Asset> {
    operation(
        state,
        device.clone(),
        DeviceAccess::Settlement,
        move |store| {
            let result = store.runner_upload_asset(
                &device.device_id,
                &id,
                &upload,
                crate::service::now_ms(),
            )?;
            store.observe_attempt(&device.device_id, &id, crate::service::now_ms())?;
            Ok(result)
        },
    )
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

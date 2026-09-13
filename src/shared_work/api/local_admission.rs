use super::*;

pub(super) fn routes() -> Router<ApiState> {
    Router::new().route("/v1/shared/runner/local-admissions", post(admit))
}
async fn admit(
    State(state): State<ApiState>,
    Extension(device): Extension<DeviceIdentity>,
    Json(input): Json<super::super::local_admission::LocalAdmission>,
) -> ApiResult<Assignment> {
    operation(
        state,
        device.clone(),
        DeviceAccess::ForwardedActor(input.actor_device_id.clone()),
        move |store| store.admit_local(&device.device_id, &input, crate::service::now_ms()),
    )
    .await
}

use super::*;
use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, State},
    http::StatusCode,
    routing::{get, post},
};
use serde_json::{Value, json};
use tls::{DeviceIdentity, TlsPeer};
type ApiResult = std::result::Result<Json<Value>, (StatusCode, Json<Value>)>;
pub(crate) fn error(error: NetworkError) -> (StatusCode, Json<Value>) {
    let status = match error {
        NetworkError::InvalidRequest | NetworkError::InvalidCsr | NetworkError::InvalidEndpoint => {
            StatusCode::BAD_REQUEST
        }
        NetworkError::Unauthorized => StatusCode::UNAUTHORIZED,
        NetworkError::StaleRevision
        | NetworkError::DeviceNameConflict
        | NetworkError::DeviceWorkUnsettled
        | NetworkError::RecoveryRequired
        | NetworkError::AuthorityRetired
        | NetworkError::JoinSuperseded => StatusCode::CONFLICT,
        NetworkError::Full => StatusCode::TOO_MANY_REQUESTS,
        NetworkError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::FORBIDDEN,
    };
    (status, Json(json!({"error":error.to_string()})))
}
fn reply<T: Serialize>(result: Result<T>) -> ApiResult {
    let value = serde_json::to_value(result.map_err(error)?)
        .map_err(|_| error(NetworkError::Unavailable))?;
    if serde_json::to_vec(&value)
        .map_err(|_| error(NetworkError::Unavailable))?
        .len()
        > 512 * 1024
    {
        return Err(error(NetworkError::Full));
    }
    Ok(Json(value))
}
fn lock(
    network: &SharedNetwork,
) -> std::result::Result<std::sync::MutexGuard<'_, DeviceNetwork>, (StatusCode, Json<Value>)> {
    network.lock().map_err(|_| error(NetworkError::Unavailable))
}
pub fn router(network: SharedNetwork) -> Router {
    Router::new()
        .route("/v1/network/enroll", post(enroll))
        .route("/v1/network/join/request", post(join_request))
        .route("/v1/network/join/status", post(join_status))
        .route("/v1/network/renew", post(renew))
        .route("/v1/network/self", get(myself))
        .route("/v1/network/presence", post(presence))
        .route("/v1/network/publish", post(publish))
        .route("/v1/network/directory", get(directory))
        .route("/v1/network/directory/query", post(directory_query))
        .route("/v1/network/grants", post(grant))
        .route("/v1/network/inspect", post(inspect))
        .route("/v1/network/jobs/accepted", post(accepted))
        .route("/v1/network/jobs/settled", post(settled))
        .route("/v1/network/introspect", post(introspect))
        .route("/v1/network/cancel-lineage", post(cancel_lineage))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .route("/v1/network/history/requests", get(history_requests))
        .route(
            "/v1/network/history/results",
            post(history_results).layer(DefaultBodyLimit::max(history::MAX_HISTORY_BODY_BYTES)),
        )
        .with_state(network)
}

async fn history_requests(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
) -> ApiResult {
    let mut owner = lock(&network)?;
    let now = crate::service::now_ms();
    if !owner.current_certificate(&id.device_id, &id.certificate_sha256, now) {
        return Err(error(NetworkError::Unauthorized));
    }
    let requests = owner.history_requests(&id.device_id, now).map_err(error)?;
    reply(Ok(json!({"requests": requests})))
}

async fn history_results(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
    Json(body): Json<history::HistorySubmission>,
) -> ApiResult {
    let mut owner = lock(&network)?;
    let now = crate::service::now_ms();
    if !owner.current_certificate(&id.device_id, &id.certificate_sha256, now) {
        return Err(error(NetworkError::Unauthorized));
    }
    let request_id = body.request_id.clone();
    owner
        .submit_history(&id.device_id, body, now)
        .map_err(error)?;
    reply(Ok(json!({"accepted": true, "request_id": request_id})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinRequestBody {
    label: String,
    csr_pem: String,
}
async fn join_request(
    State(network): State<SharedNetwork>,
    ConnectInfo(peer): ConnectInfo<TlsPeer>,
    Json(body): Json<JoinRequestBody>,
) -> ApiResult {
    reply(lock(&network)?.request_join(
        body.label,
        &body.csr_pem,
        peer.address.ip(),
        crate::service::now_ms(),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinStatusBody {
    request_id: String,
    proof_csr_pem: String,
}
async fn join_status(
    State(network): State<SharedNetwork>,
    ConnectInfo(peer): ConnectInfo<TlsPeer>,
    Json(body): Json<JoinStatusBody>,
) -> ApiResult {
    reply(lock(&network)?.join_status(
        &body.request_id,
        &body.proof_csr_pem,
        peer.address.ip(),
        crate::service::now_ms(),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Enroll {
    code: String,
    csr_pem: String,
}
async fn enroll(
    State(network): State<SharedNetwork>,
    ConnectInfo(peer): ConnectInfo<TlsPeer>,
    Json(body): Json<Enroll>,
) -> ApiResult {
    reply(lock(&network)?.enroll(
        &body.code,
        &body.csr_pem,
        peer.address.ip(),
        crate::service::now_ms(),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Renew {
    csr_pem: String,
}
async fn renew(
    State(network): State<SharedNetwork>,
    ConnectInfo(peer): ConnectInfo<TlsPeer>,
    Json(body): Json<Renew>,
) -> ApiResult {
    let cert = peer
        .certificate_sha256
        .ok_or_else(|| error(NetworkError::Unauthorized))?;
    reply(lock(&network)?.renew_authenticated(
        &cert,
        &body.csr_pem,
        peer.address.ip(),
        crate::service::now_ms(),
    ))
}
async fn myself(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
) -> ApiResult {
    reply(lock(&network)?.self_projection(&id.device_id, crate::service::now_ms()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
async fn presence(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
    Json(_): Json<Empty>,
) -> ApiResult {
    let mut owner = lock(&network)?;
    owner
        .seen(&id.device_id, id.address.ip(), crate::service::now_ms())
        .map_err(error)?;
    reply(owner.self_projection(&id.device_id, crate::service::now_ms()))
}
async fn publish(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
    Json(body): Json<Publication>,
) -> ApiResult {
    let mut owner = lock(&network)?;
    owner
        .publish(
            &id.device_id,
            body,
            id.address.ip(),
            crate::service::now_ms(),
        )
        .map_err(error)?;
    reply(Ok(json!({"published":true,"revision":owner.revision()})))
}
async fn directory(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
) -> ApiResult {
    let owner = lock(&network)?;
    reply(
        owner
            .directory(&id.device_id, None, crate::service::now_ms())
            .map(
                |peers| json!({"hub_id":owner.hub_id(),"revision":owner.revision(),"peers":peers}),
            ),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryQuery {
    parent_grant_id: String,
}
async fn directory_query(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
    Json(body): Json<DirectoryQuery>,
) -> ApiResult {
    let owner = lock(&network)?;
    reply(
        owner
            .directory(
                &id.device_id,
                Some(&body.parent_grant_id),
                crate::service::now_ms(),
            )
            .map(
                |peers| json!({"hub_id":owner.hub_id(),"revision":owner.revision(),"peers":peers}),
            ),
    )
}
async fn grant(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
    Json(body): Json<GrantRequest>,
) -> ApiResult {
    reply(lock(&network)?.grant(&id.device_id, body, crate::service::now_ms()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Introspect {
    token: String,
    actor_certificate_sha256: String,
    #[serde(default)]
    action: Action,
}
async fn introspect(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
    Json(body): Json<Introspect>,
) -> ApiResult {
    reply(lock(&network)?.introspect(
        &id.device_id,
        &body.token,
        &body.actor_certificate_sha256,
        body.action,
        crate::service::now_ms(),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelLineage {
    root_task_id: String,
}
async fn cancel_lineage(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
    Json(body): Json<CancelLineage>,
) -> ApiResult {
    lock(&network)?
        .cancel_lineage(&id.device_id, &body.root_task_id, crate::service::now_ms())
        .map_err(error)?;
    reply(Ok(json!({"cancelled":true})))
}

async fn inspect(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
    Json(body): Json<InspectRequest>,
) -> ApiResult {
    reply(lock(&network)?.inspect(&id.device_id, body, crate::service::now_ms()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Accepted {
    grant_id: String,
    job_id: String,
}
async fn accepted(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
    Json(body): Json<Accepted>,
) -> ApiResult {
    reply(lock(&network)?.accept_job(
        &id.device_id,
        &body.grant_id,
        &body.job_id,
        crate::service::now_ms(),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settled {
    grant_id: String,
    job_id: String,
    state: Terminal,
}
async fn settled(
    State(network): State<SharedNetwork>,
    Extension(id): Extension<DeviceIdentity>,
    Json(body): Json<Settled>,
) -> ApiResult {
    reply(lock(&network)?.settle_job(
        &id.device_id,
        &body.grant_id,
        &body.job_id,
        body.state,
        crate::service::now_ms(),
    ))
}

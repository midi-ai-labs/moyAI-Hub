//! Authenticated control-plane API. Generation bodies are accepted only by the companion.
mod execution_api;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    core::HubCore,
    protocol::{ClientActivity, HubError, Selection, WaitPolicy},
};

pub type SharedCore = Arc<Mutex<HubCore>>;

#[derive(Clone)]
struct ApiState {
    core: SharedCore,
    token: Arc<String>,
    client_tokens: Arc<Mutex<BTreeMap<String, ClientToken>>>,
    network: Option<crate::network::SharedNetwork>,
    gateway: Option<crate::gateway_process::GatewayControl>,
    /// Serializes only the metadata reservation/admission transaction. Discovery and
    /// generation run outside this gate; no core mutex is held over network I/O.
    prepare_gate: Arc<tokio::sync::Mutex<()>>,
}
struct ClientToken {
    secret: String,
    device: Option<String>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

/// A fresh router represents a new server session; all registration tokens expire on stop.
pub fn router(core: SharedCore, token: String) -> Router {
    router_with_gateway(core, token, None)
}

pub fn router_with_gateway(
    core: SharedCore,
    token: String,
    gateway: Option<crate::gateway_process::GatewayControl>,
) -> Router {
    model_router(ApiState {
        core,
        token: Arc::new(token),
        client_tokens: Arc::new(Mutex::new(BTreeMap::new())),
        network: None,
        gateway,
        prepare_gate: Arc::new(tokio::sync::Mutex::new(())),
    })
}
fn model_router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/catalog", get(catalog))
        .route("/v1/clients/register", post(register))
        .route("/v1/clients/heartbeat", post(heartbeat))
        .route("/v1/clients/review", post(review))
        .route("/v1/clients/disconnect", post(disconnect))
        .route("/v1/requests/prepare", post(execution_api::prepare))
        .route("/v1/turns/finish", post(execution_api::finish))
        .route("/v1/turns/cancel", post(execution_api::cancel))
        .route("/_gateway/claim", post(execution_api::claim))
        .route("/_gateway/settle", post(execution_api::settle))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(state)
}
pub fn router_with_registered_gateway(
    core: SharedCore,
    token: String,
    gateway: crate::gateway_process::GatewayControl,
    network: crate::network::SharedNetwork,
) -> Router {
    model_router(ApiState {
        core,
        token: Arc::new(token),
        client_tokens: Arc::new(Mutex::new(BTreeMap::new())),
        network: Some(network),
        gateway: Some(gateway),
        prepare_gate: Arc::new(tokio::sync::Mutex::new(())),
    })
}

/// The TLS listener must use NetworkListener; the guard authenticates the real
/// handshake and strips any attempted identity headers before model admission.
pub fn network_router(
    core: SharedCore,
    network: crate::network::SharedNetwork,
    gateway: Option<crate::gateway_process::GatewayControl>,
) -> Router {
    let state = ApiState {
        core,
        token: Arc::new(format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new())),
        client_tokens: Arc::new(Mutex::new(BTreeMap::new())),
        network: Some(network.clone()),
        gateway,
        prepare_gate: Arc::new(tokio::sync::Mutex::new(())),
    };
    model_router(state.clone())
        .merge(
            Router::new()
                .route("/v1/network/model-session", post(device_session))
                .with_state(state),
        )
        .merge(crate::network::api::router(network.clone()))
        .layer(axum::middleware::from_fn_with_state(
            network,
            crate::network::tls::guard,
        ))
}

pub fn network_router_with_diagnostics(
    core: SharedCore,
    network: crate::network::SharedNetwork,
    gateway: Option<crate::gateway_process::GatewayControl>,
    runtime: Arc<crate::network::server_runtime::NetworkServerRuntime>,
) -> Router {
    let diagnostics = Router::new()
        .route("/v1/network/diagnostics", get(network_diagnostics))
        .with_state(runtime)
        .layer(axum::middleware::from_fn_with_state(
            network.clone(),
            crate::network::tls::guard,
        ));
    network_router(core, network, gateway).merge(diagnostics)
}

async fn network_diagnostics(
    State(runtime): State<Arc<crate::network::server_runtime::NetworkServerRuntime>>,
) -> Result<Json<crate::network::server_runtime::NetworkDiagnostics>, (StatusCode, Json<Value>)> {
    runtime
        .diagnostics()
        .await
        .map(Json)
        .map_err(crate::network::api::error)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceSessionRequest {}
async fn device_session(
    State(state): State<ApiState>,
    axum::Extension(identity): axum::Extension<crate::network::tls::DeviceIdentity>,
    Json(_): Json<DeviceSessionRequest>,
) -> ApiResult {
    let mut tokens = state.client_tokens.lock().map_err(|_| failed())?;
    if tokens.len() >= 128 {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"registry_full"})),
        ));
    }
    let network = state
        .network
        .as_ref()
        .ok_or_else(denied)?
        .lock()
        .map_err(|_| failed())?;
    let projection = network
        .self_projection(&identity.device_id, now_ms())
        .map_err(crate::network::api::error)?;
    let label = projection["label"].as_str().ok_or_else(failed)?;
    let mut core = state.core.lock().map_err(|_| failed())?;
    let id = ulid::Ulid::new().to_string();
    let secret = format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new());
    core.register_client(&id, label, &identity.address.ip().to_string(), now_ms())
        .map_err(|_| failed())?;
    tokens.insert(
        id.clone(),
        ClientToken {
            secret: secret.clone(),
            device: Some(identity.device_id),
        },
    );
    let models: Vec<_> = core
        .catalog_store()
        .catalog
        .models
        .iter()
        .filter(|m| m.capabilities.contains("tools"))
        .map(|m| m.id.clone())
        .collect();
    // Hub owns this initial automatic receiver policy. Desktop does not select a
    // first model independently, and must review the exact returned revision.
    let default_selection=models.first().map(|preferred|json!({"allowed_model_ids":models,"preferred_model_id":preferred,"required_capabilities":["tools"],"wait_policy":"allow_selected_fallback","affinity_turns":3}));
    Ok(Json(
        json!({"id":id,"client_token":secret,"hub_id":core.catalog_store().hub_id,"revision":core.catalog_store().revision.to_string(),"heartbeat_interval_ms":10000,"supports_turn_heartbeat":true,"supports_delegated_execution":true,"identity_scope":"device_session","default_selection":default_selection}),
    ))
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn denied() -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":"unauthorized"})),
    )
}
fn failed() -> (StatusCode, Json<Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error":"unavailable"})),
    )
}
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
fn matches_token(left: &str, right: &str) -> bool {
    if left.len() != right.len() || left.len() > 256 {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |different, (a, b)| different | (a ^ b))
        == 0
}
fn authorize(headers: &HeaderMap, token: &str) -> Result<(), (StatusCode, Json<Value>)> {
    if headers.contains_key("origin") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error":"browser_origin_forbidden"})),
        ));
    }
    if bearer(headers).is_some_and(|candidate| matches_token(candidate, token)) {
        Ok(())
    } else {
        Err(denied())
    }
}
fn authorize_session(
    state: &ApiState,
    headers: &HeaderMap,
    token: &ClientToken,
) -> Result<(), (StatusCode, Json<Value>)> {
    authorize(headers, &token.secret)?;
    if let Some(device) = &token.device {
        let certificate = headers
            .get("x-moyai-verified-cert")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(denied)?;
        let network = state
            .network
            .as_ref()
            .ok_or_else(denied)?
            .lock()
            .map_err(|_| failed())?;
        if headers
            .get("x-moyai-verified-device")
            .and_then(|h| h.to_str().ok())
            != Some(device.as_str())
            || !network.current_certificate(device, certificate, now_ms())
        {
            return Err(denied());
        }
    }
    Ok(())
}

async fn health(State(state): State<ApiState>, headers: HeaderMap) -> ApiResult {
    authorize(&headers, &state.token)?;
    let core = state.core.lock().map_err(|_| failed())?;
    let store = core.catalog_store();
    Ok(Json(
        json!({"hub_id":store.hub_id,"software_version":store.software_version,"revision":store.revision.to_string(),"mode":"management_preview"}),
    ))
}

async fn catalog(State(state): State<ApiState>, headers: HeaderMap) -> ApiResult {
    let tokens = state.client_tokens.lock().map_err(|_| failed())?;
    if let Some(token) = bearer(&headers).and_then(|candidate| {
        tokens
            .values()
            .find(|token| matches_token(candidate, &token.secret))
    }) {
        authorize_session(&state, &headers, token)?;
    } else {
        authorize(&headers, &state.token)?;
    }
    let core = state.core.lock().map_err(|_| failed())?;
    let store = core.catalog_store();
    // Deliberately project only logical models: deployment endpoints remain local admin data.
    let changes:Vec<Value> = store.changes.iter().map(|change| json!({
        "revision":change.revision.to_string(),"at_ms":change.changed_at_ms,
        "summary":format!("models +{} / -{} / changed {}; deployments {}; policy {}; software {}",change.added_model_ids.len(),change.removed_model_ids.len(),change.changed_model_ids.len(),change.deployments_changed,change.policy_changed,change.software_changed)
    })).collect();
    Ok(Json(
        json!({"hub_id":store.hub_id,"software_version":store.software_version,"revision":store.revision.to_string(),"models":store.catalog.models,"changes":changes}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterRequest {
    label: String,
}

async fn register(
    State(state): State<ApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<RegisterRequest>,
) -> ApiResult {
    authorize(&headers, &state.token)?;
    let mut tokens = state.client_tokens.lock().map_err(|_| failed())?;
    if tokens.len() >= 128 {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"registry_full"})),
        ));
    }
    let id = ulid::Ulid::new().to_string();
    let secret = format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new());
    let mut core = state.core.lock().map_err(|_| failed())?;
    core.register_client(&id, &request.label, &peer.ip().to_string(), now_ms())
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_registration"})),
            )
        })?;
    tokens.insert(
        id.clone(),
        ClientToken {
            secret: secret.clone(),
            device: None,
        },
    );
    Ok(Json(
        json!({"id":id,"client_token":secret,"hub_id":core.catalog_store().hub_id,"revision":core.catalog_store().revision.to_string(),"heartbeat_interval_ms":10000,"supports_turn_heartbeat":true,"supports_delegated_execution":true,"identity_scope":"server_session"}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeartbeatRequest {
    id: String,
    activity: ClientActivity,
    #[serde(default)]
    active_turns: Vec<HeartbeatTurn>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeartbeatTurn {
    context: ReviewContext,
    turn_id: String,
    #[serde(default)]
    delegated: bool,
}

async fn heartbeat(
    State(state): State<ApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<HeartbeatRequest>,
) -> ApiResult {
    let tokens = state.client_tokens.lock().map_err(|_| failed())?;
    let token = tokens.get(&request.id).ok_or_else(denied)?;
    authorize_session(&state, &headers, token)?;
    let mut core = state.core.lock().map_err(|_| failed())?;
    // Refresh display IP from the transport, never from a client-supplied header or body.
    let label = core
        .client_rows(now_ms())
        .into_iter()
        .find(|client| client.id == request.id)
        .ok_or_else(denied)?
        .label;
    let active_contexts: Vec<String> = request
        .active_turns
        .iter()
        .map(|turn| {
            if turn.delegated {
                HubCore::delegated_context_id(turn.context.id(), &turn.turn_id)
            } else {
                Ok(turn.context.id().into())
            }
        })
        .collect::<Result<_, _>>()
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_request"})),
            )
        })?;
    let active_turns: Vec<_> = request
        .active_turns
        .iter()
        .zip(&active_contexts)
        .map(|(turn, context)| (context.as_str(), turn.turn_id.as_str()))
        .collect();
    core.heartbeat_with_turns(&request.id, request.activity, &active_turns, now_ms())
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_request"})),
            )
        })?;
    core.register_client(&request.id, &label, &peer.ip().to_string(), now_ms())
        .map_err(|_| failed())?;
    Ok(Json(
        json!({"id":request.id,"revision":core.catalog_store().revision.to_string()}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewContext {
    Main,
    SideChat,
}

impl ReviewContext {
    fn id(&self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::SideChat => "side_chat",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewWaitPolicy {
    WaitForPreferred,
    AllowSelectedFallback,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewSelection {
    allowed_model_ids: Vec<String>,
    preferred_model_id: String,
    required_capabilities: Vec<String>,
    wait_policy: ReviewWaitPolicy,
    affinity_turns: u32,
}

impl ReviewSelection {
    fn resolve(&self, reviewed_revision: u64) -> Result<Selection, (StatusCode, Json<Value>)> {
        let required_capabilities: std::collections::BTreeSet<String> =
            self.required_capabilities.iter().cloned().collect();
        if required_capabilities.len() != self.required_capabilities.len() {
            return Err(invalid_selection());
        }
        Ok(Selection {
            allowed_model_ids: self.allowed_model_ids.clone(),
            preferred_model_id: Some(self.preferred_model_id.clone()),
            required_capabilities,
            wait_policy: match self.wait_policy {
                ReviewWaitPolicy::WaitForPreferred => WaitPolicy::WaitForPreferred,
                ReviewWaitPolicy::AllowSelectedFallback => WaitPolicy::AnyAllowed,
            },
            affinity_turns: self.affinity_turns,
            reviewed_revision,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewRequest {
    id: String,
    context: ReviewContext,
    expected_hub_id: String,
    reviewed_revision: String,
    selection: ReviewSelection,
}

fn invalid_selection() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"invalid_selection"})),
    )
}

fn selection_failure(error: HubError, revision: u64) -> (StatusCode, Json<Value>) {
    let (status, code) = match error {
        HubError::ReviewRequired { .. } => (StatusCode::CONFLICT, "review_required"),
        HubError::ModelRemoved { .. } => (StatusCode::BAD_REQUEST, "model_removed"),
        HubError::CapabilityMismatch => (StatusCode::BAD_REQUEST, "capability_mismatch"),
        HubError::ActiveTurn => (StatusCode::CONFLICT, "active_turn"),
        HubError::RegistryFull => (StatusCode::TOO_MANY_REQUESTS, "registry_full"),
        HubError::UnknownClient => return denied(),
        _ => return invalid_selection(),
    };
    (
        status,
        Json(json!({"error":code,"current_revision":revision.to_string()})),
    )
}

async fn review(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Result<Json<ReviewRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    let Json(request) = request.map_err(|_| invalid_selection())?;
    let tokens = state.client_tokens.lock().map_err(|_| failed())?;
    let token = tokens.get(&request.id).ok_or_else(denied)?;
    authorize_session(&state, &headers, token)?;
    let mut core = state.core.lock().map_err(|_| failed())?;
    let revision = core.catalog_store().revision;
    if request.expected_hub_id != core.catalog_store().hub_id {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error":"different_hub","current_revision":revision.to_string()})),
        ));
    }
    let reviewed_revision = request
        .reviewed_revision
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0 && value.to_string() == request.reviewed_revision)
        .ok_or_else(invalid_selection)?;
    let selection = request.selection.resolve(reviewed_revision)?;
    core.save_selection(&request.id, request.context.id(), selection)
        .map_err(|error| selection_failure(error, revision))?;
    Ok(Json(
        json!({"id":request.id,"context":request.context.id(),"reviewed_revision":reviewed_revision.to_string()}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DisconnectRequest {
    id: String,
}

async fn disconnect(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Result<Json<DisconnectRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    let Json(request) = request.map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_request"})),
        )
    })?;
    {
        let mut tokens = state.client_tokens.lock().map_err(|_| failed())?;
        let token = tokens.get(&request.id).ok_or_else(denied)?;
        authorize_session(&state, &headers, token)?;
        let mut core = state.core.lock().map_err(|_| failed())?;
        core.remove_client(&request.id).map_err(|_| denied())?;
        tokens.remove(&request.id);
    }
    if let Some(gateway) = &state.gateway {
        let _ = gateway
            .cancel(&crate::gateway::CancelScope {
                client_id: request.id.clone(),
                context_id: None,
                turn_id: None,
            })
            .await;
    }
    Ok(Json(json!({"id":request.id,"disconnected":true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogStore;

    #[tokio::test]
    async fn authenticated_catalog_and_presence_do_not_expose_admin_or_provider_data() {
        let core = Arc::new(Mutex::new(
            HubCore::new(CatalogStore::empty("0.1.0")).unwrap(),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router(core, "a".repeat(32)).into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let client = reqwest::Client::new();
        assert_eq!(
            client
                .get(format!("{base}/v1/catalog"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let catalog: Value = client
            .get(format!("{base}/v1/catalog"))
            .bearer_auth("a".repeat(32))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(catalog["revision"], "1");
        assert_eq!(
            client
                .get(format!("{base}/v1/catalog"))
                .bearer_auth("a".repeat(32))
                .header("origin", "http://localhost:3000")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(catalog.get("deployments").is_none());
        assert_eq!(
            client
                .post(format!("{base}/v1/discover"))
                .bearer_auth("a".repeat(32))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        let paired: Value = client
            .post(format!("{base}/v1/clients/register"))
            .bearer_auth("a".repeat(32))
            .json(&json!({"label":"test desktop"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let heartbeat = json!({"id":paired["id"],"activity":"running"});
        assert_eq!(
            client
                .post(format!("{base}/v1/clients/heartbeat"))
                .bearer_auth("a".repeat(32))
                .json(&heartbeat)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(
            client
                .post(format!("{base}/v1/clients/heartbeat"))
                .bearer_auth(paired["client_token"].as_str().unwrap())
                .json(&heartbeat)
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
        task.abort();
    }
}

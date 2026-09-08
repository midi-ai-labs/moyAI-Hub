use super::*;
use crate::{
    catalog::Deployment,
    gateway::{CancelScope, Grant},
    gateway_process::ControlError,
    protocol::{PermitDecision, RouteDecision},
};

fn invalid_request() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"invalid_request"})),
    )
}
fn core_failure(error: HubError, revision: u64) -> (StatusCode, Json<Value>) {
    let (status, code) = match error {
        HubError::UnknownClient | HubError::WrongOwner => {
            (StatusCode::UNAUTHORIZED, "unauthorized")
        }
        HubError::StaleClient => (StatusCode::CONFLICT, "stale_client"),
        HubError::ReviewRequired { .. } => (StatusCode::CONFLICT, "review_required"),
        HubError::ActiveTurn => (StatusCode::CONFLICT, "active_turn"),
        HubError::TurnClosed => (StatusCode::CONFLICT, "turn_closed"),
        HubError::LeaseExpired => (StatusCode::CONFLICT, "lease_expired"),
        HubError::PermitExpired => (StatusCode::CONFLICT, "permit_expired"),
        HubError::SelectionRequired => (StatusCode::CONFLICT, "selection_required"),
        HubError::ModelRemoved { .. }
        | HubError::DeploymentModelMissing { .. }
        | HubError::DeploymentRequired => (StatusCode::BAD_REQUEST, "model_removed"),
        HubError::CapabilityMismatch => (StatusCode::BAD_REQUEST, "capability_mismatch"),
        HubError::RegistryFull => (StatusCode::TOO_MANY_REQUESTS, "registry_full"),
        _ => (StatusCode::BAD_REQUEST, "invalid_request"),
    };
    (
        status,
        Json(json!({"error":code,"current_revision":revision.to_string()})),
    )
}
fn gateway_failure(error: ControlError) -> (StatusCode, Json<Value>) {
    (
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        Json(json!({"error":error.code})),
    )
}
fn authorize_client(
    state: &ApiState,
    headers: &HeaderMap,
    id: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    let tokens = state.client_tokens.lock().map_err(|_| failed())?;
    authorize_session(state, headers, tokens.get(id).ok_or_else(denied)?)
}
fn model_admission_revision(
    state: &ApiState,
    headers: &HeaderMap,
    id: &str,
) -> Result<Option<u64>, (StatusCode, Json<Value>)> {
    let tokens = state.client_tokens.lock().map_err(|_| failed())?;
    let token = tokens.get(id).ok_or_else(denied)?;
    authorize_session(state, headers, token)?;
    token
        .device
        .as_ref()
        .map(|id| {
            state
                .network
                .as_ref()
                .ok_or_else(denied)?
                .lock()
                .map_err(|_| failed())?
                .admission_revision(id, now_ms())
                .map_err(crate::network::api::error)
        })
        .transpose()
}
fn authorize_gateway(
    state: &ApiState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<Value>)> {
    if headers.contains_key("origin")
        || !state.gateway.as_ref().is_some_and(|gateway| {
            bearer(headers).is_some_and(|token| gateway.token_matches(token))
        })
    {
        return Err(denied());
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Prepare {
    id: String,
    context: ReviewContext,
    turn_id: String,
    request_id: String,
    expected_hub_id: String,
    reviewed_revision: String,
}
fn check_revision(core: &HubCore, request: &Prepare) -> Result<(), (StatusCode, Json<Value>)> {
    let revision = core.catalog_store().revision;
    if request.expected_hub_id != core.catalog_store().hub_id {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error":"different_hub","current_revision":revision.to_string()})),
        ));
    }
    let parsed = request
        .reviewed_revision
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0 && value.to_string() == request.reviewed_revision)
        .ok_or_else(invalid_request)?;
    if parsed != revision {
        return Err(core_failure(
            HubError::ReviewRequired {
                current_revision: revision,
            },
            revision,
        ));
    }
    Ok(())
}
async fn refresh_presence(
    state: &ApiState,
    request: &Prepare,
) -> Result<(), (StatusCode, Json<Value>)> {
    let groups = {
        let core = state.core.lock().map_err(|_| failed())?;
        check_revision(&core, request)?;
        let deployments = core
            .routing_deployments(&request.id, request.context.id(), now_ms())
            .map_err(|error| core_failure(error, core.catalog_store().revision))?;
        let mut groups: BTreeMap<(String, String), Vec<Deployment>> = BTreeMap::new();
        for row in deployments
            .into_iter()
            .filter(|row| row.enabled && core.needs_presence_refresh(&row.id, now_ms()))
        {
            groups
                .entry((row.endpoint.clone(), row.provider_profile.clone()))
                .or_default()
                .push(row);
        }
        groups
    };
    let mut tasks = tokio::task::JoinSet::new();
    // One prepare does at most four bounded metadata reads. Later prepares continue with
    // remaining stale hosts; no generation payload enters this process.
    for ((endpoint, profile), rows) in groups.into_iter().take(4) {
        tasks.spawn(async move { (rows, crate::discovery::discover(&endpoint, &profile).await) });
    }
    while let Some(result) = tasks.join_next().await {
        let (rows, discovered) = result.map_err(|_| failed())?;
        let mut core = state.core.lock().map_err(|_| failed())?;
        for row in rows {
            if !core
                .catalog_store()
                .catalog
                .deployments
                .iter()
                .any(|current| current == &row)
            {
                continue;
            }
            match &discovered {
                Ok(discovery) => {
                    if let Some(model) = discovery
                        .models
                        .iter()
                        .find(|model| model.id == row.provider_model)
                    {
                        core.observe_present_deployment(
                            &row.id,
                            model.availability.clone(),
                            now_ms(),
                        )
                        .map_err(|_| failed())?;
                    } else {
                        core.observe_deployment(
                            &row.id,
                            crate::protocol::Availability::ModelMissing,
                            now_ms(),
                        )
                        .map_err(|_| failed())?;
                    }
                }
                Err(error) => {
                    core.observe_deployment(&row.id, error.availability.clone(), now_ms())
                        .map_err(|_| failed())?;
                }
            }
        }
    }
    Ok(())
}

pub(super) async fn prepare(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Result<Json<Prepare>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    let Json(request) = request.map_err(|_| invalid_request())?;
    model_admission_revision(&state, &headers, &request.id)?;
    let device_certificate_sha256 = state
        .client_tokens
        .lock()
        .map_err(|_| failed())?
        .get(&request.id)
        .and_then(|token| token.device.as_ref())
        .and_then(|_| {
            headers
                .get("x-moyai-verified-cert")
                .and_then(|h| h.to_str().ok())
        })
        .map(str::to_owned);
    let gateway = state.gateway.as_ref().ok_or_else(|| {
        gateway_failure(ControlError {
            status: 503,
            code: "gateway_unavailable".into(),
        })
    })?;
    refresh_presence(&state, &request).await?;
    let _admission = state.prepare_gate.lock().await;
    let device_admission_revision = model_admission_revision(&state, &headers, &request.id)?;
    let grant = {
        let mut core = state.core.lock().map_err(|_| failed())?;
        check_revision(&core, &request)?;
        let revision = core.catalog_store().revision;
        let lease = match core
            .begin_turn(
                &request.id,
                request.context.id(),
                &request.turn_id,
                now_ms(),
            )
            .map_err(|error| core_failure(error, revision))?
        {
            RouteDecision::Assigned { lease } => lease,
            waiting => return Ok(Json(serde_json::to_value(waiting).map_err(|_| failed())?)),
        };
        let permit = match core
            .acquire_permit(
                &request.id,
                request.context.id(),
                &lease.id,
                &request.request_id,
                now_ms(),
            )
            .map_err(|error| core_failure(error, revision))?
        {
            PermitDecision::Granted { permit } => permit,
            waiting => return Ok(Json(serde_json::to_value(waiting).map_err(|_| failed())?)),
        };
        let catalog = &core.catalog_store().catalog;
        let deployment = catalog
            .deployments
            .iter()
            .find(|row| row.id == permit.deployment_id)
            .cloned()
            .ok_or_else(invalid_request)?;
        let pool = catalog
            .capacity_pools
            .iter()
            .find(|row| row.id == deployment.capacity_pool_id)
            .ok_or_else(invalid_request)?;
        Grant {
            permit,
            deployment,
            pool_limit: pool.max_concurrent,
            global_limit: catalog.policy.global_max_concurrent,
            device_certificate_sha256,
            device_admission_revision,
        }
    };
    let admitted = gateway.grant(&grant).await.map_err(gateway_failure)?;
    if admitted.get("state") == Some(&json!("waiting")) {
        state
            .core
            .lock()
            .map_err(|_| failed())?
            .withdraw_declined_permit(&grant.permit);
        return Ok(Json(admitted));
    }
    {
        let core = state.core.lock().map_err(|_| failed())?;
        core.validate_execution_permit(&grant.permit.id, now_ms())
            .map_err(|error| core_failure(error, core.catalog_store().revision))?;
    }
    let token = admitted
        .get("request_token")
        .and_then(Value::as_str)
        .ok_or_else(failed)?;
    let gateway_base = if grant.device_certificate_sha256.is_some() {
        gateway.network_base_url().ok_or_else(failed)?
    } else {
        gateway.base_url()
    };
    Ok(Json(
        json!({"state":"ready","lease_id":grant.permit.lease_id,"permit_id":grant.permit.id,
        "logical_model_id":grant.permit.logical_model_id,"gateway_base_url":format!("{}/r/{}/v1",gateway_base,grant.permit.id),
        "request_token":token,"expires_at_ms":grant.permit.expires_at_ms.to_string(),"provider_profile":"openai_compatible_chat","provider_api_mode":"chat_completions"}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Turn {
    id: String,
    context: ReviewContext,
    turn_id: String,
}
async fn close_turn(
    state: ApiState,
    headers: HeaderMap,
    request: Turn,
    canceled: bool,
) -> ApiResult {
    authorize_client(&state, &headers, &request.id)?;
    {
        let mut core = state.core.lock().map_err(|_| failed())?;
        let revision = core.catalog_store().revision;
        if canceled {
            core.cancel_turn(&request.id, request.context.id(), &request.turn_id)
        } else {
            core.finish_turn(&request.id, request.context.id(), &request.turn_id)
        }
        .map_err(|error| core_failure(error, revision))?;
    }
    if let Some(gateway) = &state.gateway {
        let _ = gateway
            .cancel(&CancelScope {
                client_id: request.id.clone(),
                context_id: Some(request.context.id().into()),
                turn_id: Some(request.turn_id.clone()),
            })
            .await;
    }
    Ok(Json(
        json!({"id":request.id,"context":request.context.id(),"turn_id":request.turn_id,"closed":true}),
    ))
}
pub(super) async fn finish(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Result<Json<Turn>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    close_turn(
        state,
        headers,
        request.map_err(|_| invalid_request())?.0,
        false,
    )
    .await
}
pub(super) async fn cancel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Result<Json<Turn>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    close_turn(
        state,
        headers,
        request.map_err(|_| invalid_request())?.0,
        true,
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Permit {
    permit_id: String,
    #[serde(default)]
    device_certificate_sha256: Option<String>,
    #[serde(default)]
    device_admission_revision: Option<u64>,
}
pub(super) async fn claim(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Result<Json<Permit>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    authorize_gateway(&state, &headers)?;
    let request = request.map_err(|_| invalid_request())?.0;
    if let Some(certificate) = &request.device_certificate_sha256 {
        let network = state
            .network
            .as_ref()
            .ok_or_else(denied)?
            .lock()
            .map_err(|_| failed())?;
        network
            .authorize_model_permit(
                certificate,
                request.device_admission_revision.ok_or_else(denied)?,
                now_ms(),
            )
            .map_err(|error| match error {
                crate::network::NetworkError::Stopped => crate::network::api::error(error),
                // Preserve the private gateway's existing opaque authentication failure.
                // Revocation does not disclose a different credential state here.
                _ => denied(),
            })?;
    }
    let core = state.core.lock().map_err(|_| failed())?;
    core.validate_execution_permit(&request.permit_id, now_ms())
        .map_err(|error| core_failure(error, core.catalog_store().revision))?;
    Ok(Json(json!({"allowed":true})))
}
pub(super) async fn settle(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Result<Json<Permit>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    authorize_gateway(&state, &headers)?;
    let request = request.map_err(|_| invalid_request())?.0;
    state
        .core
        .lock()
        .map_err(|_| failed())?
        .settle_execution_permit(&request.permit_id);
    Ok(Json(json!({"settled":true})))
}

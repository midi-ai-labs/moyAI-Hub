use super::*;
use crate::identity::{self, IdentityError, Principal, SessionAudience};
use axum::Extension;

const COOKIE: &str = "moyai_admin";

pub(super) fn session_id(token: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"moyai-admin-request-session\0");
    hash.update(token.as_bytes());
    format!("{:x}", hash.finalize())
}

#[derive(Clone, Copy)]
pub(super) struct BootstrapAccess(pub bool);

pub(super) fn login_allowed(state: &WebState, local: bool) -> bool {
    state.policy.secure || local
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let values: Vec<_> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|part| part.trim().split_once('='))
        .filter(|(key, _)| *key == COOKIE)
        .map(|(_, value)| value)
        .collect();
    (values.len() == 1).then(|| values[0].to_owned())
}
fn status(error: &IdentityError) -> StatusCode {
    match error {
        IdentityError::Unauthorized => StatusCode::UNAUTHORIZED,
        IdentityError::Forbidden => StatusCode::FORBIDDEN,
        IdentityError::Invalid(_) => StatusCode::BAD_REQUEST,
        IdentityError::Conflict => StatusCode::CONFLICT,
        IdentityError::Limit => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}
fn error_response(error: IdentityError) -> Response {
    let code = status(&error);
    let message = match code {
        StatusCode::UNAUTHORIZED => {
            "管理画面を開くための認証が期限切れです。Hub PC または管理権限のあるPCから開き直してください。"
        }
        StatusCode::BAD_REQUEST => "管理画面を開くための認証を確認できません。開き直してください。",
        StatusCode::CONFLICT => "管理画面の設定が変わりました。開き直してください。",
        StatusCode::TOO_MANY_REQUESTS => {
            "処理中の操作があります。少し待ってから開き直してください。"
        }
        StatusCode::FORBIDDEN => "このPCに管理権限がありません。Hub PC で設定を確認してください。",
        _ => "利用者の認証状態を確認できません。",
    };
    (code, Json(json!({"error":message}))).into_response()
}
pub(super) fn principal(
    admin: &Admin,
    headers: &HeaderMap,
) -> Result<(Principal, bool), StatusCode> {
    let token = request_token(headers)?;
    let store = admin
        .shared_work
        .lock()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let db = store.identity_connection();
    let principal = authenticate_token(admin, db, &token).map_err(|e| status(&e))?;
    let native = identity::web_access::session_device(db, &token)
        .map_err(|e| status(&e))?
        .is_none();
    Ok((principal, native))
}
fn authenticate_token(
    admin: &Admin,
    db: &rusqlite::Connection,
    token: &str,
) -> Result<Principal, IdentityError> {
    let now = crate::service::now_ms();
    let principal = identity::authenticate(db, token, SessionAudience::WebAdmin, None, now)?;
    if let Some(proof) = identity::web_access::session_device(db, token)? {
        let network = admin
            .network
            .lock()
            .map_err(|_| IdentityError::Unavailable)?;
        identity::web_access::verify_device(db, &network, &principal.user_id, &proof, now)?;
    }
    Ok(principal)
}
pub(super) fn request_token(headers: &HeaderMap) -> Result<String, StatusCode> {
    let token = cookie_token(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    if headers.get_all("x-moyai-session").iter().count() != 1
        || !headers
            .get("x-moyai-session")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|value| equal(value.as_bytes(), session_id(&token).as_bytes()))
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(token)
}
pub(super) async fn session_response(
    state: &WebState,
    headers: &HeaderMap,
    local: bool,
    new_token: Option<Option<&str>>,
) -> Response {
    let state = state.clone();
    let headers = headers.clone();
    let token = new_token.map(|value| value.map(str::to_owned));
    let Some(admin) = state.admin.upgrade() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let Ok(permit) = admin.web_access.read_slots.clone().try_acquire_owned() else {
        return failure(StatusCode::TOO_MANY_REQUESTS);
    };
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        session_response_sync(
            &state,
            &headers,
            local,
            token.as_ref().map(|value| value.as_deref()),
        )
    })
    .await
    .unwrap_or_else(|_| failure(StatusCode::SERVICE_UNAVAILABLE))
}
fn session_response_sync(
    state: &WebState,
    headers: &HeaderMap,
    local: bool,
    new_token: Option<Option<&str>>,
) -> Response {
    let Some(admin) = state.admin.upgrade() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let csrf = match admin.web_access.session(headers, false, &state.policy) {
        Ok(v) => v,
        Err(e) => return failure(e),
    };
    let Ok(store) = admin.shared_work.lock() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let configured = match identity::has_users(store.identity_connection()) {
        Ok(v) => v,
        Err(e) => return error_response(e),
    };
    let token = match new_token {
        Some(token) => token.map(str::to_owned),
        None => cookie_token(headers),
    };
    let request_session = token.as_deref().map(session_id);
    let native_session = if let Some(token) = token.as_deref().filter(|_| local) {
        match identity::web_access::session_device(store.identity_connection(), token) {
            Ok(proof) => proof.is_none(),
            Err(error) => return error_response(error),
        }
    } else {
        false
    };
    let user = match token.filter(|_| login_allowed(state, local)) {
        Some(token) => match authenticate_token(&admin, store.identity_connection(), &token) {
            Ok(principal) => Some(principal),
            Err(IdentityError::Unauthorized | IdentityError::Forbidden) => None,
            Err(e) => return error_response(e),
        },
        None => None,
    };
    let request_session = if user.is_some() {
        request_session
    } else {
        None
    };
    let local_management = native_session && user.is_some();
    Json(json!({"csrf":csrf,"epoch":state.policy.access_id,"authentication":{"configured":configured,"user":user,"session_id":request_session,"bootstrap_allowed":false,"login_allowed":login_allowed(state,local),"local_management":local_management}})).into_response()
}
fn set_cookie(response: &mut Response, state: &WebState, token: &str, remove: bool) {
    let value = format!(
        "{COOKIE}={token}; Path=/admin; HttpOnly; SameSite=Strict; Max-Age={}{}",
        if remove { 0 } else { 8 * 60 * 60 },
        if state.policy.secure { "; Secure" } else { "" }
    );
    if let Ok(value) = HeaderValue::from_str(&value) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Access {
    ticket: String,
}

pub(super) async fn access(
    State(state): State<WebState>,
    headers: HeaderMap,
    Extension(local): Extension<BootstrapAccess>,
    Json(input): Json<Access>,
) -> Response {
    if !login_allowed(&state, local.0) {
        return failure(StatusCode::FORBIDDEN);
    }
    let Some(admin) = state.admin.upgrade() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    if let Err(code) = admin.web_access.session(&headers, true, &state.policy) {
        return failure(code);
    }
    let Ok(permit) = admin.web_access.login_slots.clone().try_acquire_owned() else {
        return failure(StatusCode::TOO_MANY_REQUESTS);
    };
    let owner = admin.clone();
    let ticket = match admin
        .web_access
        .consume_ticket(&input.ticket, &state.policy)
    {
        Ok(ticket) => ticket,
        Err(_) => return error_response(IdentityError::Unauthorized),
    };
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let store = owner
            .shared_work
            .lock()
            .map_err(|_| IdentityError::Unavailable)?;
        let db = store.identity_connection();
        let now = crate::service::now_ms();
        let network = owner
            .network
            .lock()
            .map_err(|_| IdentityError::Unavailable)?;
        if let Some(proof) = &ticket.device {
            identity::web_access::verify_device(db, &network, &ticket.user_id, proof, now)?;
        }
        identity::web_access::issue(db, &ticket.user_id, ticket.device.as_ref(), now)
    })
    .await;
    match result {
        Ok(Ok(session)) => {
            let mut response =
                session_response(&state, &headers, local.0, Some(Some(&session.token))).await;
            set_cookie(&mut response, &state, &session.token, false);
            response
        }
        Ok(Err(error)) => error_response(error),
        Err(_) => failure(StatusCode::SERVICE_UNAVAILABLE),
    }
}
pub(super) async fn logout(
    State(state): State<WebState>,
    headers: HeaderMap,
    Extension(local): Extension<BootstrapAccess>,
    Json(input): Json<Value>,
) -> Response {
    if !input.as_object().is_some_and(|v| v.is_empty()) {
        return failure(StatusCode::BAD_REQUEST);
    }
    let Some(admin) = state.admin.upgrade() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    if let Err(code) = admin.web_access.session(&headers, true, &state.policy) {
        return failure(code);
    }
    if let Some(token) = cookie_token(&headers) {
        let owner = admin.clone();
        let Ok(permit) = admin.web_access.login_slots.clone().try_acquire_owned() else {
            return failure(StatusCode::TOO_MANY_REQUESTS);
        };
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let store = owner
                .shared_work
                .lock()
                .map_err(|_| IdentityError::Unavailable)?;
            identity::logout(
                store.identity_connection(),
                &token,
                SessionAudience::WebAdmin,
                None,
            )
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return error_response(error),
            Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
        }
    }
    let mut response = session_response(&state, &headers, local.0, Some(None)).await;
    set_cookie(&mut response, &state, "", true);
    response
}

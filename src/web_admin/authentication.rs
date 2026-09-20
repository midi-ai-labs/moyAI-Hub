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
        StatusCode::UNAUTHORIZED => "ユーザー名またはパスワードを確認してください。",
        StatusCode::BAD_REQUEST => "ユーザー名・表示名・パスワードの入力条件を確認してください。",
        StatusCode::CONFLICT => "管理者の初期設定は完了しています。ログインしてください。",
        StatusCode::TOO_MANY_REQUESTS => {
            "ログイン試行の上限に達しました。時間をおいて再試行してください。"
        }
        StatusCode::FORBIDDEN => "管理者としてログインしてください。",
        _ => "利用者の認証状態を確認できません。",
    };
    (code, Json(json!({"error":message}))).into_response()
}
pub(super) fn principal(admin: &Admin, headers: &HeaderMap) -> Result<Principal, StatusCode> {
    let token = request_token(headers)?;
    let store = admin
        .shared_work
        .lock()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    identity::authenticate(
        store.identity_connection(),
        &token,
        SessionAudience::WebAdmin,
        None,
        crate::service::now_ms(),
    )
    .map_err(|e| status(&e))
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
pub(super) fn session_response(
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
    let user = match token.filter(|_| login_allowed(state, local)) {
        Some(token) => match identity::authenticate(
            store.identity_connection(),
            &token,
            SessionAudience::WebAdmin,
            None,
            crate::service::now_ms(),
        ) {
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
    Json(json!({"csrf":csrf,"epoch":state.policy.access_id,"authentication":{"configured":configured,"user":user,"session_id":request_session,"bootstrap_allowed":local&&!configured,"login_allowed":login_allowed(state,local)}})).into_response()
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
pub(super) struct Login {
    username: String,
    password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Bootstrap {
    username: String,
    display_name: String,
    password: String,
}

pub(super) async fn login(
    State(state): State<WebState>,
    headers: HeaderMap,
    Extension(local): Extension<BootstrapAccess>,
    Json(input): Json<Login>,
) -> Response {
    authenticate(state, headers, local, input.username, None, input.password).await
}
pub(super) async fn bootstrap(
    State(state): State<WebState>,
    headers: HeaderMap,
    Extension(local): Extension<BootstrapAccess>,
    Json(input): Json<Bootstrap>,
) -> Response {
    if !local.0 {
        return failure(StatusCode::FORBIDDEN);
    }
    authenticate(
        state,
        headers,
        local,
        input.username,
        Some(input.display_name),
        input.password,
    )
    .await
}
async fn authenticate(
    state: WebState,
    headers: HeaderMap,
    local: BootstrapAccess,
    username: String,
    display_name: Option<String>,
    password: String,
) -> Response {
    if !login_allowed(&state, local.0) {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"管理者ログインにはHTTPSの管理URL、またはHub PC上のローカル管理画面を利用してください。"}))).into_response();
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
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let store = owner
            .shared_work
            .lock()
            .map_err(|_| IdentityError::Unavailable)?;
        let db = store.identity_connection();
        let now = crate::service::now_ms();
        if let Some(display) = display_name {
            identity::bootstrap_admin(db, &username, &display, &password, now)?;
        }
        identity::login(
            db,
            &username,
            &password,
            SessionAudience::WebAdmin,
            None,
            now,
        )
    })
    .await;
    match result {
        Ok(Ok(session)) => {
            let mut response =
                session_response(&state, &headers, local.0, Some(Some(&session.token)));
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
        let Ok(store) = admin.shared_work.lock() else {
            return failure(StatusCode::SERVICE_UNAVAILABLE);
        };
        if let Err(e) = identity::logout(
            store.identity_connection(),
            &token,
            SessionAudience::WebAdmin,
            None,
        ) {
            return error_response(e);
        }
    }
    let mut response = session_response(&state, &headers, local.0, Some(None));
    set_cookie(&mut response, &state, "", true);
    response
}

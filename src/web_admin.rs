//! Browser management on independent HTTP and optional HTTPS/proxy listeners.
//! Host/device-authorized administrator sessions protect management operations. Origin and CSRF
//! checks remain independent of the authenticated identity.
use crate::{admin::Admin, network::tls::TlsPeer};
use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

pub(crate) fn assets_ready() -> bool {
    bundled_asset("index.html").is_some()
}

mod authentication;
pub(crate) mod server;
mod shared;
mod tickets;

/// Server-configured origin and access generation. Request headers never select a policy.
#[derive(Clone, Debug)]
pub(crate) struct WebPolicy {
    origin: String,
    authority: String,
    access_id: String,
    secure: bool,
    loopback_only: bool,
    local_management: bool,
}
impl WebPolicy {
    pub(crate) fn new(origin: &str, access_id: String) -> Result<Self, String> {
        let url = reqwest::Url::parse(origin).map_err(|_| "Web管理のURLを確認してください。")?;
        let secure = url.scheme() == "https";
        if !(secure || url.scheme() == "http")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port_or_known_default() == Some(0)
            || access_id.is_empty()
            || access_id.len() > 64
            || !access_id.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err("Web管理にはHTTPまたはHTTPSのOriginを指定してください。".into());
        }
        let origin = url.origin().ascii_serialization();
        let authority = origin
            .split_once("://")
            .expect("validated HTTP origin")
            .1
            .to_owned();
        Ok(Self {
            origin,
            authority,
            access_id,
            secure,
            loopback_only: false,
            local_management: false,
        })
    }
    fn loopback_only(mut self) -> Self {
        self.loopback_only = true;
        self
    }
    fn local_management(mut self) -> Self {
        self.loopback_only = true;
        self.local_management = true;
        self
    }
}
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; font-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

#[derive(Default)]
struct AccessState {
    epoch: Option<String>,
    csrf_seed: String,
    remote_origin: Option<String>,
    tickets: Vec<tickets::Ticket>,
}
pub(crate) struct WebAccess {
    state: Mutex<AccessState>,
    slots: Arc<tokio::sync::Semaphore>,
    login_slots: Arc<tokio::sync::Semaphore>,
    read_slots: Arc<tokio::sync::Semaphore>,
}
impl Default for WebAccess {
    fn default() -> Self {
        Self {
            state: Mutex::new(AccessState::default()),
            slots: Arc::new(tokio::sync::Semaphore::new(8)),
            login_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            read_slots: Arc::new(tokio::sync::Semaphore::new(8)),
        }
    }
}
fn secret() -> String {
    format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new())
}
fn equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && left.iter().zip(right).fold(0u8, |v, (a, b)| v | (a ^ b)) == 0
}
impl WebAccess {
    pub(crate) fn enabled(&self) -> bool {
        self.epoch().is_some()
    }
    pub(crate) fn epoch(&self) -> Option<String> {
        self.state.lock().ok().and_then(|state| state.epoch.clone())
    }
    pub(crate) fn enable(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Web管理の状態を確認できません。")?;
        *state = AccessState {
            epoch: Some(ulid::Ulid::new().to_string()),
            csrf_seed: secret(),
            ..Default::default()
        };
        Ok(())
    }
    pub(crate) fn disable(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = AccessState::default();
        }
    }
    // Anonymous clients obtain this token from the same-origin session endpoint.
    // It is a browser CSRF proof, not an administrator identity or access key.
    // Derivation keeps memory bounded regardless of the number of browsers.
    fn session(
        &self,
        headers: &HeaderMap,
        mutation: bool,
        policy: &WebPolicy,
    ) -> Result<String, StatusCode> {
        let state = self
            .state
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if state.epoch.as_deref() != Some(&policy.access_id) {
            return Err(StatusCode::NOT_FOUND);
        }
        let mut digest = Sha256::new();
        digest.update(state.csrf_seed.as_bytes());
        digest.update([0]);
        digest.update(policy.origin.as_bytes());
        let csrf = format!("{:x}", digest.finalize());
        if mutation
            && (headers.get_all("x-moyai-csrf").iter().count() != 1
                || !headers
                    .get("x-moyai-csrf")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| equal(v.as_bytes(), csrf.as_bytes())))
        {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(csrf)
    }
}

#[derive(Clone)]
struct WebState {
    admin: Weak<Admin>,
    policy: WebPolicy,
}
fn requires_hub_pc() -> Response {
    (StatusCode::FORBIDDEN,Json(json!({"error":"Hubの起動・公開先・AI接続の変更は、Hub PC の管理画面で行ってください。","code":"requires_hub_pc"}))).into_response()
}
fn failure(code: StatusCode) -> Response {
    let message = match code {
        StatusCode::FORBIDDEN => "このWeb管理操作は許可されていません。",
        StatusCode::UNAUTHORIZED => {
            "Hub PC または管理権限のあるPCから管理画面を開き直してください。"
        }
        StatusCode::TOO_MANY_REQUESTS => {
            "処理中の操作があります。少し待ってから再試行してください。"
        }
        StatusCode::NOT_FOUND => "Web管理は停止中です。Hubサーバーの起動状態を確認してください。",
        _ => "Web管理へ接続できません。",
    };
    (code, Json(json!({"error": message}))).into_response()
}
fn secured(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}
fn origin_allowed(headers: &HeaderMap, policy: &WebPolicy, method: &Method) -> bool {
    if headers.get_all(header::HOST).iter().count() != 1
        || headers.get_all(header::ORIGIN).iter().count() > 1
    {
        return false;
    }
    let navigation = (method == Method::GET || method == Method::HEAD)
        && headers.get("sec-fetch-mode").and_then(|v| v.to_str().ok()) == Some("navigate")
        && headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) == Some("document");
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| {
            value.eq_ignore_ascii_case(&policy.authority)
                || (reqwest::Url::parse(&policy.origin).is_ok_and(|url| url.port().is_none())
                    && value.eq_ignore_ascii_case(&format!(
                        "{}:{}",
                        policy.authority,
                        if policy.secure { 443 } else { 80 }
                    )))
        })
        && match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
            Some(value) => value == policy.origin,
            None => method == Method::GET || method == Method::HEAD,
        }
        && headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| matches!(v, "same-origin" | "none") || navigation)
}
async fn guard(State(state): State<WebState>, mut request: Request, next: Next) -> Response {
    let Some(admin) = state.admin.upgrade() else {
        return secured(failure(StatusCode::SERVICE_UNAVAILABLE));
    };
    if admin.web_access.epoch().as_deref() != Some(&state.policy.access_id) {
        return secured(failure(StatusCode::NOT_FOUND));
    }
    let _peer =
        if let Some(ConnectInfo(address)) = request.extensions().get::<ConnectInfo<SocketAddr>>() {
            if state.policy.loopback_only && !address.ip().is_loopback() {
                return secured(failure(StatusCode::FORBIDDEN));
            }
            *address
        } else if let Some(ConnectInfo(peer)) = request.extensions().get::<ConnectInfo<TlsPeer>>() {
            peer.address
        } else {
            return secured(failure(StatusCode::FORBIDDEN));
        };
    if !origin_allowed(request.headers(), &state.policy, request.method()) {
        return secured(failure(StatusCode::FORBIDDEN));
    }
    let direct_local = _peer.ip().is_loopback()
        && state.policy.local_management
        && reqwest::Url::parse(&state.policy.origin).is_ok_and(|url| {
            url.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            })
        })
        && !request.headers().contains_key("forwarded")
        && !request.headers().contains_key("x-forwarded-for")
        && !request.headers().contains_key("x-forwarded-host");
    request
        .extensions_mut()
        .insert(authentication::BootstrapAccess(direct_local));
    if request.method() == Method::POST
        && (request
            .headers()
            .get_all(header::CONTENT_TYPE)
            .iter()
            .count()
            != 1
            || request
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_none_or(|v| {
                    !v.split(';')
                        .next()
                        .is_some_and(|v| v.trim() == "application/json")
                }))
    {
        return secured(failure(StatusCode::FORBIDDEN));
    }
    secured(next.run(request).await)
}
pub(crate) fn router(admin: Weak<Admin>, policy: WebPolicy) -> Router {
    let state = WebState { admin, policy };
    Router::new()
        .route("/", get(|| async { Redirect::to("/admin/") }))
        .route("/admin", get(|| async { Redirect::to("/admin/") }))
        .route("/admin/", get(index))
        .route("/admin/assets/{*path}", get(asset))
        .route("/admin/session", get(session))
        .route("/admin/access", post(authentication::access))
        .route("/admin/logout", post(authentication::logout))
        .route("/admin/command", post(command))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}
async fn index() -> Response {
    asset_response("index.html")
}
fn asset_response(path: &str) -> Response {
    match bundled_asset(path) {
        Some((mime, bytes)) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn asset(Path(path): Path<String>) -> Response {
    asset_response(&format!("assets/{path}"))
}
async fn session(
    State(state): State<WebState>,
    headers: HeaderMap,
    axum::Extension(local): axum::Extension<authentication::BootstrapAccess>,
) -> Response {
    authentication::session_response(&state, &headers, local.0, None).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    command: String,
    #[serde(default = "empty_args")]
    args: Value,
}
fn empty_args() -> Value {
    json!({})
}
fn browser_command(command: &str) -> bool {
    matches!(
        command,
        "hub_web_status"
            | "hub_autostart_status"
            | "hub_autostart_set"
            | "hub_firewall_preview"
            | "hub_firewall_apply"
            | "hub_firewall_remove"
            | "hub_start"
            | "hub_stop"
            | "hub_network_start"
            | "hub_network_stop"
            | "hub_snapshot"
            | "hub_discover"
            | "hub_refresh"
            | "hub_register"
            | "hub_edit"
            | "hub_set_standard_model"
            | "hub_network_snapshot"
            | "hub_hosting_options"
            | "hub_network_settings"
            | "hub_network_diagnostics"
            | "hub_network_renew_certificate"
            | "hub_network_invite"
            | "hub_network_approve"
            | "hub_network_admission"
            | "hub_network_group"
            | "hub_network_device"
            | "hub_network_delete_device"
            | "hub_network_rule"
            | "hub_network_delete_rule"
            | "hub_network_export"
            | "hub_history_request"
            | "hub_history_result"
            | "hub_shared_snapshot"
            | "hub_shared_command"
            | "hub_shared_retention_preview"
            | "hub_shared_maintenance"
            | "hub_join_project"
    )
}
async fn command(
    State(state): State<WebState>,
    headers: HeaderMap,
    axum::Extension(local): axum::Extension<authentication::BootstrapAccess>,
    Json(input): Json<Command>,
) -> Response {
    if !authentication::login_allowed(&state, local.0) {
        return failure(StatusCode::FORBIDDEN);
    }
    let Some(admin) = state.admin.upgrade() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    if let Err(code) = admin.web_access.session(&headers, true, &state.policy) {
        return failure(code);
    }
    if !browser_command(&input.command) || !input.args.is_object() {
        return failure(StatusCode::FORBIDDEN);
    }
    if !local.0
        && !shared::owns_command(&input.command)
        && !matches!(
            input.command.as_str(),
            "hub_web_status"
                | "hub_snapshot"
                | "hub_network_snapshot"
                | "hub_hosting_options"
                | "hub_network_diagnostics"
                | "hub_network_export"
                | "hub_history_result"
        )
    {
        return requires_hub_pc();
    }
    if matches!(
        input.command.as_str(),
        "hub_autostart_status"
            | "hub_autostart_set"
            | "hub_firewall_preview"
            | "hub_firewall_apply"
            | "hub_firewall_remove"
    ) && !local.0
    {
        return failure(StatusCode::FORBIDDEN);
    }
    let Ok(permit) = admin.web_access.slots.clone().try_acquire_owned() else {
        return failure(StatusCode::TOO_MANY_REQUESTS);
    };
    // Dispatch can own blocking filesystem work. A caller timeout/disconnect must not
    // release its concurrency slot while that actual work is still running.
    let timeout_seconds = if matches!(
        input.command.as_str(),
        "hub_firewall_apply" | "hub_firewall_remove"
    ) {
        125
    } else {
        30
    };
    let operation = tokio::spawn(async move {
        // Authentication may wait on the same database/network owner as a
        // queued command. Never block the async executor on those std mutexes.
        // This task retains the request slot even after a caller disconnects.
        let owner = admin.clone();
        let auth_headers = headers.clone();
        match tokio::task::spawn_blocking(move || authentication::principal(&owner, &auth_headers))
            .await
        {
            Ok(Ok((_, native))) => {
                if !native
                    && !shared::owns_command(&input.command)
                    && !matches!(
                        input.command.as_str(),
                        "hub_web_status"
                            | "hub_snapshot"
                            | "hub_network_snapshot"
                            | "hub_hosting_options"
                            | "hub_network_diagnostics"
                            | "hub_network_export"
                            | "hub_history_result"
                    )
                {
                    return requires_hub_pc();
                }
            }
            Ok(Err(code)) => return failure(code),
            Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
        }
        if shared::owns_command(&input.command) {
            let token = match authentication::request_token(&headers) {
                Ok(token) => token,
                Err(code) => return failure(code),
            };
            return shared::command(admin, input, token, permit).await;
        }
        let _permit = permit;
        match admin.dispatch(&input.command, input.args).await {
            Ok(value) => {
                let Ok(bytes) = serde_json::to_vec(&json!({"result":value})) else {
                    return failure(StatusCode::INTERNAL_SERVER_ERROR);
                };
                if bytes.len() > 4 * 1024 * 1024 {
                    return failure(StatusCode::PAYLOAD_TOO_LARGE);
                }
                ([(header::CONTENT_TYPE, "application/json")], bytes).into_response()
            }
            Err(message) => {
                (StatusCode::BAD_REQUEST, Json(json!({"error":message}))).into_response()
            }
        }
    });
    // A lost response is not replayed: revision/identity checks remain in the shared owner.
    match tokio::time::timeout(Duration::from_secs(timeout_seconds), operation).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => failure(StatusCode::SERVICE_UNAVAILABLE),
        Err(_) => (StatusCode::GATEWAY_TIMEOUT, Json(json!({"error": "操作結果を確認できません。最新状態を取得してください。操作は自動再実行されません。"}))).into_response(),
    }
}

#[cfg(test)]
mod tests;

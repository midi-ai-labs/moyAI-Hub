//! Private local control for the foreground Hub server.
//! The browser never receives this credential; --status and --stop connect here.
pub mod launch;
mod private_file;

use crate::admin::Admin;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, Semaphore};

const DESCRIPTOR: &str = "host-control.json";
const DESCRIPTOR_LIMIT: u64 = 16 * 1024;
const COMMAND_LIMIT: usize = 2 * 1024 * 1024;
const RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const COMMAND_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    schema_version: u32,
    instance_id: String,
    pid: u32,
    port: u16,
    token: String,
    catalog_path: PathBuf,
}
impl Descriptor {
    fn validate(&self, catalog_path: &Path) -> Result<(), String> {
        if self.schema_version != 1
            || self.port == 0
            || self.pid == 0
            || self.token.len() != 52
            || !self.token.bytes().all(|b| b.is_ascii_alphanumeric())
            || self.instance_id.len() != 26
            || !self.instance_id.bytes().all(|b| b.is_ascii_alphanumeric())
            || self.catalog_path != catalog_path
        {
            return Err(
                "Hub の管理接続情報が無効です。保存先と起動中の Hub を確認してください。".into(),
            );
        }
        Ok(())
    }
    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

fn canonical_catalog(path: &Path, create: bool) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or("Hub の保存先が無効です。")?;
    let name = path.file_name().ok_or("Hub の保存先が無効です。")?;
    if create {
        std::fs::create_dir_all(parent).map_err(|_| "Hub の保存先を作成できません。")?;
    }
    let parent = std::fs::canonicalize(parent).map_err(|_| "Hub の保存先を開けません。")?;
    Ok(parent.join(name))
}
fn read_descriptor(catalog: &Path) -> Result<Descriptor, String> {
    let path = catalog
        .parent()
        .ok_or("Hub の保存先が無効です。")?
        .join(DESCRIPTOR);
    let mut file = File::open(path).map_err(|_| "Hub ホストの管理接続情報がありません。")?;
    private_file::verify(&file).map_err(|_| "Hub の管理接続情報の権限を確認できません。他の利用者と共有されていないことを確認してください。")?;
    if !file
        .metadata()
        .map_err(|_| "Hub の管理接続情報を確認できません。")?
        .is_file()
    {
        return Err("Hub の管理接続情報が通常ファイルではありません。".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(DESCRIPTOR_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Hub の管理接続情報を読めません。")?;
    if bytes.len() as u64 > DESCRIPTOR_LIMIT {
        return Err("Hub の管理接続情報が上限を超えています。".into());
    }
    let descriptor: Descriptor =
        serde_json::from_slice(&bytes).map_err(|_| "Hub の管理接続情報が壊れています。")?;
    descriptor.validate(catalog)?;
    Ok(descriptor)
}
fn write_descriptor(descriptor: &Descriptor) -> Result<(), String> {
    let directory = descriptor
        .catalog_path
        .parent()
        .ok_or("Hub の保存先が無効です。")?;
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .map_err(|_| "Hub の管理接続情報を準備できません。")?;
    // Protect the empty file handle before any credential is written.
    private_file::protect(file.as_file())
        .map_err(|_| "Hub の管理資格情報をこの利用者だけに保護できません。")?;
    serde_json::to_writer(&mut file, descriptor).map_err(|_| "Hub の管理接続情報を書けません。")?;
    file.flush()
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "Hub の管理接続情報を保存できません。")?;
    file.persist(directory.join(DESCRIPTOR))
        .map_err(|_| "Hub の管理接続情報を置き換えられません。")?;
    Ok(())
}
fn lock_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}
fn profile_busy(catalog: &Path) -> Result<bool, String> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(
            catalog
                .parent()
                .ok_or("Hub の保存先が無効です。")?
                .join("hub.lock"),
        )
        .map_err(|_| "Hub ホストの稼働を確認できません。")?;
    match fs2::FileExt::try_lock_exclusive(&lock) {
        Ok(()) => Ok(false),
        Err(error) if lock_contended(&error) => Ok(true),
        Err(_) => Err("Hub ホストの排他状態を確認できません。".into()),
    }
}

#[derive(Clone)]
pub struct HostClient {
    descriptor: Descriptor,
    http: reqwest::Client,
}
impl HostClient {
    fn from_descriptor(descriptor: Descriptor) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(35))
            .build()
            .map_err(|_| "Hub の管理接続を準備できません。")?;
        Ok(Self { descriptor, http })
    }
    /// Connect only. Used by explicit shutdown and never starts a host implicitly.
    pub async fn connect(catalog_path: &Path) -> Result<Self, String> {
        let catalog = canonical_catalog(catalog_path, false)?;
        let client = Self::from_descriptor(read_descriptor(&catalog)?)?;
        client.health().await?;
        Ok(client)
    }
    pub fn pid(&self) -> u32 {
        self.descriptor.pid
    }
    pub fn instance_id(&self) -> &str {
        &self.descriptor.instance_id
    }
    async fn health(&self) -> Result<(), String> {
        let response = self
            .http
            .get(format!("{}/host/health", self.descriptor.base_url()))
            .bearer_auth(&self.descriptor.token)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .map_err(|_| "Hub サーバーに接続できません。起動状態と保存先を確認してください。")?;
        let value = read_reply(response).await?;
        if value["closing"] != false
            || value["instance_id"] != self.descriptor.instance_id
            || value["pid"] != self.descriptor.pid
            || value["catalog_path"] != json!(self.descriptor.catalog_path)
        {
            return Err("Hub ホストの identity が保存先と一致しません。".into());
        }
        Ok(())
    }
    pub async fn dispatch(&self, command: &str, args: Value) -> Result<Value, String> {
        let request = Command {
            command: command.to_owned(),
            args,
        };
        let body = serde_json::to_vec(&request).map_err(|_| "Hub 管理要求を作成できません。")?;
        if body.len() > COMMAND_LIMIT {
            return Err("Hub 管理要求がサイズ上限を超えています。".into());
        }
        let response = self.http.post(format!("{}/host/command", self.descriptor.base_url()))
            .bearer_auth(&self.descriptor.token).header("content-type", "application/json").body(body).send().await
            .map_err(|_| "Hub の応答を確認できません。操作が完了したか不明なため、自動で再実行していません。再接続して状態を確認してください。")?;
        let value = read_reply(response).await?;
        if value["ok"] == true {
            Ok(value.get("value").cloned().unwrap_or(Value::Null))
        } else {
            Err(value["error"]
                .as_str()
                .unwrap_or("Hub の管理操作に失敗しました。")
                .to_owned())
        }
    }
    /// Explicitly terminate the host; ordinary network-stop commands keep it alive.
    pub async fn shutdown(&self) -> Result<(), String> {
        let response = self
            .http
            .post(format!("{}/host/shutdown", self.descriptor.base_url()))
            .bearer_auth(&self.descriptor.token)
            .send()
            .await
            .map_err(|_| "Hub ホストへ終了要求を送れません。自動で強制終了していません。")?;
        read_reply(response).await?;
        tokio::time::timeout(Duration::from_secs(25), async {
            loop {
                if !profile_busy(&self.descriptor.catalog_path)? {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| "Hub ホストの終了確認が期限切れです。ホストを自動で強制終了していません。")?
    }
}
async fn read_reply(mut response: reqwest::Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!(
            "Hub の管理接続が拒否されました（HTTP {}）。再接続して状態を確認してください。",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Hub 管理応答を受信できません。操作の完了状態を確認してください。")?
    {
        if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            return Err("Hub 管理応答がサイズ上限を超えています。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Hub 管理応答の形式が無効です。".into())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    command: String,
    args: Value,
}
struct HostState {
    admin: Arc<Admin>,
    descriptor: Descriptor,
    operations: Arc<Semaphore>,
    closing: AtomicBool,
    shutdown: Notify,
}
fn secret_matches(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0_u8, |v, (a, b)| v | (a ^ b))
            == 0
}
async fn authenticate(
    State(state): State<Arc<HostState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let authorized = !headers.contains_key("origin")
        && !headers.contains_key("sec-fetch-site")
        && headers.get("host").and_then(|v| v.to_str().ok())
            == Some(format!("127.0.0.1:{}", state.descriptor.port).as_str())
        && headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|token| secret_matches(token, &state.descriptor.token));
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    next.run(request).await
}
async fn health(State(state): State<Arc<HostState>>) -> Json<Value> {
    Json(
        json!({"instance_id":state.descriptor.instance_id,"pid":state.descriptor.pid,
        "catalog_path":state.descriptor.catalog_path,"closing":state.closing.load(Ordering::SeqCst)}),
    )
}
async fn dispatch(State(state): State<Arc<HostState>>, Json(request): Json<Command>) -> Response {
    if state.closing.load(Ordering::SeqCst) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"host_stopping"})),
        )
            .into_response();
    }
    if request.command.is_empty() || request.command.len() > 128 || !request.args.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_request"})),
        )
            .into_response();
    }
    let Ok(permit) = state.operations.clone().try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"host_busy"})),
        )
            .into_response();
    };
    let operation = tokio::spawn(async move {
        let _permit = permit;
        state.admin.dispatch(&request.command, request.args).await
    });
    let result = match tokio::time::timeout(COMMAND_DEADLINE, operation).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("Hub の操作処理を確認できません。再接続して状態を確認してください。".into()),
        Err(_) => Err("Hub の操作完了を時間内に確認できません。処理が続いている可能性があるため、自動で再実行していません。再接続して状態を確認してください。".into()),
    };
    let value = match result {
        Ok(value) => json!({"ok":true,"value":value}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    match serde_json::to_vec(&value) {
        Ok(bytes) if bytes.len() <= RESPONSE_LIMIT => (
            [
                ("content-type", "application/json"),
                ("cache-control", "no-store"),
            ],
            bytes,
        )
            .into_response(),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"response_limit"})),
        )
            .into_response(),
    }
}
async fn shutdown(State(state): State<Arc<HostState>>, _headers: HeaderMap) -> Json<Value> {
    state.closing.store(true, Ordering::SeqCst);
    state.shutdown.notify_one();
    Json(json!({"stopping":true}))
}
fn router(state: Arc<HostState>) -> Router {
    Router::new()
        .route("/host/health", get(health))
        .route("/host/command", post(dispatch))
        .route("/host/shutdown", post(shutdown))
        .layer(DefaultBodyLimit::max(COMMAND_LIMIT))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}
struct DescriptorOwner(Descriptor);
impl Drop for DescriptorOwner {
    fn drop(&mut self) {
        if read_descriptor(&self.0.catalog_path)
            .is_ok_and(|current| current.instance_id == self.0.instance_id)
        {
            if let Some(directory) = self.0.catalog_path.parent() {
                let _ = std::fs::remove_file(directory.join(DESCRIPTOR));
            }
        }
    }
}
/// Start the foreground server and its browser UI using the same lifetime owner.
pub async fn run_server(catalog_path: PathBuf, settings: Option<Value>) -> Result<(), String> {
    let catalog_path = canonical_catalog(&catalog_path, true)?;
    let admin = Admin::open(catalog_path.clone())?;
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .map_err(|_| "Hub のローカル管理待受を開始できません。")?;
    let port = listener
        .local_addr()
        .map_err(|_| "Hub のローカル管理待受を確認できません。")?
        .port();
    let descriptor = Descriptor {
        schema_version: 1,
        instance_id: ulid::Ulid::new().to_string(),
        pid: std::process::id(),
        port,
        token: format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new()),
        catalog_path,
    };
    let settings = match settings {
        Some(settings) => Some(settings),
        None => match std::fs::metadata(
            descriptor
                .catalog_path
                .parent()
                .unwrap()
                .join("web-management.json"),
        ) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Some(json!({"local_port":9080,"remote":{"mode":"off"}}))
            }
            _ => None,
        },
    };
    let args = settings.map_or_else(|| json!({}), |settings| json!({"settings":settings}));
    let web = match admin.dispatch("hub_web_enable", args).await {
        Ok(web) => web,
        Err(error) => {
            let _ = admin.shutdown().await;
            return Err(format!(
                "Web管理を起動できません: {error} --web-settings または --web-port で設定を指定してください。"
            ));
        }
    };
    println!("moyAI Hub: {}", web["local_url"].as_str().unwrap_or(""));
    if let Some(url) = web["remote_url"].as_str() {
        println!("Web public URL: {url}");
    }
    println!("Web管理には管理者ログインが必要です。終了は Ctrl+C または --stop を使用します。");
    let state = Arc::new(HostState {
        admin,
        descriptor: descriptor.clone(),
        operations: Arc::new(Semaphore::new(16)),
        closing: AtomicBool::new(false),
        shutdown: Notify::new(),
    });
    if let Err(error) = write_descriptor(&descriptor) {
        let _ = state.admin.shutdown().await;
        return Err(error);
    }
    let _descriptor_owner = DescriptorOwner(descriptor);
    let app = router(state.clone());
    let mut server = tokio::spawn(async move { axum::serve(listener, app).await });
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .map_err(|error| format!("Ctrl+C handler failed: {error}"))
    };
    let (listener_failed, interrupt_error) = tokio::select! {
        _ = state.shutdown.notified() => (false, None),
        _ = &mut server => (true, None),
        result = interrupt => (false, result.err()),
    };
    // A browser close never changes server ownership. Only explicit stop/Ctrl+C does.
    let result = tokio::time::timeout(Duration::from_secs(20), state.admin.shutdown())
        .await
        .map_err(|_| "Hub ホストの終了処理が期限切れです。".to_string())
        .and_then(|result| result);
    if !listener_failed {
        server.abort();
        let _ = server.await;
    }
    if listener_failed {
        return Err("Hub のローカル管理待受が終了しました。".into());
    }
    if let Some(error) = interrupt_error {
        return Err(error);
    }
    result
}

#[cfg(test)]
mod tests;

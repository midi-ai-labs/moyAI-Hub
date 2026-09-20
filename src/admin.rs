//! Server-owned management state and operations shared by Web management and CLI control.
use crate::{
    catalog::{CapacityPool, Catalog, CatalogStore, Deployment, LogicalModel},
    core::HubCore,
    discovery::{self, Discovery},
    gateway_process::{self, GatewayProcess},
    protocol::Availability,
    service::{self, SharedCore, now_ms},
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
mod autostart;
mod firewall;
mod hosting_options;
mod network_admin;
mod network_hosting;

#[cfg(test)]
pub(crate) mod test_support {
    /// Keep the real state owner contended while testing an external command's lifetime.
    /// The opaque guard exposes neither the private network field nor mutable state.
    pub(crate) fn hold_network(admin: &super::Admin) -> impl Drop + '_ {
        admin.network.lock().expect("test network owner poisoned")
    }
}

struct Server {
    bind: String,
    task: tokio::task::JoinHandle<()>,
    stop: tokio::sync::oneshot::Sender<()>,
    gateway: Option<GatewayProcess>,
}
#[derive(Clone, Serialize)]
struct Evidence {
    id: String,
    discovery: Discovery,
    observed_at_ms: u64,
}
pub struct Admin {
    pub(crate) shared_work: crate::shared_work::SharedStore,
    pub(crate) web_access: crate::web_admin::WebAccess,
    pub(crate) web_server: tokio::sync::Mutex<crate::web_admin::server::Manager>,
    core: SharedCore,
    path: PathBuf,
    _lock: File,
    server: tokio::sync::Mutex<Option<Server>>,
    evidence: Mutex<Option<Evidence>>,
    discovery_generation: AtomicU64,
    refresh_running: std::sync::atomic::AtomicBool,
    pub(crate) network: crate::network::SharedNetwork,
    network_server: tokio::sync::Mutex<Option<network_admin::NetworkServer>>,
    network_hosting: Mutex<network_hosting::Store>,
}

impl Admin {
    /// Open the one authoritative catalog and device network for this server.
    /// The service keeps the directory lock for the server's lifetime.
    pub fn open(path: PathBuf) -> Result<Arc<Self>, String> {
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or("カタログの保存先を確認してください。")?;
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("hub.lock"))
            .map_err(|error| error.to_string())?;
        lock.try_lock_exclusive()
            .map_err(|_| "このデータフォルダーの moyAI Hub は起動済みです。")?;
        if directory
            .join(crate::backup::RESTORE_INCOMPLETE)
            .try_exists()
            .map_err(|e| e.to_string())?
        {
            return Err("バックアップ復元が完了していません。元のバックアップを別の新しいデータフォルダーへ復元してください。".into());
        }
        let mut store = if path.exists() {
            if std::fs::metadata(&path)
                .map_err(|error| error.to_string())?
                .len()
                > 4 * 1024 * 1024
            {
                return Err("カタログがサイズ上限を超えています。".into());
            }
            serde_json::from_slice::<CatalogStore>(
                &std::fs::read(&path).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?
        } else {
            CatalogStore::empty(env!("CARGO_PKG_VERSION"))
        };
        store
            .reconcile_software(env!("CARGO_PKG_VERSION"), now_ms())
            .map_err(|error| error.to_string())?;
        save(&path, &store)?;
        let network = crate::network::DeviceNetwork::open(
            &directory.join("device-network.json"),
            &store.hub_id,
        )
        .map_err(|error| error.to_string())?;
        let web_server =
            crate::web_admin::server::Manager::open(directory.join("web-management.json"));
        let network_hosting = network_hosting::Store::open(directory.join("network-hosting.json"));
        let shared_work =
            crate::shared_work::SharedWork::open(&directory.join("shared-work.sqlite3"))
                .map_err(|error| error.to_string())?;
        Ok(Arc::new(Self {
            shared_work: Arc::new(Mutex::new(shared_work)),
            web_access: Default::default(),
            web_server: tokio::sync::Mutex::new(web_server),
            core: Arc::new(Mutex::new(
                HubCore::new(store).map_err(|error| error.to_string())?,
            )),
            path,
            _lock: lock,
            server: tokio::sync::Mutex::new(None),
            evidence: Mutex::new(None),
            discovery_generation: AtomicU64::new(0),
            refresh_running: std::sync::atomic::AtomicBool::new(false),
            network: Arc::new(Mutex::new(network)),
            network_server: tokio::sync::Mutex::new(None),
            network_hosting: Mutex::new(network_hosting),
        }))
    }

    /// Dispatch server management commands with camelCase top-level arguments
    /// and typed nested request payloads. Browser downloads are handled by the
    /// frontend after retrieving export data through these commands.
    pub async fn dispatch(self: &Arc<Self>, command: &str, args: Value) -> Result<Value, String> {
        if !args.is_object() {
            return Err("操作の引数を確認してください。".into());
        }
        match command {
            "hub_web_status" | "hub_web_enable" | "hub_web_disable" => {
                crate::web_admin::server::controls(self, command, args).await
            }
            "hub_snapshot" => hub_snapshot(self).await,
            "hub_discover" => output(
                hub_discover(
                    self,
                    argument(&args, "endpoint")?,
                    argument(&args, "providerProfile")?,
                )
                .await?,
            ),
            "hub_refresh" => output(hub_refresh(self).await?),
            "hub_start" => output(
                hub_start(
                    self,
                    argument(&args, "bind")?,
                    argument(&args, "token")?,
                    argument(&args, "enableGateway")?,
                )
                .await?,
            ),
            "hub_stop" => output(hub_stop(self).await?),
            "hub_network_snapshot" => network_admin::hub_network_snapshot(self).await,
            "hub_hosting_options" => hosting_options::snapshot().await,
            "hub_firewall_preview" => firewall::preview(self, argument(&args, "scope")?).await,
            "hub_firewall_apply" | "hub_firewall_remove" => {
                firewall::change(
                    self,
                    argument(&args, "scope")?,
                    argument(&args, "expectedFingerprint")?,
                    command == "hub_firewall_remove",
                )
                .await
            }
            "hub_network_renew_certificate" => {
                output(network_admin::hub_network_renew_certificate(self).await?)
            }
            "hub_network_diagnostics" => {
                output(network_admin::hub_network_diagnostics(self).await?)
            }
            "hub_network_start" => output(
                network_admin::hub_network_start(
                    self,
                    argument(&args, "bind")?,
                    argument(&args, "advertiseIp")?,
                )
                .await?,
            ),
            "hub_network_start_saved" => {
                network_admin::hub_network_start_saved(self).await?;
                network_admin::hub_network_snapshot(self).await
            }
            "hub_network_stop" => output(network_admin::hub_network_stop(self).await?),
            _ => {
                let admin = Arc::clone(self);
                let command = command.to_owned();
                // Keep disk flush, key signing and contended synchronous work
                // off the async executor so other server requests can progress.
                tokio::task::spawn_blocking(move || admin.dispatch_sync(&command, &args))
                    .await
                    .map_err(|_| "管理操作を完了できませんでした。".to_string())?
            }
        }
    }

    fn dispatch_sync(&self, command: &str, args: &Value) -> Result<Value, String> {
        use network_admin::*;
        match command {
            "hub_autostart_status" => autostart::snapshot(self),
            "hub_autostart_set" => autostart::set(
                self,
                argument(args, "enabled")?,
                argument(args, "expectedCommand")?,
            ),
            "hub_set_standard_model" => {
                let revision: String = argument(args, "revision")?;
                let model: Option<String> = argument(args, "modelId")?;
                edit_catalog(self, parse_revision(&revision)?, |catalog| {
                    if let Some(id) = &model {
                        if !catalog
                            .deployments
                            .iter()
                            .any(|d| &d.logical_model_id == id && d.enabled)
                        {
                            return Err("有効な登録先を持つモデルを選択してください。".into());
                        }
                    }
                    catalog.team_default_model_id = model;
                    Ok(())
                })?;
                output(())
            }
            "hub_register" => output(hub_register(self, argument(args, "request")?)?),
            "hub_edit" => output(hub_edit(self, argument(args, "request")?)?),
            "hub_history_request" => output(hub_history_request(
                self,
                argument(args, "deviceId")?,
                argument(args, "direction")?,
                argument(args, "offset")?,
                argument(args, "anchor")?,
                argument(args, "id")?,
            )?),
            "hub_history_result" => output(hub_history_result(self, argument(args, "requestId")?)?),
            "hub_network_settings" => output(hub_network_settings(
                self,
                argument(args, "expectedRevision")?,
                argument(args, "gatewayPort")?,
            )?),
            "hub_network_invite" => hub_network_invite(
                self,
                argument(args, "expectedRevision")?,
                argument(args, "label")?,
                argument(args, "groups")?,
            ),
            "hub_network_approve" => output(hub_network_approve(
                self,
                argument(args, "expectedRevision")?,
                argument(args, "requestId")?,
                argument(args, "label")?,
                argument(args, "groups")?,
            )?),
            "hub_network_admission" => output(hub_network_admission(
                self,
                argument(args, "expectedRevision")?,
                argument(args, "deviceId")?,
                argument(args, "stopped")?,
            )?),
            "hub_network_group" => output(hub_network_group(
                self,
                argument(args, "expectedRevision")?,
                argument(args, "id")?,
                argument(args, "label")?,
            )?),
            "hub_network_device" => output(hub_network_device(
                self,
                argument(args, "expectedRevision")?,
                argument(args, "deviceId")?,
                argument(args, "label")?,
                argument(args, "groups")?,
                argument(args, "revoked")?,
            )?),
            "hub_network_rule" => output(hub_network_rule(
                self,
                argument(args, "expectedRevision")?,
                argument(args, "rule")?,
            )?),
            "hub_network_delete_device" => output(hub_network_delete_device(
                self,
                argument(args, "expectedRevision")?,
                argument(args, "deviceId")?,
            )?),
            "hub_network_delete_rule" => output(hub_network_delete_rule(
                self,
                argument(args, "expectedRevision")?,
                argument(args, "id")?,
            )?),
            "hub_network_export" => hub_network_export(self, argument(args, "hubUrl")?),
            _ => Err("この管理操作には対応していません。".into()),
        }
    }

    /// Explicit server shutdown, independent of browser connections.
    pub async fn shutdown(&self) -> Result<(), String> {
        crate::web_admin::server::shutdown(self).await;
        hub_stop(self).await
    }
}

fn argument<T: serde::de::DeserializeOwned>(args: &Value, name: &str) -> Result<T, String> {
    serde_json::from_value(args.get(name).cloned().unwrap_or(Value::Null))
        .map_err(|_| format!("操作の引数 {name} を確認してください。"))
}

fn output<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|_| "管理操作の結果を取得できません。".into())
}

fn save(path: &Path, store: &CatalogStore) -> Result<(), String> {
    let parent = path.parent().ok_or("保存先がありません。")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "設定の一時ファイルを作成できません。")?;
    serde_json::to_writer_pretty(&mut temp, store).map_err(|_| "設定を書き込めません。")?;
    temp.write_all(b"\n")
        .map_err(|_| "設定を書き込めません。")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "設定をディスクに同期できません。")?;
    temp.persist(path)
        .map_err(|_| "設定を保存できません。元の設定は保持されています。")?;
    Ok(())
}
fn edit_catalog(
    admin: &Admin,
    revision: u64,
    edit: impl FnOnce(&mut Catalog) -> Result<(), String>,
) -> Result<(), String> {
    let mut owner = admin.core.lock().map_err(|_| "カタログを開けません。")?;
    if owner.catalog_store().revision != revision {
        return Err(
            "カタログが変更されました。最新の状態を確認して、もう一度操作してください。".into(),
        );
    }
    let mut next = owner.clone();
    let mut catalog = next.catalog_store().catalog.clone();
    edit(&mut catalog)?;
    next.replace_catalog(catalog, now_ms())
        .map_err(|error| error.to_string())?;
    save(&admin.path, next.catalog_store())?;
    *owner = next;
    Ok(())
}

fn parse_revision(value: &str) -> Result<u64, String> {
    let parsed = value.parse::<u64>().map_err(|_| "revision が無効です。")?;
    if parsed == 0 || parsed.to_string() != value {
        return Err("revision が無効です。".into());
    }
    Ok(parsed)
}
fn snapshot_value(core: &HubCore) -> Value {
    let mut store = serde_json::to_value(core.catalog_store()).expect("catalog serialization");
    store["revision"] = json!(core.catalog_store().revision.to_string());
    for (value, change) in store["changes"]
        .as_array_mut()
        .expect("changes array")
        .iter_mut()
        .zip(&core.catalog_store().changes)
    {
        value["revision"] = json!(change.revision.to_string());
    }
    let clients: Vec<Value> = core
        .client_rows(now_ms())
        .into_iter()
        .map(|client| {
            let mut value = serde_json::to_value(&client).expect("client serialization");
            for (value, context) in value["contexts"]
                .as_array_mut()
                .expect("contexts array")
                .iter_mut()
                .zip(&client.contexts)
            {
                value["reviewed_revision"] = json!(context.reviewed_revision.to_string());
            }
            value
        })
        .collect();
    json!({"store":store,"deployments":core.deployment_rows(now_ms()),"clients":clients})
}

async fn hub_snapshot(admin: &Admin) -> Result<Value, String> {
    let server = admin.server.lock().await;
    let gateway = if let Some(gateway) = server.as_ref().and_then(|server| server.gateway.as_ref())
    {
        match gateway.control.status().await {
            Ok(mut status) => {
                status["enabled"] = json!(true);
                status["ready"] = json!(status["accepting"] == true);
                status["uncertain"] = json!(false);
                status
            }
            Err(_) => {
                json!({"enabled":true,"ready":false,"uncertain":true,"reserved":0,"running":null,"responding":null,"draining":null,"quarantined_pools":[],"scope":"gateway_requests_only"})
            }
        }
    } else {
        let directory = admin.path.parent().ok_or("保存先がありません。")?;
        json!({"enabled":false,"ready":false,"uncertain":false,"stopping":gateway_process::execution_running(directory),"reserved":0,"running":0,"responding":0,"draining":0,"quarantined_pools":[],"scope":"gateway_requests_only"})
    };
    let core = admin.core.lock().map_err(|_| "カタログを開けません。")?;
    let mut value = snapshot_value(&core);
    value["server"] = json!({"running":server.as_ref().is_some_and(|server|!server.task.is_finished()),"bind":server.as_ref().map(|server|&server.bind)});
    value["gateway"] = gateway;
    Ok(value)
}

async fn hub_discover(
    admin: &Admin,
    endpoint: String,
    provider_profile: String,
) -> Result<Evidence, String> {
    let id = admin.discovery_generation.fetch_add(1, Ordering::SeqCst) + 1;
    *admin
        .evidence
        .lock()
        .map_err(|_| "モデル候補を更新できません。")? = None;
    let discovered = discovery::discover(&endpoint, &provider_profile)
        .await
        .map_err(|error| error.to_string())?;
    let evidence = Evidence {
        id: id.to_string(),
        discovery: discovered,
        observed_at_ms: now_ms(),
    };
    if admin.discovery_generation.load(Ordering::SeqCst) == id {
        *admin
            .evidence
            .lock()
            .map_err(|_| "モデル候補を更新できません。")? = Some(evidence.clone());
    }
    Ok(evidence)
}

#[derive(Deserialize)]
struct Register {
    revision: String,
    evidence_id: String,
    endpoint: String,
    provider_profile: String,
    model_id: String,
    label: String,
    logical_model_id: Option<String>,
    capacity_pool_id: Option<String>,
    capacity: u32,
    #[serde(default)]
    allow_tools: bool,
}
fn hub_register(admin: &Admin, request: Register) -> Result<(), String> {
    let evidence = admin
        .evidence
        .lock()
        .map_err(|_| "モデル候補を確認できません。")?
        .clone()
        .ok_or("モデル候補を取得してください。")?;
    if evidence.id != request.evidence_id
        || now_ms().saturating_sub(evidence.observed_at_ms) > 120_000
        || evidence.discovery.endpoint != discovery::normalize_endpoint(&request.endpoint)?
        || evidence.discovery.provider_profile != request.provider_profile
    {
        return Err(
            "ホスト情報が変わったか、候補の有効期限が切れました。モデルを再取得してください。"
                .into(),
        );
    }
    let mut model = evidence
        .discovery
        .models
        .iter()
        .find(|model| model.id == request.model_id)
        .ok_or("選択したモデルを再取得してください。")?
        .clone();
    if request.allow_tools {
        model.capabilities.insert("tools".into());
    }
    let deployment_id = ulid::Ulid::new().to_string();
    edit_catalog(admin, parse_revision(&request.revision)?, |catalog| {
        if catalog.deployments.iter().any(|row| {
            row.endpoint == evidence.discovery.endpoint
                && row.provider_model == model.id
                && row.provider_profile == request.provider_profile
        }) {
            return Err("このホストのモデルは登録済みです。".into());
        }
        let logical_id = if let Some(id) = request.logical_model_id.filter(|id| !id.is_empty()) {
            let existing = catalog
                .models
                .iter()
                .find(|m| m.id == id)
                .ok_or("選択した論理モデルは削除されています。")?;
            if existing.capabilities != model.capabilities {
                return Err(
                    "確認できた capability が異なります。新しい論理モデルとして登録してください。"
                        .into(),
                );
            }
            id
        } else {
            let id = ulid::Ulid::new().to_string();
            catalog.models.push(LogicalModel {
                id: id.clone(),
                label: request.label.trim().to_owned(),
                capabilities: model.capabilities.clone(),
            });
            id
        };
        let pool_id = if let Some(id) = request.capacity_pool_id.filter(|id| !id.is_empty()) {
            if !catalog.capacity_pools.iter().any(|pool| pool.id == id) {
                return Err("選択した容量プールは削除されています。".into());
            }
            id
        } else {
            let id = ulid::Ulid::new().to_string();
            catalog.capacity_pools.push(CapacityPool {
                id: id.clone(),
                label: evidence.discovery.endpoint.clone(),
                max_concurrent: request.capacity,
            });
            id
        };
        catalog.deployments.push(Deployment {
            id: deployment_id.clone(),
            logical_model_id: logical_id,
            capacity_pool_id: pool_id,
            label: request.label.trim().to_owned(),
            endpoint: evidence.discovery.endpoint.clone(),
            provider_model: model.id.clone(),
            provider_profile: request.provider_profile.clone(),
            enabled: true,
        });
        Ok(())
    })?;
    admin
        .core
        .lock()
        .map_err(|_| "状態を更新できません。")?
        .observe_present_deployment(&deployment_id, model.availability, now_ms())
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[derive(Deserialize)]
struct Edit {
    revision: String,
    deployment_id: String,
    label: String,
    capacity: u32,
    enabled: bool,
    remove: bool,
}
fn hub_edit(admin: &Admin, request: Edit) -> Result<(), String> {
    edit_catalog(admin, parse_revision(&request.revision)?, |catalog| {
        let row = catalog
            .deployments
            .iter()
            .find(|row| row.id == request.deployment_id)
            .ok_or("この配置は削除されています。")?
            .clone();
        if request.remove {
            catalog.deployments.retain(|d| d.id != row.id);
            if !catalog
                .deployments
                .iter()
                .any(|d| d.logical_model_id == row.logical_model_id)
            {
                catalog.models.retain(|m| m.id != row.logical_model_id);
                if catalog.team_default_model_id.as_deref() == Some(&row.logical_model_id) {
                    catalog.team_default_model_id = None;
                }
            }
            if !catalog
                .deployments
                .iter()
                .any(|d| d.capacity_pool_id == row.capacity_pool_id)
            {
                catalog
                    .capacity_pools
                    .retain(|p| p.id != row.capacity_pool_id);
            }
        } else {
            let target = catalog
                .deployments
                .iter_mut()
                .find(|d| d.id == row.id)
                .ok_or("配置がありません。")?;
            target.label = request.label.trim().to_owned();
            target.enabled = request.enabled;
            catalog
                .capacity_pools
                .iter_mut()
                .find(|p| p.id == row.capacity_pool_id)
                .ok_or("容量プールがありません。")?
                .max_concurrent = request.capacity;
        }
        Ok(())
    })
}

async fn hub_refresh(admin: &Admin) -> Result<(), String> {
    use std::sync::atomic::AtomicBool;
    struct Reset<'a>(&'a AtomicBool);
    impl Drop for Reset<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    if admin.refresh_running.swap(true, Ordering::SeqCst) {
        return Err("モデルの確認中です。".into());
    }
    let _reset = Reset(&admin.refresh_running);
    let mut groups: BTreeMap<(String, String), Vec<Deployment>> = BTreeMap::new();
    for row in &admin
        .core
        .lock()
        .map_err(|_| "カタログを開けません。")?
        .catalog_store()
        .catalog
        .deployments
    {
        groups
            .entry((row.endpoint.clone(), row.provider_profile.clone()))
            .or_default()
            .push(row.clone());
    }
    let mut queue = groups.into_iter();
    let mut pending = tokio::task::JoinSet::new();
    loop {
        while pending.len() < 4 {
            let Some(((endpoint, profile), rows)) = queue.next() else {
                break;
            };
            pending.spawn(async move { (rows, discovery::discover(&endpoint, &profile).await) });
        }
        let Some(result) = pending.join_next().await else {
            break;
        };
        let (rows, result) = result.map_err(|_| "モデル確認が中断されました。")?;
        let mut core = admin.core.lock().map_err(|_| "状態を更新できません。")?;
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
            let availability = match &result {
                Ok(discovered) => discovered
                    .models
                    .iter()
                    .find(|m| m.id == row.provider_model)
                    .map(|m| m.availability.clone())
                    .unwrap_or(Availability::ModelMissing),
                Err(error) => error.availability.clone(),
            };
            let present = result.as_ref().ok().is_some_and(|discovery| {
                discovery
                    .models
                    .iter()
                    .any(|model| model.id == row.provider_model)
            });
            (if present {
                core.observe_present_deployment(&row.id, availability, now_ms())
            } else {
                core.observe_deployment(&row.id, availability, now_ms())
            })
            .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

async fn hub_start(
    admin: &Admin,
    bind: String,
    token: String,
    enable_gateway: Option<bool>,
) -> Result<(), String> {
    let address: SocketAddr = bind
        .trim()
        .parse()
        .map_err(|_| "待受先を IP:port で指定してください。")?;
    if !address.ip().is_loopback() {
        return Err(
            "この開発版の待受は loopback のみです。127.0.0.1 または [::1] を指定してください。"
                .into(),
        );
    }
    if address.port() == 0
        || token.len() < 32
        || token.len() > 256
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
    {
        return Err(
            "ポートとトークンを確認してください。トークンは英数字・-._~ で 32〜256 文字です。"
                .into(),
        );
    }
    let mut server = admin.server.lock().await;
    if server
        .as_ref()
        .is_some_and(|server| !server.task.is_finished())
    {
        return Err("サーバーはすでに起動しています。".into());
    }
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|_| "待受を開始できません。IP とポートの利用状況を確認してください。")?;
    let gateway = if enable_gateway.unwrap_or(false) {
        Some(
            GatewayProcess::start(
                &std::env::current_exe().map_err(|_| "Gateway 実行ファイルを確認できません。")?,
                format!("http://{address}"),
                admin.path.parent().ok_or("保存先がありません。")?,
            )
            .await?,
        )
    } else {
        None
    };
    let router = service::router_with_gateway(
        admin.core.clone(),
        token,
        gateway.as_ref().map(|gateway| gateway.control.clone()),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await;
    });
    *server = Some(Server {
        bind: address.to_string(),
        task,
        stop,
        gateway,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_snapshot_and_input_preserve_full_revision_precision() {
        let mut store = CatalogStore::empty("0.1.0");
        store.revision = 9_007_199_254_740_993;
        let core = HubCore::new(store).unwrap();
        assert_eq!(
            snapshot_value(&core)["store"]["revision"],
            "9007199254740993"
        );
        assert_eq!(
            parse_revision("9007199254740993").unwrap(),
            9_007_199_254_740_993
        );
        for input in ["01", "0", "+1", "1.0", "18446744073709551616"] {
            assert!(parse_revision(input).is_err());
        }
    }

    #[tokio::test]
    async fn headless_dispatch_preserves_revision_arguments_and_persisted_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        let admin = Admin::open(path.clone()).unwrap();
        let catalog = admin.dispatch("hub_snapshot", json!({})).await.unwrap();
        assert!(catalog["store"]["revision"].is_string());
        assert_eq!(catalog["server"]["running"], false);
        let network = admin
            .dispatch("hub_network_snapshot", json!({}))
            .await
            .unwrap();
        let revision = network["revision"].clone();
        assert_eq!(
            admin
                .dispatch(
                    "hub_network_group",
                    json!({"expectedRevision": revision, "label": "開発端末"}),
                )
                .await
                .unwrap(),
            Value::Null,
        );
        let changed = admin
            .dispatch("hub_network_snapshot", json!({}))
            .await
            .unwrap();
        assert_eq!(changed["groups"].as_array().unwrap().len(), 1);
        assert_eq!(changed["groups"][0]["label"], "開発端末");
        assert_ne!(changed["revision"], revision);
        assert!(
            admin
                .dispatch(
                    "hub_network_group",
                    json!({"expectedRevision": revision, "label": "古い操作"}),
                )
                .await
                .is_err()
        );
        assert!(
            admin
                .dispatch(
                    "hub_network_settings",
                    json!({"expectedRevision": changed["revision"], "gatewayPort": "invalid"}),
                )
                .await
                .is_err()
        );
        assert!(admin.dispatch("unknown_command", json!({})).await.is_err());
        assert!(admin.dispatch("hub_snapshot", Value::Null).await.is_err());
        admin.shutdown().await.unwrap();
        drop(admin);
        let reopened = Admin::open(path).unwrap();
        let restored = reopened
            .dispatch("hub_network_snapshot", json!({}))
            .await
            .unwrap();
        assert_eq!(restored["groups"], changed["groups"]);
        assert_eq!(restored["revision"], changed["revision"]);
    }

    #[test]
    fn catalog_owner_lock_is_retained_until_admin_is_dropped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        let admin = Admin::open(path.clone()).unwrap();
        assert!(Admin::open(path.clone()).is_err());
        drop(admin);
        assert!(Admin::open(path).is_ok());
    }
}

async fn hub_stop(admin: &Admin) -> Result<(), String> {
    network_admin::stop_network(admin).await?;
    stop_server(admin).await
}

async fn stop_server(admin: &Admin) -> Result<(), String> {
    let mut server = admin.server.lock().await;
    let mut confirmed = true;
    if let Some(mut current) = server.take() {
        if let Some(gateway) = current.gateway.take() {
            confirmed = gateway.stop().await;
        }
        let _ = current.stop.send(());
        if tokio::time::timeout(std::time::Duration::from_secs(2), &mut current.task)
            .await
            .is_err()
        {
            current.task.abort();
        }
    }
    let mut core = admin
        .core
        .lock()
        .map_err(|_| "接続状態を更新できません。")?;
    for row in core.client_rows(now_ms()) {
        let _ = core.remove_client(&row.id);
    }
    if confirmed {
        Ok(())
    } else {
        Err("新しい受付を停止しましたが、Hub の中継プロセスを終了できませんでした。Hub を再起動してください。モデル提供側の稼働状態とは別の問題です。".into())
    }
}

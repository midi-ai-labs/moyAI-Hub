use fs2::FileExt;
use moyai_hub::{
    catalog::{CapacityPool, Catalog, CatalogStore, Deployment, LogicalModel},
    core::HubCore,
    discovery::{self, Discovery},
    gateway_process::{self, GatewayProcess},
    protocol::Availability,
    service::{self, SharedCore, now_ms},
};
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
use tauri::{Manager, State};
mod network_admin;

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
struct Admin {
    core: SharedCore,
    path: PathBuf,
    _lock: File,
    server: tokio::sync::Mutex<Option<Server>>,
    evidence: Mutex<Option<Evidence>>,
    discovery_generation: AtomicU64,
    refresh_running: std::sync::atomic::AtomicBool,
    network: moyai_hub::network::SharedNetwork,
    network_server: tokio::sync::Mutex<Option<network_admin::NetworkServer>>,
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

#[tauri::command]
async fn hub_snapshot(admin: State<'_, Admin>) -> Result<Value, String> {
    let server = admin.server.lock().await;
    let gateway = if let Some(gateway) = server.as_ref().and_then(|server| server.gateway.as_ref())
    {
        match gateway.control.status().await {
            Ok(mut status) => {
                status["enabled"] = json!(true);
                status["ready"] = json!(
                    status["accepting"] == true
                        && status["quarantined_pools"]
                            .as_array()
                            .is_some_and(Vec::is_empty)
                );
                status["uncertain"] = json!(
                    !status["quarantined_pools"]
                        .as_array()
                        .is_some_and(Vec::is_empty)
                );
                status
            }
            Err(_) => {
                json!({"enabled":true,"ready":false,"uncertain":true,"reserved":0,"running":null,"responding":null,"draining":null,"quarantined_pools":[],"scope":"gateway_requests_only"})
            }
        }
    } else {
        json!({"enabled":false,"ready":false,"uncertain":gateway_process::uncertain(admin.path.parent().ok_or("保存先がありません。")?),"reserved":0,"running":0,"responding":0,"draining":0,"quarantined_pools":[],"scope":"gateway_requests_only"})
    };
    let core = admin.core.lock().map_err(|_| "カタログを開けません。")?;
    let mut value = snapshot_value(&core);
    value["server"] = json!({"running":server.as_ref().is_some_and(|server|!server.task.is_finished()),"bind":server.as_ref().map(|server|&server.bind)});
    value["gateway"] = gateway;
    Ok(value)
}

#[tauri::command]
async fn hub_discover(
    admin: State<'_, Admin>,
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
#[tauri::command]
fn hub_register(admin: State<'_, Admin>, request: Register) -> Result<(), String> {
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
    edit_catalog(&admin, parse_revision(&request.revision)?, |catalog| {
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
#[tauri::command]
fn hub_edit(admin: State<'_, Admin>, request: Edit) -> Result<(), String> {
    edit_catalog(&admin, parse_revision(&request.revision)?, |catalog| {
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

#[tauri::command]
async fn hub_refresh(admin: State<'_, Admin>) -> Result<(), String> {
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

#[tauri::command]
async fn hub_start(
    admin: State<'_, Admin>,
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
}

#[tauri::command]
async fn hub_stop(admin: State<'_, Admin>) -> Result<(), String> {
    network_admin::stop_network(&admin).await?;
    stop_server(&admin).await
}

async fn stop_server(admin: &Admin) -> Result<(), String> {
    let mut server = admin.server.lock().await;
    if let Some(mut current) = server.take() {
        if let Some(gateway) = current.gateway.take() {
            let _ = gateway.stop().await;
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
    Ok(())
}

#[tauri::command]
async fn hub_acknowledge_gateway(
    admin: State<'_, Admin>,
    provider_quiescent: bool,
) -> Result<(), String> {
    if !provider_quiescent {
        return Err("provider の実行が終了していることを確認してください。".into());
    }
    let server = admin.server.lock().await;
    if server.is_some() {
        return Err("Hub サーバーを停止してから確認してください。".into());
    }
    gateway_process::acknowledge_quiescent(admin.path.parent().ok_or("保存先がありません。")?)
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let directory = std::env::var_os("MOYAI_HUB_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?);
            std::fs::create_dir_all(&directory)?;
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(directory.join("hub.lock"))?;
            lock.try_lock_exclusive().map_err(|_| {
                std::io::Error::other("このデータフォルダーの moyAI Hub は起動済みです。")
            })?;
            let path = directory.join("catalog.json");
            let mut store = if path.exists() {
                let metadata = std::fs::metadata(&path)?;
                if metadata.len() > 4 * 1024 * 1024 {
                    return Err("カタログがサイズ上限を超えています。".into());
                }
                serde_json::from_slice::<CatalogStore>(&std::fs::read(&path)?)?
            } else {
                CatalogStore::empty(env!("CARGO_PKG_VERSION"))
            };
            store.reconcile_software(env!("CARGO_PKG_VERSION"), now_ms())?;
            save(&path, &store).map_err(std::io::Error::other)?;
            let network = moyai_hub::network::DeviceNetwork::open(
                &directory.join("device-network.json"),
                &store.hub_id,
            )?;
            app.manage(Admin {
                core: Arc::new(Mutex::new(HubCore::new(store)?)),
                path,
                _lock: lock,
                server: tokio::sync::Mutex::new(None),
                evidence: Mutex::new(None),
                discovery_generation: AtomicU64::new(0),
                refresh_running: std::sync::atomic::AtomicBool::new(false),
                network: Arc::new(Mutex::new(network)),
                network_server: tokio::sync::Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            hub_snapshot,
            hub_discover,
            hub_register,
            hub_edit,
            hub_refresh,
            hub_start,
            hub_stop,
            hub_acknowledge_gateway,
            network_admin::hub_network_snapshot,
            network_admin::hub_network_invite,
            network_admin::hub_network_group,
            network_admin::hub_network_device,
            network_admin::hub_network_rule,
            network_admin::hub_network_delete_rule,
            network_admin::hub_network_start,
            network_admin::hub_network_stop,
            network_admin::hub_network_export,
            network_admin::hub_network_save_config
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    // A normal window close follows the same bounded gateway drain as
                    // the Stop command. Only confirmed child settlement clears its marker.
                    let _ = network_admin::stop_network(&app.state::<Admin>()).await;
                    let _ = stop_server(&app.state::<Admin>()).await;
                    app.exit(0);
                });
            }
        })
        .run(tauri::generate_context!())
        .expect("moyAI Hub を起動できませんでした。");
}

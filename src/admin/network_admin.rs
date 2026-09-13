use super::*;
use crate::network::{self, DeviceNetwork, Effect, NetworkError, Rule, Subject, Usage};
use std::collections::BTreeSet;

pub(super) struct NetworkServer {
    bind: String,
    task: tokio::task::JoinHandle<()>,
    stop: tokio::sync::oneshot::Sender<()>,
    runtime: Arc<network::server_runtime::NetworkServerRuntime>,
    renewal: tokio::task::JoinHandle<()>,
}
fn message(error: NetworkError) -> String {
    match error {
        NetworkError::StaleRevision => {
            "端末設定が変更されました。最新の状態を確認してもう一度操作してください。"
        }
        NetworkError::Revoked => "失効した端末は元に戻せません。新しい鍵での参加申請が必要です。",
        NetworkError::Full => "登録または許可の上限に達しています。",
        NetworkError::InvalidRequest => "入力内容と対象を確認してください。",
        NetworkError::DeviceNameConflict => "同じ端末名が登録または申請されています。前後の空白・大文字小文字を変えるだけでは区別できません。別の端末名を入力してください。",
        NetworkError::DeviceWorkUnsettled => "この端末に関係する受入済みタスクの終了を確認できないため削除できません。Desktopで停止または完了を確認し、最新情報を取得してから再試行してください。オフラインだけでは完了と判断しません。",
        _ => "端末設定を処理できませんでした。元の設定は保持されています。",
    }
    .into()
}
fn owner(admin: &Admin) -> Result<std::sync::MutexGuard<'_, DeviceNetwork>, String> {
    admin
        .network
        .lock()
        .map_err(|_| "端末設定を開けません。".into())
}

fn history_message(error: NetworkError) -> String {
    match error {
        NetworkError::Unavailable => {
            "端末の接続を確認できません。保存済みの取得結果は引き続き確認できます。"
        }
        NetworkError::Full => {
            "この端末の履歴を取得中です。完了または期限切れを待ってから再取得してください。"
        }
        NetworkError::Revoked | NetworkError::Unauthorized => {
            "端末の有効な参加資格を確認できません。接続端末の状態を確認してください。"
        }
        _ => "履歴の対象または取得結果を確認できません。端末を選んで履歴を再取得してください。",
    }
    .into()
}

pub(super) fn hub_history_request(
    admin: &Admin,
    device_id: String,
    direction: network::history::HistoryDirection,
    offset: usize,
    anchor: Option<String>,
    id: Option<String>,
) -> Result<network::history::HistorySnapshot, String> {
    owner(admin)?
        .request_history(
            &device_id,
            network::history::HistoryQuery {
                direction,
                offset,
                anchor,
                id,
            },
            now_ms(),
        )
        .map_err(history_message)
}

pub(super) fn hub_history_result(
    admin: &Admin,
    request_id: String,
) -> Result<network::history::HistorySnapshot, String> {
    owner(admin)?
        .history_result(&request_id, now_ms())
        .map_err(history_message)
}

pub(super) async fn hub_network_snapshot(admin: &Admin) -> Result<Value, String> {
    let server = admin.network_server.lock().await;
    let mut value = owner(admin)?.snapshot(now_ms());
    let certificate = server
        .as_ref()
        .map(|server| server.runtime.certificate())
        .transpose()
        .map_err(message)?;
    value["server"] = json!({"running":server.as_ref().is_some_and(|s|!s.task.is_finished()),"bind":server.as_ref().map(|s|&s.bind),
        "gateway_bind":server.as_ref().and_then(|s|s.runtime.gateway_endpoint()),"certificate":certificate});
    value["hosting"] = admin
        .network_hosting
        .lock()
        .map_err(|_| "ネットワーク起動設定を開けません。")?
        .snapshot();
    Ok(value)
}
pub(super) fn hub_network_settings(
    admin: &Admin,
    expected_revision: String,
    gateway_port: u16,
) -> Result<(), String> {
    owner(admin)?
        .set_gateway_port(&expected_revision, gateway_port)
        .map_err(message)
}
pub(super) async fn hub_network_renew_certificate(admin: &Admin) -> Result<(), String> {
    let server = admin.network_server.lock().await;
    let server = server
        .as_ref()
        .filter(|server| !server.task.is_finished())
        .ok_or("端末ネットワークを開始してください。")?;
    server.runtime.renew(true, now_ms()).await.map_err(|_| {
        "証明書を更新できませんでした。現在の証明書の期限とGatewayの稼働状態を確認してください。"
    })?;
    Ok(())
}
pub(super) async fn hub_network_diagnostics(
    admin: &Admin,
) -> Result<network::server_runtime::NetworkDiagnostics, String> {
    let server = admin.network_server.lock().await;
    let server = server
        .as_ref()
        .filter(|server| !server.task.is_finished())
        .ok_or("端末ネットワークを開始してください。")?;
    server.runtime.diagnostics().await.map_err(message)
}
pub(super) fn hub_network_invite(
    admin: &Admin,
    expected_revision: String,
    label: String,
    groups: BTreeSet<String>,
) -> Result<Value, String> {
    owner(admin)?
        .invite(&expected_revision, label, groups, now_ms())
        .map_err(message)
}
pub(super) fn hub_network_approve(
    admin: &Admin,
    expected_revision: String,
    request_id: String,
    label: String,
    groups: BTreeSet<String>,
) -> Result<(), String> {
    owner(admin)?
        .approve_join(&expected_revision, &request_id, label, groups, now_ms())
        .map_err(message)
}
pub(super) fn hub_network_admission(
    admin: &Admin,
    expected_revision: String,
    device_id: String,
    stopped: bool,
) -> Result<(), String> {
    owner(admin)?
        .set_admission(&expected_revision, &device_id, stopped)
        .map_err(message)
}
pub(super) fn hub_network_group(
    admin: &Admin,
    expected_revision: String,
    id: Option<String>,
    label: String,
) -> Result<(), String> {
    owner(admin)?
        .set_group(&expected_revision, id, label)
        .map_err(message)
}
pub(super) fn hub_network_device(
    admin: &Admin,
    expected_revision: String,
    device_id: String,
    label: String,
    groups: BTreeSet<String>,
    revoked: bool,
) -> Result<(), String> {
    owner(admin)?
        .set_device(&expected_revision, &device_id, label, groups, revoked)
        .map_err(message)
}
pub(super) fn hub_network_delete_device(
    admin: &Admin,
    expected_revision: String,
    device_id: String,
) -> Result<(), String> {
    owner(admin)?
        .delete_device(&expected_revision, &device_id, now_ms())
        .map_err(message)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuleDraft {
    id: Option<String>,
    from: Subject,
    to: Subject,
    usage: Usage,
    effect: Effect,
    max_depth: u8,
    profile_id: Option<String>,
}
pub(super) fn hub_network_rule(
    admin: &Admin,
    expected_revision: String,
    rule: RuleDraft,
) -> Result<(), String> {
    owner(admin)?
        .set_rule(
            &expected_revision,
            Rule {
                id: rule.id.unwrap_or_default(),
                from: rule.from,
                to: rule.to,
                usage: rule.usage,
                effect: rule.effect,
                max_depth: rule.max_depth,
                profile_id: rule.profile_id,
            },
        )
        .map_err(message)
}
pub(super) fn hub_network_delete_rule(
    admin: &Admin,
    expected_revision: String,
    id: String,
) -> Result<(), String> {
    owner(admin)?
        .delete_rule(&expected_revision, &id)
        .map_err(message)
}

pub(super) async fn hub_network_start(
    admin: &Arc<Admin>,
    bind: String,
    advertise_ip: String,
) -> Result<(), String> {
    let settings = network_hosting::Settings::new(&bind, &advertise_ip)?;
    let mut network_server = admin.network_server.lock().await;
    if network_server
        .as_ref()
        .is_some_and(|s| !s.task.is_finished())
    {
        return Err("端末ネットワークはすでに起動しています。".into());
    }
    start_network(admin, &mut network_server, settings, true).await
}

pub(super) async fn hub_network_start_saved(admin: &Arc<Admin>) -> Result<(), String> {
    // Keep the existing lifetime owner locked through lookup and start. Parallel
    // launch clicks see the running instance rather than replace its listeners.
    let mut network_server = admin.network_server.lock().await;
    if network_server
        .as_ref()
        .is_some_and(|server| !server.task.is_finished())
    {
        return Ok(());
    }
    let settings = admin
        .network_hosting
        .lock()
        .map_err(|_| "ネットワーク起動設定を開けません。")?
        .configured()?;
    start_network(admin, &mut network_server, settings, false).await
}

async fn start_network(
    admin: &Arc<Admin>,
    network_server: &mut Option<NetworkServer>,
    settings: network_hosting::Settings,
    save_on_start: bool,
) -> Result<(), String> {
    let address = settings.bind;
    let public_ip = settings.advertise_ip;
    let mut server = admin.server.lock().await;
    if server.is_some() {
        return Err("モデルサーバーを停止してから端末ネットワークを開始してください。必要な Gateway も一緒に起動します。".into());
    }
    let (certificate, key, ca, gateway_port) = {
        let mut network = owner(admin)?;
        network.reset_runtime();
        let (cert, key) = network
            .server_certificate(public_ip, now_ms())
            .map_err(message)?;
        (
            cert,
            key,
            network.ca_certificate().to_string(),
            network.gateway_port(),
        )
    };
    let acceptor = network::tls::acceptor(&certificate, &key, &ca, true).map_err(message)?;
    let identity = network::certificate::CertificateIdentity::new(
        network::certificate::PreparedIdentity::new(
            &certificate,
            &key,
            &ca,
            public_ip,
            true,
            now_ms(),
        )
        .map_err(message)?,
    );
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|_| "TLS の待受を開始できません。IP とポートを確認してください。")?;
    let local = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "内部管理の待受を開始できません。")?;
    let local_address = local
        .local_addr()
        .map_err(|_| "内部管理の待受を確認できません。")?;
    let gateway = GatewayProcess::start_with_network(
        &std::env::current_exe().map_err(|_| "実行ファイルを確認できません。")?,
        format!("http://{local_address}"),
        admin.path.parent().ok_or("保存先がありません。")?,
        Some(gateway_process::GatewayNetwork {
            bind: SocketAddr::new(address.ip(), gateway_port),
            advertise_ip: public_ip,
            certificate_pem: certificate,
            private_key_pem: key,
            ca_certificate_pem: ca,
        }),
    )
    .await?;
    let gateway_control = gateway.control.clone();
    let metadata_endpoint = format!("https://{}", SocketAddr::new(public_ip, address.port()));
    let runtime = network::server_runtime::NetworkServerRuntime::new(
        admin.network.clone(),
        identity.clone(),
        gateway_control.clone(),
        metadata_endpoint.clone(),
        public_ip,
    );
    // All fallible startup preparation is complete, including bound listeners
    // and the ready Gateway. Publish the config before spawning their tasks;
    // a failed atomic replacement leaves the old config and no new service.
    if save_on_start {
        let saved = admin
            .network_hosting
            .lock()
            .map_err(|_| "ネットワーク起動設定を開けません。".to_string())
            .and_then(|mut hosting| hosting.save(settings));
        if let Err(error) = saved {
            let stopped = gateway.stop().await;
            drop(listener);
            drop(local);
            return Err(if stopped {
                format!("{error} ネットワークの起動を取り消しました。")
            } else {
                format!(
                    "{error} Hubの中継プロセスの終了を確認できません。Hubを再起動してください。"
                )
            });
        }
    }
    let local_router = service::router_with_registered_gateway(
        admin.core.clone(),
        format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new()),
        gateway_control.clone(),
        admin.network.clone(),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = axum::serve(
            local,
            local_router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await;
    });
    *server = Some(Server {
        bind: local_address.to_string(),
        task,
        stop,
        gateway: Some(gateway),
    });
    let router = service::network_router_with_diagnostics(
        admin.core.clone(),
        admin.network.clone(),
        Some(gateway_control),
        runtime.clone(),
    )
    .merge(crate::shared_work::api::router(
        admin.shared_work.clone(),
        admin.network.clone(),
    ));
    let listener = network::tls::NetworkListener::new(listener, acceptor)
        .with_identity(identity)
        .with_authority(SocketAddr::new(public_ip, address.port()).to_string());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<network::tls::TlsPeer>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await;
    });
    let renewal_owner = runtime.clone();
    let renewal = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            let _ = renewal_owner.renew(false, now_ms()).await;
        }
    });
    *network_server = Some(NetworkServer {
        bind: metadata_endpoint,
        task,
        stop,
        runtime,
        renewal,
    });
    Ok(())
}
pub(super) async fn stop_network(admin: &Admin) -> Result<(), String> {
    let mut owner = admin.network_server.lock().await;
    if let Some(mut server) = owner.take() {
        server.renewal.abort();
        let _ = server.renewal.await;
        // Close admission before canceling this Hub's model relay connections.
        // Provider processes and accepted remote jobs have their own lifecycle.
        let _ = server.stop.send(());
        admin
            .network
            .lock()
            .map_err(|_| "端末設定を開けません。")?
            .reset_runtime();
        let gateway_result = super::stop_server(admin).await;
        if tokio::time::timeout(std::time::Duration::from_secs(2), &mut server.task)
            .await
            .is_err()
        {
            server.task.abort();
        }
        gateway_result?;
    }
    Ok(())
}
pub(super) async fn hub_network_stop(admin: &Admin) -> Result<(), String> {
    stop_network(admin).await
}

fn export_value(admin: &Admin, hub_url: &str) -> Result<Value, String> {
    let url = reqwest::Url::parse(hub_url).map_err(|_| "Hub の HTTPS URL を指定してください。")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("Hub の HTTPS URL を指定してください。".into());
    }
    let network = owner(admin)?;
    let ca = network.ca_certificate();
    let toml = format!(
        "[device_network]\nhub_url = {}\nca_certificate_pem = {}\n",
        serde_json::to_string(hub_url.trim_end_matches('/'))
            .map_err(|_| "URL を書き出せません。")?,
        serde_json::to_string(ca).map_err(|_| "公開証明書を書き出せません。")?
    );
    Ok(json!({"toml":toml,"ca_certificate_pem":ca}))
}
pub(super) fn hub_network_export(admin: &Admin, hub_url: String) -> Result<Value, String> {
    export_value(admin, &hub_url)
}

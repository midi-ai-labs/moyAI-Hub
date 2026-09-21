//! Independent browser listeners and their one persisted configuration/lifecycle owner.
use super::{WebPolicy, router};
use crate::{
    admin::Admin,
    network::tls::{NetworkListener, TlsPeer},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::Write,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebSettings {
    #[serde(default)]
    pub local_port: u16,
    #[serde(default)]
    pub remote: Remote,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Remote {
    #[default]
    Off,
    Http {
        bind: String,
        public_origin: String,
    },
    Https {
        bind: String,
        public_origin: String,
        certificate_path: PathBuf,
        private_key_path: PathBuf,
    },
    Proxy {
        bind: String,
        public_origin: String,
    },
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    schema: u32,
    settings: WebSettings,
}

fn canonical_origin(value: &str, scheme: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "公開先のURLを確認してください。")?;
    if value.len() > 2048
        || url.scheme() != scheme
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.port() == Some(0)
    {
        return Err(format!(
            "公開先はパスを含まない{scheme} URLで指定してください。"
        ));
    }
    Ok(url.origin().ascii_serialization())
}
fn remote_address(bind: &str, proxy: bool) -> Result<SocketAddr, String> {
    let address: SocketAddr = bind
        .parse()
        .map_err(|_| "Web管理の待受先を IP:port で指定してください。")?;
    if address.port() == 0
        || address.ip().is_multicast()
        || (proxy && address.ip() != Ipv4Addr::LOCALHOST)
    {
        return Err(
            "Web管理の待受ポートは1以上、プロキシの待受IPは127.0.0.1を指定してください。".into(),
        );
    }
    Ok(address)
}
impl WebSettings {
    fn validated(mut self) -> Result<Self, String> {
        match &mut self.remote {
            Remote::Off => {}
            Remote::Http {
                bind,
                public_origin,
            } => {
                *bind = remote_address(bind, false)?.to_string();
                *public_origin = canonical_origin(public_origin, "http")?;
            }
            Remote::Https {
                bind,
                public_origin,
                certificate_path,
                private_key_path,
            } => {
                *bind = remote_address(bind, false)?.to_string();
                *public_origin = canonical_origin(public_origin, "https")?;
                if !certificate_path.is_absolute() || !private_key_path.is_absolute() {
                    return Err("証明書と秘密鍵は絶対パスで指定してください。".into());
                }
            }
            Remote::Proxy {
                bind,
                public_origin,
            } => {
                *bind = remote_address(bind, true)?.to_string();
                *public_origin = canonical_origin(public_origin, "https")?;
            }
        }
        Ok(self)
    }
}

pub(crate) struct Manager {
    path: PathBuf,
    settings: WebSettings,
    settings_error: Option<String>,
    running: Option<Running>,
}
struct Running {
    local_url: String,
    remote_url: Option<String>,
    endpoints: Vec<Endpoint>,
}
struct Endpoint {
    task: JoinHandle<()>,
    stop: Option<oneshot::Sender<()>>,
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Running {
    fn alive(&self) -> bool {
        self.endpoints
            .iter()
            .all(|endpoint| !endpoint.task.is_finished())
    }
    async fn stop(mut self) {
        for endpoint in &mut self.endpoints {
            if let Some(stop) = endpoint.stop.take() {
                let _ = stop.send(());
            }
        }
        for endpoint in &mut self.endpoints {
            if tokio::time::timeout(Duration::from_secs(2), &mut endpoint.task)
                .await
                .is_err()
            {
                endpoint.task.abort();
                let _ = (&mut endpoint.task).await;
            }
        }
    }
}
impl Manager {
    fn load_settings(path: &Path) -> Result<WebSettings, String> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(WebSettings::default());
            }
            Err(_) => return Err("Web管理設定を開けません。".into()),
        };
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(file, 32 * 1024 + 1), &mut bytes)
            .map_err(|_| "Web管理設定を読み取れません。")?;
        if bytes.len() > 32 * 1024 {
            return Err("Web管理設定がサイズ上限を超えています。".into());
        }
        let saved: Saved =
            serde_json::from_slice(&bytes).map_err(|_| "Web管理設定の形式を確認してください。")?;
        if saved.schema != 1 {
            return Err("Web管理設定のバージョンに対応していません。".into());
        }
        saved.settings.validated()
    }
    pub(crate) fn open(path: PathBuf) -> Self {
        let (settings, settings_error) = match Self::load_settings(&path) {
            Ok(settings) => (settings, None),
            Err(error) => (WebSettings::default(), Some(error)),
        };
        Self {
            path,
            settings,
            settings_error,
            running: None,
        }
    }
    async fn stop(&mut self, admin: &Admin) {
        // Invalidate access before waiting for HTTP connection teardown. Accepted
        // Admin operations keep their own operation/slot owner until completion.
        admin.web_access.disable();
        if let Some(running) = self.running.take() {
            running.stop().await;
        }
    }
    fn status(&self, admin: &Admin) -> Value {
        let running = self.running.as_ref().filter(|running| running.alive());
        json!({ "enabled": running.is_some() && admin.web_access.enabled(), "epoch": admin.web_access.epoch(),
            "local_url": running.map(|running| &running.local_url), "remote_url": running.and_then(|running| running.remote_url.as_ref()), "settings": self.settings, "settings_error": self.settings_error })
    }
    async fn enable(&mut self, admin: &Arc<Admin>, settings: WebSettings) -> Result<(), String> {
        if self.running.as_ref().is_some_and(Running::alive) {
            return Err("Web管理は既に公開中です。設定変更は停止後に行ってください。".into());
        }
        self.stop(admin).await;
        if !super::assets_ready() {
            return Err(
                "Web画面が含まれていません。フロントエンドを含むHubをビルドしてください。".into(),
            );
        }
        let settings = settings.validated()?;
        let prepared = settings.clone();
        let acceptor = tokio::task::spawn_blocking(move || match prepared.remote {
            Remote::Https {
                certificate_path,
                private_key_path,
                public_origin,
                ..
            } => https_acceptor(&certificate_path, &private_key_path, &public_origin).map(Some),
            _ => Ok(None),
        })
        .await
        .map_err(|_| "Web管理の証明書を確認できません。")??;
        // Bind both sockets before issuing any management access. Failure drops
        // all prepared sockets and keeps the last successfully saved settings.
        let local = TcpListener::bind((Ipv4Addr::LOCALHOST, settings.local_port))
            .await
            .map_err(|_| "ローカルWeb管理の待受を開始できません。ポートを確認してください。")?;
        let local_origin = format!(
            "http://{}",
            local
                .local_addr()
                .map_err(|_| "ローカルWeb管理の待受を確認できません。")?
        );
        let remote = match &settings.remote {
            Remote::Off => None,
            Remote::Http {
                bind,
                public_origin,
            }
            | Remote::Https {
                bind,
                public_origin,
                ..
            }
            | Remote::Proxy {
                bind,
                public_origin,
            } => {
                let listener = TcpListener::bind(bind).await.map_err(
                    |_| "リモートWeb管理の待受を開始できません。IPとポートを確認してください。",
                )?;
                Some((listener, public_origin.clone()))
            }
        };
        admin.web_access.enable()?;
        let access_id = admin
            .web_access
            .epoch()
            .ok_or("Web管理の起動状態を確認できません。")?;
        let policies = (|| {
            let local = WebPolicy::new(&local_origin, access_id.clone())?.local_management();
            let remote = remote
                .as_ref()
                .map(|(_, origin)| {
                    let policy = WebPolicy::new(origin, access_id.clone())?;
                    Ok::<_, String>(if matches!(settings.remote, Remote::Proxy { .. }) {
                        policy.loopback_only()
                    } else {
                        policy
                    })
                })
                .transpose()?;
            Ok::<_, String>((local, remote))
        })();
        let (local_policy, remote_policy) = match policies {
            Ok(policies) => policies,
            Err(error) => {
                admin.web_access.disable();
                return Err(error);
            }
        };
        admin.web_access.set_remote_origin(
            remote_policy
                .as_ref()
                .filter(|policy| policy.secure)
                .map(|policy| policy.origin.clone()),
        )?;
        let local_endpoint = serve_http(local, router(Arc::downgrade(admin), local_policy));
        let mut running = Running {
            local_url: format!("{local_origin}/admin/"),
            remote_url: remote
                .as_ref()
                .map(|(_, origin)| format!("{origin}/admin/")),
            endpoints: vec![local_endpoint],
        };
        if let Some((listener, _)) = remote {
            let app = router(
                Arc::downgrade(admin),
                remote_policy.expect("remote listener has its policy"),
            );
            running.endpoints.push(match acceptor {
                Some(acceptor) => serve_https(listener, acceptor, app),
                None => serve_http(listener, app),
            });
        }
        let destination = self.path.clone();
        let saved_settings = settings.clone();
        let saved = tokio::task::spawn_blocking(move || save(&destination, saved_settings))
            .await
            .map_err(|_| "Web管理設定を保存できません。".to_string())
            .and_then(|result| result);
        if let Err(error) = saved {
            admin.web_access.disable();
            running.stop().await;
            return Err(error);
        }
        self.settings = settings;
        self.settings_error = None;
        self.running = Some(running);
        Ok(())
    }
}
fn read_pem(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(
        |_| "証明書または秘密鍵を開けません。絶対パスと読み取り権限を確認してください。",
    )?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, 1024 * 1024 + 1), &mut bytes)
        .map_err(|_| "証明書または秘密鍵を読み取れません。")?;
    if bytes.len() > 1024 * 1024 {
        return Err("証明書または秘密鍵が1MiBの上限を超えています。".into());
    }
    String::from_utf8(bytes).map_err(|_| "証明書と秘密鍵をPEM形式で指定してください。".into())
}
fn https_acceptor(
    certificate_path: &Path,
    private_key_path: &Path,
    origin: &str,
) -> Result<TlsAcceptor, String> {
    let certificate = read_pem(certificate_path)?;
    let key = read_pem(private_key_path)?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certs = CertificateDer::pem_slice_iter(certificate.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "証明書のPEM形式を確認してください。")?;
    let leaf = certs.first().ok_or("証明書がありません。")?;
    use x509_parser::prelude::FromDer;
    let (_, parsed) = x509_parser::certificate::X509Certificate::from_der(leaf.as_ref())
        .map_err(|_| "証明書の形式を確認してください。")?;
    if !parsed.validity().is_valid() {
        return Err("Web管理の証明書が有効期間外です。新しい証明書を指定してください。".into());
    }
    let url = reqwest::Url::parse(origin).map_err(|_| "公開先のHTTPS URLを確認してください。")?;
    let name = rustls::pki_types::ServerName::try_from(
        url.host_str()
            .ok_or("公開先のホスト名を確認してください。")?
            .trim_matches(['[', ']'])
            .to_owned(),
    )
    .map_err(|_| "公開先のホスト名を確認してください。")?;
    let parsed = rustls::server::ParsedCertificate::try_from(leaf)
        .map_err(|_| "証明書の形式を確認してください。")?;
    // Check identity and validity without inventing a trust-root authority: the
    // browser/operator still owns chain trust for this external certificate.
    rustls::client::verify_server_name(&parsed, &name)
        .map_err(|_| "証明書のSANが公開先のホスト名またはIPと一致しません。")?;
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes())
        .map_err(|_| "秘密鍵のPEM形式を確認してください。")?;
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|_| "証明書と秘密鍵の組み合わせを確認してください。")?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}
fn save(path: &Path, settings: WebSettings) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Web管理設定の保存先を確認してください。")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "Web管理設定の一時ファイルを作成できません。")?;
    serde_json::to_writer_pretty(
        &mut temp,
        &Saved {
            schema: 1,
            settings,
        },
    )
    .map_err(|_| "Web管理設定を書き込めません。")?;
    temp.write_all(b"\n")
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "Web管理設定をディスクに同期できません。")?;
    temp.persist(path)
        .map_err(|_| "Web管理設定を保存できません。以前の設定を保持しました。")?;
    Ok(())
}
fn serve_http(listener: TcpListener, app: axum::Router) -> Endpoint {
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await;
    });
    Endpoint {
        task,
        stop: Some(stop),
    }
}
fn serve_https(listener: TcpListener, acceptor: TlsAcceptor, app: axum::Router) -> Endpoint {
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let listener = NetworkListener::new(listener, acceptor);
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<TlsPeer>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await;
    });
    Endpoint {
        task,
        stop: Some(stop),
    }
}
pub(crate) async fn controls(
    admin: &Arc<Admin>,
    command: &str,
    args: Value,
) -> Result<Value, String> {
    let admin = admin.clone();
    let command = command.to_owned();
    // Client cancellation must not abandon sockets or release the start/stop
    // owner during a bind, durable save, or graceful stop.
    tokio::spawn(async move {
        let mut manager = admin.web_server.lock().await;
        if manager
            .running
            .as_ref()
            .is_some_and(|running| !running.alive())
        {
            manager.stop(&admin).await;
        }
        match command.as_str() {
            "hub_web_open" => {
                if !args.as_object().is_some_and(|args| args.is_empty()) {
                    return Err("操作の引数を確認してください。".into());
                }
                let local_url = manager
                    .running
                    .as_ref()
                    .ok_or("管理画面は停止中です。")?
                    .local_url
                    .clone();
                let origin = reqwest::Url::parse(&local_url)
                    .map_err(|_| "管理画面のURLを確認できません。")?
                    .origin()
                    .ascii_serialization();
                let store = admin
                    .shared_work
                    .lock()
                    .map_err(|_| "Hubの保存情報を確認できません。")?;
                let owner = crate::identity::web_access::local_owner(
                    store.identity_connection(),
                    crate::service::now_ms(),
                )
                .map_err(|_| "Hub PC の管理権限を確認できません。")?;
                return Ok(
                    json!({"url":admin.web_access.issue_ticket(&origin,owner.user_id,None)?}),
                );
            }
            "hub_web_enable" => {
                let settings = match args.get("settings") {
                    None | Some(Value::Null) => {
                        if let Some(error) = &manager.settings_error {
                            return Err(format!(
                                "{error} Web管理設定を明示して開始し直してください。"
                            ));
                        }
                        manager.settings.clone()
                    }
                    Some(value) => serde_json::from_value(value.clone())
                        .map_err(|_| "Web管理設定の入力を確認してください。")?,
                };
                manager.enable(&admin, settings).await?;
            }
            "hub_web_disable" => {
                manager.stop(&admin).await;
            }
            _ => {}
        };
        Ok(manager.status(&admin))
    })
    .await
    .map_err(|_| "Web管理操作を完了できませんでした。".to_string())?
}
pub(crate) async fn shutdown(admin: &Admin) {
    admin.web_server.lock().await.stop(admin).await;
}

#[cfg(test)]
mod tests;

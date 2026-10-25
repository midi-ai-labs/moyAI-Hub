//! Process ownership and metadata-only companion control. Credentials travel through stdin.
use crate::gateway::{CancelScope, Gateway, GatewayTlsUpdate, Grant};
use crate::network::certificate::{CertificateIdentity, CertificateSnapshot, PreparedIdentity};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

const LOCK: &str = "gateway-execution.lock";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Init {
    hub_url: String,
    control_token: String,
    data_directory: PathBuf,
    #[serde(default)]
    network: Option<GatewayNetwork>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayNetwork {
    pub bind: std::net::SocketAddr,
    pub advertise_ip: std::net::IpAddr,
    pub certificate_pem: String,
    pub private_key_pem: String,
    pub ca_certificate_pem: String,
}

// Known pre-admission failures can be reported without implying provider execution.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartupFailure {
    startup_error: StartupError,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StartupError {
    NetworkBindFailed,
}

#[derive(Clone)]
pub struct GatewayControl {
    pub(crate) credential_directory: Option<PathBuf>,
    base_url: String,
    token: String,
    client: reqwest::Client,
    network_base_url: Option<String>,
}
#[derive(Debug)]
pub struct ControlError {
    pub status: u16,
    pub code: String,
}
impl GatewayControl {
    pub fn for_loopback(base_url: String, token: String) -> Result<Self, String> {
        let url = reqwest::Url::parse(&base_url).map_err(|_| "Gateway の待受先が無効です。")?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || url.port().is_none()
            || url.path() != "/"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || token.len() < 32
            || token.len() > 256
        {
            return Err("Gateway の待受先または認証が無効です。".into());
        }
        Ok(Self {
            credential_directory: None,
            base_url: base_url.trim_end_matches('/').into(),
            network_base_url: None,
            token,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "Gateway client を作成できません。")?,
        })
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    pub fn network_base_url(&self) -> Option<&str> {
        self.network_base_url.as_deref()
    }
    pub fn with_network_base_url(mut self, base: String) -> Result<Self, String> {
        let url = reqwest::Url::parse(&base).map_err(|_| "Gateway TLS の待受先が無効です。")?;
        let ip = url
            .host_str()
            .and_then(|s| s.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok());
        if url.scheme() != "https"
            || ip.is_none_or(|ip| ip.is_unspecified() || ip.is_multicast())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err("Gateway TLS の待受先が無効です。".into());
        }
        self.network_base_url = Some(base.trim_end_matches('/').into());
        Ok(self)
    }
    pub fn token_matches(&self, candidate: &str) -> bool {
        candidate.len() == self.token.len()
            && candidate
                .bytes()
                .zip(self.token.bytes())
                .fold(0_u8, |v, (a, b)| v | (a ^ b))
                == 0
    }
    async fn request<T: Serialize>(
        &self,
        path: &str,
        body: Option<&T>,
    ) -> Result<Value, ControlError> {
        let builder = if let Some(body) = body {
            self.client
                .post(format!("{}{path}", self.base_url))
                .json(body)
        } else {
            self.client.get(format!("{}{path}", self.base_url))
        };
        let response = builder
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map_err(|_| ControlError {
                status: 503,
                code: "gateway_unavailable".into(),
            })?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await.map_err(|_| ControlError {
            status: 503,
            code: "gateway_unavailable".into(),
        })?;
        if bytes.len() > 64 * 1024 {
            return Err(ControlError {
                status: 503,
                code: "gateway_unavailable".into(),
            });
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| ControlError {
            status: 503,
            code: "gateway_unavailable".into(),
        })?;
        if !(200..300).contains(&status) {
            return Err(ControlError {
                status,
                code: value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("gateway_unavailable")
                    .into(),
            });
        }
        Ok(value)
    }
    pub async fn grant(&self, grant: &Grant) -> Result<Value, ControlError> {
        self.request("/control/grants", Some(grant)).await
    }
    pub async fn cancel(&self, scope: &CancelScope) -> Result<Value, ControlError> {
        self.request("/control/cancel", Some(scope)).await
    }
    pub async fn status(&self) -> Result<Value, ControlError> {
        self.request::<Value>("/control/state", None).await
    }
    pub async fn certificate(&self) -> Result<CertificateSnapshot, ControlError> {
        let value = self.request::<Value>("/control/tls", None).await?;
        serde_json::from_value(value).map_err(|_| ControlError {
            status: 503,
            code: "invalid_gateway_certificate".into(),
        })
    }
    pub async fn install_certificate(
        &self,
        update: &GatewayTlsUpdate,
    ) -> Result<CertificateSnapshot, ControlError> {
        let value = self.request("/control/tls", Some(update)).await?;
        serde_json::from_value(value).map_err(|_| ControlError {
            status: 503,
            code: "invalid_gateway_certificate".into(),
        })
    }
}

pub struct GatewayProcess {
    pub control: GatewayControl,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}
impl GatewayProcess {
    pub async fn start(
        executable: &Path,
        hub_url: String,
        directory: &Path,
    ) -> Result<Self, String> {
        Self::start_with_network(executable, hub_url, directory, None).await
    }
    pub async fn start_with_network(
        executable: &Path,
        hub_url: String,
        directory: &Path,
        network: Option<GatewayNetwork>,
    ) -> Result<Self, String> {
        if execution_running(directory) {
            return Err(
                "前回の Gateway の終了処理中です。完了後にもう一度開始してください。".into(),
            );
        }
        let secret = format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new());
        let init = Init {
            hub_url,
            control_token: secret.clone(),
            data_directory: directory.to_path_buf(),
            network,
        };
        let mut command = Command::new(executable);
        command
            .arg("--gateway")
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.as_std_mut().creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|_| "Gateway を起動できません。モデル要求は受け付けていません。")?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or("Gateway の初期化パイプを開けません。")?;
        let mut bytes = serde_json::to_vec(&init).map_err(|_| "Gateway を初期化できません。")?;
        bytes.push(b'\n');
        stdin
            .write_all(&bytes)
            .await
            .map_err(|_| "Gateway へ初期化情報を送れません。")?;
        let mut stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or("Gateway の応答パイプを開けません。")?,
        );
        let mut ready = String::new();
        tokio::time::timeout(Duration::from_secs(8), stdout.read_line(&mut ready))
            .await
            .map_err(|_| "Gateway の起動確認が期限切れです。")?
            .map_err(|_| "Gateway の起動を確認できません。")?;
        let value: Value =
            serde_json::from_str(&ready).map_err(|_| "Gateway の起動応答が無効です。")?;
        if let Ok(failure) = serde_json::from_value::<StartupFailure>(value.clone()) {
            drop(stdin);
            if !matches!(tokio::time::timeout(Duration::from_secs(2), child.wait()).await, Ok(Ok(status)) if status.success())
            {
                return Err("Gateway の起動に失敗し、子プロセスの終了を確認できません。".into());
            }
            return Err(match failure.startup_error {
                StartupError::NetworkBindFailed => "Gateway の待受を開始できません。指定した IP とポートの使用状況を確認してください。".into(),
            });
        }
        let base = value
            .get("base_url")
            .and_then(Value::as_str)
            .ok_or("Gateway の待受先がありません。")?;
        let mut control = GatewayControl::for_loopback(base.into(), secret)?;
        control.credential_directory = Some(directory.to_path_buf());
        if let Some(base) = value.get("network_base_url").and_then(Value::as_str) {
            control = control.with_network_base_url(base.into())?;
        }
        Ok(Self {
            control,
            child,
            stdin: Some(stdin),
            stdout,
        })
    }
    pub async fn stop(mut self) -> bool {
        // EOF closes admission and cancels only this companion's HTTP connections.
        drop(self.stdin.take());
        let mut completion = String::new();
        let completed = tokio::time::timeout(
            Duration::from_secs(3),
            self.stdout.read_line(&mut completion),
        )
        .await;
        let clean = matches!(completed,Ok(Ok(count)) if count>0)
            && serde_json::from_str::<Value>(&completion)
                .ok()
                .is_some_and(|v| v.get("clean") == Some(&Value::Bool(true)));
        if clean
            && matches!(
                tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await,
                Ok(Ok(_))
            )
        {
            return true;
        }
        // Bound local shutdown even if the child is unresponsive. No provider
        // process is killed and no provider-idle acknowledgement is required.
        self.child.start_kill().is_ok()
            && matches!(
                tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await,
                Ok(Ok(_))
            )
    }
}

/// This lock represents a local companion process, not provider execution.
pub fn execution_running(directory: &Path) -> bool {
    let Ok(lock) = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(LOCK))
    else {
        return true;
    };
    fs2::FileExt::try_lock_exclusive(&lock).is_err()
}

pub async fn run_companion() -> Result<(), io::Error> {
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut input = String::new();
    tokio::time::timeout(Duration::from_secs(10), stdin.read_line(&mut input))
        .await
        .map_err(|_| io::Error::other("gateway initialization timeout"))??;
    if input.len() > 64 * 1024 {
        return Err(io::Error::other("gateway initialization too large"));
    }
    let init: Init = serde_json::from_str(&input)
        .map_err(|_| io::Error::other("invalid gateway initialization"))?;
    let _profile_lease =
        crate::maintenance::ProfileLease::shared(&init.data_directory).map_err(io::Error::other)?;
    let _lock: File = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(init.data_directory.join(LOCK))?;
    fs2::FileExt::try_lock_exclusive(&_lock)?;
    // Legacy gateway-uncertain.json is retained as historical data, but is not
    // evidence about provider activity and must not gate this process's startup.
    let gateway =
        Gateway::new_with_credentials(init.hub_url, init.control_token, Some(init.data_directory))
            .map_err(|_| io::Error::other("gateway client unavailable"))?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base_url = format!("http://{}/", listener.local_addr()?);
    // Bind and validate every listener before starting either admission loop.
    let (network_listener, network_base_url) = if let Some(network) = init.network {
        let listener = match tokio::net::TcpListener::bind(network.bind).await {
            Ok(listener) => listener,
            Err(_) => {
                let reply = StartupFailure {
                    startup_error: StartupError::NetworkBindFailed,
                };
                let mut bytes = serde_json::to_vec(&reply).map_err(io::Error::other)?;
                bytes.push(b'\n');
                let mut stdout = tokio::io::stdout();
                stdout.write_all(&bytes).await?;
                stdout.flush().await?;
                return Ok(());
            }
        };
        let public = std::net::SocketAddr::new(network.advertise_ip, listener.local_addr()?.port());
        let acceptor = crate::network::tls::acceptor(
            &network.certificate_pem,
            &network.private_key_pem,
            &network.ca_certificate_pem,
            false,
        )
        .map_err(|_| io::Error::other("gateway TLS unavailable"))?;
        let identity = CertificateIdentity::new(
            PreparedIdentity::new(
                &network.certificate_pem,
                &network.private_key_pem,
                &network.ca_certificate_pem,
                network.advertise_ip,
                false,
                crate::service::now_ms(),
            )
            .map_err(|_| io::Error::other("gateway TLS identity unavailable"))?,
        );
        gateway
            .configure_network_tls(
                identity.clone(),
                network.ca_certificate_pem,
                network.advertise_ip,
            )
            .map_err(|_| io::Error::other("gateway TLS identity unavailable"))?;
        let listener = crate::network::tls::NetworkListener::new(listener, acceptor)
            .with_identity(identity)
            .with_idle(Duration::from_secs(305))
            .with_authority(public.to_string());
        (Some(listener), Some(format!("https://{public}")))
    } else {
        (None, None)
    };
    let router = gateway.clone().router();
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let network_server = network_listener.map(|listener| {
        let router = gateway.clone().network_router();
        tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<crate::network::tls::TlsPeer>(),
            )
            .await
        })
    });
    let mut stdout = tokio::io::stdout();
    let ready_sent = stdout
        .write_all(
            format!(
                "{}\n",
                json!({"base_url":base_url,"network_base_url":network_base_url})
            )
            .as_bytes(),
        )
        .await
        .is_ok()
        && stdout.flush().await.is_ok();
    input.clear();
    // Hub death and explicit stop both close this pipe. No parent heartbeat guesses are used.
    if ready_sent {
        let _ = stdin.read_line(&mut input).await;
    }
    let clean = gateway.drain().await;
    server.abort();
    let _ = server.await;
    if let Some(server) = network_server {
        server.abort();
        let _ = server.await;
    }
    // Keep the local process lock until all listeners and our HTTP tasks stop.
    stdout
        .write_all(format!("{}\n", json!({"clean":clean})).as_bytes())
        .await?;
    stdout.flush().await?;
    Ok(())
}

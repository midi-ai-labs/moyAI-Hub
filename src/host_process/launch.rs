//! Explicit interactive launch. The foreground server remains the lifetime owner.
use super::{HostClient, canonical_catalog, lock_contended, profile_busy};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    net::IpAddr,
    path::Path,
    process::Stdio,
    time::Duration,
};

#[derive(Debug, Serialize)]
pub struct LaunchReport {
    pub pid: u32,
    pub already_running: bool,
    pub web_url: String,
    pub network_running: bool,
    pub needs_setup: bool,
    pub network_error: Option<String>,
    pub firewall_status: LaunchFirewallStatus,
    pub firewall_error: Option<String>,
}

/// A read-only OS check, independent of listener startup and remote reachability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchFirewallStatus {
    NotChecked,
    NotRequired,
    Matching,
    Missing,
    Mismatch,
    Conflict,
    Unknown,
}

impl LaunchFirewallStatus {
    pub fn requires_setup(self) -> bool {
        matches!(
            self,
            Self::Missing | Self::Mismatch | Self::Conflict | Self::Unknown
        )
    }
}

fn loopback_listeners(state: &Value) -> bool {
    ["bind", "gateway_bind"].into_iter().all(|key| {
        state["server"][key]
            .as_str()
            .and_then(|value| reqwest::Url::parse(value).ok())
            .filter(|url| url.scheme() == "https")
            .and_then(|url| {
                url.host_str()?
                    .trim_matches(['[', ']'])
                    .parse::<IpAddr>()
                    .ok()
            })
            .is_some_and(|ip| ip.is_loopback())
    })
}

async fn inspect_firewall(client: &HostClient, state: &Value, report: &mut LaunchReport) {
    if !report.network_running {
        return;
    }
    if loopback_listeners(state) {
        report.firewall_status = LaunchFirewallStatus::NotRequired;
        return;
    }
    match client.dispatch("hub_firewall_status", json!({})).await {
        Ok(status) => {
            report.firewall_status = match status["state"].as_str() {
                Some("matching") => LaunchFirewallStatus::Matching,
                Some("missing") => LaunchFirewallStatus::Missing,
                Some("mismatch") => LaunchFirewallStatus::Mismatch,
                Some("conflict") => LaunchFirewallStatus::Conflict,
                _ => {
                    report.firewall_error = Some(
                        "Windowsの受信許可を確認できません。管理画面で確認してください。".into(),
                    );
                    LaunchFirewallStatus::Unknown
                }
            };
        }
        Err(error) => {
            report.firewall_status = LaunchFirewallStatus::Unknown;
            report.firewall_error = Some(error);
        }
    }
}

/// The short-lived CLI owns these handles. Children get explicitly assigned
/// stdio; inheriting the CLI's original pipe handles would keep its caller
/// waiting for EOF for the entire server lifetime on Windows.
pub fn isolate_launcher_stdio() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(which: u32) -> *mut c_void;
            fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
        }
        for which in [-10_i32, -11, -12] {
            let handle = unsafe { GetStdHandle(which as u32) };
            if !handle.is_null()
                && handle as isize != -1
                && unsafe { SetHandleInformation(handle, 1, 0) } == 0
            {
                return Err(format!(
                    "起動元の入出力を分離できません: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
    }
    Ok(())
}

async fn verify_gateway(client: &HostClient, report: &mut LaunchReport) {
    if !report.network_running {
        return;
    }
    match client.dispatch("hub_snapshot", json!({})).await {
        Ok(state) if state["gateway"]["ready"] == true && state["server"]["running"] == true => {}
        Ok(_) => {
            report.network_running = false;
            report.network_error =
                Some("Gateway の応答を確認できません。管理画面で状態を確認してください。".into());
        }
        Err(error) => {
            report.network_running = false;
            report.network_error = Some(error);
        }
    }
}

async fn launch_lock(directory: &Path) -> Result<File, String> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("host-launch.lock"))
        .map_err(|_| "Hub の起動待ち合わせファイルを開けません。")?;
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            match fs2::FileExt::try_lock_exclusive(&lock) {
                Ok(()) => return Ok(lock),
                Err(error) if lock_contended(&error) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(_) => return Err("Hub の起動を待ち合わせできません。".into()),
            }
        }
    })
    .await
    .map_err(|_| "別の Hub 起動操作が処理中です。少し待ってから再度開いてください。".to_owned())?
}

fn startup_log(directory: &Path) -> Result<File, String> {
    let path = directory.join("hub-startup.log");
    if std::fs::metadata(&path).is_ok_and(|file| file.len() > 1024 * 1024) {
        std::fs::rename(&path, directory.join("hub-startup.previous.log"))
            .map_err(|_| "Hub の起動ログを整理できません。")?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| "Hub の起動ログを開けません。".into())
}

async fn start_server(
    executable: &Path,
    catalog: &Path,
    web_args: &[OsString],
) -> Result<HostClient, String> {
    let directory = catalog.parent().ok_or("Hub の保存先が無効です。")?;
    let log = startup_log(directory)?;
    let errors = log
        .try_clone()
        .map_err(|_| "Hub の起動ログを準備できません。")?;
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("--data-dir")
        .arg(directory)
        .args(web_args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors))
        .kill_on_drop(false);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW; no launcher console ownership.
    let mut child = command
        .spawn()
        .map_err(|e| format!("Hub を起動できません: {e}"))?;
    let ready = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(client) = HostClient::connect(catalog).await {
                return Ok(client);
            }
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err(format!(
                    "Hub の起動に失敗しました。IP・ポートと設定を確認してください。起動ログ: {}",
                    directory.join("hub-startup.log").display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    match ready {
        Ok(Ok(client)) => Ok(client),
        failure => {
            // Only this still-unready child is ours to terminate. Never replace a running Hub.
            let _ = child.kill().await;
            let _ = child.wait().await;
            let error = match failure {
                Ok(Err(error)) => error,
                _ => format!(
                    "Hub の起動確認が期限切れになりました。起動ログ: {}",
                    directory.join("hub-startup.log").display()
                ),
            };
            Err(error)
        }
    }
}

/// The browser launch target must be the server's validated local management URL.
pub fn local_management_url(value: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "Web管理のURLが無効です。")?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/admin/"
        || url.port_or_known_default().is_none_or(|port| port == 0)
    {
        return Err("Hub のローカルWeb管理URLを確認できません。".into());
    }
    Ok(url.to_string())
}

pub async fn launch_server(
    executable: &Path,
    catalog_path: &Path,
    web_args: &[OsString],
) -> Result<LaunchReport, String> {
    let catalog = canonical_catalog(catalog_path, true)?;
    let directory = catalog.parent().ok_or("Hub の保存先が無効です。")?;
    let _launch_lock = launch_lock(directory).await?;
    let (client, already_running) = match HostClient::connect(&catalog).await {
        Ok(client) => {
            if !web_args.is_empty() {
                return Err(
                    "Hub は起動済みです。Web設定を変える場合は停止してから起動してください。"
                        .into(),
                );
            }
            (client, true)
        }
        Err(error) => {
            if profile_busy(&catalog)? {
                return Err(format!(
                    "起動中の Hub に接続できません。別の Hub は起動していません。{error}"
                ));
            }
            (start_server(executable, &catalog, web_args).await?, false)
        }
    };
    let web = client.dispatch("hub_web_status", json!({})).await?;
    let web_url = local_management_url(web["local_url"].as_str().unwrap_or(""))?;
    let mut report = LaunchReport {
        pid: client.pid(),
        already_running,
        web_url,
        network_running: false,
        needs_setup: false,
        network_error: None,
        firewall_status: LaunchFirewallStatus::NotChecked,
        firewall_error: None,
    };
    let before = match client.dispatch("hub_network_snapshot", json!({})).await {
        Ok(state) => state,
        Err(error) => {
            report.network_error = Some(error);
            return Ok(report);
        }
    };
    let running = before["server"]["running"] == true;
    let hosting = &before["hosting"];
    if running {
        report.network_running = before["server"]["gateway_bind"].is_string();
        if !report.network_running {
            report.network_error = Some("端末ネットワークは稼働中ですが Gateway を確認できません。管理画面で状態を確認してください。".into());
        }
        verify_gateway(&client, &mut report).await;
        inspect_firewall(&client, &before, &mut report).await;
        return Ok(report);
    }
    if !hosting.is_object() {
        report.network_error = Some(
            "起動中の Hub がワンクリック起動に未対応です。停止して新しい版から起動してください。"
                .into(),
        );
    } else if let Some(error) = hosting["error"].as_str() {
        report.network_error = Some(error.to_owned());
    } else if hosting["configured"] == false {
        report.needs_setup = true;
    } else {
        match client.dispatch("hub_network_start_saved", json!({})).await {
            Ok(_) => {
                let state = match client.dispatch("hub_network_snapshot", json!({})).await {
                    Ok(state) => state,
                    Err(error) => {
                        report.network_error = Some(error);
                        return Ok(report);
                    }
                };
                report.network_running = state["server"]["running"] == true
                    && state["server"]["gateway_bind"].is_string();
                if !report.network_running {
                    report.network_error = Some("Hub・Gateway の開始完了を確認できません。管理画面で状態を確認してください。".into());
                }
                verify_gateway(&client, &mut report).await;
                inspect_firewall(&client, &state, &mut report).await;
            }
            Err(error) => report.network_error = Some(error),
        }
    }
    Ok(report)
}

pub fn browser_launch_url(
    url: &str,
    already_running: bool,
    firewall_status: LaunchFirewallStatus,
) -> Result<String, String> {
    let (base, ticket) = url
        .split_once("#access=")
        .ok_or("管理画面の認証を準備できません。")?;
    let url = local_management_url(base)?;
    if ticket.len() != 64 || !ticket.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("管理画面の認証を準備できません。".into());
    }
    Ok(format!(
        "{url}#access={ticket}&notice={}{}",
        if already_running {
            "hub-already-running"
        } else {
            "hub-started"
        },
        if firewall_status.requires_setup() {
            "&section=network-step-firewall"
        } else {
            ""
        },
    ))
}

pub fn open_browser(
    url: &str,
    already_running: bool,
    firewall_status: LaunchFirewallStatus,
) -> Result<(), String> {
    let url = browser_launch_url(url, already_running, firewall_status)?;
    let error = "ブラウザーを開けません。Hub PC の moyAI Team Management を開き直してください。";
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            System::Com::{
                COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
            },
            UI::{
                Shell::{
                    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
                },
                WindowsAndMessaging::SW_SHOWNORMAL,
            },
        };
        let operation = "open".encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let url = url.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let initialized = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        };
        if initialized < 0 {
            return Err(error.into());
        }
        let mut execute = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            // The CLI exits shortly after this call. Do not opt into ASYNCOK;
            // NOASYNC alone does not guarantee synchronous URI activation.
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: operation.as_ptr(),
            lpFile: url.as_ptr(),
            nShow: SW_SHOWNORMAL,
            ..Default::default()
        };
        // Pass the validated URL directly to its Windows association, without
        // command-shell parsing or URL-bearing OS error messages.
        let opened = unsafe { ShellExecuteExW(&mut execute) };
        unsafe { CoUninitialize() };
        if opened == 0 {
            return Err(error.into());
        }
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "macos")]
        let mut command = std::process::Command::new("open");
        #[cfg(not(target_os = "macos"))]
        let mut command = std::process::Command::new("xdg-open");
        command
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().map_err(|_| error.to_owned())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_target_accepts_only_local_management_urls() {
        assert_eq!(
            local_management_url("http://127.0.0.1:9080/admin/").unwrap(),
            "http://127.0.0.1:9080/admin/"
        );
        for value in [
            "https://127.0.0.1:9080/admin/",
            "http://example.com/admin/",
            "http://user@127.0.0.1/admin/",
            "http://127.0.0.1/admin/?x=1",
            "http://127.0.0.1/admin/#x",
            "http://127.0.0.1/host/health",
            "file:///test",
            "http://127.0.0.1:0/admin/",
        ] {
            assert!(local_management_url(value).is_err(), "{value}");
        }
    }

    #[test]
    fn browser_launch_notice_keeps_the_validated_local_origin() {
        let url = "http://127.0.0.1:9081/admin/";
        let access = format!("{url}#access={}", "a".repeat(64));
        assert_eq!(
            browser_launch_url(&access, true, LaunchFirewallStatus::Matching).unwrap(),
            format!("{access}&notice=hub-already-running")
        );
        assert_eq!(
            browser_launch_url(&access, false, LaunchFirewallStatus::NotChecked).unwrap(),
            format!("{access}&notice=hub-started")
        );
        for status in [
            LaunchFirewallStatus::Missing,
            LaunchFirewallStatus::Mismatch,
            LaunchFirewallStatus::Conflict,
            LaunchFirewallStatus::Unknown,
        ] {
            for running in [false, true] {
                let target = browser_launch_url(&access, running, status).unwrap();
                assert!(target.ends_with("&section=network-step-firewall"));
                assert!(target.starts_with(&access));
            }
        }
        assert!(
            browser_launch_url(
                "http://example.com/admin/",
                true,
                LaunchFirewallStatus::Unknown
            )
            .is_err()
        );
        assert!(
            browser_launch_url(
                "http://127.0.0.1:9081/admin/#other",
                true,
                LaunchFirewallStatus::Unknown
            )
            .is_err()
        );
    }

    #[test]
    fn browser_open_rejects_invalid_targets_without_disclosing_access() {
        let ticket = "a".repeat(64);
        for value in [
            format!("https://example.com/admin/#access={ticket}"),
            format!("http://127.0.0.1:9081/admin/?redirect=elsewhere#access={ticket}"),
            format!("http://127.0.0.1:9081/admin/#access={ticket}&extra=1"),
            format!("http://127.0.0.1:9081/admin/#access={}g", "a".repeat(63)),
            "http://127.0.0.1:9081/admin/#access=too-short".to_owned(),
        ] {
            let error = open_browser(&value, true, LaunchFirewallStatus::Unknown).unwrap_err();
            assert!(!error.contains(&ticket));
            assert!(!error.contains("http"));
        }
    }

    #[test]
    fn only_two_loopback_listeners_skip_the_inbound_rule_check() {
        for address in ["127.0.0.1", "[::1]"] {
            assert!(loopback_listeners(&json!({"server":{
                "bind":format!("https://{address}:9471"),
                "gateway_bind":format!("https://{address}:11473")
            }})));
        }
        for gateway in [
            "https://192.168.1.10:11473",
            "https://0.0.0.0:11473",
            "https://localhost:11473",
            "http://127.0.0.1:11473",
            "invalid",
        ] {
            assert!(!loopback_listeners(&json!({"server":{
                "bind":"https://127.0.0.1:9471","gateway_bind":gateway
            }})));
        }
        assert!(!loopback_listeners(
            &json!({"server":{"bind":"https://127.0.0.1:9471"}})
        ));
    }

    #[tokio::test]
    async fn repeated_and_resumed_launch_inspect_rules_without_requesting_a_change() {
        use super::super::{Descriptor, write_descriptor};
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        use std::sync::{Arc, Mutex};

        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("project_sandbox/hub-launch-firewall-tests");
        std::fs::create_dir_all(&fixtures).unwrap();
        for start_running in [true, false] {
            for (reply, expected) in [
                (
                    json!({"ok":true,"value":{"state":"matching"}}),
                    LaunchFirewallStatus::Matching,
                ),
                (
                    json!({"ok":true,"value":{"state":"missing"}}),
                    LaunchFirewallStatus::Missing,
                ),
                (
                    json!({"ok":true,"value":{"state":"mismatch"}}),
                    LaunchFirewallStatus::Mismatch,
                ),
                (
                    json!({"ok":true,"value":{"state":"conflict"}}),
                    LaunchFirewallStatus::Conflict,
                ),
                (
                    json!({"ok":true,"value":{"state":"new-unknown-state"}}),
                    LaunchFirewallStatus::Unknown,
                ),
                (
                    json!({"ok":false,"error":"OS inspection unavailable"}),
                    LaunchFirewallStatus::Unknown,
                ),
            ] {
                let directory = tempfile::tempdir_in(&fixtures).unwrap();
                let catalog =
                    canonical_catalog(&directory.path().join("catalog.json"), true).unwrap();
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let descriptor = Descriptor {
                    schema_version: 1,
                    instance_id: "A".repeat(26),
                    pid: std::process::id(),
                    port: listener.local_addr().unwrap().port(),
                    token: "a".repeat(52),
                    catalog_path: catalog.clone(),
                };
                write_descriptor(&descriptor).unwrap();
                let health = json!({"closing":false,"instance_id":descriptor.instance_id,
                    "pid":descriptor.pid,"catalog_path":catalog});
                let calls = Arc::new(Mutex::new(Vec::<String>::new()));
                let recorded = calls.clone();
                let running = Arc::new(Mutex::new(start_running));
                let router = Router::new()
                    .route(
                        "/host/health",
                        get(move || {
                            let health = health.clone();
                            async move { Json(health) }
                        }),
                    )
                    .route(
                        "/host/command",
                        post(move |Json(request): Json<Value>| {
                            let reply = reply.clone();
                            let recorded = recorded.clone();
                            let running = running.clone();
                            async move {
                                let command = request["command"].as_str().unwrap();
                                recorded.lock().unwrap().push(command.to_owned());
                                let value = match command {
                                    "hub_web_status" => {
                                        json!({"local_url":"http://127.0.0.1:9080/admin/"})
                                    }
                                    "hub_network_snapshot" => json!({"server":{
                                    "running":*running.lock().unwrap(),
                                    "bind":"https://192.168.1.10:9471",
                                    "gateway_bind":"https://192.168.1.10:11473"
                                },"hosting":{"configured":true}}),
                                    "hub_snapshot" => {
                                        json!({"server":{"running":true},"gateway":{"ready":true}})
                                    }
                                    "hub_network_start_saved" => {
                                        *running.lock().unwrap() = true;
                                        json!({})
                                    }
                                    "hub_firewall_status" => return Json(reply),
                                    other => panic!("unexpected launcher command: {other}"),
                                };
                                Json(json!({"ok":true,"value":value}))
                            }
                        }),
                    );
                let server =
                    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
                let report = launch_server(Path::new("unused-existing-host.exe"), &catalog, &[])
                    .await
                    .unwrap();
                server.abort();
                let _ = server.await;
                assert!(report.already_running && report.network_running);
                assert!(!report.needs_setup);
                assert!(report.network_error.is_none());
                assert_eq!(report.firewall_status, expected);
                assert_eq!(
                    report.firewall_error.is_some(),
                    expected == LaunchFirewallStatus::Unknown
                );
                let calls = calls.lock().unwrap();
                assert_eq!(
                    calls
                        .iter()
                        .filter(|v| v.as_str() == "hub_firewall_status")
                        .count(),
                    1
                );
                assert_eq!(
                    calls
                        .iter()
                        .filter(|v| v.as_str() == "hub_network_start_saved")
                        .count(),
                    usize::from(!start_running)
                );
                assert!(!calls.iter().any(|v| matches!(
                    v.as_str(),
                    "hub_firewall_apply" | "hub_firewall_remove" | "hub_web_open"
                )));
            }
        }
    }
}

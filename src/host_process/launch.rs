//! Explicit interactive launch. The foreground server remains the lifetime owner.
use super::{HostClient, canonical_catalog, lock_contended, profile_busy};
use serde::Serialize;
use serde_json::json;
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    path::Path,
    process::Stdio,
    time::Duration,
};

#[derive(Debug, Serialize)]
pub struct LaunchReport {
    pub pid: u32,
    pub web_url: String,
    pub network_running: bool,
    pub needs_setup: bool,
    pub network_error: Option<String>,
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
    let client = match HostClient::connect(&catalog).await {
        Ok(client) => {
            if !web_args.is_empty() {
                return Err(
                    "Hub は起動済みです。Web設定を変える場合は停止してから起動してください。"
                        .into(),
                );
            }
            client
        }
        Err(error) => {
            if profile_busy(&catalog)? {
                return Err(format!(
                    "起動中の Hub に接続できません。別の Hub は起動していません。{error}"
                ));
            }
            start_server(executable, &catalog, web_args).await?
        }
    };
    let web = client.dispatch("hub_web_status", json!({})).await?;
    let web_url = local_management_url(web["local_url"].as_str().unwrap_or(""))?;
    let mut report = LaunchReport {
        pid: client.pid(),
        web_url,
        network_running: false,
        needs_setup: false,
        network_error: None,
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
            }
            Err(error) => report.network_error = Some(error),
        }
    }
    verify_gateway(&client, &mut report).await;
    Ok(report)
}

pub fn open_browser(url: &str) -> Result<(), String> {
    let url = local_management_url(url)?;
    #[cfg(windows)]
    let mut command = {
        let windows = std::env::var_os("WINDIR").ok_or("Windowsの場所を確認できません。")?;
        std::process::Command::new(std::path::PathBuf::from(windows).join("explorer.exe"))
    };
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
        .spawn()
        .map_err(|_| format!("ブラウザーを開けません。次のURLを開いてください: {url}"))?;
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
}

use moyai_hub::host_process::{self, HostClient};
use serde_json::{Value, json};
use std::{ffi::OsString, io::Read, path::PathBuf};

pub const HELP: &str = "moyAI Hub — Rust server and browser management\n\n\
Usage: moyai-hub [--data-dir PATH] [--web-port PORT | --web-settings FILE]\n\
       moyai-hub --launch [--data-dir PATH] [--no-browser]\n\
       moyai-hub --status [--data-dir PATH]\n\
       moyai-hub --stop [--data-dir PATH]\n\n\
Starts Web management automatically. First start: http://127.0.0.1:9080/admin/\n\
Use --launch to open authenticated management on this PC, or open it from an approved administrator device. No Hub ID/password is required.\n\
--web-port PORT       Local HTTP port (0 selects a free port); saves on success.\n\
--web-settings FILE   JSON Web settings; see docs/web-management.md for LAN/TLS.\n\
--data-dir PATH       Hub data directory; MOYAI_HUB_DATA_DIR also supported.\n\
--launch              Resume saved hosting, check inbound rules, open browser.\n\
--no-browser          With --launch, print the result without opening a browser.\n\
--status             Print current Web URLs and running state; never starts Hub.\n\
--stop               Stop this data directory's Hub and its Gateway.\n\
--backup-dir PATH    Copy a stopped Hub to a new checksummed backup directory.\n\
--restore-from PATH  Restore a verified backup to a new --data-dir directory.\n\
--acknowledge-runners-stopped  Required for restore after stopping all affected Runners.\n\
Ctrl+C also stops the foreground server. Closing a browser does not stop Hub.\n";

#[derive(Debug, PartialEq)]
pub enum Action {
    Help,
    Gateway,
    Serve {
        data: Option<PathBuf>,
        port: Option<u16>,
        settings: Option<PathBuf>,
    },
    Launch {
        data: Option<PathBuf>,
        port: Option<u16>,
        settings: Option<PathBuf>,
        no_browser: bool,
    },
    Status(Option<PathBuf>),
    Stop(Option<PathBuf>),
    Backup {
        data: Option<PathBuf>,
        destination: PathBuf,
    },
    Restore {
        data: Option<PathBuf>,
        source: PathBuf,
        acknowledge_runners_stopped: bool,
    },
}

pub fn parse(args: Vec<OsString>) -> Result<Action, String> {
    if args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h")) {
        return Ok(Action::Help);
    }
    if args.len() == 1 && args[0] == "--gateway" {
        return Ok(Action::Gateway);
    }
    let (mut data, mut port, mut settings, mut mode) = (None, None, None, None);
    let mut no_browser = false;
    let mut maintenance_path = None;
    let mut acknowledge_runners_stopped = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--backup-dir" | "--restore-from") if mode.is_none() => {
                mode = arg.to_str().map(str::to_owned);
                maintenance_path = Some(PathBuf::from(
                    args.next().ok_or("Maintenance needs a directory path")?,
                ));
            }
            Some("--acknowledge-runners-stopped") if !acknowledge_runners_stopped => {
                acknowledge_runners_stopped = true
            }
            Some("--data-dir") if data.is_none() => {
                data = Some(PathBuf::from(args.next().ok_or("--data-dir needs a path")?));
            }
            Some("--web-port") if port.is_none() => {
                port = Some(
                    args.next()
                        .and_then(|v| v.to_str()?.parse::<u16>().ok())
                        .ok_or("--web-port must be 0–65535")?,
                );
            }
            Some("--web-settings") if settings.is_none() => {
                settings = Some(PathBuf::from(
                    args.next().ok_or("--web-settings needs a JSON file")?,
                ));
            }
            Some("--status" | "--stop" | "--launch") if mode.is_none() => {
                mode = arg.to_str().map(str::to_owned)
            }
            Some("--no-browser") if !no_browser => no_browser = true,
            _ => {
                return Err(format!(
                    "Unknown or repeated option: {}",
                    arg.to_string_lossy()
                ));
            }
        }
    }
    if port.is_some() && settings.is_some() {
        return Err("Use either --web-port or --web-settings".into());
    }
    if matches!(mode.as_deref(), Some("--backup-dir" | "--restore-from"))
        && (port.is_some() || settings.is_some() || no_browser)
    {
        return Err(
            "Backup/restore require a stopped Hub and do not accept hosting options".into(),
        );
    }
    if acknowledge_runners_stopped && mode.as_deref() != Some("--restore-from") {
        return Err("--acknowledge-runners-stopped applies only to restore".into());
    }
    if mode.as_deref() == Some("--restore-from") && (!acknowledge_runners_stopped || data.is_none())
    {
        return Err("Restore requires a new --data-dir and --acknowledge-runners-stopped".into());
    }
    if matches!(mode.as_deref(), Some("--status" | "--stop"))
        && (port.is_some() || settings.is_some())
    {
        return Err("Web settings apply only when starting Hub".into());
    }
    if no_browser && mode.as_deref() != Some("--launch") {
        return Err("--no-browser applies only with --launch".into());
    }
    match mode.as_deref() {
        Some("--backup-dir") => Ok(Action::Backup {
            data,
            destination: maintenance_path.expect("parsed path"),
        }),
        Some("--restore-from") => Ok(Action::Restore {
            data,
            source: maintenance_path.expect("parsed path"),
            acknowledge_runners_stopped,
        }),
        Some("--status") => Ok(Action::Status(data)),
        Some("--stop") => Ok(Action::Stop(data)),
        Some("--launch") => Ok(Action::Launch {
            data,
            port,
            settings,
            no_browser,
        }),
        _ => Ok(Action::Serve {
            data,
            port,
            settings,
        }),
    }
}

fn catalog(data: Option<PathBuf>) -> Result<PathBuf, String> {
    let directory = data
        .or_else(|| std::env::var_os("MOYAI_HUB_DATA_DIR").map(PathBuf::from))
        .map(Ok)
        .unwrap_or_else(default_data)?;
    if directory.as_os_str().is_empty() {
        return Err("Hub data directory must not be empty".into());
    }
    let directory = if directory.is_absolute() {
        directory
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(directory)
    };
    Ok(directory.join("catalog.json"))
}

fn default_data() -> Result<PathBuf, String> {
    // Keep the installed Hub profile location stable across upgrades.
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|v| PathBuf::from(v).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".local/share")))
    };
    base.map(|v| v.join("ai.midi.moyai.hub"))
        .ok_or_else(|| "Specify --data-dir or MOYAI_HUB_DATA_DIR".into())
}

fn web_settings(port: Option<u16>, settings: Option<PathBuf>) -> Result<Option<Value>, String> {
    if let Some(port) = port {
        return Ok(Some(json!({"local_port":port,"remote":{"mode":"off"}})));
    }
    let Some(path) = settings else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| format!("Cannot open Web settings: {e}"))?
        .take(32 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 32 * 1024 {
        return Err("Web settings exceed 32 KiB".into());
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid Web settings JSON: {e}"))?;
    if !value.is_object() {
        return Err("Web settings must be a JSON object".into());
    }
    Ok(Some(value))
}

pub async fn run(action: Action) -> Result<(), String> {
    match action {
        Action::Help => Ok(()),
        Action::Backup { data, destination } => {
            let catalog = catalog(data)?;
            let manifest = moyai_hub::backup::backup(
                catalog.parent().ok_or("Hub directory missing")?,
                &destination,
            )?;
            println!(
                "{}",
                serde_json::to_string(&manifest).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        Action::Restore {
            data,
            source,
            acknowledge_runners_stopped,
        } => {
            let catalog = catalog(data)?;
            let manifest = moyai_hub::backup::restore(
                &source,
                catalog.parent().ok_or("Hub directory missing")?,
                acknowledge_runners_stopped,
            )?;
            println!(
                "{}",
                serde_json::to_string(&manifest).map_err(|e| e.to_string())?
            );
            eprintln!(
                "Restored with devices and shared environments paused. Reconcile earlier execution, then explicitly resume from administration."
            );
            Ok(())
        }
        Action::Gateway => moyai_hub::gateway_process::run_companion()
            .await
            .map_err(|error| error.to_string()),
        Action::Serve {
            data,
            port,
            settings,
        } => host_process::run_server(catalog(data)?, web_settings(port, settings)?).await,
        Action::Launch {
            data,
            port,
            settings,
            no_browser,
        } => {
            // Validate public settings before creating a detached process.
            web_settings(port, settings.clone())?;
            host_process::launch::isolate_launcher_stdio()?;
            let mut web_args = Vec::<OsString>::new();
            if let Some(port) = port {
                web_args.extend(["--web-port".into(), port.to_string().into()]);
            }
            if let Some(settings) = settings {
                web_args.extend(["--web-settings".into(), settings.into_os_string()]);
            }
            let catalog_path = catalog(data)?;
            let report = host_process::launch::launch_server(
                &std::env::current_exe().map_err(|e| e.to_string())?,
                &catalog_path,
                &web_args,
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string(&report).map_err(|e| e.to_string())?
            );
            eprintln!(
                "{} (PID {})",
                if report.already_running {
                    "moyAI Hub は起動済みです。同じ Hub を利用します。"
                } else {
                    "moyAI Hub を起動しました。"
                },
                report.pid
            );
            if !no_browser {
                let access = HostClient::connect(&catalog_path)
                    .await?
                    .dispatch("hub_web_open", serde_json::json!({}))
                    .await?;
                host_process::launch::open_browser(
                    access["url"]
                        .as_str()
                        .ok_or("管理画面の認証を準備できません。")?,
                    report.already_running,
                    report.firewall_status,
                )?;
            }
            if let Some(error) = report.network_error {
                return Err(format!(
                    "Web管理は起動していますが、ホスティングを開始できません: {error}"
                ));
            }
            if report.needs_setup {
                eprintln!(
                    "Web管理が利用できます。初回だけ端末ネットワークのIP・ポートを設定して開始してください。次回から同じ設定で再開します。"
                );
            }
            if report.firewall_status.requires_setup() {
                eprintln!(
                    "Hub・Gatewayは起動しています。別のPCへ接続ファイルを渡す前に、管理画面の「Windowsで受信を許可」で設定を確認してください。Windowsの設定は自動変更していません。"
                );
                if let Some(error) = report.firewall_error {
                    eprintln!("受信許可の確認: {error}");
                }
            }
            Ok(())
        }
        Action::Stop(data) => {
            HostClient::connect(&catalog(data)?)
                .await?
                .shutdown()
                .await?;
            println!("moyAI Hub stopped.");
            Ok(())
        }
        Action::Status(data) => {
            let client = HostClient::connect(&catalog(data)?).await?;
            let web = client.dispatch("hub_web_status", json!({})).await?;
            println!("{}", json!({"pid":client.pid(),"web":web}));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn server_start_and_control_options_have_explicit_precedence() {
        assert_eq!(
            parse(vec![]).unwrap(),
            Action::Serve {
                data: None,
                port: None,
                settings: None
            }
        );
        assert_eq!(
            parse(args(&[
                "--data-dir",
                "directory with spaces",
                "--web-port",
                "0"
            ]))
            .unwrap(),
            Action::Serve {
                data: Some("directory with spaces".into()),
                port: Some(0),
                settings: None
            }
        );
        assert_eq!(
            parse(args(&["--stop", "--data-dir", "data"])).unwrap(),
            Action::Stop(Some("data".into()))
        );
        for invalid in [
            vec!["--host", "catalog.json"],
            vec!["--stop-host", "catalog.json"],
            vec!["--web-port", "65536"],
            vec!["--web-port", "-1"],
            vec!["--web-port"],
            vec!["--stop", "--web-port", "1"],
            vec!["--status", "--stop"],
            vec!["--web-port", "1", "--web-settings", "x"],
            vec!["--data-dir", "x", "--data-dir", "y"],
            vec!["--no-browser"],
            vec!["--launch", "--stop"],
            vec!["--launch", "--no-browser", "--no-browser"],
        ] {
            assert!(parse(args(&invalid)).is_err());
        }
    }
    #[test]
    fn interactive_launch_keeps_profile_and_web_options() {
        assert_eq!(
            parse(args(&[
                "--launch",
                "--data-dir",
                "directory with spaces",
                "--web-port",
                "0",
                "--no-browser"
            ]))
            .unwrap(),
            Action::Launch {
                data: Some("directory with spaces".into()),
                port: Some(0),
                settings: None,
                no_browser: true
            }
        );
    }
    #[test]
    fn maintenance_requires_explicit_restore_destination_and_stopped_runners() {
        assert_eq!(
            parse(args(&["--backup-dir", "saved", "--data-dir", "profile"])).unwrap(),
            Action::Backup {
                data: Some("profile".into()),
                destination: "saved".into()
            }
        );
        assert_eq!(
            parse(args(&[
                "--restore-from",
                "saved",
                "--data-dir",
                "new profile",
                "--acknowledge-runners-stopped"
            ]))
            .unwrap(),
            Action::Restore {
                data: Some("new profile".into()),
                source: "saved".into(),
                acknowledge_runners_stopped: true
            }
        );
        for invalid in [
            vec!["--restore-from", "saved", "--data-dir", "new"],
            vec!["--restore-from", "saved", "--acknowledge-runners-stopped"],
            vec!["--backup-dir", "saved", "--acknowledge-runners-stopped"],
            vec!["--backup-dir", "saved", "--web-port", "0"],
            vec!["--backup-dir", "saved", "--restore-from", "saved"],
        ] {
            assert!(parse(args(&invalid)).is_err(), "{invalid:?}");
        }
    }
    #[test]
    fn settings_file_is_bounded_and_requires_an_object() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("web.json");
        std::fs::write(&path, r#"{"local_port":9081,"remote":{"mode":"off"}}"#).unwrap();
        assert_eq!(
            web_settings(None, Some(path.clone())).unwrap().unwrap()["local_port"],
            9081
        );
        for invalid in [b"[]".to_vec(), vec![b' '; 32769]] {
            std::fs::write(&path, invalid).unwrap();
            assert!(web_settings(None, Some(path.clone())).is_err());
        }
    }
}

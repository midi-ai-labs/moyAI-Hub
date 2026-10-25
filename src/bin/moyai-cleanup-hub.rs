//! Explicit reset of one stopped Hub profile; installed modules and workspaces stay in place.
use std::{ffi::OsString, path::PathBuf, process::ExitCode};

const HELP: &str = "moyai-cleanup-hub\n  --preview\n  --reset-user-data --confirm-delete [--data-dir PATH]\n\nWindowsで引数なし起動すると削除対象を表示して確認します。\nHubの設定・端末登録・共有会話・保存した成果を初期化します。\nHub/Gatewayと関連する仕事を停止してください。端末は再参加が必要です。\nDesktopの保存データ、導入済み実行ファイル、作業フォルダーは削除しません。\n";

#[derive(Default)]
struct Options {
    data: Option<PathBuf>,
    preview: bool,
    reset: bool,
    confirmed: bool,
    help: bool,
    version: bool,
}
fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--preview") if !options.preview => options.preview = true,
            Some("--reset-user-data") if !options.reset => options.reset = true,
            Some("--confirm-delete") if !options.confirmed => options.confirmed = true,
            Some("--data-dir") if options.data.is_none() => {
                options.data = Some(PathBuf::from(
                    args.next()
                        .ok_or("--data-dirに保存先を指定してください。")?,
                ))
            }
            Some("--help" | "-h") if !options.help => options.help = true,
            Some("--version" | "-V") if !options.version => options.version = true,
            _ => {
                return Err(format!(
                    "不明または重複した引数です: {}",
                    arg.to_string_lossy()
                ));
            }
        }
    }
    if options.reset != options.confirmed || (options.preview && options.reset) {
        return Err(
            "確認なしの初期化には --reset-user-data --confirm-delete の両方を指定してください。"
                .into(),
        );
    }
    Ok(options)
}

#[cfg(windows)]
fn confirm(message: &str) -> Result<bool, String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let title = "moyAI Hub 保存データの初期化\0"
        .encode_utf16()
        .collect::<Vec<_>>();
    let message = format!("{message}\n\nこのHubの保存データを削除しますか？")
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    };
    if result == 0 {
        Err("確認画面を開けません。削除していません。".into())
    } else {
        Ok(result == IDYES)
    }
}
#[cfg(not(windows))]
fn confirm(_message: &str) -> Result<bool, String> {
    Err("--previewで対象を確認し、初期化する場合は --reset-user-data --confirm-delete を指定してください。".into())
}

fn pause() {
    println!("終了するにはキーを押してください。");
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let _ = std::process::Command::new(PathBuf::from(root).join("System32/cmd.exe"))
                .args(["/d", "/c", "pause >nul"])
                .status();
        }
    }
}

fn run(options: Options) -> Result<(), String> {
    let implicit = options.data.is_none() && std::env::var_os("MOYAI_HUB_DATA_DIR").is_none();
    let defaults = if implicit {
        Some(moyai_hub::data_paths::default_paths()?)
    } else {
        moyai_hub::data_paths::default_paths().ok()
    };
    let directory = moyai_hub::data_paths::resolve(options.data, false)?;
    let defaults = defaults.filter(|p| p.current == directory || p.legacy == directory);
    let message = format!(
        "削除対象: {}\n\nHubの設定、モデル接続情報、端末登録と鍵、共有会話、保存した成果を削除します。\n参加済みのPCは、このHubへ再参加する必要があります。\nDesktopの保存データ、Hub本体、プロジェクトの作業フォルダーは残します。\nHub・Gatewayと関連する仕事を停止してから実行してください。",
        directory.display()
    );
    println!("{message}");
    if options.preview {
        println!("確認のみです。削除していません。");
        return Ok(());
    }
    if !options.reset && !confirm(&message)? {
        println!("cleanupを取り消しました。削除していません。");
        return Ok(());
    }
    let removed = moyai_hub::maintenance::reset(&directory, defaults.as_ref())?;
    if removed {
        println!("cleanupが正常に完了しました。");
    } else {
        println!("cleanupが正常に完了しました。削除対象の保存データはありませんでした。");
    }
    Ok(())
}

fn main() -> ExitCode {
    let options = match parse(std::env::args_os().skip(1)) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}\n{HELP}");
            return ExitCode::from(2);
        }
    };
    if options.help {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if options.version {
        println!("moyai-cleanup-hub {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let interactive = !options.preview && !options.reset;
    let result = run(options);
    let exit = match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cleanupを完了できませんでした。\n{error}");
            ExitCode::FAILURE
        }
    };
    if interactive {
        pause();
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_reset_needs_both_flags_and_keeps_custom_profile() {
        assert!(parse([OsString::from("--reset-user-data")]).is_err());
        let value = parse(
            [
                "--reset-user-data",
                "--confirm-delete",
                "--data-dir",
                "C:/Hub Data",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert!(value.reset && value.confirmed);
        assert_eq!(value.data, Some(PathBuf::from("C:/Hub Data")));
        assert!(
            parse(["--preview", "--reset-user-data", "--confirm-delete"].map(OsString::from))
                .is_err()
        );
    }
}

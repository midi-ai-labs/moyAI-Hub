#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod desktop;

fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--gateway")) {
        let runtime = tokio::runtime::Runtime::new().expect("gateway runtime unavailable");
        if runtime
            .block_on(moyai_hub::gateway_process::run_companion())
            .is_err()
        {
            std::process::exit(1);
        }
        return;
    }
    desktop::run();
}

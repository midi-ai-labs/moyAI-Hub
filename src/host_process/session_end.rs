//! Notifications that Windows is ending the console, the sign-in session or the PC.
//!
//! The Hub host loads user32, and Windows does not call the console control
//! handler of such a process for sign-out or shutdown. Those reach a top-level
//! window as WM_ENDSESSION instead, so the host listens on both routes and
//! records which one fired.
#[cfg(windows)]
use std::sync::{Condvar, Mutex};
#[cfg(windows)]
use std::time::Duration;

pub(super) const CONSOLE_CLOSE: &str = "Windowsのコンソールを閉じる操作";
pub(super) const SIGN_OUT: &str = "Windowsのサインアウト";
pub(super) const SHUTDOWN: &str = "Windowsの終了";

pub(super) fn is_session_end(reason: &str) -> bool {
    [CONSOLE_CLOSE, SIGN_OUT, SHUTDOWN].contains(&reason)
}

/// Wait for Windows to end the console, the sign-in session or the PC. Never
/// completes when neither route can be prepared; the Hub keeps running and
/// records why, because these notifications only add a clean, recorded exit.
pub(super) async fn wait() -> &'static str {
    #[cfg(windows)]
    {
        let (sender, mut window_reason) = tokio::sync::mpsc::unbounded_channel();
        if let Err(error) = window::start(sender) {
            super::log_event(format_args!(
                "Windowsの終了・サインアウトの通知を受け取るウィンドウを準備できません。{error}"
            ));
        }
        use tokio::signal::windows::{ctrl_close, ctrl_logoff, ctrl_shutdown};
        let console =
            (|| Ok::<_, std::io::Error>((ctrl_close()?, ctrl_logoff()?, ctrl_shutdown()?)))();
        let (route, reason) = match console {
            Ok((mut close, mut logoff, mut shutdown)) => tokio::select! {
                _ = close.recv() => ("コンソールの通知", CONSOLE_CLOSE),
                _ = logoff.recv() => ("コンソールの通知", SIGN_OUT),
                _ = shutdown.recv() => ("コンソールの通知", SHUTDOWN),
                Some(reason) = window_reason.recv() => ("ウィンドウの通知", reason),
            },
            Err(error) => {
                super::log_event(format_args!(
                    "Windowsの終了・サインアウトのコンソールの通知を受け取れません。{error}"
                ));
                match window_reason.recv().await {
                    Some(reason) => ("ウィンドウの通知", reason),
                    None => std::future::pending().await,
                }
            }
        };
        // Record receipt first: Windows may end the process before the exit finishes.
        super::log_event(format_args!("{reason}の通知を受け取りました（{route}）。"));
        return reason;
    }
    #[allow(unreachable_code)]
    std::future::pending().await
}

/// Tell the window thread that the host finished its exit, so WM_ENDSESSION can return.
pub(super) fn exit_finished() {
    #[cfg(windows)]
    {
        let (finished, changed) = &window::FINISHED;
        *finished
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        changed.notify_all();
    }
}

#[cfg(windows)]
mod window {
    use super::*;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, ENDSESSION_LOGOFF, GetMessageW, MSG,
        RegisterClassW, TranslateMessage, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW,
    };

    type Sender = tokio::sync::mpsc::UnboundedSender<&'static str>;
    static SENDER: Mutex<Option<Sender>> = Mutex::new(None);
    pub(super) static FINISHED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
    /// Windows may end the process soon after WM_ENDSESSION returns; wait a
    /// little longer than the host's session-end exit deadline.
    const WAIT_FOR_EXIT: Duration = Duration::from_secs(5);

    unsafe extern "system" fn procedure(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            // The Hub never blocks the end of the session.
            WM_QUERYENDSESSION => 1,
            WM_ENDSESSION if wparam != 0 => {
                let reason = if (lparam as u32) & ENDSESSION_LOGOFF != 0 {
                    SIGN_OUT
                } else {
                    SHUTDOWN
                };
                let sender = SENDER
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                if let Some(sender) = sender
                    && sender.send(reason).is_ok()
                {
                    let (finished, changed) = &FINISHED;
                    let guard = finished
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let _ = changed.wait_timeout_while(guard, WAIT_FOR_EXIT, |done| !*done);
                }
                0
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    /// Create a hidden top-level window on its own thread. Message-only
    /// windows do not receive WM_ENDSESSION, so this window has no parent.
    pub(super) fn start(sender: Sender) -> Result<(), String> {
        *SENDER
            .lock()
            .map_err(|_| "通知の受け取り先を準備できません。")? = Some(sender);
        let (ready, created) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("hub-session-end".into())
            .spawn(move || {
                let class: Vec<u16> = "moyAIHubSessionEnd\0".encode_utf16().collect();
                let registration = WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    lpszClassName: class.as_ptr(),
                    ..Default::default()
                };
                let window = unsafe {
                    if RegisterClassW(&registration) == 0 {
                        std::ptr::null_mut()
                    } else {
                        CreateWindowExW(
                            0,
                            class.as_ptr(),
                            class.as_ptr(),
                            0,
                            0,
                            0,
                            0,
                            0,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            std::ptr::null(),
                        )
                    }
                };
                let _ = ready.send(!window.is_null());
                if window.is_null() {
                    return;
                }
                let mut message: MSG = unsafe { std::mem::zeroed() };
                while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
                    unsafe {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            })
            .map_err(|_| "通知を受け取るスレッドを開始できません。")?;
        match created.recv() {
            Ok(true) => Ok(()),
            _ => Err("通知を受け取るウィンドウを作成できません。".into()),
        }
    }
}

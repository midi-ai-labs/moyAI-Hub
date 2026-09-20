//! Explicit logon startup for this OS user and exact Hub profile, never a service.
use super::*;
use sha2::{Digest, Sha256};

fn registration(executable: &Path, directory: &Path) -> Result<(String, String), String> {
    let executable = executable
        .to_str()
        .ok_or("実行ファイルの場所を確認できません。")?;
    let directory = directory.to_str().ok_or("保存先を確認できません。")?;
    if executable.contains(['"', '\r', '\n']) || directory.contains(['"', '\r', '\n']) {
        return Err("自動開始のパスを確認してください。".into());
    }
    let command = format!("\"{executable}\" --launch --no-browser --data-dir \"{directory}\"");
    if command.encode_utf16().count() > 260 {
        return Err(
            "自動開始コマンドがWindowsの上限を超えています。短い設置先と保存先を使用してください。"
                .into(),
        );
    }
    #[cfg(windows)]
    let identity = directory
        .replace('/', "\\")
        .trim_start_matches("\\\\?\\")
        .to_lowercase();
    #[cfg(not(windows))]
    let identity = directory.to_owned();
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    Ok((format!("moyAI Hub {}", &digest[..16]), command))
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::{Foundation::*, System::Registry::*};
    const PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
    fn open(write: bool) -> Result<Option<Key>, String> {
        let mut raw = std::ptr::null_mut();
        let status = if write {
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    wide(PATH).as_ptr(),
                    0,
                    std::ptr::null(),
                    0,
                    KEY_SET_VALUE,
                    std::ptr::null(),
                    &mut raw,
                    std::ptr::null_mut(),
                )
            }
        } else {
            unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    wide(PATH).as_ptr(),
                    0,
                    KEY_QUERY_VALUE,
                    &mut raw,
                )
            }
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err("サインイン時の自動起動の登録を開けません。".into());
        }
        Ok(Some(Key(raw)))
    }
    pub(super) fn read(name: &str) -> Result<Option<String>, String> {
        let Some(key) = open(false)? else {
            return Ok(None);
        };
        let mut size = 0;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                wide(name).as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS || size > 4096 || size % 2 != 0 {
            return Err("サインイン時の自動起動の登録を読み取れません。".into());
        }
        let mut value = vec![0u16; (size as usize / 2).max(1)];
        let mut kind = 0;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                wide(name).as_ptr(),
                std::ptr::null(),
                &mut kind,
                value.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS || kind != REG_SZ {
            return Err("サインイン時の自動起動の登録形式を確認できません。".into());
        }
        while value.last() == Some(&0) {
            value.pop();
        }
        String::from_utf16(&value)
            .map(Some)
            .map_err(|_| "サインイン時の自動起動の登録を読み取れません。".into())
    }
    pub(super) fn write(name: &str, command: Option<&str>) -> Result<(), String> {
        let key = open(true)?.ok_or("サインイン時の自動起動の登録を開けません。")?;
        let status = if let Some(command) = command {
            let value = wide(command);
            unsafe {
                RegSetValueExW(
                    key.0,
                    wide(name).as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            }
        } else {
            unsafe { RegDeleteValueW(key.0, wide(name).as_ptr()) }
        };
        if status != ERROR_SUCCESS && !(command.is_none() && status == ERROR_FILE_NOT_FOUND) {
            return Err("サインイン時の自動起動の変更を保存できません。".into());
        }
        Ok(())
    }
}

pub(super) fn snapshot(admin: &Admin) -> Result<Value, String> {
    let directory = std::fs::canonicalize(admin.path.parent().ok_or("保存先がありません。")?)
        .map_err(|_| "保存先を確認できません。")?;
    let executable = std::env::current_exe().map_err(|_| "実行ファイルを確認できません。")?;
    let (name, command) = registration(&executable, &directory)?;
    #[cfg(windows)]
    let existing = registry::read(&name)?;
    #[cfg(not(windows))]
    let existing: Option<String> = None;
    Ok(
        json!({"supported":cfg!(windows),"enabled":existing.as_deref()==Some(&command),"conflict":existing.is_some() && existing.as_deref()!=Some(&command),"command":command,"name":name}),
    )
}
pub(super) fn set(admin: &Admin, enabled: bool, expected_command: String) -> Result<Value, String> {
    let snapshot = snapshot(admin)?;
    if snapshot["supported"] != true {
        return Err("サインイン時の自動起動はWindowsで利用できます。".into());
    }
    let command = snapshot["command"]
        .as_str()
        .ok_or("開始方法を確認できません。")?;
    if command != expected_command || snapshot["conflict"] == true {
        return Err(
            "起動先または既存の自動開始登録が変わりました。最新情報を確認してください。".into(),
        );
    }
    #[cfg(windows)]
    registry::write(
        snapshot["name"]
            .as_str()
            .ok_or("登録名を確認できません。")?,
        enabled.then_some(command),
    )?;
    #[cfg(not(windows))]
    let _ = enabled;
    self::snapshot(admin)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logon_command_keeps_profile_and_never_opens_a_browser() {
        let (a, command) =
            registration(Path::new("C:/Hub/moyai-hub.exe"), Path::new("C:/Hub Data")).unwrap();
        assert!(command.contains("--launch --no-browser --data-dir \"C:/Hub Data\""));
        let (b, _) =
            registration(Path::new("C:/Hub/moyai-hub.exe"), Path::new("C:/Other")).unwrap();
        assert_ne!(a, b);
        assert!(registration(Path::new("bad\"path"), Path::new("data")).is_err());
    }
}

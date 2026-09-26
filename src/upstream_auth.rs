//! Provider credentials stay on the Hub host, outside catalogs and backup inventories.
use crate::host_process::private_file;
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const DIRECTORY: &str = "upstream-credentials";
const MAX_FILE_BYTES: u64 = 16 * 1024;
const INVALID_KEY: &str = "APIキーは空白や改行を含めず、4096文字以内で入力してください。";
const UNAVAILABLE: &str = "保存済みのAPIキーを使用できません。HubでAPIキーを設定し直してください。";

// No Debug implementation: credential material must not enter diagnostics.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    schema_version: u32,
    endpoint: String,
    provider_profile: String,
    key: String,
}

pub fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > 4096 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(INVALID_KEY.into());
    }
    Ok(())
}

fn normalized_endpoint(endpoint: &str, profile: &str) -> Result<String, String> {
    if crate::provider::ProviderProfile::parse(profile).is_none() {
        return Err(UNAVAILABLE.into());
    }
    let endpoint = crate::discovery::normalize_endpoint(endpoint).map_err(|_| UNAVAILABLE)?;
    if endpoint.len() > 2048 {
        return Err(UNAVAILABLE.into());
    }
    Ok(endpoint)
}

fn credential_path(directory: &Path, id: &str) -> Result<PathBuf, String> {
    let parsed = id.parse::<ulid::Ulid>().map_err(|_| UNAVAILABLE)?;
    if parsed.to_string() != id {
        return Err(UNAVAILABLE.into());
    }
    Ok(directory.join(DIRECTORY).join(format!("{id}.json")))
}

fn regular(metadata: &fs::Metadata, directory: bool) -> bool {
    if metadata.file_type().is_symlink()
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

fn check_directory(directory: &Path) -> Result<(), String> {
    if !regular(
        &fs::symlink_metadata(directory).map_err(|_| UNAVAILABLE)?,
        true,
    ) {
        return Err(UNAVAILABLE.into());
    }
    Ok(())
}

/// Create a new immutable credential before a catalog starts referring to it.
pub fn store(directory: &Path, endpoint: &str, profile: &str, key: &str) -> Result<String, String> {
    validate_key(key)?;
    let endpoint = normalized_endpoint(endpoint, profile)?;
    let private_directory = directory.join(DIRECTORY);
    match fs::create_dir(&private_directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("APIキーの保存先を作成できません。".into()),
    }
    check_directory(&private_directory)?;
    let id = ulid::Ulid::new().to_string();
    let destination = credential_path(directory, &id)?;
    let mut temporary = tempfile::NamedTempFile::new_in(&private_directory)
        .map_err(|_| "APIキーの保存を準備できません。")?;
    // The empty handle is protected before any key bytes reach disk.
    private_file::protect(temporary.as_file())
        .map_err(|_| "APIキーをこのPCの利用者だけが読める状態で保存できません。")?;
    serde_json::to_writer(
        &mut temporary,
        &Credential {
            schema_version: 1,
            endpoint,
            provider_profile: profile.into(),
            key: key.into(),
        },
    )
    .map_err(|_| "APIキーを書き込めません。")?;
    temporary
        .flush()
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| "APIキーを保存できません。")?;
    temporary
        .persist_noclobber(destination)
        .map_err(|_| "APIキーを保存できません。")?;
    Ok(id)
}

fn open_credential(path: &Path) -> Result<File, String> {
    check_directory(path.parent().ok_or(UNAVAILABLE)?)?;
    if !regular(&fs::symlink_metadata(path).map_err(|_| UNAVAILABLE)?, false) {
        return Err(UNAVAILABLE.into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    let file = options.open(path).map_err(|_| UNAVAILABLE)?;
    let metadata = file.metadata().map_err(|_| UNAVAILABLE)?;
    if !regular(&metadata, false) || metadata.len() > MAX_FILE_BYTES {
        return Err(UNAVAILABLE.into());
    }
    private_file::verify(&file).map_err(|_| UNAVAILABLE)?;
    Ok(file)
}

/// Missing or unusable configured credentials never become an unauthenticated request.
pub fn resolve(
    directory: &Path,
    credential_id: Option<&str>,
    endpoint: &str,
    profile: &str,
) -> Result<Option<HeaderValue>, String> {
    let Some(id) = credential_id else {
        return Ok(None);
    };
    let endpoint = normalized_endpoint(endpoint, profile)?;
    let path = credential_path(directory, id)?;
    let mut bytes = Vec::new();
    open_credential(&path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| UNAVAILABLE)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(UNAVAILABLE.into());
    }
    let credential: Credential = serde_json::from_slice(&bytes).map_err(|_| UNAVAILABLE)?;
    if credential.schema_version != 1
        || credential.endpoint != endpoint
        || credential.provider_profile != profile
    {
        return Err(UNAVAILABLE.into());
    }
    validate_key(&credential.key).map_err(|_| UNAVAILABLE)?;
    let mut header =
        HeaderValue::from_str(&format!("Bearer {}", credential.key)).map_err(|_| UNAVAILABLE)?;
    header.set_sensitive(true);
    Ok(Some(header))
}

/// Remove an unreferenced immutable entry; absence is already the desired result.
pub fn remove(directory: &Path, id: &str) -> Result<(), String> {
    let path = credential_path(directory, id)?;
    let parent = path.parent().ok_or(UNAVAILABLE)?;
    if !parent.exists() {
        return Ok(());
    }
    check_directory(parent)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if regular(&metadata, false) => {}
        Ok(_) => return Err(UNAVAILABLE.into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(UNAVAILABLE.into()),
    }
    fs::remove_file(path).map_err(|_| "保存済みのAPIキーを削除できません。".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENDPOINT: &str = "http://127.0.0.1:8119";
    const PROFILE: &str = "openai_compatible_chat";
    const KEY: &str = "test-provider-key-secret";

    #[test]
    fn stored_credentials_reload_with_private_permissions_and_redacted_headers() {
        let directory = tempfile::tempdir().unwrap();
        let id = store(directory.path(), &format!("{ENDPOINT}/v1/"), PROFILE, KEY).unwrap();
        let file = File::open(credential_path(directory.path(), &id).unwrap()).unwrap();
        private_file::verify(&file).unwrap();
        let header = resolve(directory.path(), Some(&id), ENDPOINT, PROFILE)
            .unwrap()
            .unwrap();
        assert_eq!(header.to_str().unwrap(), format!("Bearer {KEY}"));
        assert!(header.is_sensitive());
        assert!(!format!("{header:?}").contains(KEY));
        let second = store(directory.path(), ENDPOINT, PROFILE, "replacement-key").unwrap();
        assert_ne!(id, second);
        assert_eq!(
            resolve(directory.path(), Some(&id), ENDPOINT, PROFILE)
                .unwrap()
                .unwrap(),
            header
        );
    }

    #[test]
    fn configured_credentials_fail_closed_when_missing_mismatched_or_removed() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            resolve(directory.path(), None, ENDPOINT, PROFILE)
                .unwrap()
                .is_none()
        );
        let id = store(directory.path(), ENDPOINT, PROFILE, KEY).unwrap();
        for (endpoint, profile) in [
            ("http://127.0.0.1:8118", PROFILE),
            (ENDPOINT, "openai_compatible_responses"),
        ] {
            let error = resolve(directory.path(), Some(&id), endpoint, profile).unwrap_err();
            assert!(!error.contains(KEY));
        }
        remove(directory.path(), &id).unwrap();
        remove(directory.path(), &id).unwrap();
        assert!(resolve(directory.path(), Some(&id), ENDPOINT, PROFILE).is_err());
    }

    #[test]
    fn unsafe_keys_and_ids_are_rejected_without_leaking_input() {
        let directory = tempfile::tempdir().unwrap();
        for key in [
            "",
            "a b",
            "key\nvalue",
            "key\rvalue",
            "キー",
            &"x".repeat(4097),
        ] {
            assert!(validate_key(key).is_err());
            assert!(store(directory.path(), ENDPOINT, PROFILE, key).is_err());
        }
        for id in ["../catalog", "C:\\outside", "not-an-id"] {
            assert!(resolve(directory.path(), Some(id), ENDPOINT, PROFILE).is_err());
            assert!(remove(directory.path(), id).is_err());
        }
        assert!(!directory.path().join(DIRECTORY).exists());
    }

    #[test]
    fn invalid_or_oversized_stored_credentials_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let id = store(directory.path(), ENDPOINT, PROFILE, KEY).unwrap();
        let path = credential_path(directory.path(), &id).unwrap();
        for body in [
            format!("{{\"key\":\"{KEY}\"}}"),
            "x".repeat(MAX_FILE_BYTES as usize + 1),
            serde_json::json!({"schema_version":1,"endpoint":ENDPOINT,"provider_profile":PROFILE,"key":"invalid\nkey"}).to_string(),
        ] {
            fs::write(&path, body).unwrap();
            let error = resolve(directory.path(), Some(&id), ENDPOINT, PROFILE).unwrap_err();
            assert!(!error.contains(KEY));
        }
    }

    #[cfg(unix)]
    #[test]
    fn shared_permissions_and_symbolic_links_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().unwrap();
        let id = store(directory.path(), ENDPOINT, PROFILE, KEY).unwrap();
        let path = credential_path(directory.path(), &id).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(resolve(directory.path(), Some(&id), ENDPOINT, PROFILE).is_err());
        let target = directory.path().join("saved-key.json");
        fs::rename(&path, &target).unwrap();
        symlink(&target, &path).unwrap();
        assert!(resolve(directory.path(), Some(&id), ENDPOINT, PROFILE).is_err());
        assert!(remove(directory.path(), &id).is_err());
        assert!(target.exists());
    }
}

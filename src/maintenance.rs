//! Offline profile maintenance shares lifetime exclusion with Hub, launcher and Gateway.
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}

fn regular_metadata(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(error)?;
    if metadata.file_type().is_symlink() {
        return Err(format!("リンク先は初期化しません: {}", path.display()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(format!(
                "再解析ポイントは初期化しません: {}",
                path.display()
            ));
        }
    }
    Ok(metadata)
}

fn check_ancestors(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        if ancestor.try_exists().map_err(error)? {
            regular_metadata(ancestor)?;
        }
    }
    Ok(())
}

pub fn check_regular_file(path: &Path) -> Result<(), String> {
    check_ancestors(path)?;
    if !regular_metadata(path)?.is_file() {
        return Err(format!("通常ファイルではありません: {}", path.display()));
    }
    Ok(())
}

pub fn check_regular_tree(path: &Path) -> Result<(), String> {
    check_ancestors(path)?;
    let mut pending = vec![(path.to_path_buf(), 0usize)];
    let mut count = 0usize;
    while let Some((path, depth)) = pending.pop() {
        count += 1;
        if count > 100_005 || depth > 64 {
            return Err("Hub保存先の項目数・階層が上限を超えています。".into());
        }
        let metadata = regular_metadata(&path)?;
        if metadata.is_dir() {
            for child in fs::read_dir(&path).map_err(error)? {
                pending.push((child.map_err(error)?.path(), depth + 1));
            }
        } else if !metadata.is_file() {
            return Err(format!(
                "保存先に通常ファイル以外の項目があります: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

/// The lock lives outside the deletable profile. A new process cannot recreate a profile during cleanup.
pub struct ProfileLease {
    _file: File,
}
impl ProfileLease {
    fn acquire(directory: &Path, exclusive: bool) -> Result<Self, String> {
        let directory = crate::data_paths::absolute(directory.to_path_buf())?;
        let parent = directory
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or("Hub専用の保存先を指定してください。")?;
        let name = directory
            .file_name()
            .ok_or("Hub専用の保存先を指定してください。")?;
        check_ancestors(parent)?;
        fs::create_dir_all(parent).map_err(error)?;
        let canonical = fs::canonicalize(parent).map_err(error)?.join(name);
        let identity = canonical.to_string_lossy();
        #[cfg(windows)]
        let identity = identity.to_lowercase();
        let hash = format!("{:x}", Sha256::digest(identity.as_bytes()));
        let lock = parent.join(format!(".moyai-hub-profile-{hash}.lock"));
        check_ancestors(&lock)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x00200000);
        }
        let file = options.open(lock).map_err(error)?;
        if exclusive {
            FileExt::try_lock_exclusive(&file)
        } else {
            FileExt::try_lock_shared(&file)
        }
        .map_err(|_| {
            "Hub保存先を使用中です。Hub・Gateway・起動処理を終了してからやり直してください。"
                .to_owned()
        })?;
        Ok(Self { _file: file })
    }
    pub fn shared(directory: &Path) -> Result<Self, String> {
        crate::data_paths::require_profile_access(directory)?;
        let lease = Self::acquire(directory, false)?;
        crate::data_paths::require_profile_access(directory)?;
        Ok(lease)
    }
    fn exclusive(directory: &Path) -> Result<Self, String> {
        Self::acquire(directory, true)
    }
}

pub struct OfflineGuard {
    _lease: ProfileLease,
    _locks: Vec<File>,
    _files: Vec<(std::path::PathBuf, File)>,
}
impl OfflineGuard {
    pub fn acquire(directory: &Path, require_work_stopped: bool) -> Result<Self, String> {
        let lease = ProfileLease::exclusive(directory)?;
        let mut guard = Self {
            _lease: lease,
            _locks: Vec::new(),
            _files: Vec::new(),
        };
        if !directory.try_exists().map_err(error)? {
            return Ok(guard);
        }
        check_regular_tree(directory)?;
        for name in ["hub.lock", "host-launch.lock", "gateway-execution.lock"] {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(directory.join(name))
                .map_err(error)?;
            file.try_lock_exclusive().map_err(|_| {
                "HubまたはGatewayが稼働中です。停止してからやり直してください。".to_owned()
            })?;
            guard._locks.push(file);
        }
        if require_work_stopped {
            require_clear_work(directory)?;
        }
        hold_tree_files(directory, &mut guard._files)?;
        Ok(guard)
    }

    pub(crate) fn ensure_directory(&mut self, directory: &Path) -> Result<(), String> {
        if !directory.exists() {
            fs::create_dir(directory).map_err(error)?;
        }
        check_regular_tree(directory)?;
        if self._locks.is_empty() {
            for name in ["hub.lock", "host-launch.lock", "gateway-execution.lock"] {
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(directory.join(name))
                    .map_err(error)?;
                file.try_lock_exclusive().map_err(|_| {
                    "移行先のHubまたはGatewayが稼働中です。移行していません。".to_owned()
                })?;
                self._locks.push(file);
            }
        }
        Ok(())
    }

    /// Payload files must already be moved/removed. Never recursively erase a recreated namespace.
    pub(crate) fn remove_empty_directory(&mut self, directory: &Path) -> Result<(), String> {
        for entry in fs::read_dir(directory).map_err(error)? {
            let entry = entry.map_err(error)?;
            if !entry.file_type().map_err(error)?.is_file()
                || !matches!(
                    entry.file_name().to_str(),
                    Some("hub.lock" | "host-launch.lock" | "gateway-execution.lock")
                )
            {
                return Err(format!(
                    "旧保存先に別の項目が現れたため、上書き・削除しません: {}",
                    entry.path().display()
                ));
            }
        }
        for name in ["hub.lock", "host-launch.lock", "gateway-execution.lock"] {
            let path = directory.join(name);
            if path.exists() {
                fs::remove_file(path).map_err(error)?;
            }
        }
        self._locks.clear();
        fs::remove_dir(directory).map_err(error)
    }

    pub(crate) fn release_file_pin(&mut self, logical_path: &Path) -> bool {
        let Some(index) = self
            ._files
            .iter()
            .position(|(path, _)| path == logical_path)
        else {
            return false;
        };
        self._files.remove(index);
        true
    }

    pub(crate) fn pin_relocated_file(
        &mut self,
        logical_path: &Path,
        actual_path: &Path,
    ) -> Result<(), String> {
        check_regular_file(actual_path)?;
        let file = pin_file(actual_path)?;
        self._files.push((logical_path.to_path_buf(), file));
        Ok(())
    }
}

fn pin_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1 | 4).custom_flags(0x00200000);
    }
    options.open(path).map_err(|e| {
        format!(
            "保存データが使用中か、読み取れません: {}: {e}",
            path.display()
        )
    })
}

fn hold_tree_files(
    directory: &Path,
    files: &mut Vec<(std::path::PathBuf, File)>,
) -> Result<(), String> {
    for child in fs::read_dir(directory).map_err(error)? {
        let child = child.map_err(error)?;
        let path = child.path();
        if child.file_type().map_err(error)?.is_dir() {
            hold_tree_files(&path, files)?;
            continue;
        }
        if matches!(
            child.file_name().to_str(),
            Some("hub.lock" | "host-launch.lock" | "gateway-execution.lock")
        ) {
            continue;
        }
        let file = pin_file(&path)?;
        files.push((path, file));
    }
    Ok(())
}

fn table(db: &rusqlite::Connection, name: &str) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get(0),
    )
    .map_err(error)
}

fn require_clear_work(directory: &Path) -> Result<(), String> {
    let catalog = directory.join("catalog.json");
    let network = directory.join("device-network.json");
    if catalog.exists() && network.exists() {
        if fs::metadata(&catalog).map_err(error)?.len() > 4 * 1024 * 1024 {
            return Err("Hubカタログがサイズ上限を超え、仕事の停止を確認できません。".into());
        }
        let catalog: crate::catalog::CatalogStore =
            serde_json::from_slice(&fs::read(&catalog).map_err(error)?)
                .map_err(|_| "Hubの資格と仕事の停止状態を確認できません。")?;
        let network = crate::network::DeviceNetwork::inspect_existing(&network, &catalog.hub_id)
            .map_err(|_| "端末登録と仕事の停止状態を確認できません。")?;
        if network.has_unsettled_saved_work() {
            return Err("旧接続方式で受け付けた仕事の停止を確認できません。該当するRunnerと仕事を清算してから初期化してください。".into());
        }
    }
    let path = directory.join("shared-work.sqlite3");
    if !path.exists() {
        if directory.join("catalog.json").exists() && directory.join("device-network.json").exists()
        {
            return Err(
                "Hubの会話DBがなく、共有の仕事の停止状態を確認できません。初期化は行っていません。"
                    .into(),
            );
        }
        return Ok(());
    }
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(error)?;
    db.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(error)?;
    let integrity: String = db
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(error)?;
    if integrity != "ok" {
        return Err("Hubの会話DBの整合性を確認できません。保存データは削除していません。".into());
    }
    if table(&db, "shared_jobs")? && db.query_row("SELECT EXISTS(SELECT 1 FROM shared_jobs WHERE state NOT IN ('succeeded','failed','cancelled'))", [], |r| r.get::<_, bool>(0)).map_err(error)? {
        return Err("待機中・実行中・子の完了待ち・取消処理中の共有の仕事があります。Hubの管理画面で完了または取消を確認してから初期化してください。".into());
    }
    if table(&db, "shared_attempts")? {
        let has_uncertainty: bool = db
            .prepare("PRAGMA table_info(shared_attempts)")
            .map_err(error)?
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)?
            .iter()
            .any(|c| c == "uncertainty_reason");
        let sql = if has_uncertainty {
            "SELECT EXISTS(SELECT 1 FROM shared_attempts WHERE state IN ('assigned','running','stop_requested') OR uncertainty_reason IS NOT NULL)"
        } else {
            "SELECT EXISTS(SELECT 1 FROM shared_attempts WHERE state IN ('assigned','running','stop_requested'))"
        };
        if db
            .query_row(sql, [], |r| r.get::<_, bool>(0))
            .map_err(error)?
        {
            return Err("実行中または停止状態を確認できない共有の仕事があります。Hubを起動して仕事とRunnerの停止を確認してから初期化してください。".into());
        }
    }
    if table(&db, "shared_service_leases")? && db.query_row("SELECT EXISTS(SELECT 1 FROM shared_service_leases WHERE state IN ('active','uncertain'))", [], |r| r.get::<_, bool>(0)).map_err(error)? {
        return Err("停止を確認できない保持中のアプリがあります。Hubで停止を確認してから初期化してください。".into());
    }
    Ok(())
}

/// Remove only one explicitly selected, stopped Hub profile. Workspaces are not traversed by stored paths.
pub fn reset(
    directory: &Path,
    default: Option<&crate::data_paths::DefaultPaths>,
) -> Result<bool, String> {
    let directory = crate::data_paths::absolute(directory.to_path_buf())?;
    crate::data_paths::require_profile_access(&directory)?;
    let allowed_default = default.is_some_and(|p| directory == p.current || directory == p.legacy);
    if !allowed_default {
        validate_custom_target(&directory)?;
    }
    let _guard = OfflineGuard::acquire(&directory, true)?;
    crate::data_paths::require_profile_access(&directory)?;
    if !directory.exists() {
        if let Some(default) = default {
            crate::data_paths::retire_legacy(default)?;
        }
        return Ok(false);
    }
    if !allowed_default {
        validate_custom_target(&directory)?;
    }
    if directory.join("catalog.json").exists() {
        crate::admin::require_profile_firewall_removed(&directory)?;
    }
    crate::admin::retire_profile_autostart(&directory)?;
    fs::remove_dir_all(&directory).map_err(|e| format!("Hub保存データの削除を完了できませんでした。残った場所を確認して再実行してください: {}: {e}", directory.display()))?;
    if directory.exists() {
        return Err("Hub保存先が残っています。正常完了として扱いません。".into());
    }
    if let Some(default) = default {
        crate::data_paths::retire_legacy(default)?;
    }
    Ok(true)
}

fn validate_custom_target(directory: &Path) -> Result<(), String> {
    check_regular_tree(directory)?;
    let catalog = directory.join("catalog.json");
    check_regular_file(&catalog)?;
    if fs::metadata(&catalog).map_err(error)?.len() > 4 * 1024 * 1024 {
        return Err("Hubカタログがサイズ上限を超えています。削除していません。".into());
    }
    let store: crate::catalog::CatalogStore = serde_json::from_slice(
        &fs::read(&catalog).map_err(error)?,
    )
    .map_err(|_| "このフォルダーがHubの保存先であることを確認できません。削除していません。")?;
    store
        .validate()
        .map_err(|_| "Hubカタログの整合性を確認できません。削除していません。")?;
    let network = directory.join("device-network.json");
    check_regular_file(&network)?;
    crate::network::DeviceNetwork::inspect_existing(&network, &store.hub_id)
        .map_err(|_| "Hubカタログと端末登録・資格の一致を確認できません。削除していません。")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> tempfile::TempDir {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("project_sandbox/hub-cleanup-tests");
        fs::create_dir_all(&parent).unwrap();
        tempfile::tempdir_in(parent).unwrap()
    }
    #[test]
    fn profile_lifetime_excludes_reset_and_reset_preserves_sibling_data() {
        let tmp = fixture();
        let profile = tmp.path().join("hub");
        let desktop = tmp.path().join("desktop");
        fs::create_dir(&profile).unwrap();
        fs::create_dir(&desktop).unwrap();
        fs::create_dir(profile.join("assets")).unwrap();
        fs::write(profile.join("assets/fixture.bin"), b"saved Hub asset").unwrap();
        fs::write(desktop.join("keep"), b"desktop").unwrap();
        let paths = crate::data_paths::DefaultPaths {
            current: profile.clone(),
            legacy: tmp.path().join("legacy"),
            marker: tmp.path().join("migrations/hub.json"),
        };
        let lease = ProfileLease::shared(&profile).unwrap();
        assert!(reset(&profile, Some(&paths)).is_err());
        drop(lease);
        assert!(reset(&profile, Some(&paths)).unwrap());
        assert!(!profile.exists());
        assert_eq!(fs::read(desktop.join("keep")).unwrap(), b"desktop");
        assert!(paths.marker.exists());
        assert!(!reset(&profile, Some(&paths)).unwrap());
    }
    #[test]
    fn uncertain_attempts_and_retained_services_refuse_deletion() {
        let tmp = fixture();
        let profile = tmp.path().join("hub");
        fs::create_dir(&profile).unwrap();
        let db = rusqlite::Connection::open(profile.join("shared-work.sqlite3")).unwrap();
        db.execute_batch("CREATE TABLE shared_attempts(state TEXT,uncertainty_reason TEXT); INSERT INTO shared_attempts VALUES('failed','outcome_unknown'); CREATE TABLE shared_service_leases(state TEXT);").unwrap();
        drop(db);
        assert!(OfflineGuard::acquire(&profile, true).is_err());
        let db = rusqlite::Connection::open(profile.join("shared-work.sqlite3")).unwrap();
        db.execute_batch(
            "DELETE FROM shared_attempts; INSERT INTO shared_service_leases VALUES('active');",
        )
        .unwrap();
        drop(db);
        assert!(OfflineGuard::acquire(&profile, true).is_err());
        assert!(profile.join("shared-work.sqlite3").exists());
    }
    #[test]
    fn arbitrary_custom_directory_is_not_a_cleanup_target() {
        let tmp = fixture();
        let path = tmp.path().join("workspace");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep.txt"), b"workspace").unwrap();
        let before = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>();
        assert!(reset(&path, None).is_err());
        assert!(path.join("keep.txt").exists());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
        assert_eq!(
            fs::read_dir(tmp.path())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect::<Vec<_>>(),
            before
        );
    }
    #[test]
    fn custom_profile_identity_validation_is_read_only_and_matches_catalog() {
        let tmp = fixture();
        let profile = tmp.path().join("custom-hub");
        let admin = crate::admin::Admin::open(profile.join("catalog.json")).unwrap();
        drop(admin);
        let network = fs::read(profile.join("device-network.json")).unwrap();
        let catalog = fs::read(profile.join("catalog.json")).unwrap();
        validate_custom_target(&profile).unwrap();
        assert_eq!(
            fs::read(profile.join("device-network.json")).unwrap(),
            network
        );
        assert_eq!(fs::read(profile.join("catalog.json")).unwrap(), catalog);
        let mut value: serde_json::Value = serde_json::from_slice(&catalog).unwrap();
        value["hub_id"] = "different-hub".into();
        fs::write(
            profile.join("catalog.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(validate_custom_target(&profile).is_err());
        assert_eq!(
            fs::read(profile.join("device-network.json")).unwrap(),
            network
        );
    }
    #[test]
    fn pending_jobs_need_explicit_completion_or_cancellation_before_reset() {
        let tmp = fixture();
        let profile = tmp.path().join("hub");
        fs::create_dir(&profile).unwrap();
        let db = rusqlite::Connection::open(profile.join("shared-work.sqlite3")).unwrap();
        db.execute_batch(
            "CREATE TABLE shared_jobs(state TEXT); INSERT INTO shared_jobs VALUES('queued');",
        )
        .unwrap();
        drop(db);
        assert!(OfflineGuard::acquire(&profile, true).is_err());
        let db = rusqlite::Connection::open(profile.join("shared-work.sqlite3")).unwrap();
        db.execute_batch("UPDATE shared_jobs SET state='cancelled';")
            .unwrap();
        drop(db);
        assert!(OfflineGuard::acquire(&profile, true).is_ok());
    }
}

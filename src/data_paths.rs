//! The default profile has one current location. Legacy profiles move once; credentials are never copied.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct DefaultPaths {
    pub current: PathBuf,
    pub legacy: PathBuf,
    pub marker: PathBuf,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Migration {
    schema: u32,
    state: MigrationState,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum MigrationState {
    Moving,
    Retired,
}

pub fn windows_paths(local: &Path, roaming: &Path) -> DefaultPaths {
    let root = local.join("midi-ai-labs").join("moyai-data");
    DefaultPaths {
        current: root.join("hub"),
        legacy: roaming.join("ai.midi.moyai.hub"),
        marker: root.join("migrations").join("hub.json"),
    }
}

pub fn default_paths() -> Result<DefaultPaths, String> {
    #[cfg(windows)]
    {
        let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATAを確認できません。")?;
        let roaming = std::env::var_os("APPDATA").ok_or("APPDATAを確認できません。")?;
        Ok(windows_paths(Path::new(&local), Path::new(&roaming)))
    }
    #[cfg(not(windows))]
    {
        let base = if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|v| PathBuf::from(v).join("Library/Application Support"))
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".local/share")))
        }
        .ok_or("Specify --data-dir or MOYAI_HUB_DATA_DIR")?;
        let current = base.join("ai.midi.moyai.hub");
        Ok(DefaultPaths {
            legacy: current.clone(),
            marker: current.with_extension("migration.json"),
            current,
        })
    }
}

pub fn absolute(path: PathBuf) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("Hub data directory must not be empty".into());
    }
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path))
    }
}

pub fn resolve(explicit: Option<PathBuf>, migrate: bool) -> Result<PathBuf, String> {
    if let Some(path) =
        explicit.or_else(|| std::env::var_os("MOYAI_HUB_DATA_DIR").map(PathBuf::from))
    {
        let path = absolute(path)?;
        require_profile_access(&path)?;
        return Ok(path);
    }
    let paths = default_paths()?;
    if migrate {
        migrate_default(&paths)?;
    }
    locate(&paths)
}

fn nonempty(path: &Path) -> Result<bool, String> {
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(false);
    }
    // Resolution runs while Hub may be saving or collecting assets. Inspect
    // only the root here; offline migration/cleanup own the locked tree audit.
    crate::maintenance::check_regular_directory(path)?;
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !matches!(
            entry.file_name().to_str(),
            Some("hub.lock" | "host-launch.lock" | "gateway-execution.lock")
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn migration_state(paths: &DefaultPaths) -> Result<Option<MigrationState>, String> {
    if !paths.marker.try_exists().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    crate::maintenance::check_regular_file(&paths.marker)?;
    let bytes = fs::read(&paths.marker).map_err(|e| e.to_string())?;
    if bytes.len() > 4096 {
        return Err("Hub保存先の移行記録が不正です。".into());
    }
    let value: Migration =
        serde_json::from_slice(&bytes).map_err(|_| "Hub保存先の移行記録を確認できません。")?;
    if value.schema != 1 {
        return Err("Hub保存先の移行記録に対応していません。".into());
    }
    Ok(Some(value.state))
}

fn retired(paths: &DefaultPaths) -> Result<bool, String> {
    match migration_state(paths)? {
        Some(MigrationState::Moving) => Err(moving_message(paths)),
        Some(MigrationState::Retired) => Ok(true),
        None => Ok(false),
    }
}

/// Rules retain their original profile identity after the one-time default move.
pub(crate) fn firewall_profiles(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let directory = fs::canonicalize(directory).map_err(|e| e.to_string())?;
    let mut profiles = vec![directory.clone()];
    #[cfg(windows)]
    {
        let paths = default_paths()?;
        if same_path(&directory, &paths.current) && retired(&paths)? && !nonempty(&paths.legacy)? {
            profiles.push(paths.legacy);
        }
    }
    Ok(profiles)
}

fn moving_message(paths: &DefaultPaths) -> String {
    format!(
        "Hub保存先の移動が未完了です。欠けた資格・カタログを新規作成せず、起動・初期化を中止しました。旧 {} / 新 {} / 移行記録 {} を確認してください。自動で統合・再取込は行いません。",
        paths.legacy.display(),
        paths.current.display(),
        paths.marker.display()
    )
}

fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut lexical = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    lexical.pop();
                }
                _ => lexical.push(component.as_os_str()),
            }
        }
        let value = lexical.to_string_lossy();
        #[cfg(windows)]
        {
            value
                .replace('/', "\\")
                .trim_start_matches("\\\\?\\")
                .trim_end_matches('\\')
                .to_lowercase()
        }
        #[cfg(not(windows))]
        {
            value.into_owned()
        }
    };
    normalize(left) == normalize(right)
}

pub fn require_access(paths: &DefaultPaths, directory: &Path) -> Result<(), String> {
    if (same_path(directory, &paths.current) || same_path(directory, &paths.legacy))
        && migration_state(paths)? == Some(MigrationState::Moving)
    {
        return Err(moving_message(paths));
    }
    Ok(())
}

/// Explicit --data-dir and library owners must not bypass an interrupted default move.
pub fn require_profile_access(directory: &Path) -> Result<(), String> {
    #[cfg(windows)]
    if let Ok(paths) = default_paths() {
        require_access(&paths, directory)?;
    }
    #[cfg(not(windows))]
    let _ = directory;
    Ok(())
}

pub fn locate(paths: &DefaultPaths) -> Result<PathBuf, String> {
    if paths.current == paths.legacy {
        return Ok(paths.current.clone());
    }
    let legacy = nonempty(&paths.legacy)?;
    let current = nonempty(&paths.current)?;
    let was_retired = retired(paths)?;
    if legacy && (current || was_retired) {
        return Err(format!(
            "Hubの旧保存先にもデータが残っています。自動統合・再取込は行いません。旧: {} / 現在: {}。必要な内容を確認し、対象を --data-dir で明示してください。",
            paths.legacy.display(),
            paths.current.display()
        ));
    }
    Ok(if legacy {
        paths.legacy.clone()
    } else {
        paths.current.clone()
    })
}

pub fn retire_legacy(paths: &DefaultPaths) -> Result<(), String> {
    if paths.current == paths.legacy {
        return Ok(());
    }
    write_migration(paths, MigrationState::Retired)
}

fn write_migration(paths: &DefaultPaths, state: MigrationState) -> Result<(), String> {
    let parent = paths
        .marker
        .parent()
        .ok_or("Hub移行記録の場所が不正です。")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    crate::maintenance::check_regular_tree(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, &Migration { schema: 1, state }).map_err(|e| e.to_string())?;
    file.write_all(b"\n")
        .and_then(|_| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    file.persist(&paths.marker).map_err(|e| e.to_string())?;
    Ok(())
}

fn clear_moving(paths: &DefaultPaths) -> Result<(), String> {
    match migration_state(paths)? {
        Some(MigrationState::Moving) => {
            fs::remove_file(&paths.marker).map_err(|e| e.to_string())?;
        }
        Some(MigrationState::Retired) => {
            return Err("移行完了記録は削除しません。状態を確認してください。".into());
        }
        None => return Ok(()),
    }
    if let Some(parent) = paths.marker.parent() {
        if fs::read_dir(parent)
            .map_err(|e| e.to_string())?
            .next()
            .is_none()
        {
            let _ = fs::remove_dir(parent);
        }
    }
    Ok(())
}

pub fn migrate_default(paths: &DefaultPaths) -> Result<(), String> {
    if paths.current == paths.legacy {
        return Ok(());
    }
    // The ordinary launch lock is inside the destination profile, which must
    // not be created/populated before deciding whether to move the old one.
    // Serialize that decision outside both profiles, including its marker.
    let _migration_lock = migration_lock(paths)?;
    let selected = locate(paths)?;
    if selected != paths.legacy {
        // Mark an established current profile too: deleting it must never resurrect a later old profile.
        if nonempty(&paths.current)? && !retired(paths)? {
            retire_legacy(paths)?;
        }
        return Ok(());
    }
    let mut old_guard = crate::maintenance::OfflineGuard::acquire(&paths.legacy, false)?;
    let mut new_guard = crate::maintenance::OfflineGuard::acquire(&paths.current, false)?;
    if locate(paths)? != paths.legacy {
        return Err("Hub保存先が変更されました。やり直してください。".into());
    }
    new_guard.ensure_directory(&paths.current)?;
    // Old executable/profile rules are retained for explicit removal from the
    // new local management UI. They must not prevent access to that UI.
    let mut movement = ProfileMove::inventory(&paths.legacy, &paths.current)?;
    let web_paths =
        crate::web_admin::server::prepare_profile_relocation(&paths.legacy, &paths.current)?;
    // File renames retain quiescence handles on Windows; directory renames cannot do so.
    // The old three Hub locks stay in their namespace throughout the transaction.
    write_migration(paths, MigrationState::Moving)?;
    let autostart = match crate::admin::retire_profile_autostart(&paths.legacy) {
        Ok(registration) => registration,
        Err(error) => {
            clear_moving(paths).map_err(|clear| format!("自動起動を確認できず移行していません。移行記録の解除も完了できませんでした: {clear}。元の原因: {error}"))?;
            return Err(error);
        }
    };
    if let Err(error) = movement.execute() {
        let rollback = rollback_move(
            paths,
            &mut movement,
            &mut old_guard,
            &mut new_guard,
            autostart.as_ref(),
            None,
        );
        return Err(format!(
            "旧Hub保存先を移動できません。資格情報は複製していません。{rollback} 原因: {error}"
        ));
    }
    if let Some(relocation) = &web_paths {
        if let Err(error) = apply_web_paths(paths, &mut old_guard, relocation) {
            let rollback = rollback_move(
                paths,
                &mut movement,
                &mut old_guard,
                &mut new_guard,
                autostart.as_ref(),
                web_paths.as_ref(),
            );
            return Err(format!(
                "Hubの証明書パスを移行できませんでした。{rollback} 原因: {error}"
            ));
        }
    }
    if let Err(error) = retire_legacy(paths) {
        let rollback = rollback_move(
            paths,
            &mut movement,
            &mut old_guard,
            &mut new_guard,
            autostart.as_ref(),
            web_paths.as_ref(),
        );
        return Err(format!(
            "Hub保存先の移行記録を保存できませんでした。{rollback} 原因: {error}"
        ));
    }
    movement.release_placeholders();
    let old_cleanup = movement
        .remove_empty_source_directories()
        .and_then(|_| old_guard.remove_empty_directory(&paths.legacy));
    if let Err(error) = old_cleanup {
        eprintln!(
            "Hubデータの移行は完了しましたが、空になった旧保存先の整理を完了できませんでした。別の項目は削除していません: {}: {error}",
            paths.legacy.display()
        );
    }
    if autostart.is_some() {
        crate::admin::enable_profile_autostart(&paths.current).map_err(|e| format!("Hubデータの移行は {} へ完了しましたが、自動起動は再登録できませんでした。旧自動起動は解除済みです。新しいHub管理画面で自動起動を設定してください。{e}", paths.current.display()))?;
    }
    Ok(())
}

fn migration_lock(paths: &DefaultPaths) -> Result<File, String> {
    let parent = paths
        .marker
        .parent()
        .ok_or("Hub移行記録の場所が不正です。")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    crate::maintenance::check_regular_directory(parent)?;
    let path = paths.marker.with_extension("lock");
    if path.try_exists().map_err(|e| e.to_string())? {
        crate::maintenance::check_regular_file(&path)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(file),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                if std::time::Instant::now() >= deadline {
                    return Err(
                        "別のHubが保存先の移行を処理中です。少し待ってから開き直してください。"
                            .into(),
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => return Err(format!("Hub保存先の移行を待ち合わせできません: {error}")),
        }
    }
}

fn restore_autostart(registration: Option<&crate::admin::RetiredProfileAutostart>) -> String {
    match registration
        .map(crate::admin::restore_profile_autostart)
        .transpose()
    {
        Ok(_) if registration.is_some() => "元の自動起動登録は復元しました。".into(),
        Ok(_) => "自動起動登録の変更はありません。".into(),
        Err(error) => format!(
            "元の自動起動登録を復元できませんでした。Windowsの自動起動設定を確認してください: {error}"
        ),
    }
}

fn rollback_move(
    paths: &DefaultPaths,
    movement: &mut ProfileMove,
    old_guard: &mut crate::maintenance::OfflineGuard,
    new_guard: &mut crate::maintenance::OfflineGuard,
    registration: Option<&crate::admin::RetiredProfileAutostart>,
    web_paths: Option<&crate::web_admin::server::SavedPathRelocation>,
) -> String {
    if let Some(relocation) = web_paths {
        let logical = paths.legacy.join("web-management.json");
        old_guard.release_file_pin(&logical);
        if let Err(error) = relocation.restore(&paths.current) {
            return format!(
                "Web管理設定を元の内容へ戻せないため、ファイルの復元を中止しました。移動中の記録を残し、自動起動は停止した状態です。旧 {} / 新 {} を確認してください: {error}",
                paths.legacy.display(),
                paths.current.display()
            );
        }
        if let Err(error) =
            old_guard.pin_relocated_file(&logical, &paths.current.join("web-management.json"))
        {
            return format!(
                "復元したWeb管理設定の排他を確認できないため、ファイルの復元を中止しました。移動中の記録と両保存先を保持しました: {error}"
            );
        }
    }
    match movement.rollback() {
        Ok(()) => {
            if let Err(error) = clear_moving(paths) {
                return format!(
                    "データは旧保存先 {} へ戻しましたが、移動中の記録を解除できませんでした。自動起動は停止した状態です。移行記録 {} を確認してください: {error}",
                    paths.legacy.display(),
                    paths.marker.display()
                );
            }
            let cleanup = movement
                .remove_empty_destination_directories()
                .and_then(|_| new_guard.remove_empty_directory(&paths.current));
            let cleanup = cleanup
                .err()
                .map(|e| {
                    format!(
                        "移行先に残った項目は保持しました: {}: {e}",
                        paths.current.display()
                    )
                })
                .unwrap_or_default();
            format!(
                "データは旧保存先 {} へ戻しました。{}{cleanup}",
                paths.legacy.display(),
                restore_autostart(registration)
            )
        }
        Err(error) => format!(
            "旧保存先への復元を完了できませんでした。データは旧 {} と新 {} に分かれているため、自動起動は停止した状態です。上書きせず保存先を確認してください。復元エラー: {error}",
            paths.legacy.display(),
            paths.current.display()
        ),
    }
}

fn apply_web_paths(
    paths: &DefaultPaths,
    guard: &mut crate::maintenance::OfflineGuard,
    relocation: &crate::web_admin::server::SavedPathRelocation,
) -> Result<(), String> {
    let logical = paths.legacy.join("web-management.json");
    if !guard.release_file_pin(&logical) {
        return Err(
            "Web管理設定の排他ハンドルを確認できません。設定の更新は行っていません。".into(),
        );
    }
    // Both profile leases, namespace locks and every other file pin remain held during this replacement.
    relocation.apply(&paths.current)?;
    guard.pin_relocated_file(&logical, &paths.current.join("web-management.json"))
}

struct ProfileMove {
    source: PathBuf,
    destination: PathBuf,
    files: Vec<PathBuf>,
    directories: Vec<PathBuf>,
    moved: Vec<PathBuf>,
    placeholders: Vec<File>,
}

fn profile_lock(path: &Path) -> bool {
    path.parent().is_some_and(|p| p.as_os_str().is_empty())
        && matches!(
            path.file_name().and_then(|s| s.to_str()),
            Some("hub.lock" | "host-launch.lock" | "gateway-execution.lock")
        )
}

impl ProfileMove {
    fn inventory(source: &Path, destination: &Path) -> Result<Self, String> {
        crate::maintenance::check_regular_tree(source)?;
        let mut pending = vec![source.to_path_buf()];
        let (mut files, mut directories) = (Vec::new(), Vec::new());
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path();
                let relative = path
                    .strip_prefix(source)
                    .map_err(|e| e.to_string())?
                    .to_path_buf();
                if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                    directories.push(relative);
                    pending.push(path);
                } else if !profile_lock(&relative) {
                    files.push(relative);
                }
            }
        }
        directories.sort_by_key(|p| p.components().count());
        files.sort_by_key(|path| (path != Path::new("shared-work.sqlite3"), path.clone()));
        Ok(Self {
            source: source.into(),
            destination: destination.into(),
            files,
            directories,
            moved: Vec::new(),
            placeholders: Vec::new(),
        })
    }

    fn execute(&mut self) -> Result<(), String> {
        for path in &self.directories {
            fs::create_dir(self.destination.join(path)).map_err(|e| e.to_string())?;
        }
        if !self
            .files
            .iter()
            .any(|p| p == Path::new("shared-work.sqlite3"))
        {
            self.block_old_database()?;
        }
        for index in 0..self.files.len() {
            let path = self.files[index].clone();
            move_file_no_replace(&self.source.join(&path), &self.destination.join(&path))?;
            self.moved.push(path.clone());
            if path == Path::new("shared-work.sqlite3") {
                self.block_old_database()?;
            }
        }
        Ok(())
    }

    fn block_old_database(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_DELETE_ON_CLOSE;
            let file = OpenOptions::new().read(true).write(true).create_new(true).share_mode(0).custom_flags(FILE_FLAG_DELETE_ON_CLOSE).open(self.source.join("shared-work.sqlite3")).map_err(|e| format!("移動中の旧DB保存先を排他できません。別の処理が旧保存先を使用していないか確認してください: {e}"))?;
            self.placeholders.push(file);
        }
        Ok(())
    }

    fn release_placeholders(&mut self) {
        self.placeholders.clear();
    }

    fn rollback(&mut self) -> Result<(), String> {
        self.release_placeholders();
        while let Some(path) = self.moved.last() {
            move_file_no_replace(&self.destination.join(path), &self.source.join(path))?;
            self.moved.pop();
        }
        Ok(())
    }

    fn remove_empty_source_directories(&self) -> Result<(), String> {
        self.remove_empty_directories(&self.source)
    }
    fn remove_empty_destination_directories(&self) -> Result<(), String> {
        self.remove_empty_directories(&self.destination)
    }
    fn remove_empty_directories(&self, root: &Path) -> Result<(), String> {
        for path in self.directories.iter().rev() {
            let directory = root.join(path);
            if directory.exists() {
                fs::remove_dir(directory).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

fn move_file_no_replace(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        let source = source
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        // Default migration is Windows-only; these paths support isolated compatibility fixtures.
        if destination.exists() {
            return Err(format!("移動先を上書きしません: {}", destination.display()));
        }
        fs::rename(source, destination).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, DefaultPaths) {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("project_sandbox/hub-profile-tests");
        fs::create_dir_all(&parent).unwrap();
        let temp = tempfile::tempdir_in(parent).unwrap();
        let paths = windows_paths(&temp.path().join("local"), &temp.path().join("roaming"));
        (temp, paths)
    }
    #[test]
    fn legacy_moves_once_without_copying_credentials_and_reset_does_not_reimport() {
        let (_temp, paths) = fixture();
        fs::create_dir_all(&paths.legacy).unwrap();
        fs::write(
            paths.legacy.join("device-network.json"),
            b"credential fixture",
        )
        .unwrap();
        migrate_default(&paths).unwrap();
        assert!(!paths.legacy.exists());
        assert_eq!(
            fs::read(paths.current.join("device-network.json")).unwrap(),
            b"credential fixture"
        );
        fs::remove_dir_all(&paths.current).unwrap();
        assert_eq!(locate(&paths).unwrap(), paths.current);
        fs::create_dir_all(&paths.legacy).unwrap();
        fs::write(paths.legacy.join("catalog.json"), b"recreated old profile").unwrap();
        assert!(migrate_default(&paths).is_err());
        assert!(!paths.current.exists());
    }
    #[test]
    fn locating_a_live_profile_does_not_traverse_changing_assets() {
        let (_temp, paths) = fixture();
        let mut nested = paths.current.join("shared-work.assets");
        // Traversal depth restrictions belong to offline destructive operations,
        // not opening the management screen of an already running Hub.
        for _ in 0..66 {
            nested = nested.join("a");
        }
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(locate(&paths).unwrap(), paths.current);
        assert!(crate::maintenance::check_regular_tree(&paths.current).is_err());
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..200 {
                    let file = tempfile::NamedTempFile::new_in(&paths.current).unwrap();
                    file.persist(paths.current.join("saved.json")).unwrap();
                }
            });
            for _ in 0..200 {
                assert_eq!(locate(&paths).unwrap(), paths.current);
            }
        });
    }
    #[test]
    fn two_nonempty_profiles_are_never_merged() {
        let (_temp, paths) = fixture();
        for path in [&paths.legacy, &paths.current] {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join("catalog.json"), b"fixture").unwrap();
        }
        assert!(migrate_default(&paths).is_err());
        assert!(paths.legacy.join("catalog.json").exists());
        assert!(paths.current.join("catalog.json").exists());
    }
    #[test]
    fn a_running_owner_prevents_migration() {
        let (_temp, paths) = fixture();
        fs::create_dir_all(&paths.legacy).unwrap();
        fs::write(paths.legacy.join("catalog.json"), b"fixture").unwrap();
        let owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(paths.legacy.join("hub.lock"))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&owner).unwrap();
        assert!(migrate_default(&paths).is_err());
        assert!(paths.legacy.join("catalog.json").exists());
    }
    #[test]
    fn a_post_inventory_conflict_rolls_back_credentials_and_the_moving_marker() {
        let (_temp, mut paths) = fixture();
        fs::create_dir_all(&paths.legacy).unwrap();
        fs::write(
            paths.legacy.join("device-network.json"),
            b"credential fixture",
        )
        .unwrap();
        fs::write(paths.legacy.join("not-a-directory"), b"keep").unwrap();
        paths.marker = paths.current.join("not-a-directory/hub.json");
        let mut old_guard =
            crate::maintenance::OfflineGuard::acquire(&paths.legacy, false).unwrap();
        let mut new_guard =
            crate::maintenance::OfflineGuard::acquire(&paths.current, false).unwrap();
        new_guard.ensure_directory(&paths.current).unwrap();
        let mut movement = ProfileMove::inventory(&paths.legacy, &paths.current).unwrap();
        // Inject the conflict after inventory. Default resolution now takes its
        // migration lock before inventory and rejects this artificial layout.
        write_migration(&paths, MigrationState::Moving).unwrap();
        assert!(movement.execute().is_err());
        assert!(paths.current.join("device-network.json").exists());
        let rollback = rollback_move(
            &paths,
            &mut movement,
            &mut old_guard,
            &mut new_guard,
            None,
            None,
        );
        assert!(rollback.contains("戻しました"));
        assert_eq!(
            fs::read(paths.legacy.join("device-network.json")).unwrap(),
            b"credential fixture"
        );
        assert!(!paths.current.exists());
        assert_eq!(
            fs::read(paths.legacy.join("not-a-directory")).unwrap(),
            b"keep"
        );
    }
    #[test]
    fn individual_moves_retain_quiescence_handles_and_old_namespace_locks() {
        let (_temp, paths) = fixture();
        fs::create_dir_all(paths.legacy.join("shared-work.assets")).unwrap();
        fs::write(
            paths.legacy.join("shared-work.sqlite3"),
            b"database fixture",
        )
        .unwrap();
        fs::write(
            paths.legacy.join("device-network.json"),
            b"credential fixture",
        )
        .unwrap();
        fs::write(
            paths.legacy.join("shared-work.assets/result"),
            b"asset fixture",
        )
        .unwrap();
        let _old = crate::maintenance::OfflineGuard::acquire(&paths.legacy, false).unwrap();
        let mut new = crate::maintenance::OfflineGuard::acquire(&paths.current, false).unwrap();
        new.ensure_directory(&paths.current).unwrap();
        let mut movement = ProfileMove::inventory(&paths.legacy, &paths.current).unwrap();
        movement.execute().unwrap();
        let old_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(paths.legacy.join("hub.lock"))
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&old_lock).is_err());
        #[cfg(windows)]
        assert!(
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(paths.legacy.join("shared-work.sqlite3"))
                .is_err()
        );
        movement.rollback().unwrap();
        assert_eq!(
            fs::read(paths.legacy.join("device-network.json")).unwrap(),
            b"credential fixture"
        );
        assert_eq!(
            fs::read(paths.legacy.join("shared-work.sqlite3")).unwrap(),
            b"database fixture"
        );
        assert_eq!(
            fs::read(paths.legacy.join("shared-work.assets/result")).unwrap(),
            b"asset fixture"
        );
        assert!(!paths.current.join("device-network.json").exists());
    }
    #[test]
    fn moving_record_blocks_both_default_namespaces_and_keeps_other_profiles_available() {
        let (temp, paths) = fixture();
        fs::create_dir_all(&paths.legacy).unwrap();
        fs::create_dir_all(&paths.current).unwrap();
        write_migration(&paths, MigrationState::Moving).unwrap();
        assert!(require_access(&paths, &paths.current).is_err());
        assert!(require_access(&paths, &paths.legacy).is_err());
        assert!(require_access(&paths, &paths.current.join(".")).is_err());
        assert!(require_access(&paths, &temp.path().join("custom-profile")).is_ok());
        clear_moving(&paths).unwrap();
        assert!(require_access(&paths, &paths.current).is_ok());
        retire_legacy(&paths).unwrap();
        assert!(clear_moving(&paths).is_err());
        assert!(retired(&paths).unwrap());
    }

    fn write_https_settings(paths: &DefaultPaths, key: &Path) -> Vec<u8> {
        fs::create_dir_all(paths.legacy.join("tls")).unwrap();
        fs::write(paths.legacy.join("tls/cert.pem"), b"certificate fixture").unwrap();
        let body = serde_json::to_vec_pretty(&serde_json::json!({"schema":1,"settings":{"local_port":9080,"remote":{"mode":"https","bind":"127.0.0.1:9443","public_origin":"https://management.example","certificate_path":paths.legacy.join("tls/cert.pem"),"private_key_path":key}}})).unwrap();
        fs::write(paths.legacy.join("web-management.json"), &body).unwrap();
        body
    }

    #[test]
    fn migration_rebases_only_owned_https_paths_and_preserves_database_and_external_files() {
        let (_temp, paths) = fixture();
        let external = paths.legacy.with_file_name("ai.midi.moyai.hub-other");
        fs::create_dir_all(&external).unwrap();
        let key = external.join("key.pem");
        fs::write(&key, b"external private key fixture").unwrap();
        write_https_settings(&paths, &key);
        let database = paths.legacy.join("shared-work.sqlite3");
        let db = rusqlite::Connection::open(&database).unwrap();
        db.execute_batch("CREATE TABLE records(note TEXT,workspace_path TEXT);")
            .unwrap();
        let note = format!(
            "History mentioning {} must stay as written",
            paths.legacy.display()
        );
        db.execute(
            "INSERT INTO records VALUES(?1,?2)",
            rusqlite::params![note, external.display().to_string()],
        )
        .unwrap();
        drop(db);
        let before_database = fs::read(&database).unwrap();
        fs::create_dir_all(paths.legacy.join("shared-work.assets")).unwrap();
        fs::write(
            paths.legacy.join("shared-work.assets/saved-result"),
            note.as_bytes(),
        )
        .unwrap();
        let network_hosting =
            br#"{"schema_version":1,"bind":"127.0.0.1:9471","advertise_ip":"127.0.0.1"}"#;
        fs::write(paths.legacy.join("network-hosting.json"), network_hosting).unwrap();
        migrate_default(&paths).unwrap();
        let settings: serde_json::Value =
            serde_json::from_slice(&fs::read(paths.current.join("web-management.json")).unwrap())
                .unwrap();
        assert_eq!(
            Path::new(
                settings["settings"]["remote"]["certificate_path"]
                    .as_str()
                    .unwrap()
            ),
            paths.current.join("tls/cert.pem").as_path()
        );
        assert_eq!(
            Path::new(
                settings["settings"]["remote"]["private_key_path"]
                    .as_str()
                    .unwrap()
            ),
            key.as_path()
        );
        assert_eq!(
            fs::read(paths.current.join("tls/cert.pem")).unwrap(),
            b"certificate fixture"
        );
        assert_eq!(
            fs::read(paths.current.join("shared-work.sqlite3")).unwrap(),
            before_database
        );
        assert_eq!(
            fs::read(paths.current.join("shared-work.assets/saved-result")).unwrap(),
            note.as_bytes()
        );
        assert_eq!(
            fs::read(paths.current.join("network-hosting.json")).unwrap(),
            network_hosting
        );
        assert_eq!(fs::read(&key).unwrap(), b"external private key fixture");
        assert!(!paths.legacy.exists());
    }

    #[test]
    fn https_paths_and_original_settings_bytes_are_restored_before_file_rollback() {
        let (_temp, paths) = fixture();
        let original = write_https_settings(&paths, &paths.legacy.join("tls/key.pem"));
        fs::write(paths.legacy.join("tls/key.pem"), b"private key fixture").unwrap();
        let mut old = crate::maintenance::OfflineGuard::acquire(&paths.legacy, false).unwrap();
        let mut new = crate::maintenance::OfflineGuard::acquire(&paths.current, false).unwrap();
        new.ensure_directory(&paths.current).unwrap();
        let mut movement = ProfileMove::inventory(&paths.legacy, &paths.current).unwrap();
        let relocation =
            crate::web_admin::server::prepare_profile_relocation(&paths.legacy, &paths.current)
                .unwrap()
                .unwrap();
        write_migration(&paths, MigrationState::Moving).unwrap();
        movement.execute().unwrap();
        apply_web_paths(&paths, &mut old, &relocation).unwrap();
        let settings: serde_json::Value =
            serde_json::from_slice(&fs::read(paths.current.join("web-management.json")).unwrap())
                .unwrap();
        assert_eq!(
            Path::new(
                settings["settings"]["remote"]["private_key_path"]
                    .as_str()
                    .unwrap()
            ),
            paths.current.join("tls/key.pem").as_path()
        );
        let rollback = rollback_move(
            &paths,
            &mut movement,
            &mut old,
            &mut new,
            None,
            Some(&relocation),
        );
        assert!(rollback.contains("戻しました"), "{rollback}");
        assert_eq!(
            fs::read(paths.legacy.join("web-management.json")).unwrap(),
            original
        );
        assert_eq!(
            fs::read(paths.legacy.join("tls/key.pem")).unwrap(),
            b"private key fixture"
        );
        assert!(!paths.current.exists());
        assert!(!paths.marker.exists());
    }
    #[test]
    fn changed_https_metadata_keeps_payload_and_pending_record_in_place() {
        let (_temp, paths) = fixture();
        write_https_settings(&paths, &paths.legacy.join("tls/key.pem"));
        fs::write(paths.legacy.join("tls/key.pem"), b"private key fixture").unwrap();
        let mut old = crate::maintenance::OfflineGuard::acquire(&paths.legacy, false).unwrap();
        let mut new = crate::maintenance::OfflineGuard::acquire(&paths.current, false).unwrap();
        new.ensure_directory(&paths.current).unwrap();
        let mut movement = ProfileMove::inventory(&paths.legacy, &paths.current).unwrap();
        let relocation =
            crate::web_admin::server::prepare_profile_relocation(&paths.legacy, &paths.current)
                .unwrap()
                .unwrap();
        write_migration(&paths, MigrationState::Moving).unwrap();
        movement.execute().unwrap();
        apply_web_paths(&paths, &mut old, &relocation).unwrap();
        // Model an external edit in the metadata replacement window, retaining every other exclusion.
        assert!(old.release_file_pin(&paths.legacy.join("web-management.json")));
        #[cfg(windows)]
        {
            assert!(
                OpenOptions::new()
                    .write(true)
                    .open(paths.current.join("tls/key.pem"))
                    .is_err()
            );
            assert!(
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(paths.legacy.join("shared-work.sqlite3"))
                    .is_err()
            );
        }
        let mut changed: serde_json::Value =
            serde_json::from_slice(&fs::read(paths.current.join("web-management.json")).unwrap())
                .unwrap();
        changed["settings"]["remote"]["public_origin"] = "https://changed.example".into();
        let changed = serde_json::to_vec(&changed).unwrap();
        fs::write(paths.current.join("web-management.json"), &changed).unwrap();
        let rollback = rollback_move(
            &paths,
            &mut movement,
            &mut old,
            &mut new,
            None,
            Some(&relocation),
        );
        assert!(rollback.contains("復元を中止"));
        assert_eq!(
            fs::read(paths.current.join("web-management.json")).unwrap(),
            changed
        );
        assert_eq!(
            fs::read(paths.current.join("tls/key.pem")).unwrap(),
            b"private key fixture"
        );
        assert!(!paths.legacy.join("tls/key.pem").exists());
        assert_eq!(
            migration_state(&paths).unwrap(),
            Some(MigrationState::Moving)
        );
        assert!(require_access(&paths, &paths.current).is_err());
        assert!(require_access(&paths, &paths.legacy).is_err());
    }
}

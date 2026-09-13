//! Offline maintenance uses the same Hub directory lock and a checksummed, closed file inventory.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const FILES: &[&str] = &[
    "catalog.json",
    "device-network.json",
    "web-management.json",
    "network-hosting.json",
    "shared-work.sqlite3",
];
const MAX_BYTES: u64 = 3 * 1024 * 1024 * 1024;
const ASSETS: &str = "shared-work.assets";
const MAX_MANIFEST: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_005;
pub(crate) const RESTORE_INCOMPLETE: &str = "restore-incomplete";
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub created_at_ms: u64,
    pub files: Vec<Entry>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub byte_length: u64,
    pub sha256: String,
}
fn err(value: impl std::fmt::Display) -> String {
    value.to_string()
}
fn lock(directory: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("hub.lock"))
        .map_err(err)?;
    file.try_lock_exclusive()
        .map_err(|_| "Stop this data directory's Hub before backup or restore".to_owned())?;
    Ok(file)
}
fn regular(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Backup files must be regular local files".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("Backup cannot follow reparse points".into());
        }
    }
    Ok(())
}
fn digest(path: &Path) -> Result<(u64, String), String> {
    digest_file(&mut checked_file(path)?)
}
fn checked_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    let file = options.open(path).map_err(err)?;
    let metadata = file.metadata().map_err(err)?;
    if !metadata.is_file() {
        return Err("Backup files must be regular files".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("Backup cannot follow a reparse point".into());
        }
    }
    Ok(file)
}
fn checked_directory(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(3).custom_flags(0x02000000 | 0x00200000);
    }
    let file = options.open(path).map_err(err)?;
    let metadata = file.metadata().map_err(err)?;
    if !metadata.is_dir()
        || fs::symlink_metadata(path)
            .map_err(err)?
            .file_type()
            .is_symlink()
    {
        return Err("Backup asset directory must be a regular directory".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("Backup cannot follow an asset directory reparse point".into());
        }
    }
    Ok(file)
}
fn hash_name(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn allowed_entry(entry: &Entry, version: u32) -> bool {
    if !hash_name(&entry.sha256) {
        return false;
    }
    FILES.contains(&entry.name.as_str())
        || (version == 2
            && entry
                .name
                .strip_prefix("shared-work.assets/")
                .is_some_and(|name| {
                    hash_name(name) && name == entry.sha256 && entry.byte_length <= 64 * 1024 * 1024
                }))
}
fn retained_blobs(database: &Path) -> Result<Vec<(String, u64)>, String> {
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(err)?;
    let version: u32 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(err)?;
    if version < 9 {
        return Ok(Vec::new());
    }
    let values = crate::shared_work::blob_store::retained_inventory(&db).map_err(err)?;
    if values.iter().map(|(_, size)| size).sum::<u64>() > 2 * 1024 * 1024 * 1024 {
        return Err("Backup asset references exceed their limit".into());
    }
    Ok(values)
}
fn require_asset_inventory(database: &Path, entries: &[Entry]) -> Result<(), String> {
    let expected = retained_blobs(database)?
        .into_iter()
        .map(|(hash, size)| (format!("{ASSETS}/{hash}"), size))
        .collect::<std::collections::BTreeMap<_, _>>();
    let actual = entries
        .iter()
        .filter(|entry| entry.name.starts_with("shared-work.assets/"))
        .map(|entry| (entry.name.clone(), entry.byte_length))
        .collect::<std::collections::BTreeMap<_, _>>();
    if expected != actual {
        return Err("Backup does not contain the exact retained asset inventory".into());
    }
    Ok(())
}
fn digest_file(file: &mut File) -> Result<(u64, String), String> {
    let mut buffer = [0u8; 64 * 1024];
    let mut digest = Sha256::new();
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(err)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > MAX_BYTES {
            return Err("Backup file exceeds 3 GiB".into());
        }
        digest.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0)).map_err(err)?;
    Ok((size, format!("{:x}", digest.finalize())))
}
fn new_directory(path: &Path) -> Result<PathBuf, String> {
    // Requiring a new destination avoids overwriting an installed profile or a previous backup.
    fs::create_dir(path).map_err(|e| format!("Use a new destination directory: {e}"))?;
    fs::canonicalize(path).map_err(err)
}
fn copy_new(source: &Path, target: &Path) -> Result<(), String> {
    copy_handle_new(&mut checked_file(source)?, target)
}
fn copy_handle_new(input: &mut File, target: &Path) -> Result<(), String> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(err)?;
    std::io::copy(input, &mut output).map_err(err)?;
    output.sync_all().map_err(err)
}
pub fn backup(source: &Path, destination: &Path) -> Result<Manifest, String> {
    let source = fs::canonicalize(source).map_err(err)?;
    let _lock = lock(&source)?;
    let destination = new_directory(destination)?;
    let mut entries = Vec::new();
    for &name in FILES {
        let path = source.join(name);
        if !path.exists() {
            continue;
        }
        regular(&path)?;
        let target = destination.join(name);
        if name == "shared-work.sqlite3" {
            let db = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(err)?;
            let check: String = db
                .query_row("PRAGMA quick_check", [], |r| r.get(0))
                .map_err(err)?;
            if check != "ok" {
                return Err("Shared database did not pass integrity verification".into());
            }
            db.execute(
                "VACUUM main INTO ?1",
                [target.to_str().ok_or("Backup path must be UTF-8")?],
            )
            .map_err(err)?;
        } else {
            copy_new(&path, &target)?;
        }
        let (byte_length, sha256) = digest(&target)?;
        entries.push(Entry {
            name: name.into(),
            byte_length,
            sha256,
        });
    }
    if ["catalog.json", "device-network.json", "shared-work.sqlite3"]
        .iter()
        .any(|name| !entries.iter().any(|e| e.name == *name))
    {
        return Err("Backup source is not an initialized Hub profile".into());
    }
    let blobs = retained_blobs(&destination.join("shared-work.sqlite3"))?;
    if !blobs.is_empty() {
        let _source_assets = checked_directory(&source.join(ASSETS))?;
        fs::create_dir(destination.join(ASSETS)).map_err(err)?;
        for (sha256, byte_length) in blobs {
            let name = format!("{ASSETS}/{sha256}");
            let mut held = crate::shared_work::blob_store::open_verified(
                &source.join("shared-work.sqlite3"),
                &sha256,
                byte_length,
            )
            .map_err(err)?;
            copy_handle_new(held.file_mut(), &destination.join(&name))?;
            entries.push(Entry {
                name,
                byte_length,
                sha256,
            });
        }
    }
    let manifest = Manifest {
        version: 2,
        created_at_ms: crate::service::now_ms(),
        files: entries,
    };
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(err)?;
    if bytes.len() > MAX_MANIFEST {
        return Err("Backup manifest exceeds 32 MiB".into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination.join("manifest.json"))
        .map_err(err)?;
    file.write_all(&bytes).map_err(err)?;
    file.sync_all().map_err(err)?;
    Ok(manifest)
}
pub fn restore(
    source: &Path,
    destination: &Path,
    acknowledge_runners_stopped: bool,
) -> Result<Manifest, String> {
    if !acknowledge_runners_stopped {
        return Err("Restore requires --acknowledge-runners-stopped after all affected Runners and their managed processes are stopped".into());
    }
    let source = fs::canonicalize(source).map_err(err)?;
    let mut bytes = Vec::new();
    checked_file(&source.join("manifest.json"))?
        .take(MAX_MANIFEST as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > MAX_MANIFEST {
        return Err("Backup manifest exceeds 32 MiB".into());
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(err)?;
    if !matches!(manifest.version, 1 | 2) || manifest.files.len() > MAX_ENTRIES {
        return Err("Unsupported backup manifest".into());
    }
    let _source_assets = if manifest
        .files
        .iter()
        .any(|e| e.name.starts_with("shared-work.assets/"))
    {
        Some(checked_directory(&source.join(ASSETS))?)
    } else {
        None
    };
    let mut names = std::collections::BTreeSet::new();
    for entry in &manifest.files {
        if !allowed_entry(entry, manifest.version) || !names.insert(&entry.name) {
            return Err("Backup manifest contains an unknown or repeated file".into());
        }
        let mut file = checked_file(&source.join(&entry.name))?;
        if digest_file(&mut file)? != (entry.byte_length, entry.sha256.clone()) {
            return Err(format!("Backup checksum mismatch: {}", entry.name));
        }
    }
    if ["catalog.json", "device-network.json", "shared-work.sqlite3"]
        .iter()
        .any(|name| !names.contains(&name.to_string()))
    {
        return Err("Backup is missing required Hub files".into());
    }
    let destination = new_directory(destination)?;
    let _lock = lock(&destination)?;
    let mut activation = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination.join(RESTORE_INCOMPLETE))
        .map_err(err)?;
    activation
        .write_all(
            b"Restore fencing has not completed. Restore the backup into another new directory.\n",
        )
        .map_err(err)?;
    activation.sync_all().map_err(err)?;
    drop(activation);
    if _source_assets.is_some() {
        fs::create_dir(destination.join(ASSETS)).map_err(err)?;
    }
    for entry in &manifest.files {
        let target = destination.join(&entry.name);
        // Revalidate each held source immediately before copying. The incomplete marker
        // keeps a changed source or interrupted copy from activating a partial profile.
        let mut file = checked_file(&source.join(&entry.name))?;
        if digest_file(&mut file)? != (entry.byte_length, entry.sha256.clone()) {
            return Err(format!("Backup changed before copy: {}", entry.name));
        }
        copy_handle_new(&mut file, &target)?;
        if digest(&target)? != (entry.byte_length, entry.sha256.clone()) {
            return Err(format!(
                "Restored content changed during copy: {}",
                entry.name
            ));
        }
    }
    require_asset_inventory(&destination.join("shared-work.sqlite3"), &manifest.files)?;
    let mut shared = crate::shared_work::SharedWork::open(&destination.join("shared-work.sqlite3"))
        .map_err(err)?;
    shared
        .fence_restored_backup(crate::service::now_ms())
        .map_err(err)?;
    let catalog: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join("catalog.json")).map_err(err)?)
            .map_err(err)?;
    let hub_id = catalog["hub_id"]
        .as_str()
        .ok_or("Restored catalog has no Hub identity")?;
    let mut network =
        crate::network::DeviceNetwork::open(&destination.join("device-network.json"), hub_id)
            .map_err(err)?;
    let snapshot = network.snapshot(crate::service::now_ms());
    for device in snapshot["devices"]
        .as_array()
        .ok_or("Restored device inventory is invalid")?
    {
        if device["revoked"] == true {
            continue;
        }
        network
            .set_admission(
                &network.revision(),
                device["device_id"]
                    .as_str()
                    .ok_or("Invalid restored device")?,
                true,
            )
            .map_err(err)?;
    }
    // A restored older queue never authorizes automatic effects. Reconcile stopped attempts,
    // then explicitly enable devices and environments from the ordinary administration UI.
    drop(network);
    drop(shared);
    fs::remove_file(destination.join(RESTORE_INCOMPLETE)).map_err(err)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_restore_cannot_start_before_queue_is_fenced() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("interrupted");
        fs::create_dir(&destination).unwrap();
        let admin = crate::admin::Admin::open(destination.join("catalog.json")).unwrap();
        drop(admin);
        // The copied profile is valid, but the restore process died before fencing it.
        fs::write(destination.join("restore-incomplete"), b"1").unwrap();
        assert!(crate::admin::Admin::open(destination.join("catalog.json")).is_err());
    }
    #[test]
    fn backup_restore_includes_shared_files_and_rejects_a_missing_reference() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        drop(crate::admin::Admin::open(source.join("catalog.json")).unwrap());
        let mut work =
            crate::shared_work::SharedWork::open(&source.join("shared-work.sqlite3")).unwrap();
        let actor = crate::identity::bootstrap_admin(
            work.identity_connection(),
            "admin",
            "Admin",
            "backup-password-12345",
            1,
        )
        .unwrap();
        work.create_project(&actor, "project", "Project").unwrap();
        crate::identity::set_project_role(
            work.identity_connection(),
            &actor,
            "project",
            &actor.user_id,
            Some(crate::identity::ProjectRole::Contributor),
            2,
        )
        .unwrap();
        let bytes = [0u8, 255, 128, 42];
        let asset = work
            .upload_input(
                &actor,
                "project",
                &crate::shared_work::AssetUpload {
                    request_id: "backup-file".into(),
                    name: "data.bin".into(),
                    content_base64: STANDARD.encode(bytes),
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                },
                3,
            )
            .unwrap();
        drop(work);
        let saved = temp.path().join("saved");
        let mut manifest = backup(&source, &saved).unwrap();
        assert_eq!(manifest.version, 2);
        assert_eq!(
            manifest
                .files
                .iter()
                .filter(|e| e.name.starts_with("shared-work.assets/"))
                .count(),
            1
        );
        let restored = temp.path().join("restored");
        restore(&saved, &restored, true).unwrap();
        let work =
            crate::shared_work::SharedWork::open(&restored.join("shared-work.sqlite3")).unwrap();
        assert_eq!(
            STANDARD
                .decode(work.asset(&actor, &asset.id).unwrap().content_base64)
                .unwrap(),
            bytes
        );
        drop(work);
        manifest
            .files
            .retain(|e| !e.name.starts_with("shared-work.assets/"));
        fs::write(
            saved.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let missing = temp.path().join("missing");
        assert!(restore(&saved, &missing, true).is_err());
        assert!(missing.join(RESTORE_INCOMPLETE).exists());
        assert!(crate::admin::Admin::open(missing.join("catalog.json")).is_err());
    }
    #[test]
    fn backup_restore_verifies_bytes_requires_stopped_owners_and_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("hub");
        fs::create_dir(&source).unwrap();
        let admin = crate::admin::Admin::open(source.join("catalog.json")).unwrap();
        assert!(backup(&source, &temp.path().join("busy")).is_err());
        drop(admin);
        let saved = temp.path().join("backup");
        backup(&source, &saved).unwrap();
        assert!(restore(&saved, &temp.path().join("missing-ack"), false).is_err());
        let target = temp.path().join("restored");
        restore(&saved, &target, true).unwrap();
        assert!(restore(&saved, &target, true).is_err());
        fs::write(saved.join("catalog.json"), b"{}").unwrap();
        assert!(restore(&saved, &temp.path().join("corrupt"), true).is_err());
        assert!(!temp.path().join("corrupt").exists());
    }
}

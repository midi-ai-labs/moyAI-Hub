//! Immutable content files belong to the database's adjacent managed directory.
//! Durable bytes precede DB references; GC follows committed reference removal.
use super::{Connection, Result, WorkError};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const MAX_BLOB: u64 = 64 * 1024 * 1024;
const MAX_PHYSICAL: u64 = 2 * 1024 * 1024 * 1024 + MAX_BLOB;
const MAX_FILES: usize = 100_256;
const GC_LIMIT: usize = 128;

pub(crate) fn assets_directory(database: &Path) -> PathBuf {
    database.with_extension("assets")
}
fn database_path(db: &Connection) -> Result<PathBuf> {
    db.path()
        .filter(|path| !path.is_empty() && *path != ":memory:")
        .map(PathBuf::from)
        .ok_or(WorkError::Conflict)
}
fn hash_valid(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn regular(metadata: &fs::Metadata, directory: bool) -> Result<()> {
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(WorkError::Conflict);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(WorkError::Conflict);
        }
    }
    Ok(())
}

struct Root {
    path: PathBuf,
    _anchors: Vec<File>,
}
impl Root {
    fn open(database: &Path, create: bool) -> Result<Self> {
        let database = if database.is_absolute() {
            database.to_owned()
        } else {
            std::env::current_dir()?.join(database)
        };
        let root = assets_directory(&database);
        let mut ancestors = root.ancestors().skip(1).collect::<Vec<_>>();
        ancestors.reverse();
        let mut anchors = Vec::new();
        for path in ancestors {
            anchors.push(open_directory(path)?);
        }
        if create {
            match fs::create_dir(&root) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        anchors.push(open_directory(&root)?);
        Ok(Self {
            path: root,
            _anchors: anchors,
        })
    }
    fn file(&self, hash: &str) -> Result<File> {
        if !hash_valid(hash) {
            return Err(WorkError::Conflict);
        }
        let path = self.path.join(hash);
        regular(&fs::symlink_metadata(&path)?, false)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1).custom_flags(0x00200000); // no write/delete sharing, open reparse point itself
        }
        let file = options.open(&path)?;
        regular(&file.metadata()?, false)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let path_metadata = fs::symlink_metadata(&path)?;
            regular(&path_metadata, false)?;
            let opened = file.metadata()?;
            if (opened.dev(), opened.ino()) != (path_metadata.dev(), path_metadata.ino()) {
                return Err(WorkError::Conflict);
            }
        }
        Ok(file)
    }
}
fn open_directory(path: &Path) -> Result<File> {
    regular(&fs::symlink_metadata(path)?, true)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .access_mode(0x81)
            .share_mode(3)
            .custom_flags(0x02000000 | 0x00200000);
    }
    let file = options.open(path)?;
    regular(&file.metadata()?, true)?;
    Ok(file)
}
fn verify(file: &mut File, hash: &str, length: u64) -> Result<()> {
    if !hash_valid(hash) || length > MAX_BLOB || file.metadata()?.len() != length {
        return Err(WorkError::Conflict);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > length {
            return Err(WorkError::Conflict);
        }
        digest.update(&buffer[..count]);
    }
    if total != length || format!("{:x}", digest.finalize()) != hash {
        return Err(WorkError::Conflict);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(())
}
pub(crate) struct VerifiedBlob {
    file: File,
    _root: Root,
}
impl VerifiedBlob {
    pub(crate) fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }
}
pub(crate) fn open_verified(database: &Path, hash: &str, length: u64) -> Result<VerifiedBlob> {
    let root = Root::open(database, false)?;
    let mut file = root.file(hash)?;
    verify(&mut file, hash, length)?;
    Ok(VerifiedBlob { file, _root: root })
}
pub(crate) fn retained_inventory(db: &Connection) -> Result<Vec<(String, u64)>> {
    let mut statement=db.prepare("SELECT sha256,MIN(byte_length),MAX(byte_length) FROM shared_assets WHERE purged_at_ms IS NULL GROUP BY sha256 ORDER BY sha256 LIMIT 100001")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if rows.len() > 100_000
        || rows
            .iter()
            .any(|(hash, min, max)| !hash_valid(hash) || min != max || *max > MAX_BLOB)
    {
        return Err(WorkError::Conflict);
    }
    Ok(rows
        .into_iter()
        .map(|(hash, length, _)| (hash, length))
        .collect())
}
fn entries(root: &Root) -> Result<Vec<(String, u64)>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(&root.path)? {
        if entries.len() >= MAX_FILES {
            return Err(WorkError::Capacity);
        }
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| WorkError::Conflict)?;
        if !hash_valid(&name) && !name.starts_with(".pending-") {
            return Err(WorkError::Conflict);
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        regular(&metadata, false)?;
        if metadata.len() > MAX_BLOB {
            return Err(WorkError::Capacity);
        }
        entries.push((name, metadata.len()));
    }
    entries.sort_unstable();
    Ok(entries)
}
fn collect(db: &Connection, root: &Root) -> Result<(u64, usize)> {
    let retained = retained_inventory(db)?
        .into_iter()
        .map(|(hash, _)| hash)
        .collect::<BTreeSet<_>>();
    let mut removed = 0usize;
    let mut physical = 0u64;
    let mut count = 0usize;
    for (name, size) in entries(root)? {
        if !retained.contains(&name) && removed < GC_LIMIT {
            match fs::remove_file(root.path.join(&name)) {
                Ok(()) => {
                    removed += 1;
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {} // committed metadata remains authoritative; retry collection later
            }
        }
        physical = physical.saturating_add(size);
        count += 1;
    }
    Ok((physical, count))
}
pub(super) fn gc(db: &Connection) -> Result<()> {
    let database = database_path(db)?;
    if !assets_directory(&database).exists() {
        return Ok(());
    }
    collect(db, &Root::open(&database, false)?)?;
    Ok(())
}
pub(super) fn persist(db: &Connection, hash: &str, bytes: &[u8]) -> Result<()> {
    if !hash_valid(hash)
        || bytes.len() as u64 > MAX_BLOB
        || format!("{:x}", Sha256::digest(bytes)) != hash
    {
        return Err(WorkError::Conflict);
    }
    let root = Root::open(&database_path(db)?, true)?;
    let target = root.path.join(hash);
    match root.file(hash) {
        Ok(mut file) => return verify(&mut file, hash, bytes.len() as u64),
        Err(WorkError::FileStorage(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let (physical, count) = collect(db, &root)?;
    if physical.saturating_add(bytes.len() as u64) > MAX_PHYSICAL || count >= 100_000 {
        return Err(WorkError::Capacity);
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".pending-")
        .tempfile_in(&root.path)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(&target) {
        Ok(file) => {
            file.sync_all()?;
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut file = root.file(hash)?;
            verify(&mut file, hash, bytes.len() as u64)?;
        }
        Err(error) => return Err(error.error.into()),
    }
    // The file library publishes by name. Verify the published entry itself, not
    // only the temporary handle, before any database reference may commit.
    let mut published = root.file(hash)?;
    verify(&mut published, hash, bytes.len() as u64)?;
    #[cfg(unix)]
    root._anchors
        .last()
        .ok_or(WorkError::Conflict)?
        .sync_all()?;
    Ok(())
}
pub(super) fn read(db: &Connection, hash: &str, length: u64) -> Result<Vec<u8>> {
    let mut held = open_verified(&database_path(db)?, hash, length)?;
    let mut bytes = Vec::with_capacity(length as usize);
    held.file_mut().take(length + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != length || format!("{:x}", Sha256::digest(&bytes)) != hash {
        return Err(WorkError::Conflict);
    }
    Ok(bytes)
}
/// Called before the V9 marker commits. Any failure leaves all legacy BLOBs intact.
pub(super) fn migrate(db: &Connection) -> Result<()> {
    let mut statement=db.prepare("SELECT id,sha256,byte_length,length(content) FROM shared_assets WHERE purged_at_ms IS NULL ORDER BY id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let hash: String = row.get(1)?;
        let length: u64 = row.get(2)?;
        let stored_length: u64 = row.get(3)?;
        if length > MAX_BLOB || stored_length != length {
            return Err(WorkError::Conflict);
        }
        let bytes: Vec<u8> = db.query_row(
            "SELECT content FROM shared_assets WHERE id=?1",
            [id],
            |row| row.get(0),
        )?;
        if bytes.len() as u64 != length {
            return Err(WorkError::Conflict);
        }
        persist(db, &hash, &bytes)?;
    }
    drop(rows);
    drop(statement);
    db.execute("UPDATE shared_assets SET content=x''", [])?;
    Ok(())
}

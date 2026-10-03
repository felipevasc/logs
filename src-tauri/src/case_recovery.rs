//! Verified recovery for the Case datastore and its immutable asset closure.
//! This module never adopts a Case and never treats an IPC `verified` flag as
//! authority. Originals remain untouched while a recovery generation is built.
use crate::case_evidence::{AuthoritySnapshot, RecoveryReceipt, RecoverySource};
use fs2::FileExt;
use rusqlite::{
    backup::{Backup, StepResult},
    Connection, OpenFlags,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

#[path = "case_recovery_restore.rs"]
mod restore;
pub(crate) use restore::{restore_fresh, RestoreReceipt};
#[cfg(test)]
use restore::restore_current;
#[path = "case_profiles.rs"]
pub(crate) mod profiles;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RestoredProfileProof {
    pub database: Asset,
    pub authority: AuthoritySnapshot,
}

/// Validate a freshly restored, still-unselected profile. SQLite creates the
/// bounded temporary snapshot; no ordinary File is opened on its live DB.
pub(crate) fn verify_restored_profile(
    root: &Path,
    recovery: &VerifiedRecovery,
    receipt: &RestoreReceipt,
    expected: Option<&RestoredProfileProof>,
    work: &Work<'_>,
) -> Result<RestoredProfileProof, String> {
    recovery.validate_with(work)?;
    if !receipt.committed
        || receipt.reconcile_required
        || receipt.recovery_id != recovery.manifest.id
        || receipt.manifest_sha256 != recovery.manifest_sha256
    {
        return Err(INVALID.into());
    }
    let lease = RootLease::shared(root, work)?;
    let conn = readonly(&lease.root.join("investigations.sqlite3"))?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    assert_no_permits(&conn)?;
    let authority = crate::case_evidence::snapshot_authority(&conn)?;
    if authority != receipt.after {
        return Err(INVALID.into());
    }
    let marker:String=conn.query_row("SELECT CASE WHEN octet_length(value)<=8388608 THEN value END FROM metadata WHERE key='case-recovery-last-restore'",[],|row|row.get(0)).map_err(|e|e.to_string())?;
    let marker: RestoreReceipt = serde_json::from_str(&marker).map_err(|e| e.to_string())?;
    if marker.request_id != receipt.request_id
        || marker.manifest_sha256 != receipt.manifest_sha256
        || marker.after != receipt.after
    {
        return Err(INVALID.into());
    }
    let stage = tempfile::Builder::new()
        .prefix("profile-verification-")
        .tempdir_in(lease.root.join("case-recovery-v1"))
        .map_err(|e| e.to_string())?;
    let path = stage.path().join("investigations.sqlite3");
    let mut destination = Connection::open(&path).map_err(|e| e.to_string())?;
    destination
        .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
        .map_err(|e| e.to_string())?;
    backup_database(&conn, &mut destination, work)?;
    integrity_check(&destination, work)?;
    drop(destination);
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(conn);
    let mut file = regular(stage.path(), "investigations.sqlite3")?;
    let bytes = file.metadata().map_err(|e| e.to_string())?.len();
    if bytes > TOTAL_LIMIT {
        return Err(LIMIT.into());
    }
    let database = Asset {
        relative_path: "investigations.sqlite3".into(),
        bytes,
        sha256: hash_reader(&mut file, bytes, work)?,
    };
    for asset in &recovery.manifest.assets {
        verify_asset(&lease.root, asset, work)?;
    }
    for absent in &recovery.manifest.unavailable_references {
        let directory = crate::reference_store::recovery_directory(
            &lease.root,
            &absent.owner,
            &absent.descriptor,
        )
        .map_err(|e| e.to_string())?;
        match std::fs::symlink_metadata(directory) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            _ => return Err(INVALID.into()),
        }
    }
    lease.validate()?;
    let proof = RestoredProfileProof {
        database,
        authority,
    };
    if expected.is_some_and(|value| value != &proof) {
        return Err(INVALID.into());
    }
    Ok(proof)
}

const FORMAT: u32 = 1;
const MANIFEST_LIMIT: usize = 8 << 20;
const ENTRY_LIMIT: usize = 16_384;
const TOTAL_LIMIT: u64 = 32 << 30;
// Recovery preserves raw legacy definitions even when the interpretation
// reader refuses their smaller 2/3 MiB limits. Never turn an excessive or
// malformed original into a missing file in the verified migration snapshot.
const LEGACY_INTERPRETATION_BYTES: u64 = 64 << 20;
const LEGACY_INTERPRETATION_FILES: [&str; 7] = [
    "codes.json", "system_codes.json", "ts_configs.json", "formats.json",
    "detections.json", "detection-rules.json", "threat-rules.json",
];
const COPY_BUFFER: usize = 256 << 10;
const INVALID: &str =
    "CASE_RECOVERY_INVALID: A cópia de recuperação está incompleta ou foi alterada.";
const LIMIT: &str = "CASE_RECOVERY_LIMIT: A cópia excede os limites de recuperação; os originais foram preservados.";

pub(crate) struct Work<'a> {
    pub cancelled: &'a dyn Fn() -> bool,
    pub progress: &'a dyn Fn(&str, u64, Option<u64>),
}
impl Work<'_> {
    fn check(&self) -> Result<(), String> {
        if (self.cancelled)() {
            Err(
                "CASE_RECOVERY_CANCELLED: Recuperação cancelada; nenhuma adoção foi publicada."
                    .into(),
            )
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Asset {
    pub relative_path: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interpretation_assets_version: Option<u32>,
    id: String,
    authority: AuthoritySnapshot,
    database: Asset,
    assets: Vec<Asset>,
    unavailable_references: Vec<UnavailableReference>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UnavailableReference {
    owner: crate::reference_store::Owner,
    descriptor: crate::analysis_context::ReferenceDescriptor,
}

fn open_directory(path: &Path) -> Result<File, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(INVALID.into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x02000000 | 0x00200000);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_dir() {
        return Err(INVALID.into());
    }
    Ok(file)
}
fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        open_directory(path)?
            .sync_all()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
fn lock_file(path: &Path, create: bool) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file()
        || std::fs::symlink_metadata(path)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err(INVALID.into());
    }
    Ok(file)
}

/// The shared root lease is a future-reclamation invariant. Published native,
/// reference, image and exclusion assets must not be removed under this lease.
/// Existing cleanup removes only owned pending files; it never reclaims these
/// published assets. Restore obtains exclusive quiescence separately.
pub(crate) struct RootLease {
    root: PathBuf,
    directory: File,
    _file: Arc<File>,
}
impl RootLease {
    pub(crate) fn shared(root: &Path, work: &Work<'_>) -> Result<Self, String> {
        Self::acquire(root, false, work)
    }
    fn exclusive(root: &Path, work: &Work<'_>) -> Result<Self, String> {
        Self::acquire(root, true, work)
    }
    fn acquire(root: &Path, exclusive: bool, work: &Work<'_>) -> Result<Self, String> {
        work.check()?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let directory = open_directory(&root)?;
        let file = lock_file(&root.join("case-recovery-root.lock"), true)?;
        let result = if exclusive {
            FileExt::try_lock_exclusive(&file)
        } else {
            FileExt::try_lock_shared(&file)
        };
        result.map_err(|_| {
            "CASE_RECOVERY_BUSY: A investigação está em uso; tente novamente.".to_string()
        })?;
        let lease = Self {
            root,
            directory,
            _file: Arc::new(file),
        };
        lease.validate()?;
        work.check()?;
        Ok(lease)
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        let current = open_directory(&self.root)?;
        if crate::sources::file_identity(&self.directory).is_none()
            || crate::sources::file_identity(&current)
                != crate::sources::file_identity(&self.directory)
        {
            return Err(INVALID.into());
        }
        let lock = lock_file(&self.root.join("case-recovery-root.lock"), false)?;
        if crate::sources::file_identity(&lock) != crate::sources::file_identity(&self._file) {
            return Err(INVALID.into());
        }
        Ok(())
    }
}

pub(crate) struct VerifiedRecovery {
    root: PathBuf,
    directory: PathBuf,
    manifest: Manifest,
    manifest_sha256: String,
    root_directory: File,
    _generation: File,
}
impl VerifiedRecovery {
    pub(crate) fn authority(&self) -> &AuthoritySnapshot {
        &self.manifest.authority
    }
    pub(crate) fn snapshot_assets_root(&self) -> PathBuf {
        self.directory.join("assets")
    }
    /// Old recovery generations predate the complete legacy interpretation
    /// closure. Absence there cannot prove the source profile had no settings.
    pub(crate) fn has_interpretation_assets(&self) -> bool {
        self.manifest.interpretation_assets_version == Some(1)
    }
    pub(crate) fn snapshot_database_path(&self) -> PathBuf {
        self.directory.join("investigations.sqlite3")
    }
    pub(crate) fn receipt(&self) -> RecoveryReceipt {
        RecoveryReceipt {
            schema_version: FORMAT,
            recovery_id: self.manifest.id.clone(),
            source: match &self.manifest.authority.store {
                Some(store) => RecoverySource::Native {
                    store: store.clone(),
                },
                None => RecoverySource::Legacy {
                    body_revision: self.manifest.authority.body_revision.clone(),
                },
            },
            manifest_sha256: self.manifest_sha256.clone(),
            verified: true,
        }
    }
    /// Revalidates bytes and copied authority; admission never accepts a stale
    /// receipt alone. Caller also compares live authority before adopting.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let work = Work {
            cancelled: &crate::operations::cancelled,
            progress: &|_, _, _| {},
        };
        self.validate_with(&work)
    }
    fn validate_with(&self, work: &Work<'_>) -> Result<(), String> {
        let current_root = open_directory(&self.root)?;
        if crate::sources::file_identity(&current_root)
            != crate::sources::file_identity(&self.root_directory)
        {
            return Err(INVALID.into());
        }
        let bytes = bounded_read(&self.directory.join("manifest.json"), MANIFEST_LIMIT, work)?;
        if digest(&bytes) != self.manifest_sha256 {
            return Err(INVALID.into());
        }
        verify_asset(&self.directory, &self.manifest.database, work)?;
        for asset in &self.manifest.assets {
            verify_asset(&self.directory.join("assets"), asset, work)?;
        }
        let conn = readonly(&self.snapshot_database_path())?;
        conn.execute_batch("BEGIN DEFERRED")
            .map_err(|e| e.to_string())?;
        assert_no_permits(&conn)?;
        if crate::case_evidence::snapshot_authority(&conn)? != self.manifest.authority {
            return Err(INVALID.into());
        }
        let mut actual = BTreeMap::new();
        let unavailable =
            enumerate_dependencies(&self.directory.join("assets"), &conn, work, &mut |asset| {
                actual.insert(asset.relative_path.clone(), asset);
                Ok(())
            })?;
        let expected: BTreeMap<_, _> = self
            .manifest
            .assets
            .iter()
            .map(|asset| (asset.relative_path.clone(), asset.clone()))
            .collect();
        if actual != expected
            || serde_json::to_vec(&unavailable).map_err(|e| e.to_string())?
                != serde_json::to_vec(&self.manifest.unavailable_references)
                    .map_err(|e| e.to_string())?
        {
            return Err(INVALID.into());
        }
        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        work.check()
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn relative(value: &str) -> Result<&Path, String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 4096
        || value.contains('\\')
        || !path.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(INVALID.into());
    }
    Ok(path)
}
fn regular(root: &Path, name: &str) -> Result<File, String> {
    let relative = relative(name)?;
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component.as_os_str());
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(INVALID.into());
        }
    }
    crate::case_archive_format::open_regular(&path)
}
// Never open/close a plain File for an active SQLite database: on Unix that
// releases all this process's POSIX locks for that inode, even another thread's.
fn readonly(path: &Path) -> Result<Connection, String> {
    let before = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !before.file_type().is_file() {
        return Err(INVALID.into());
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|e| e.to_string())?;
    let after = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !after.file_type().is_file() {
        return Err(INVALID.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return Err(INVALID.into());
        }
    }
    conn.busy_timeout(Duration::from_millis(100))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA cache_size=-1024; PRAGMA temp_store=FILE;")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn bounded_read(path: &Path, limit: usize, work: &Work<'_>) -> Result<Vec<u8>, String> {
    work.check()?;
    let parent = path.parent().ok_or(INVALID)?;
    let mut file = regular(
        parent,
        path.file_name().and_then(|v| v.to_str()).ok_or(INVALID)?,
    )?;
    if file.metadata().map_err(|e| e.to_string())?.len() > limit as u64 {
        return Err(LIMIT.into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    work.check()?;
    if bytes.len() > limit {
        return Err(LIMIT.into());
    }
    Ok(bytes)
}
fn hash_reader(file: &mut File, expected: u64, work: &Work<'_>) -> Result<String, String> {
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0u8; COPY_BUFFER];
    loop {
        work.check()?;
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        bytes = bytes.checked_add(count as u64).ok_or(LIMIT)?;
        if bytes > expected {
            return Err(INVALID.into());
        }
        hash.update(&buffer[..count]);
    }
    if bytes != expected {
        return Err(INVALID.into());
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn verify_asset(root: &Path, asset: &Asset, work: &Work<'_>) -> Result<(), String> {
    if !valid_hash(&asset.sha256) || asset.bytes > TOTAL_LIMIT {
        return Err(INVALID.into());
    }
    let mut source = regular(root, &asset.relative_path)?;
    if source.metadata().map_err(|e| e.to_string())?.len() != asset.bytes
        || hash_reader(&mut source, asset.bytes, work)? != asset.sha256
    {
        return Err(INVALID.into());
    }
    Ok(())
}
fn copy_asset(
    root: &Path,
    destination: &Path,
    asset: &Asset,
    work: &Work<'_>,
) -> Result<(), String> {
    if !valid_hash(&asset.sha256) || asset.bytes > TOTAL_LIMIT {
        return Err(INVALID.into());
    }
    let mut source = regular(root, &asset.relative_path)?;
    if source.metadata().map_err(|e| e.to_string())?.len() != asset.bytes {
        return Err(INVALID.into());
    }
    let path = destination.join(relative(&asset.relative_path)?);
    std::fs::create_dir_all(path.parent().ok_or(INVALID)?).map_err(|e| e.to_string())?;
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0u8; COPY_BUFFER];
    loop {
        work.check()?;
        let count = source.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        bytes = bytes.checked_add(count as u64).ok_or(LIMIT)?;
        if bytes > asset.bytes {
            return Err(INVALID.into());
        }
        hash.update(&buffer[..count]);
        target
            .write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
    }
    if bytes != asset.bytes || format!("{:x}", hash.finalize()) != asset.sha256 {
        return Err(INVALID.into());
    }
    target.sync_all().map_err(|e| e.to_string())?;
    work.check()
}

fn backup_database(
    source: &Connection,
    destination: &mut Connection,
    work: &Work<'_>,
) -> Result<(), String> {
    let page_size: u64 = source
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if page_size == 0 || page_size > 65536 {
        return Err(INVALID.into());
    }
    let backup = Backup::new(source, destination).map_err(|e| e.to_string())?;
    let mut busy_since = None;
    loop {
        work.check()?;
        let result = backup.step(64).map_err(|e| e.to_string())?;
        let progress = backup.progress();
        let total = u64::try_from(progress.pagecount)
            .map_err(|_| INVALID)?
            .checked_mul(page_size)
            .ok_or(LIMIT)?;
        if total > TOTAL_LIMIT {
            return Err(LIMIT.into());
        }
        (work.progress)(
            "database",
            total.saturating_sub(progress.remaining.max(0) as u64 * page_size),
            Some(total),
        );
        match result {
            StepResult::Done => return work.check(),
            StepResult::More => busy_since = None,
            StepResult::Busy | StepResult::Locked => {
                let since = busy_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_secs(2) {
                    return Err("CASE_RECOVERY_BUSY: O banco está ocupado; tente novamente.".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => return Err(INVALID.into()),
        }
    }
}

struct Limited<W> {
    writer: W,
    bytes: usize,
}
impl<W: Write> Write for Limited<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MANIFEST_LIMIT.saturating_sub(self.bytes) {
            return Err(std::io::Error::other(LIMIT));
        }
        let n = self.writer.write(bytes)?;
        self.bytes += n;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

/// The enumerator must read the COPIED database and describe every referenced
/// immutable asset, including inactive/orphan history. It may not mutate the
/// source profile. Metadata is bounded before retaining each descriptor.
fn prepare_with(
    root: &Path,
    work: &Work<'_>,
    enumerate: impl FnOnce(
        &Connection,
        &mut dyn FnMut(Asset) -> Result<(), String>,
    ) -> Result<Vec<UnavailableReference>, String>,
) -> Result<VerifiedRecovery, String> {
    let root_lease = RootLease::shared(root, work)?;
    let root = root_lease.root.clone();
    let recovery_root = root.join("case-recovery-v1");
    std::fs::create_dir_all(&recovery_root).map_err(|e| e.to_string())?;
    let staging = tempfile::Builder::new()
        .prefix("pending-")
        .tempdir_in(&recovery_root)
        .map_err(|e| e.to_string())?;
    let database_path = staging.path().join("investigations.sqlite3");
    let source = readonly(&root.join("investigations.sqlite3"))?;
    let mut destination = Connection::open(&database_path).map_err(|e| e.to_string())?;
    backup_database(&source, &mut destination, work)?;
    drop(source);
    destination
        .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
        .map_err(|e| e.to_string())?;
    work.check()?;
    integrity_check(&destination, work)?;
    destination
        .execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    assert_no_permits(&destination)?;
    let authority = crate::case_evidence::snapshot_authority(&destination)?;
    let mut assets = Vec::new();
    let mut seen = BTreeSet::new();
    let mut total = std::fs::metadata(&database_path)
        .map_err(|e| e.to_string())?
        .len();
    let mut metadata = Limited {
        writer: std::io::sink(),
        bytes: 0,
    };
    let unavailable_references = enumerate(&destination, &mut |asset| {
        work.check()?;
        asset_path(&asset.relative_path)?;
        if assets.len() >= ENTRY_LIMIT
            || !valid_hash(&asset.sha256)
            || !seen.insert(asset.relative_path.clone())
        {
            return Err(INVALID.into());
        }
        total = total.checked_add(asset.bytes).ok_or(LIMIT)?;
        if total > TOTAL_LIMIT {
            return Err(LIMIT.into());
        }
        serde_json::to_writer(&mut metadata, &asset).map_err(|e| e.to_string())?;
        assets.push(asset);
        Ok(())
    })?;
    destination
        .execute_batch("COMMIT")
        .map_err(|e| e.to_string())?;
    drop(destination);
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&database_path)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    let preflight = Manifest {
        version: FORMAT,
        interpretation_assets_version: Some(1),
        id: uuid::Uuid::nil().to_string(),
        authority: authority.clone(),
        database: Asset {
            relative_path: "investigations.sqlite3".into(),
            bytes: total,
            sha256: "0".repeat(64),
        },
        assets: assets.clone(),
        unavailable_references: unavailable_references.clone(),
    };
    serde_json::to_writer(
        Limited {
            writer: std::io::sink(),
            bytes: 0,
        },
        &preflight,
    )
    .map_err(|e| e.to_string())?;
    drop(preflight);
    for (index, asset) in assets.iter().enumerate() {
        copy_asset(&root, &staging.path().join("assets"), asset, work)?;
        (work.progress)("assets", index as u64 + 1, Some(assets.len() as u64));
    }
    let database_bytes = std::fs::metadata(&database_path)
        .map_err(|e| e.to_string())?
        .len();
    let database_hash = hash_reader(
        &mut File::open(&database_path).map_err(|e| e.to_string())?,
        database_bytes,
        work,
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    let manifest = Manifest {
        version: FORMAT,
        interpretation_assets_version: Some(1),
        id: id.clone(),
        authority,
        database: Asset {
            relative_path: "investigations.sqlite3".into(),
            bytes: database_bytes,
            sha256: database_hash,
        },
        assets,
        unavailable_references,
    };
    let path = staging.path().join("manifest.json");
    {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let mut writer = Limited {
            writer: file,
            bytes: 0,
        };
        serde_json::to_writer(&mut writer, &manifest).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
        writer.writer.sync_all().map_err(|e| e.to_string())?;
    }
    let bytes = bounded_read(&path, MANIFEST_LIMIT, work)?;
    let manifest_sha256 = digest(&bytes);
    work.check()?;
    let generation = lock_file(&staging.path().join("lease"), true)?;
    generation.sync_all().map_err(|e| e.to_string())?;
    FileExt::try_lock_shared(&generation).map_err(|e| e.to_string())?;
    let mut result = VerifiedRecovery {
        root,
        directory: staging.path().to_path_buf(),
        manifest,
        manifest_sha256,
        root_directory: root_lease
            .directory
            .try_clone()
            .map_err(|e| e.to_string())?,
        _generation: generation,
    };
    result.validate_with(work)?;
    sync_directory(staging.path())?;
    work.check()?;
    let destination = recovery_root.join(&id);
    // Windows requires closing this otherwise unshared staging handle before
    // rename. No client has received the random generation name yet.
    drop(result._generation);
    std::fs::rename(staging.path(), &destination).map_err(|e| e.to_string())?;
    sync_directory(&recovery_root)?;
    result.directory = destination;
    result._generation = lock_file(&result.directory.join("lease"), false)?;
    FileExt::try_lock_shared(&result._generation).map_err(|e| e.to_string())?;
    Ok(result)
}

fn asset_path(value: &str) -> Result<&Path, String> {
    let path = relative(value)?;
    let pieces: Vec<_> = value.split('/').collect();
    let uuid = |value: &str, suffix: &str| {
        value
            .strip_suffix(suffix)
            .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok_and(|id| id.to_string() == v))
    };
    let valid = match pieces.as_slice() {
        ["case-images", id] => valid_hash(id),
        ["exclusions-v1", id] => uuid(id, ".sqlite3"),
        ["evidence-v1", id] => uuid(id, ".batch") || uuid(id, ".manifest"),
        ["case-bootstrap-v1", generation, name] => {
            valid_hash(generation)
                && matches!(*name, "cases.json" | "cases.backup.json" | "manifest.json")
        }
        ["references-v1", owner, version, name] => {
            valid_hash(owner)
                && valid_hash(version)
                && matches!(
                    *name,
                    "source.jsonl" | "rows.sqlite" | "ready.json" | "lease"
                )
        }
        ["derived_fields.json"] => true,
        [name] if LEGACY_INTERPRETATION_FILES.contains(name) => true,
        ["sigma", rest @ ..] if !rest.is_empty() => path.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yml") || extension.eq_ignore_ascii_case("yaml")
        }),
        _ => false,
    };
    if !valid {
        return Err(INVALID.into());
    }
    Ok(path)
}
fn table_exists(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [name],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}
fn assert_no_permits(conn: &Connection) -> Result<(), String> {
    if table_exists(conn, "native_evidence_permits")?
        && conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_evidence_permits)",
                [],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|e| e.to_string())?
    {
        return Err("CASE_RECOVERY_PERMIT: A cópia contém uma autorização de escrita inesperada; os originais foram preservados.".into());
    }
    Ok(())
}
fn integrity_check(conn: &Connection, work: &Work<'_>) -> Result<(), String> {
    work.check()?;
    conn.progress_handler(1000, Some(crate::operations::cancelled));
    let result = conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0));
    conn.progress_handler(0, None::<fn() -> bool>);
    let result = result.map_err(|e| e.to_string())?;
    work.check()?;
    if result != "ok" {
        return Err(INVALID.into());
    }
    Ok(())
}
fn describe(root: &Path, name: &str, limit: u64, work: &Work<'_>) -> Result<Asset, String> {
    asset_path(name)?;
    let mut file = regular(root, name)?;
    let bytes = file.metadata().map_err(|e| e.to_string())?.len();
    if bytes > limit {
        return Err(LIMIT.into());
    }
    Ok(Asset {
        relative_path: name.into(),
        bytes,
        sha256: hash_reader(&mut file, bytes, work)?,
    })
}

fn legacy_interpretation_dependencies(
    root: &Path,
    work: &Work<'_>,
    emit: &mut dyn FnMut(Asset) -> Result<(), String>,
) -> Result<(), String> {
    for name in LEGACY_INTERPRETATION_FILES {
        work.check()?;
        match std::fs::symlink_metadata(root.join(name)) {
            Ok(_) => emit(describe(root, name, LEGACY_INTERPRETATION_BYTES, work)?)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.to_string()),
        }
    }
    let sigma = root.join("sigma");
    match std::fs::symlink_metadata(&sigma) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(INVALID.into());
        }
        Ok(_) => (),
    }
    let mut pending = vec![sigma];
    let mut examined = 0usize;
    while let Some(directory) = pending.pop() {
        work.check()?;
        for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
            work.check()?;
            examined = examined.checked_add(1).filter(|count| *count <= ENTRY_LIMIT).ok_or(LIMIT)?;
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            // Match the Sigma reader's rule discovery without following links
            // into arbitrary directories outside the selected profile.
            if kind.is_symlink() { continue; }
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case("yml") || extension.eq_ignore_ascii_case("yaml")
            }) {
                if !kind.is_file() { return Err(INVALID.into()); }
                let name = path.strip_prefix(root).map_err(|_| INVALID)?
                    .to_str().ok_or(INVALID)?.replace('\\', "/");
                emit(describe(root, &name, LEGACY_INTERPRETATION_BYTES, work)?)?;
            }
        }
    }
    Ok(())
}
fn image_ids(text: &str) -> Result<BTreeSet<String>, String> {
    let raw = crate::case_evidence::checked_document(text)?;
    let mut result = BTreeSet::new();
    let mut inspected = 0usize;
    let mut root = raw.members()?;
    while let Some(field) = root.next()? {
        if !matches!(field.key()?.as_str(), "items" | "caseTrails") {
            continue;
        }
        if field.value.kind() == b'n' {
            continue;
        }
        let mut items = field.value.elements()?;
        while let Some(item) = items.next()? {
            crate::operations::check()?;
            if item.kind() != b'{' {
                continue;
            }
            let mut fields = item.members()?;
            while let Some(field) = fields.next()? {
                if field.key()? != "attachments" || field.value.kind() == b'n' {
                    continue;
                }
                let mut refs = field.value.elements()?;
                while let Some(reference) = refs.next()? {
                    inspected += 1;
                    if inspected > 1000 {
                        return Err(LIMIT.into());
                    }
                    let mut members = reference.members()?;
                    let mut id = None;
                    while let Some(member) = members.next()? {
                        let key = member.key()?;
                        if matches!(key.as_str(), "base64" | "dataUrl" | "data_url") {
                            return Err(INVALID.into());
                        }
                        if key == "id" {
                            if id.is_some() || member.value.get().len() > 128 {
                                return Err(INVALID.into());
                            }
                            let value: String = serde_json::from_str(member.value.get())
                                .map_err(|e| e.to_string())?;
                            if !valid_hash(&value) {
                                return Err(INVALID.into());
                            }
                            id = Some(value);
                        }
                    }
                    result.insert(id.ok_or(INVALID)?);
                }
            }
        }
    }
    Ok(result)
}
fn enumerate_dependencies(
    root: &Path,
    conn: &Connection,
    work: &Work<'_>,
    emit: &mut dyn FnMut(Asset) -> Result<(), String>,
) -> Result<Vec<UnavailableReference>, String> {
    let mut seen = BTreeMap::<String, (u64, String)>::new();
    let metadata_bytes = std::cell::Cell::new(0usize);
    let mut add = |asset: Asset| -> Result<(), String> {
        asset_path(&asset.relative_path)?;
        if let Some(old) = seen.get(&asset.relative_path) {
            return if *old == (asset.bytes, asset.sha256.clone()) {
                Ok(())
            } else {
                Err(INVALID.into())
            };
        }
        if seen.len() >= ENTRY_LIMIT {
            return Err(LIMIT.into());
        }
        metadata_bytes.set(
            metadata_bytes
                .get()
                .checked_add(asset.relative_path.len() + asset.sha256.len() + 128)
                .filter(|v| *v <= MANIFEST_LIMIT)
                .ok_or(LIMIT)?,
        );
        seen.insert(
            asset.relative_path.clone(),
            (asset.bytes, asset.sha256.clone()),
        );
        emit(asset)
    };
    crate::case_evidence::native_dependencies(conn, |asset| {
        add(Asset {
            relative_path: asset.relative_path,
            bytes: asset.bytes,
            sha256: asset.sha256,
        })
    })?;
    let mut examined_exclusions = 0usize;
    for (table, column) in [
        ("exclusion_batches", "body"),
        ("exclusion_masks", "payload"),
    ] {
        if !table_exists(conn, table)? {
            continue;
        }
        let mut statement = conn
            .prepare(&format!(
                "SELECT id,octet_length({column}),CASE WHEN octet_length({column})<=8388608 THEN {column} ELSE NULL END FROM {table} ORDER BY id"
            ))
            .map_err(|e| e.to_string())?;
        let mut rows = statement.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            work.check()?;
            examined_exclusions += 1;
            if examined_exclusions > ENTRY_LIMIT {
                return Err(LIMIT.into());
            }
            if row.get::<_, u64>(1).map_err(|e| e.to_string())? > 8 << 20 {
                return Err(LIMIT.into());
            }
            let id: String = row.get(0).map_err(|e| e.to_string())?;
            let text: String = row.get(2).map_err(|e| e.to_string())?;
            let payload = if table == "exclusion_batches" {
                let batch: crate::exclusion_store::BatchInfo =
                    serde_json::from_str(&text).map_err(|e| e.to_string())?;
                if batch.id != id {
                    return Err(INVALID.into());
                }
                batch.payload().clone()
            } else {
                serde_json::from_str::<crate::exclusion_store::PayloadMeta>(&text)
                    .map_err(|e| e.to_string())?
            };
            add(Asset {
                relative_path: format!("exclusions-v1/{}.sqlite3", payload.id()),
                bytes: payload.bytes(),
                sha256: payload.sha256().into(),
            })?;
        }
    }
    if table_exists(conn, "cases")? {
        let mut statement = conn
            .prepare("SELECT octet_length(body),CASE WHEN octet_length(body)<=67108864 THEN body ELSE NULL END FROM cases ORDER BY id")
            .map_err(|e| e.to_string())?;
        let mut rows = statement.query([]).map_err(|e| e.to_string())?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            work.check()?;
            count += 1;
            if count > 1000 || row.get::<_, u64>(0).map_err(|e| e.to_string())? > 64 << 20 {
                return Err(LIMIT.into());
            }
            let text: String = row.get(1).map_err(|e| e.to_string())?;
            // Recovery only needs attachment identities. Unrelated unsupported
            // metadata numbers remain opaque rather than blocking preservation.
            for id in image_ids(&text)? {
                let asset = describe(root, &format!("case-images/{id}"), 16 << 20, work)?;
                if asset.sha256 != id {
                    return Err(INVALID.into());
                }
                add(asset)?;
            }
        }
    }
    let mut unavailable = Vec::new();
    if table_exists(conn, "case_analysis")? {
        let mut statement = conn.prepare("SELECT case_id,octet_length(body),CASE WHEN octet_length(body)<=4194304 THEN body ELSE NULL END FROM case_analysis ORDER BY case_id").map_err(|e|e.to_string())?;
        let mut rows = statement.query([]).map_err(|e| e.to_string())?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            work.check()?;
            count += 1;
            if count > 1000 || row.get::<_, u64>(1).map_err(|e| e.to_string())? > 4 << 20 {
                return Err(LIMIT.into());
            }
            let id: String = row.get(0).map_err(|e| e.to_string())?;
            let text: String = row.get(2).map_err(|e| e.to_string())?;
            let snapshot: crate::analysis_context::Snapshot =
                serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if snapshot.case_id != id {
                return Err(INVALID.into());
            }
            let owner = crate::reference_store::Owner {
                case_id: snapshot.case_id,
                analysis_id: snapshot.analysis_id,
            };
            for descriptor in snapshot.config.references {
                work.check()?;
                let directory =
                    crate::reference_store::recovery_directory(root, &owner, &descriptor)
                        .map_err(|e| e.to_string())?;
                match std::fs::symlink_metadata(&directory) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        // Absence is observed, not inferred to be intentional.
                        // A partially present version is always an error below.
                        let entry = UnavailableReference {
                            owner: owner.clone(),
                            descriptor,
                        };
                        metadata_bytes.set(
                            metadata_bytes
                                .get()
                                .checked_add(
                                    serde_json::to_vec(&entry).map_err(|e| e.to_string())?.len(),
                                )
                                .filter(|v| *v <= MANIFEST_LIMIT)
                                .ok_or(LIMIT)?,
                        );
                        if unavailable.len() >= 4096 {
                            return Err(LIMIT.into());
                        }
                        unavailable.push(entry);
                        continue;
                    }
                    Ok(m) if m.file_type().is_dir() => {}
                    Ok(_) => return Err(INVALID.into()),
                    Err(e) => return Err(e.to_string()),
                }
                let reader =
                    crate::reference_store::open_source(root, &owner, &descriptor, work.cancelled)
                        .map_err(|e| e.to_string())?;
                drop(reader);
                for (name, limit) in [
                    ("source.jsonl", 256 << 20),
                    ("rows.sqlite", 512 << 20),
                    ("ready.json", 64 << 10),
                    ("lease", 0),
                ] {
                    let path = directory.join(name);
                    let relative = path
                        .strip_prefix(root)
                        .map_err(|_| INVALID)?
                        .to_str()
                        .ok_or(INVALID)?
                        .replace('\\', "/");
                    add(describe(root, &relative, limit, work)?)?;
                }
            }
        }
    }
    let legacy = root.join("derived_fields.json");
    match std::fs::symlink_metadata(&legacy) {
        Ok(_) => add(describe(root, "derived_fields.json", 4 << 20, work)?)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    legacy_interpretation_dependencies(root, work, &mut add)?;
    Ok(unavailable)
}

/// Copies a consistent SQLite snapshot and every required immutable asset.
/// Missing whole reference versions remain explicit observed-unavailable state.
pub(crate) fn prepare(root: &Path, work: &Work<'_>) -> Result<VerifiedRecovery, String> {
    prepare_with(root, work, |conn, emit| {
        enumerate_dependencies(root, conn, work, emit)
    })
}

fn validate_manifest(manifest: &Manifest) -> Result<(), String> {
    if manifest.version != FORMAT
        || manifest.interpretation_assets_version.is_some_and(|version| version != 1)
        || uuid::Uuid::parse_str(&manifest.id).is_err()
        || manifest.database.relative_path != "investigations.sqlite3"
        || !valid_hash(&manifest.database.sha256)
        || manifest.assets.len() > ENTRY_LIMIT
        || manifest.unavailable_references.len() > 4096
    {
        return Err(INVALID.into());
    }
    let mut seen = BTreeSet::new();
    let mut total = manifest.database.bytes;
    for asset in &manifest.assets {
        asset_path(&asset.relative_path)?;
        if !valid_hash(&asset.sha256) || !seen.insert(&asset.relative_path) {
            return Err(INVALID.into());
        }
        total = total
            .checked_add(asset.bytes)
            .filter(|v| *v <= TOTAL_LIMIT)
            .ok_or(LIMIT)?;
    }
    if total > TOTAL_LIMIT {
        return Err(LIMIT.into());
    }
    Ok(())
}
/// Reopen by an application-generated UUID. Client paths and verified flags are
/// never accepted. Full bytes/closure/authority verification precedes the handle.
pub(crate) fn load_verified(
    root: &Path,
    id: &str,
    work: &Work<'_>,
) -> Result<VerifiedRecovery, String> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(INVALID.into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let root_directory = open_directory(&root)?;
    let directory = root.join("case-recovery-v1").join(id);
    let _directory = open_directory(&directory)?;
    let generation = lock_file(&directory.join("lease"), false)?;
    FileExt::try_lock_shared(&generation).map_err(|_| "CASE_RECOVERY_BUSY".to_string())?;
    let bytes = bounded_read(&directory.join("manifest.json"), MANIFEST_LIMIT, work)?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    validate_manifest(&manifest)?;
    if manifest.id != id {
        return Err(INVALID.into());
    }
    let value = VerifiedRecovery {
        root,
        directory,
        manifest,
        manifest_sha256: digest(&bytes),
        root_directory,
        _generation: generation,
    };
    value.validate_with(work)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn work() -> Work<'static> {
        Work {
            cancelled: &|| false,
            progress: &|_, _, _| {},
        }
    }
    fn profile() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT); INSERT INTO metadata VALUES('revision','7'); CREATE TABLE cases(id TEXT PRIMARY KEY,body TEXT,position INTEGER);").unwrap();
        conn.execute("INSERT INTO cases VALUES('c',?1,0)",[r#"{"id":"c","notes":"original","items":[{"id":"item","rows":[{"id":1,"fields":{"wide":18446744073709551615,"decimal":1.000,"negative":-0.0},"future":{"key":"kept"}}]}]}"#]).unwrap();
        drop(conn);
        root
    }
    #[test]
    fn verified_snapshot_reopens_preserves_exact_body_and_ignores_later_notes() {
        let root = profile();
        let recovery = prepare(root.path(), &work()).unwrap();
        let receipt = recovery.receipt();
        assert_eq!(recovery.authority().body_revision, "7");
        let conn = readonly(&recovery.snapshot_database_path()).unwrap();
        let body: String = conn
            .query_row("SELECT body FROM cases", [], |r| r.get(0))
            .unwrap();
        drop(conn);
        assert!(body.contains("18446744073709551615"));
        assert!(body.contains("1.000"));
        assert!(body.contains("-0.0"));
        let live = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        live.execute("UPDATE cases SET body='{}'", []).unwrap();
        drop(live);
        drop(recovery);
        let reopened = load_verified(root.path(), &receipt.recovery_id, &work()).unwrap();
        assert_eq!(reopened.receipt().manifest_sha256, receipt.manifest_sha256);
        assert_eq!(
            readonly(&reopened.snapshot_database_path())
                .unwrap()
                .query_row::<String, _, _>("SELECT body FROM cases", [], |r| r.get(0))
                .unwrap(),
            body
        );
    }
    #[test]
    fn recovery_preserves_all_legacy_interpretation_bytes_and_nested_sigma() {
        let root = profile();
        for name in LEGACY_INTERPRETATION_FILES {
            std::fs::write(root.path().join(name), format!("malformed original: {name}\n")).unwrap();
        }
        // Too large for activation, but still recoverable as exact raw input.
        let oversized = vec![b'x'; (3 << 20) + 1];
        std::fs::write(root.path().join("codes.json"), &oversized).unwrap();
        std::fs::create_dir_all(root.path().join("sigma/windows")).unwrap();
        std::fs::write(root.path().join("sigma/windows/rule.YAML"), b"invalid: [ original\n").unwrap();
        std::fs::write(root.path().join("sigma/README.txt"), b"not an active Sigma source").unwrap();
        let recovery = prepare(root.path(), &work()).unwrap();
        assert!(recovery.has_interpretation_assets());
        assert_eq!(recovery.manifest.assets.len(), LEGACY_INTERPRETATION_FILES.len() + 1);
        let assets = recovery.snapshot_assets_root();
        for name in LEGACY_INTERPRETATION_FILES {
            assert_eq!(std::fs::read(assets.join(name)).unwrap(), std::fs::read(root.path().join(name)).unwrap());
        }
        assert_eq!(std::fs::read(assets.join("sigma/windows/rule.YAML")).unwrap(), b"invalid: [ original\n");
        assert!(!assets.join("sigma/README.txt").exists());
        let (_, diagnostics) = crate::case_interpretation::local_legacy(&assets);
        assert!(diagnostics.iter().any(|issue| issue.code == "legacy_interpretation_unavailable_codes"));
        assert!(diagnostics.iter().any(|issue| issue.code == "legacy_interpretation_unavailable_security_sigma"));
        std::fs::write(root.path().join("codes.json"), b"later live edit").unwrap();
        recovery.validate().unwrap();
        assert_eq!(std::fs::read(assets.join("codes.json")).unwrap(), oversized);
    }
    #[test]
    fn recovery_manifest_distinguishes_legacy_unknown_from_verified_absence() {
        let root = profile();
        let recovery = prepare(root.path(), &work()).unwrap();
        assert!(recovery.has_interpretation_assets());
        assert!(recovery.manifest.assets.is_empty());
        let mut encoded = serde_json::to_value(&recovery.manifest).unwrap();
        encoded.as_object_mut().unwrap().remove("interpretationAssetsVersion");
        let mut legacy: Manifest = serde_json::from_value(encoded).unwrap();
        assert!(legacy.interpretation_assets_version.is_none());
        validate_manifest(&legacy).unwrap();
        legacy.interpretation_assets_version = Some(2);
        assert!(validate_manifest(&legacy).is_err());
    }
    #[test]
    fn recovery_refuses_excessive_legacy_assets_without_replacing_originals() {
        let root = profile();
        let path = root.path().join("codes.json");
        let file = File::create(&path).unwrap();
        file.set_len(LEGACY_INTERPRETATION_BYTES + 1).unwrap();
        drop(file);
        let error = prepare(root.path(), &work()).err().unwrap();
        assert!(error.contains("CASE_RECOVERY_LIMIT"));
        assert_eq!(std::fs::metadata(path).unwrap().len(), LEGACY_INTERPRETATION_BYTES + 1);
    }
    #[test]
    fn cancellation_and_corrupt_copy_do_not_adopt_or_change_original_database() {
        let root = profile();
        assert!(prepare(
            root.path(),
            &Work {
                cancelled: &|| true,
                progress: &|_, _, _| {}
            }
        )
        .is_err());
        let recovery = prepare(root.path(), &work()).unwrap();
        let copied = recovery.snapshot_database_path();
        drop(recovery);
        let mut bytes = std::fs::read(&copied).unwrap();
        bytes[100] ^= 1;
        std::fs::write(&copied, bytes).unwrap();
        let id = copied
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert!(load_verified(root.path(), id, &work()).is_err());
        let live = readonly(&root.path().join("investigations.sqlite3")).unwrap();
        assert_eq!(
            live.query_row::<String, _, _>(
                "SELECT value FROM metadata WHERE key='revision'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            "7"
        );
        assert!(!table_exists(&live, "native_evidence_store").unwrap());
    }
    #[test]
    fn completed_recovery_does_not_hold_root_quiescence_indefinitely() {
        let root = profile();
        let recovery = prepare(root.path(), &work()).unwrap();
        let exclusive = RootLease::exclusive(root.path(), &work()).unwrap();
        assert!(RootLease::shared(root.path(), &work()).is_err());
        recovery.validate().unwrap();
        drop(exclusive);
        RootLease::shared(root.path(), &work()).unwrap();
    }
    #[test]
    fn backup_preserves_references_and_observed_absence_but_rejects_partial_versions() {
        let root = profile();
        let bytes = b"{\"id\":1,\"label\":\"first\"}\n";
        let descriptor = crate::analysis_context::ReferenceDescriptor {
            interpretation_version: 1,
            schema_version: 1,
            id: "r".into(),
            name: "reference".into(),
            content_sha256: digest(bytes),
            format: "jsonl".into(),
            columns: vec!["id".into(), "label".into()],
            key_columns: vec!["id".into()],
            duplicate_policy: "reject".into(),
        };
        let owner = crate::reference_store::Owner {
            case_id: "c".into(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
        };
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        conn.execute_batch(
            "CREATE TABLE case_analysis(case_id TEXT PRIMARY KEY,body TEXT NOT NULL);",
        )
        .unwrap();
        let body = serde_json::json!({"schemaVersion":1,"caseId":owner.case_id,"analysisId":owner.analysis_id,"configRevision":0,"visibilityRevision":0,"config":{"derivedFields":[],"references":[descriptor]},"migrationDiagnostics":[]});
        conn.execute(
            "INSERT INTO case_analysis VALUES('c',?1)",
            [body.to_string()],
        )
        .unwrap();
        drop(conn);
        let absent = prepare(root.path(), &work()).unwrap();
        assert_eq!(absent.manifest.unavailable_references.len(), 1);
        crate::reference_store::prepare_jsonl(
            root.path(),
            &owner,
            &descriptor,
            &bytes[..],
            crate::reference_store::Limits::default(),
            &|| false,
        )
        .unwrap();
        let available = prepare(root.path(), &work()).unwrap();
        assert!(available.manifest.unavailable_references.is_empty());
        assert_eq!(available.manifest.assets.len(), 4);
        let reader = crate::reference_store::open(
            &available.snapshot_assets_root(),
            &owner,
            &descriptor,
            &|| false,
        )
        .unwrap();
        assert_eq!(reader.prepared().row_count, 1);
        drop(reader);
        let before = crate::case_evidence::snapshot_authority(
            &readonly(&root.path().join("investigations.sqlite3")).unwrap(),
        )
        .unwrap();
        let error = restore_current(
            root.path(),
            &absent,
            &before,
            &uuid::Uuid::new_v4().to_string(),
            &work(),
            || {},
        )
        .err()
        .unwrap();
        assert!(error.contains("ASSET_CONFLICT"));
        let version =
            crate::reference_store::recovery_directory(root.path(), &owner, &descriptor).unwrap();
        std::fs::remove_file(version.join("ready.json")).unwrap();
        assert!(prepare(root.path(), &work()).is_err());
    }
    #[test]
    fn image_reference_discovery_does_not_decode_unrelated_numeric_metadata() {
        let id = "a".repeat(64);
        let text = format!(
            r#"{{"id":"c","unknown":1e400,"items":[{{"attachments":[{{"id":"{id}","name":"image"}}],"rows":[{{"fields":{{"number":18446744073709551616}}}}]}}]}}"#
        );
        assert_eq!(image_ids(&text).unwrap(), BTreeSet::from([id]));
    }
    #[test]
    fn recovery_refuses_committed_write_permits_before_publication() {
        let root = profile();
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        conn.execute_batch("CREATE TABLE native_evidence_permits(case_id TEXT PRIMARY KEY,nonce TEXT NOT NULL);INSERT INTO native_evidence_permits VALUES('c','unexpected');").unwrap();
        drop(conn);
        let error = prepare(root.path(), &work()).err().unwrap();
        assert!(error.contains("CASE_RECOVERY_PERMIT"));
    }
    #[test]
    fn recovery_dependency_paths_cannot_escape_or_admit_arbitrary_files() {
        for path in [
            "../outside",
            "/absolute",
            "case-images/../private",
            "unknown/file",
            "references-v1/a/b/source.jsonl",
            "case-images/a\\b",
            "case-bootstrap-v1/not-a-hash/cases.json",
            "sigma/../codes.json",
            "sigma/private.json",
            "sigma/rule.yaml/../other.yaml",
            "credentials.json",
        ] {
            assert!(asset_path(path).is_err(), "{path}");
        }
        assert!(asset_path(&format!("case-images/{}", "a".repeat(64))).is_ok());
        for name in ["cases.json", "cases.backup.json", "manifest.json"] {
            assert!(asset_path(&format!("case-bootstrap-v1/{}/{name}", "a".repeat(64))).is_ok());
        }
        assert!(asset_path(&format!("case-bootstrap-v1/{}/other.json", "a".repeat(64))).is_err());
        for name in LEGACY_INTERPRETATION_FILES { assert!(asset_path(name).is_ok()); }
        assert!(asset_path("sigma/windows/subdirectory/rule.YAML").is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn interpretation_recovery_never_follows_sigma_or_catalog_symlinks() {
        use std::os::unix::fs::symlink;
        let root = profile();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("private.yaml"), b"not authorized as a profile rule").unwrap();
        std::fs::create_dir(root.path().join("sigma")).unwrap();
        symlink(outside.path(), root.path().join("sigma/external")).unwrap();
        symlink(outside.path().join("private.yaml"), root.path().join("sigma/linked.yaml")).unwrap();
        let recovery = prepare(root.path(), &work()).unwrap();
        assert!(recovery.manifest.assets.is_empty());
        assert!(!recovery.snapshot_assets_root().join("sigma/external/private.yaml").exists());
        symlink(outside.path().join("private.yaml"), root.path().join("codes.json")).unwrap();
        assert!(prepare(root.path(), &work()).is_err());
        assert_eq!(std::fs::read(outside.path().join("private.yaml")).unwrap(), b"not authorized as a profile rule");
    }
    #[cfg(unix)]
    #[test]
    fn readonly_rejects_initial_fifo_and_symlink_without_plain_database_handles() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt, os::unix::fs::symlink};
        let root = profile();
        let path = root.path().join("pipe");
        let c = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(readonly(&path).is_err());
        let link = root.path().join("link");
        symlink(root.path().join("investigations.sqlite3"), &link).unwrap();
        assert!(readonly(&link).is_err());
    }
    #[test]
    fn sqlite_writer_lock_child() {
        let Some(path) = std::env::var_os("CASE_RECOVERY_LOCK_CHILD") else {
            return;
        };
        let conn = Connection::open(path).unwrap();
        conn.busy_timeout(Duration::ZERO).unwrap();
        assert!(
            conn.execute_batch("BEGIN IMMEDIATE").is_err(),
            "another process bypassed the active writer lock"
        );
    }
    #[test]
    fn sqlite_only_readonly_open_keeps_other_thread_writer_lock() {
        let root = profile();
        let path = root.path().join("investigations.sqlite3");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=DELETE; BEGIN IMMEDIATE")
            .unwrap();
        let reader = readonly(&path).unwrap();
        drop(reader);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "case_recovery::tests::sqlite_writer_lock_child",
                "--test-threads=1",
            ])
            .env("CASE_RECOVERY_LOCK_CHILD", &path)
            .status()
            .unwrap();
        assert!(status.success());
        conn.execute_batch("ROLLBACK").unwrap();
    }
}

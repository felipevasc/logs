//! Reversible exclusion ledger. Immutable, versioned membership payloads are
//! staged outside the investigation transaction; only metadata publication and
//! visibility CAS happen inside it. This module never deletes original logs or
//! modifies evidence. Query consumers compile/cache the streamed visible union;
//! `contains` is for small probes, not for one SQLite lookup per scanned event.
use crate::analysis_context::{Identity, Snapshot};
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

const FORMAT: u32 = 1;
const MAX_REF: usize = 4096;
const MAX_LOCATOR: usize = 512;
const MAX_SOURCES_BYTES: usize = 1 << 20;
const MAX_RECEIPT_BYTES: usize = 64 << 10;
const BATCH_ROWS: usize = 64;
const MAX_MEMBER_BYTES: usize = MAX_REF + MAX_LOCATOR + 64 + std::mem::size_of::<Member>() + 32;

#[derive(Clone, Debug)]
pub struct Budget {
    pub max_payload_bytes: u64,
    pub max_members: u64,
    pub max_open_payloads: usize,
    pub stream_buffer_bytes: usize,
    /// Sum of serialized dictionaries, separately bounded from row buffers.
    pub dictionary_bytes: u64,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            max_payload_bytes: 4 << 30,
            max_members: 100_000_000,
            max_open_payloads: 128,
            stream_buffer_bytes: 8 << 20,
            dictionary_bytes: 8 << 20,
        }
    }
}
pub struct Work<'a> {
    pub cancelled: &'a dyn Fn() -> bool,
    /// Phase, completed, optional total. Staging rows are not a fake percentage.
    pub progress: &'a dyn Fn(&str, u64, Option<u64>),
}
impl Work<'_> {
    fn check(&self) -> Result<(), String> {
        if (self.cancelled)() {
            Err("Operação cancelada.".into())
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Admission {
    pub analysis: Identity,
    /// Captured by the command. The command validates it while holding its
    /// source consistency guard through publication; this ledger has no AppState.
    pub source_receipt: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceDescriptor {
    /// Version fingerprint, explicitly not necessarily a full content hash.
    pub version: String,
    /// Parser/metadata generation or a persisted evidence-record namespace.
    pub record_space: String,
    pub label: String,
    /// Ordinary indexed references are prefix + ':' + byte offset. Storing the
    /// prefix once avoids repeating its 64 characters for every member.
    pub event_ref_prefix: Option<String>,
}
impl SourceDescriptor {
    pub fn key(&self) -> Result<String, String> {
        if self.version.is_empty()
            || self.version.len() > 512
            || self.record_space.is_empty()
            || self.record_space.len() > 512
            || self.label.len() > 4096
            || self
                .event_ref_prefix
                .as_ref()
                .is_some_and(|p| p.is_empty() || p.len() > MAX_REF)
        {
            return Err("Proveniência da exclusão inválida ou excessiva.".into());
        }
        let bytes =
            serde_json::to_vec(&(&self.version, &self.record_space, &self.event_ref_prefix))
                .map_err(|e| e.to_string())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Locator {
    ByteOffset(u64),
    StableRecord(String),
}
impl Locator {
    fn encode(&self) -> Result<Vec<u8>, String> {
        match self {
            Self::ByteOffset(offset) => {
                let mut out = vec![0];
                out.extend(offset.to_be_bytes());
                Ok(out)
            }
            Self::StableRecord(key) if !key.is_empty() && key.len() <= MAX_LOCATOR => {
                let mut out = vec![1];
                out.extend(key.as_bytes());
                Ok(out)
            }
            _ => Err("Localizador estável do registro inválido.".into()),
        }
    }
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        match bytes.split_first() {
            Some((0, rest)) if rest.len() == 8 => Ok(Self::ByteOffset(u64::from_be_bytes(
                rest.try_into().unwrap(),
            ))),
            Some((1, rest)) if !rest.is_empty() && rest.len() <= MAX_LOCATOR => {
                Ok(Self::StableRecord(
                    std::str::from_utf8(rest)
                        .map_err(|_| "Localizador corrompido.")?
                        .into(),
                ))
            }
            _ => Err("Localizador de exclusão corrompido.".into()),
        }
    }
}
#[derive(Clone, Debug)]
pub struct InputMember {
    pub source_index: usize,
    pub locator: Locator,
    pub event_ref: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    pub source_key: String,
    pub locator: Locator,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub key: Key,
    pub event_ref: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum Purpose {
    Exclude,
    RestoreSelection { batch_id: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PayloadMeta {
    format_version: u32,
    id: String,
    sha256: String,
    bytes: u64,
    members: u64,
    dictionary_bytes: u64,
}
impl PayloadMeta {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }
    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchInfo {
    pub id: String,
    pub analysis_id: String,
    pub created_at_ms: u64,
    pub label: String,
    pub reason: String,
    pub scope: Value,
    pub source_receipt: Value,
    pub members: u64,
    pub active: bool,
    pub restored_at_ms: Option<u64>,
    payload: PayloadMeta,
}
impl BatchInfo {
    pub(crate) fn payload(&self) -> &PayloadMeta {
        &self.payload
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub analysis_context: Snapshot,
    pub batch_id: String,
    pub selected_members: u64,
    /// Membership union accounting is prepared separately. Batch size is never
    /// misrepresented as how many records became visible after overlaps.
    pub newly_visible: Option<u64>,
}
pub struct Staged {
    directory: PendingDirectory,
    admission: Admission,
    purpose: Purpose,
    payload: PayloadMeta,
}
impl Staged {
    pub fn members(&self) -> u64 {
        self.payload.members
    }
    pub fn admission(&self) -> &Admission {
        &self.admission
    }
}
const PENDING_KIND: &str = "loginsight-exclusion-stage";
const PENDING_MARKER: &str = ".owner.json";
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingMarker {
    version: u32,
    kind: String,
    directory: String,
    root: PathBuf,
    nonce: String,
    created_at_ms: u64,
    directory_id: Option<(u64, u64)>,
}
#[cfg(unix)]
fn pending_directory_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    metadata
        .file_type()
        .is_dir()
        .then_some((metadata.dev(), metadata.ino()))
}
#[cfg(windows)]
fn pending_directory_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x02000000)
        .open(path)
        .ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.ok()?;
    Some((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}
#[cfg(not(any(unix, windows)))]
fn pending_directory_id(_path: &Path) -> Option<(u64, u64)> {
    None
}
struct PendingDirectory {
    directory: Option<tempfile::TempDir>,
    // Outside the removable directory: the owner lock stays held during its
    // entire cleanup on Windows as well as Unix.
    owner: File,
    owner_path: PathBuf,
}
impl PendingDirectory {
    fn path(&self) -> &Path {
        self.directory
            .as_ref()
            .expect("owned pending directory")
            .path()
    }
    fn create(dir: &Path) -> Result<Self, String> {
        Self::create_at(dir, now_ms())
    }
    fn create_at(dir: &Path, created_at_ms: u64) -> Result<Self, String> {
        let root = root(dir);
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        if !std::fs::symlink_metadata(&root)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_dir()
        {
            return Err("Diretório de preparação não é um diretório regular.".into());
        }
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let directory = tempfile::Builder::new()
            .prefix("pending-")
            .tempdir_in(&root)
            .map_err(|e| e.to_string())?;
        let leaf = directory
            .path()
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or("Diretório temporário inválido.")?
            .to_string();
        let owner_path = root.join(format!("{leaf}.owner"));
        let owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&owner_path)
            .map_err(|e| e.to_string())?;
        owner.try_lock_exclusive().map_err(|e| e.to_string())?;
        let mut pending = Self {
            directory: Some(directory),
            owner,
            owner_path,
        };
        let marker = PendingMarker {
            version: 1,
            kind: PENDING_KIND.into(),
            directory: leaf,
            root,
            nonce: uuid::Uuid::new_v4().to_string(),
            created_at_ms,
            directory_id: pending_directory_id(pending.path()),
        };
        let bytes = serde_json::to_vec(&marker).map_err(|e| e.to_string())?;
        if bytes.len() > 4096 {
            return Err("Marcador temporário excessivo.".into());
        }
        use std::io::Write;
        pending
            .owner
            .write_all(&bytes)
            .and_then(|_| pending.owner.sync_all())
            .map_err(|e| e.to_string())?;
        let mut inside = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(pending.path().join(PENDING_MARKER))
            .map_err(|e| e.to_string())?;
        inside
            .write_all(&bytes)
            .and_then(|_| inside.sync_all())
            .map_err(|e| e.to_string())?;
        sync_dir(pending.path())?;
        sync_dir(pending.owner_path.parent().unwrap())?;
        Ok(pending)
    }
}
impl Drop for PendingDirectory {
    fn drop(&mut self) {
        let removed = self
            .directory
            .take()
            .is_none_or(|directory| directory.close().is_ok());
        // Keep ownership evidence if cleanup failed. The file lease remains held
        // until this destructor returns; a future cleaner can retry safely.
        if removed {
            let _ = std::fs::remove_file(&self.owner_path);
        }
    }
}
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCleanup {
    pub inspected: usize,
    pub removed: usize,
    pub skipped: usize,
    pub scan_limited: bool,
}
fn small_regular(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.file_type().is_file() || metadata.len() > 4096 {
        return Err("Marcador temporário inválido.".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4096 {
        return Err("Marcador temporário excessivo.".into());
    }
    Ok(bytes)
}
/// Compatible single-pass adapter. Retain PendingSweep across bounded passes
/// when a directory may contain a large stable prefix of published files.
pub fn cleanup_pending(
    dir: &Path,
    minimum_age: Duration,
    max_entries: usize,
    work: &Work<'_>,
) -> Result<PendingCleanup, String> {
    PendingSweep::new().pass(dir, minimum_age, max_entries, work)
}
fn cleanup_pending_at(
    dir: &Path,
    minimum_age: Duration,
    max_entries: usize,
    work: &Work<'_>,
    now: u64,
) -> Result<PendingCleanup, String> {
    PendingSweep::new().pass_at(dir, minimum_age, max_entries, work, now)
}
/// One caller-owned directory stream, with no collected path list. Retaining
/// it advances later passes past already-inspected prefixes. A changed root
/// or directory object resets the stream; EOF starts a new sweep next time.
#[derive(Default)]
pub struct PendingSweep {
    root: Option<(PathBuf, Option<(u64, u64)>)>,
    entries: Option<std::fs::ReadDir>,
}
impl PendingSweep {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn pass(
        &mut self,
        dir: &Path,
        minimum_age: Duration,
        max_entries: usize,
        work: &Work<'_>,
    ) -> Result<PendingCleanup, String> {
        self.pass_at(dir, minimum_age, max_entries, work, now_ms())
    }
    fn pass_at(
        &mut self,
        dir: &Path,
        minimum_age: Duration,
        max_entries: usize,
        work: &Work<'_>,
        now: u64,
    ) -> Result<PendingCleanup, String> {
        work.check()?;
        let mut summary = PendingCleanup::default();
        let path = root(dir);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.entries = None;
                self.root = None;
                return Ok(summary);
            }
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.file_type().is_dir() {
            self.entries = None;
            self.root = None;
            return Ok(summary);
        }
        let root = path.canonicalize().map_err(|e| e.to_string())?;
        let root_id = pending_directory_id(&root);
        if root_id.is_none()
            || self
                .root
                .as_ref()
                .is_none_or(|(path, id)| path != &root || *id != root_id)
            || self.entries.is_none()
        {
            self.entries = Some(std::fs::read_dir(&root).map_err(|e| e.to_string())?);
            self.root = Some((root.clone(), root_id));
        }
        let minimum = minimum_age.as_millis().min(u64::MAX as u128) as u64;
        let limit = max_entries.min(4096);
        loop {
            work.check()?;
            if summary.inspected >= limit {
                summary.scan_limited = true;
                break;
            }
            let Some(entry) = self.entries.as_mut().expect("opened sweep").next() else {
                self.entries = None;
                break;
            };
            summary.inspected += 1;
            let entry = entry.map_err(|e| e.to_string())?;
            let Some(leaf) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !leaf.starts_with("pending-")
                || leaf.len() > 96
                || !leaf.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || !entry.file_type().map_err(|e| e.to_string())?.is_dir()
            {
                continue;
            }
            let target = entry.path();
            let owner_path = root.join(format!("{leaf}.owner"));
            let result = (|| -> Result<bool, String> {
                let bytes = small_regular(&owner_path)?;
                let owner = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&owner_path)
                    .map_err(|e| e.to_string())?;
                if owner.try_lock_exclusive().is_err() {
                    return Ok(false);
                }
                // Re-read after taking ownership, and reject path or marker changes.
                let mut read = &owner;
                let mut locked = Vec::new();
                read.by_ref()
                    .take(4097)
                    .read_to_end(&mut locked)
                    .map_err(|e| e.to_string())?;
                if locked != bytes
                    || small_regular(&owner_path)? != bytes
                    || small_regular(&target.join(PENDING_MARKER))? != bytes
                {
                    return Ok(false);
                }
                let marker: PendingMarker =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                if marker.version != 1
                    || marker.kind != PENDING_KIND
                    || marker.directory != leaf
                    || marker.root != root
                    || uuid::Uuid::parse_str(&marker.nonce).is_err()
                    || marker.created_at_ms > now
                    || now - marker.created_at_ms < minimum
                    || marker.directory_id.is_none()
                    || pending_directory_id(&target) != marker.directory_id
                    || target.canonicalize().map_err(|e| e.to_string())? != root.join(&leaf)
                {
                    return Ok(false);
                }
                let mut files = Vec::new();
                for child in std::fs::read_dir(&target).map_err(|e| e.to_string())? {
                    work.check()?;
                    let child = child.map_err(|e| e.to_string())?;
                    let name = child.file_name();
                    let Some(name) = name.to_str() else {
                        return Ok(false);
                    };
                    if files.len() >= 3
                        || !matches!(
                            name,
                            PENDING_MARKER | "payload.sqlite3" | "payload.sqlite3-journal"
                        )
                        || !child.file_type().map_err(|e| e.to_string())?.is_file()
                    {
                        return Ok(false);
                    }
                    files.push(child.path());
                }
                if pending_directory_id(&root) != root_id
                    || pending_directory_id(&target) != marker.directory_id
                {
                    return Ok(false);
                }
                for file in files.iter().filter(|file| {
                    file.file_name().and_then(|v| v.to_str()) != Some(PENDING_MARKER)
                }) {
                    work.check()?;
                    std::fs::remove_file(file).map_err(|e| e.to_string())?;
                }
                work.check()?;
                // Complete these final metadata removals with the same exclusive
                // owner lease held; cancellation before them leaves a valid marker.
                std::fs::remove_file(target.join(PENDING_MARKER)).map_err(|e| e.to_string())?;
                std::fs::remove_dir(&target).map_err(|e| e.to_string())?;
                std::fs::remove_file(&owner_path).map_err(|e| e.to_string())?;
                drop(owner);
                Ok(true)
            })();
            match result {
                Ok(true) => summary.removed += 1,
                Ok(false) | Err(_) => summary.skipped += 1,
            }
            work.check()?;
        }
        Ok(summary)
    }
}

fn root(dir: &Path) -> PathBuf {
    dir.join("exclusions-v1")
}
fn db_path(dir: &Path, id: &str) -> Result<PathBuf, String> {
    uuid::Uuid::parse_str(id).map_err(|_| "Identificador de exclusão inválido.")?;
    Ok(root(dir).join(format!("{id}.sqlite3")))
}
fn ledger_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS exclusion_batches(id TEXT PRIMARY KEY,analysis_id TEXT NOT NULL,body TEXT NOT NULL,active INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS exclusion_analysis ON exclusion_batches(analysis_id,active); CREATE TABLE IF NOT EXISTS exclusion_masks(id TEXT PRIMARY KEY,batch_id TEXT NOT NULL,payload TEXT NOT NULL);").map_err(|e|e.to_string())
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn encoded<T: Serialize>(value: &T, max: usize) -> Result<String, String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    if text.len() > max {
        Err("Metadados da exclusão excederam o limite.".into())
    } else {
        Ok(text)
    }
}
fn stamp(file: &File) -> Result<(u64, Option<SystemTime>), String> {
    let m = file.metadata().map_err(|e| e.to_string())?;
    Ok((m.len(), m.modified().ok()))
}
fn hash_file(path: &Path, work: &Work<'_>) -> Result<(u64, String), String> {
    if !std::fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_file()
    {
        return Err("Payload de exclusão ausente ou não regular.".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let before = stamp(&file)?;
    let mut buffer = vec![0u8; 64 << 10];
    let mut digest = Sha256::new();
    let mut completed = 0;
    let mut reported = Instant::now();
    (work.progress)("verifying", 0, Some(before.0));
    loop {
        work.check()?;
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        completed += read as u64;
        if reported.elapsed() >= Duration::from_millis(150) {
            (work.progress)("verifying", completed, Some(before.0));
            reported = Instant::now();
        }
    }
    if stamp(&file)? != before || stamp(&File::open(path).map_err(|e| e.to_string())?)? != before {
        return Err("Payload de exclusão mudou durante a validação.".into());
    }
    (work.progress)("verifying", completed, Some(before.0));
    Ok((before.0, format!("{:x}", digest.finalize())))
}
fn sync_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub fn stage(
    dir: &Path,
    admission: Admission,
    purpose: Purpose,
    sources: &[SourceDescriptor],
    members: impl IntoIterator<Item = Result<InputMember, String>>,
    budget: &Budget,
    work: &Work<'_>,
) -> Result<Staged, String> {
    stage_with(
        dir,
        admission,
        purpose,
        sources,
        |emit| {
            for member in members {
                emit(member?)?;
            }
            Ok(())
        },
        budget,
        work,
    )
}
/// A synchronous producer can transfer exact selected IDs without Event hydration
/// or an extra thread/queue. Any rejected input latches failure even if a buggy
/// producer swallows the callback error; no partial batch can become prepared.
pub fn stage_with(
    dir: &Path,
    admission: Admission,
    purpose: Purpose,
    sources: &[SourceDescriptor],
    produce: impl FnOnce(&mut dyn FnMut(InputMember) -> Result<(), String>) -> Result<(), String>,
    budget: &Budget,
    work: &Work<'_>,
) -> Result<Staged, String> {
    stage_impl(
        dir, admission, purpose, sources, produce, budget, work, true,
    )
}
fn stage_impl(
    dir: &Path,
    admission: Admission,
    purpose: Purpose,
    sources: &[SourceDescriptor],
    produce: impl FnOnce(&mut dyn FnMut(InputMember) -> Result<(), String>) -> Result<(), String>,
    budget: &Budget,
    work: &Work<'_>,
    existing_analysis: bool,
) -> Result<Staged, String> {
    work.check()?;
    if existing_analysis {
        ensure_ledger(dir)?;
        crate::analysis_context::visibility_read_at(dir, &admission.analysis, |_| Ok(()))?;
    }
    encoded(&admission.source_receipt, MAX_RECEIPT_BYTES)?;
    if !admission.source_receipt.is_object() {
        return Err("Recibo de fonte ausente ou inválido.".into());
    }
    if budget.max_payload_bytes < 4096
        || budget.max_members == 0
        || budget.max_open_payloads == 0
        || budget.stream_buffer_bytes < MAX_MEMBER_BYTES * 4
        || budget.dictionary_bytes == 0
    {
        return Err("Orçamento de exclusões inválido.".into());
    }
    if let Purpose::RestoreSelection { batch_id } = &purpose {
        uuid::Uuid::parse_str(batch_id).map_err(|_| "Lote de restauração inválido.")?;
    }
    encoded(&sources, MAX_SOURCES_BYTES)?;
    let dictionary_bytes = sources.iter().try_fold(0u64, |total, source| {
        let bytes = encoded(source, MAX_SOURCES_BYTES)?.len() as u64 + 64;
        total
            .checked_add(bytes)
            .ok_or_else(|| "Dicionário de exclusão excessivo.".to_string())
    })?;
    if dictionary_bytes > budget.dictionary_bytes {
        return Err("Dicionário de fontes excede o orçamento da análise.".into());
    }
    let mut sorted: Vec<_> = sources
        .iter()
        .enumerate()
        .map(|(index, source)| Ok((source.key()?, index, source)))
        .collect::<Result<_, String>>()?;
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    if sorted.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(
            "Descritores de fonte duplicados; unifique a mesma geração antes de excluir.".into(),
        );
    }
    std::fs::create_dir_all(root(dir)).map_err(|e| e.to_string())?;
    let directory = PendingDirectory::create(dir)?;
    let path = directory.path().join("payload.sqlite3");
    let mut conn = Connection::open(&path).map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA page_size=4096; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=-512; PRAGMA temp_store=FILE; PRAGMA mmap_size=0; CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE sources(id INTEGER PRIMARY KEY,key TEXT UNIQUE NOT NULL,body TEXT NOT NULL); CREATE TABLE members(source_id INTEGER NOT NULL,locator BLOB NOT NULL,event_ref TEXT,PRIMARY KEY(source_id,locator)) WITHOUT ROWID;").map_err(|e|e.to_string())?;
    conn.pragma_update(
        None,
        "max_page_count",
        (budget.max_payload_bytes / 4096).min(i64::MAX as u64) as i64,
    )
    .map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("INSERT INTO meta VALUES('format','1'),('id',?1)", [&id])
        .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO meta VALUES('admission',?1),('purpose',?2)",
        params![
            encoded(&admission, MAX_RECEIPT_BYTES * 2)?,
            encoded(&purpose, 4096)?
        ],
    )
    .map_err(|e| e.to_string())?;
    let mut mapping = vec![0usize; sources.len()];
    for (sid, (key, original, source)) in sorted.iter().enumerate() {
        mapping[*original] = sid;
        tx.execute(
            "INSERT INTO sources VALUES(?1,?2,?3)",
            params![sid as i64, key, encoded(source, MAX_SOURCES_BYTES)?],
        )
        .map_err(|e| e.to_string())?;
    }
    let mut count = 0u64;
    let mut examined = 0u64;
    let mut reported = Instant::now();
    (work.progress)("staging", 0, None);
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO members VALUES(?1,?2,?3) ON CONFLICT(source_id,locator) DO NOTHING",
            )
            .map_err(|e| e.to_string())?;
        let mut existing = tx
            .prepare("SELECT event_ref FROM members WHERE source_id=?1 AND locator=?2")
            .map_err(|e| e.to_string())?;
        let mut failure: Option<String> = None;
        let produced = produce(&mut |input| {
            if let Some(error) = &failure {
                return Err(error.clone());
            }
            let result: Result<(), String> = (|| {
                if examined % 256 == 0 {
                    work.check()?;
                    if reported.elapsed() >= Duration::from_millis(150) {
                        (work.progress)("staging", examined, None);
                        reported = Instant::now();
                    }
                }
                let source = sources
                    .get(input.source_index)
                    .ok_or("Registro faz referência a uma fonte desconhecida.")?;
                if input.event_ref.is_empty() || input.event_ref.len() > MAX_REF {
                    return Err("Referência estável do registro inválida ou excessiva.".into());
                }
                let locator = input.locator.encode()?;
                let sid = mapping[input.source_index] as i64;
                let reconstructed = match (&source.event_ref_prefix, &input.locator) {
                    (Some(prefix), Locator::ByteOffset(offset)) => {
                        Some(format!("{prefix}:{offset}"))
                    }
                    _ => None,
                };
                let reference = (reconstructed.as_deref() != Some(input.event_ref.as_str()))
                    .then_some(input.event_ref.as_str());
                let inserted = insert
                    .execute(params![sid, &locator, reference])
                    .map_err(|e| {
                        format!(
                            "Falha ao preparar exclusões (orçamento de disco ou persistência): {e}"
                        )
                    })?;
                if inserted == 0 {
                    let previous: Option<String> = existing
                        .query_row(params![sid, &locator], |r| r.get(0))
                        .map_err(|e| e.to_string())?;
                    if previous.as_deref() != reference {
                        return Err(
                            "O mesmo localizador tem referências de evento conflitantes.".into(),
                        );
                    }
                } else {
                    count += 1;
                    if count > budget.max_members {
                        return Err("A exclusão excede o orçamento de registros.".into());
                    }
                }
                examined = examined
                    .checked_add(1)
                    .ok_or("Contagem de exclusão excedeu o limite.")?;
                Ok(())
            })();
            if let Err(error) = &result {
                failure = Some(error.clone());
            }
            result
        });
        if let Some(error) = failure {
            return Err(error);
        }
        produced?;
    }
    if count == 0 {
        return Err("Nenhum registro foi selecionado para a exclusão ou restauração.".into());
    }
    work.check()?;
    tx.execute("INSERT INTO meta VALUES('members',?1)", [count.to_string()])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    conn.close().map_err(|(_, e)| e.to_string())?;
    // Windows FlushFileBuffers requires a writable file handle.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    let (bytes, sha256) = hash_file(&path, work)?;
    if bytes > budget.max_payload_bytes {
        return Err("Payload de exclusão excede o orçamento de disco.".into());
    }
    sync_dir(directory.path())?;
    let payload = PayloadMeta {
        format_version: FORMAT,
        id,
        sha256,
        bytes,
        members: count,
        dictionary_bytes,
    };
    if let Purpose::RestoreSelection { batch_id } = &purpose {
        if existing_analysis {
            let original =
                crate::analysis_context::visibility_read_at(dir, &admission.analysis, |conn| {
                    info(conn, &admission.analysis.analysis_id, batch_id)
                })?;
            if !original.active {
                return Err("O lote já foi restaurado integralmente.".into());
            }
            let original = Payload::open(db_path(dir, batch_id)?, &original.payload, work)?;
            let selected = Payload::open(path, &payload, work)?;
            verify_subset(&original, &selected, work, budget.stream_buffer_bytes)?;
        }
    }
    (work.progress)("staged", count, Some(count));
    Ok(Staged {
        directory,
        admission,
        purpose,
        payload,
    })
}

fn validate_payload_schema(conn: &Connection) -> Result<(), String> {
    fn normalized(sql: &str) -> String {
        sql.chars()
            .filter(|c| !c.is_ascii_whitespace())
            .flat_map(char::to_lowercase)
            .collect()
    }
    let expected = [
        ("table", "meta", "meta", Some("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)")),
        ("table", "sources", "sources", Some("CREATE TABLE sources(id INTEGER PRIMARY KEY,key TEXT UNIQUE NOT NULL,body TEXT NOT NULL)")),
        ("table", "members", "members", Some("CREATE TABLE members(source_id INTEGER NOT NULL,locator BLOB NOT NULL,event_ref TEXT,PRIMARY KEY(source_id,locator)) WITHOUT ROWID")),
        ("index", "sqlite_autoindex_meta_1", "meta", None),
        ("index", "sqlite_autoindex_sources_1", "sources", None),
    ];
    let mut seen = std::collections::BTreeSet::new();
    let mut stmt = conn
        .prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if seen.len() >= expected.len() {
            return Err("Esquema de exclusão contém objetos não permitidos.".into());
        }
        let kind = row
            .get_ref(0)
            .map_err(|e| e.to_string())?
            .as_str()
            .map_err(|e| e.to_string())?;
        let name = row
            .get_ref(1)
            .map_err(|e| e.to_string())?
            .as_str()
            .map_err(|e| e.to_string())?;
        let table = row
            .get_ref(2)
            .map_err(|e| e.to_string())?
            .as_str()
            .map_err(|e| e.to_string())?;
        let sql = row.get_ref(3).map_err(|e| e.to_string())?;
        let sql = match sql {
            rusqlite::types::ValueRef::Null => None,
            rusqlite::types::ValueRef::Text(value) if value.len() <= 4096 => {
                Some(std::str::from_utf8(value).map_err(|e| e.to_string())?)
            }
            _ => return Err("Esquema de exclusão inválido.".into()),
        };
        let Some((i, entry)) = expected
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.0 == kind && entry.1 == name && entry.2 == table)
        else {
            return Err("Esquema de exclusão contém objetos não permitidos.".into());
        };
        if !seen.insert(i) || entry.3.map(normalized) != sql.map(normalized) {
            return Err("Esquema de exclusão incompatível.".into());
        }
    }
    if seen.len() != expected.len() {
        return Err("Esquema de exclusão incompleto.".into());
    }
    Ok(())
}

struct Payload {
    conn: Connection,
    path: PathBuf,
    file: std::sync::Arc<File>,
    stamp: (u64, Option<SystemTime>),
    sources: Vec<(String, SourceDescriptor)>,
    meta: PayloadMeta,
}
impl Payload {
    fn open(path: PathBuf, meta: &PayloadMeta, work: &Work<'_>) -> Result<Self, String> {
        if meta.format_version != FORMAT || meta.sha256.len() != 64 || meta.bytes == 0 {
            return Err("Formato de payload de exclusão não suportado.".into());
        }
        if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() != meta.bytes {
            return Err("Tamanho do payload de exclusão não confere.".into());
        }
        let (bytes, digest) = hash_file(&path, work)?;
        if bytes != meta.bytes || digest != meta.sha256 {
            return Err("Exclusões corrompidas: a integridade do payload não confere. A análise permanece bloqueada até recuperar ou restaurar o lote.".into());
        }
        let file = File::open(&path).map_err(|e| e.to_string())?;
        FileExt::lock_shared(&file).map_err(|e| e.to_string())?;
        let stamp = stamp(&file)?;
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| e.to_string())?;
        // Bound SQLite values before row decoding can allocate owned strings.
        unsafe {
            rusqlite::ffi::sqlite3_limit(
                conn.handle(),
                rusqlite::ffi::SQLITE_LIMIT_LENGTH,
                2 << 20,
            );
        }
        conn.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; PRAGMA cache_size=-64; PRAGMA temp_store=FILE; PRAGMA mmap_size=0;").map_err(|e|e.to_string())?;
        validate_payload_schema(&conn)?;
        for (key, expected) in [
            ("format", FORMAT.to_string()),
            ("id", meta.id.clone()),
            ("members", meta.members.to_string()),
        ] {
            let actual: String = conn
                .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            if actual != expected {
                return Err("Metadados do payload de exclusão não conferem.".into());
            }
        }
        let sources = {
            let mut stmt = conn
                .prepare("SELECT id,key,body FROM sources ORDER BY id")
                .map_err(|e| e.to_string())?;
            let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
            let mut sources: Vec<(String, SourceDescriptor)> = Vec::new();
            let mut size = 0usize;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                work.check()?;
                let id: i64 = row.get(0).map_err(|e| e.to_string())?;
                let key: String = row.get(1).map_err(|e| e.to_string())?;
                let body: String = row.get(2).map_err(|e| e.to_string())?;
                size = size
                    .checked_add(body.len() + key.len())
                    .ok_or("Metadados de exclusão excessivos.")?;
                if size > MAX_SOURCES_BYTES * 2 || id != sources.len() as i64 {
                    return Err("Dicionário de fontes da exclusão inválido ou excessivo.".into());
                }
                let source: SourceDescriptor =
                    serde_json::from_str(&body).map_err(|e| e.to_string())?;
                if source.key()? != key || sources.last().is_some_and(|last| last.0 >= key) {
                    return Err("Identidade do dicionário de fontes não confere.".into());
                }
                sources.push((key, source));
            }
            if size as u64 != meta.dictionary_bytes {
                return Err("Tamanho do dicionário de exclusão não confere.".into());
            }
            sources
        };
        Ok(Self {
            conn,
            path,
            file: std::sync::Arc::new(file),
            stamp,
            sources,
            meta: meta.clone(),
        })
    }
    fn unchanged(&self) -> Result<(), String> {
        if stamp(&self.file)? != self.stamp
            || stamp(&File::open(&self.path).map_err(|e| e.to_string())?)? != self.stamp
        {
            return Err(
                "Payload de exclusão alterado; recarregue e verifique a integridade.".into(),
            );
        }
        Ok(())
    }
    fn member(
        &self,
        sid: i64,
        locator: &[u8],
        reference: Option<String>,
    ) -> Result<Member, String> {
        let source = usize::try_from(sid)
            .ok()
            .and_then(|sid| self.sources.get(sid))
            .ok_or("Fonte de exclusão corrompida.")?;
        let locator = Locator::decode(locator)?;
        let event_ref = match (reference, &source.1.event_ref_prefix, &locator) {
            (Some(reference), _, _) if !reference.is_empty() && reference.len() <= MAX_REF => {
                reference
            }
            (None, Some(prefix), Locator::ByteOffset(offset)) => format!("{prefix}:{offset}"),
            _ => return Err("Referência de exclusão corrompida.".into()),
        };
        Ok(Member {
            key: Key {
                source_key: source.0.clone(),
                locator,
            },
            event_ref,
        })
    }
    fn contains(&self, member: &Member) -> Result<bool, String> {
        self.unchanged()?;
        let Ok(sid) = self
            .sources
            .binary_search_by(|source| source.0.cmp(&member.key.source_key))
        else {
            return Ok(false);
        };
        let key = member.key.locator.encode()?;
        let found: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT event_ref FROM members WHERE source_id=?1 AND locator=?2",
                params![sid as i64, &key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(reference) = found else {
            return Ok(false);
        };
        if self.member(sid as i64, &key, reference)?.event_ref != member.event_ref {
            return Err("Referência do registro diverge da proveniência arquivada.".into());
        }
        Ok(true)
    }
    fn cursor(&self, buffer_bytes: usize) -> Cursor<'_> {
        Cursor {
            payload: self,
            last_sid: -1,
            last_locator: Vec::new(),
            buffer: VecDeque::new(),
            finished: false,
            read: 0,
            check_total: true,
            buffer_bytes,
        }
    }
}
struct Cursor<'a> {
    payload: &'a Payload,
    last_sid: i64,
    last_locator: Vec<u8>,
    buffer: VecDeque<Member>,
    finished: bool,
    read: u64,
    check_total: bool,
    buffer_bytes: usize,
}
impl Cursor<'_> {
    fn next(&mut self, work: &Work<'_>) -> Result<Option<Member>, String> {
        work.check()?;
        if let Some(member) = self.buffer.pop_front() {
            return Ok(Some(member));
        }
        if self.finished {
            return Ok(None);
        }
        self.payload.unchanged()?;
        let mut stmt=self.payload.conn.prepare("SELECT source_id,locator,event_ref FROM members WHERE (source_id,locator)>(?1,?2) ORDER BY source_id,locator LIMIT ?3").map_err(|e|e.to_string())?;
        let mut rows = stmt
            .query(params![
                self.last_sid,
                &self.last_locator,
                BATCH_ROWS as i64
            ])
            .map_err(|e| e.to_string())?;
        let mut bytes = 0usize;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            work.check()?;
            let sid: i64 = row.get(0).map_err(|e| e.to_string())?;
            let locator: Vec<u8> = row.get(1).map_err(|e| e.to_string())?;
            let reference: Option<String> = row.get(2).map_err(|e| e.to_string())?;
            let member = self.payload.member(sid, &locator, reference)?;
            bytes += member.event_ref.len()
                + member.key.source_key.len()
                + locator.len()
                + std::mem::size_of::<Member>();
            self.last_sid = sid;
            self.last_locator = locator;
            self.buffer.push_back(member);
            self.read += 1;
            if bytes >= self.buffer_bytes.max(1) {
                break;
            }
        }
        if self.buffer.is_empty() {
            self.finished = true;
            if self.check_total && self.read != self.payload.meta.members {
                return Err("Contagem do payload de exclusão não confere.".into());
            }
        }
        Ok(self.buffer.pop_front())
    }
}
fn ensure_ledger(dir: &Path) -> Result<(), String> {
    let conn = crate::case_store::context_connection(dir)?;
    ledger_schema(&conn)
}
fn info(conn: &Connection, analysis_id: &str, id: &str) -> Result<BatchInfo, String> {
    let body: String = conn
        .query_row(
            "SELECT body FROM exclusion_batches WHERE analysis_id=?1 AND id=?2",
            params![analysis_id, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("Lote de exclusão não pertence a esta análise.")?;
    let batch: BatchInfo = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if batch.id != id || batch.analysis_id != analysis_id || batch.payload.id != id {
        return Err("Identidade do lote de exclusão inválida.".into());
    }
    Ok(batch)
}
fn masks(conn: &Connection, id: &str) -> Result<Vec<PayloadMeta>, String> {
    let mut stmt = conn
        .prepare("SELECT payload FROM exclusion_masks WHERE batch_id=?1 ORDER BY id")
        .map_err(|e| e.to_string())?;
    let result = stmt
        .query_map([id], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .map(|row| {
            serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect();
    result
}
fn verify_subset(
    original: &Payload,
    selected: &Payload,
    work: &Work<'_>,
    buffer: usize,
) -> Result<(), String> {
    let per = (buffer / 2).saturating_sub(MAX_MEMBER_BYTES).max(1);
    let mut base = original.cursor(per);
    let mut mask = selected.cursor(per);
    let mut current = base.next(work)?;
    let mut reported = Instant::now();
    (work.progress)("validating_restore", 0, None);
    while let Some(member) = mask.next(work)? {
        while current
            .as_ref()
            .is_some_and(|candidate| candidate.key < member.key)
        {
            current = base.next(work)?;
            if reported.elapsed() >= Duration::from_millis(150) {
                (work.progress)("validating_restore", base.read, None);
                reported = Instant::now();
            }
        }
        if current.as_ref() != Some(&member) {
            return Err(
                "A restauração contém registros que não pertencem ao lote original.".into(),
            );
        }
    }
    Ok(())
}
fn active_payloads(conn: &Connection, analysis_id: &str) -> Result<usize, String> {
    conn.query_row("SELECT (SELECT count(*) FROM exclusion_batches WHERE analysis_id=?1 AND active=1)+(SELECT count(*) FROM exclusion_masks m JOIN exclusion_batches b ON b.id=m.batch_id WHERE b.analysis_id=?1 AND b.active=1)",[analysis_id],|r|r.get(0)).map_err(|e|e.to_string())
}

fn active_dictionary_bytes(conn: &Connection, analysis_id: &str) -> Result<u64, String> {
    #[derive(Deserialize)]
    struct OnlyPayload {
        payload: PayloadMeta,
    }
    let mut sum = 0u64;
    let mut stmt=conn.prepare("SELECT body,0 FROM exclusion_batches WHERE analysis_id=?1 AND active=1 UNION ALL SELECT m.payload,1 FROM exclusion_masks m JOIN exclusion_batches b ON b.id=m.batch_id WHERE b.analysis_id=?1 AND b.active=1").map_err(|e|e.to_string())?;
    let mut rows = stmt.query([analysis_id]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let body: String = row.get(0).map_err(|e| e.to_string())?;
        let mask: bool = row.get(1).map_err(|e| e.to_string())?;
        let payload = if mask {
            serde_json::from_str::<PayloadMeta>(&body).map_err(|e| e.to_string())?
        } else {
            serde_json::from_str::<OnlyPayload>(&body)
                .map_err(|e| e.to_string())?
                .payload
        };
        sum = sum
            .checked_add(payload.dictionary_bytes)
            .ok_or("Dicionário de exclusão excessivo.")?;
    }
    Ok(sum)
}
fn check_view_budget(handles: usize, dictionaries: u64, budget: &Budget) -> Result<(), String> {
    if handles > budget.max_open_payloads
        || handles
            .checked_mul(MAX_MEMBER_BYTES * 2)
            .is_none_or(|minimum| minimum > budget.stream_buffer_bytes)
    {
        return Err("A análise atingiu o orçamento de payloads ou buffers ativos; restaure lotes antes de adicionar outro.".into());
    }
    if dictionaries > budget.dictionary_bytes {
        return Err("Dicionários de fontes ativos excedem o orçamento da análise.".into());
    }
    Ok(())
}

/// Publishes only an already prepared payload. The caller holds its source
/// consistency guard through this call, and commits its operation token on success.
pub fn publish(
    dir: &Path,
    staged: Staged,
    label: &str,
    reason: &str,
    scope: Value,
    budget: &Budget,
    work: &Work<'_>,
) -> Result<Receipt, String> {
    work.check()?;
    if label.len() > 4096 || reason.len() > 8192 {
        return Err("Descrição da exclusão excessiva.".into());
    }
    encoded(&scope, MAX_RECEIPT_BYTES)?;
    if staged.payload.bytes > budget.max_payload_bytes
        || staged.payload.members > budget.max_members
    {
        return Err("O payload preparado excede o orçamento atual; refaça a operação.".into());
    }
    ensure_ledger(dir)?;
    let target = db_path(dir, &staged.payload.id)?;
    if target.exists() {
        return Err("O payload já existe; publicação recusada para preservar a identidade.".into());
    }
    // Files have been checksummed and synced during staging. A crash between
    // rename and COMMIT leaves a UUID-owned orphan, never partial visibility.
    std::fs::rename(staged.directory.path().join("payload.sqlite3"), &target)
        .map_err(|e| e.to_string())?;
    sync_dir(&root(dir))?;
    (work.progress)("publishing", 0, None);
    let id = staged.payload.id.clone();
    let selected = staged.payload.members;
    let (snapshot, batch_id) = crate::analysis_context::visibility_update_at(
        dir,
        &staged.admission.analysis,
        work.cancelled,
        |tx| {
            ledger_schema(tx)?;
            let handles = active_payloads(tx, &staged.admission.analysis.analysis_id)?
                .checked_add(1)
                .ok_or("Quantidade de payloads excessiva.")?;
            let dictionaries = active_dictionary_bytes(tx, &staged.admission.analysis.analysis_id)?
                .checked_add(staged.payload.dictionary_bytes)
                .ok_or("Dicionário de exclusão excessivo.")?;
            check_view_budget(handles, dictionaries, budget)?;
            match &staged.purpose {
                Purpose::Exclude => {
                    let batch = BatchInfo {
                        id: id.clone(),
                        analysis_id: staged.admission.analysis.analysis_id.clone(),
                        created_at_ms: now_ms(),
                        label: label.into(),
                        reason: reason.into(),
                        scope,
                        source_receipt: staged.admission.source_receipt.clone(),
                        members: selected,
                        active: true,
                        restored_at_ms: None,
                        payload: staged.payload.clone(),
                    };
                    tx.execute(
                        "INSERT INTO exclusion_batches VALUES(?1,?2,?3,1)",
                        params![
                            id,
                            batch.analysis_id,
                            encoded(&batch, MAX_RECEIPT_BYTES * 3)?
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                    Ok(id.clone())
                }
                Purpose::RestoreSelection { batch_id } => {
                    let batch = info(tx, &staged.admission.analysis.analysis_id, batch_id)?;
                    if !batch.active {
                        return Err("O lote já foi restaurado integralmente.".into());
                    }
                    tx.execute(
                        "INSERT INTO exclusion_masks VALUES(?1,?2,?3)",
                        params![id, batch_id, encoded(&staged.payload, 4096)?],
                    )
                    .map_err(|e| e.to_string())?;
                    Ok(batch_id.clone())
                }
            }
        },
    )?;
    // A cancellation arriving after COMMIT cannot turn publication into failure.
    (work.progress)("published", selected, Some(selected));
    Ok(Receipt {
        analysis_context: snapshot,
        batch_id,
        selected_members: selected,
        newly_visible: None,
    })
}

pub fn restore_batch(
    dir: &Path,
    admission: &Admission,
    batch_id: &str,
    work: &Work<'_>,
) -> Result<Receipt, String> {
    ensure_ledger(dir)?;
    let (snapshot, members) = crate::analysis_context::visibility_update_at(
        dir,
        &admission.analysis,
        work.cancelled,
        |tx| {
            let mut batch = info(tx, &admission.analysis.analysis_id, batch_id)?;
            if !batch.active {
                return Err("O lote já foi restaurado integralmente.".into());
            }
            batch.active = false;
            batch.restored_at_ms = Some(now_ms());
            tx.execute(
                "UPDATE exclusion_batches SET body=?1,active=0 WHERE id=?2",
                params![encoded(&batch, MAX_RECEIPT_BYTES * 3)?, batch_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(batch.members)
        },
    )?;
    Ok(Receipt {
        analysis_context: snapshot,
        batch_id: batch_id.into(),
        selected_members: members,
        newly_visible: None,
    })
}

pub fn list(
    dir: &Path,
    expected: &Identity,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<BatchInfo>, String> {
    ensure_ledger(dir)?;
    crate::analysis_context::visibility_read_at(dir, expected, |conn| {
        let before = match after {
            Some(id) => {
                info(conn, &expected.analysis_id, id)?;
                conn.query_row(
                    "SELECT rowid FROM exclusion_batches WHERE id=?1",
                    [id],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(|e| e.to_string())?
            }
            None => i64::MAX,
        };
        let mut stmt=conn.prepare("SELECT id FROM exclusion_batches WHERE analysis_id=?1 AND rowid<?2 ORDER BY rowid DESC LIMIT ?3").map_err(|e|e.to_string())?;
        let ids = stmt
            .query_map(
                params![expected.analysis_id, before, limit.clamp(1, 100) as i64],
                |r| r.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        ids.iter()
            .map(|id| info(conn, &expected.analysis_id, id))
            .collect()
    })
}

/// Immutable file handles retain the payload lease without carrying SQLite
/// connections into parallel query execution.
#[derive(Clone)]
pub(crate) struct MembershipLease {
    file: std::sync::Arc<File>,
    path: PathBuf,
    stamp: (u64, Option<SystemTime>),
}
impl MembershipLease {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if stamp(&self.file)? != self.stamp
            || stamp(&File::open(&self.path).map_err(|e| e.to_string())?)? != self.stamp
        {
            return Err(
                "Payload de exclusão alterado; verifique a integridade antes de consultar.".into(),
            );
        }
        Ok(())
    }
}
struct ActiveBatch {
    original: Payload,
    masks: Vec<Payload>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    ActiveUnion,
    ExcludedInBatch(String),
}
pub struct VerifiedView {
    pub identity: Identity,
    scope: Scope,
    batches: Vec<ActiveBatch>,
    buffer_bytes: usize,
}
impl VerifiedView {
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub(crate) fn leases(&self) -> Vec<MembershipLease> {
        self.batches
            .iter()
            .flat_map(|batch| std::iter::once(&batch.original).chain(batch.masks.iter()))
            .map(|payload| MembershipLease {
                file: std::sync::Arc::clone(&payload.file),
                path: payload.path.clone(),
                stamp: payload.stamp,
            })
            .collect()
    }
    pub fn has_active_batches(&self) -> bool {
        !self.batches.is_empty()
    }
    pub fn validate(&self) -> Result<(), String> {
        for batch in &self.batches {
            batch.original.unchanged()?;
            for mask in &batch.masks {
                mask.unchanged()?;
            }
        }
        Ok(())
    }
    pub fn contains(&self, member: &Member) -> Result<bool, String> {
        self.validate()?;
        for batch in &self.batches {
            if batch.original.contains(member)? {
                let mut restored = false;
                for mask in &batch.masks {
                    if mask.contains(member)? {
                        restored = true;
                        break;
                    }
                }
                if !restored {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    /// Ordered union minus per-batch restoration masks. Only bounded lookahead
    /// buffers are retained; early false stops without consuming the remainder.
    pub fn visit(
        &self,
        work: &Work<'_>,
        mut visitor: impl FnMut(&Member) -> Result<bool, String>,
    ) -> Result<(), String> {
        self.validate()?;
        let handles = self
            .batches
            .iter()
            .map(|b| 1 + b.masks.len())
            .sum::<usize>()
            .max(1);
        let per = (self.buffer_bytes / handles)
            .saturating_sub(MAX_MEMBER_BYTES)
            .max(1);
        let mut cursors: Vec<_> = self
            .batches
            .iter()
            .map(|batch| BatchCursor::new(batch, per))
            .collect();
        let mut heads: Vec<_> = cursors
            .iter_mut()
            .map(|cursor| cursor.next(work))
            .collect::<Result<_, _>>()?;
        loop {
            work.check()?;
            let Some(key) = heads.iter().flatten().map(|m| &m.key).min().cloned() else {
                return self.validate();
            };
            let mut found = None;
            for (index, head) in heads.iter_mut().enumerate() {
                if head.as_ref().is_some_and(|m| m.key == key) {
                    let member = head.take().unwrap();
                    if found
                        .as_ref()
                        .is_some_and(|previous: &Member| previous.event_ref != member.event_ref)
                    {
                        return Err(
                            "Lotes sobrepostos têm referências conflitantes para o mesmo registro."
                                .into(),
                        );
                    }
                    found = Some(member);
                    *head = cursors[index].next(work)?;
                }
            }
            if !visitor(found.as_ref().unwrap())? {
                return self.validate();
            }
        }
    }
    pub fn sources(&self) -> BTreeMap<String, SourceDescriptor> {
        self.batches
            .iter()
            .flat_map(|batch| batch.original.sources.iter().cloned())
            .collect()
    }
}
struct BatchCursor<'a> {
    original: Cursor<'a>,
    masks: Vec<Cursor<'a>>,
    heads: Vec<Option<Member>>,
    initialized: bool,
}
impl<'a> BatchCursor<'a> {
    fn new(batch: &'a ActiveBatch, buffer: usize) -> Self {
        Self {
            original: batch.original.cursor(buffer),
            masks: batch.masks.iter().map(|p| p.cursor(buffer)).collect(),
            heads: vec![None; batch.masks.len()],
            initialized: false,
        }
    }
    fn next(&mut self, work: &Work<'_>) -> Result<Option<Member>, String> {
        if !self.initialized {
            for (i, mask) in self.masks.iter_mut().enumerate() {
                self.heads[i] = mask.next(work)?;
            }
            self.initialized = true;
        }
        while let Some(member) = self.original.next(work)? {
            let mut restored = false;
            for (index, mask) in self.masks.iter_mut().enumerate() {
                while self.heads[index]
                    .as_ref()
                    .is_some_and(|head| head.key < member.key)
                {
                    self.heads[index] = mask.next(work)?;
                }
                if let Some(head) = self.heads[index]
                    .as_ref()
                    .filter(|head| head.key == member.key)
                {
                    if head.event_ref != member.event_ref {
                        return Err("Máscara de restauração diverge do registro original.".into());
                    }
                    restored = true;
                }
            }
            if !restored {
                return Ok(Some(member));
            }
        }
        Ok(None)
    }
}
pub fn visibility(
    dir: &Path,
    expected: &Identity,
    budget: &Budget,
    work: &Work<'_>,
) -> Result<VerifiedView, String> {
    ensure_ledger(dir)?;
    let metadata = crate::analysis_context::visibility_read_at(dir, expected, |conn| {
        check_view_budget(
            active_payloads(conn, &expected.analysis_id)?,
            active_dictionary_bytes(conn, &expected.analysis_id)?,
            budget,
        )?;
        let mut stmt = conn
            .prepare(
                "SELECT id FROM exclusion_batches WHERE analysis_id=?1 AND active=1 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let ids = stmt
            .query_map([&expected.analysis_id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        ids.iter()
            .map(|id| Ok((info(conn, &expected.analysis_id, id)?, masks(conn, id)?)))
            .collect::<Result<Vec<_>, String>>()
    })?;
    let mut batches = Vec::new();
    for (batch, masks) in metadata {
        work.check()?;
        let original = Payload::open(db_path(dir, &batch.id)?, &batch.payload, work)?;
        let masks = masks
            .iter()
            .map(|meta| Payload::open(db_path(dir, &meta.id)?, meta, work))
            .collect::<Result<_, _>>()?;
        batches.push(ActiveBatch { original, masks });
    }
    Ok(VerifiedView {
        identity: expected.clone(),
        scope: Scope::ActiveUnion,
        batches,
        buffer_bytes: budget.stream_buffer_bytes,
    })
}

/// Explicit currently-excluded-in-one-batch scope. Original history is served
/// by archive_page; this scope never creates an implicit unfiltered bypass.
pub fn batch_visibility(
    dir: &Path,
    expected: &Identity,
    batch_id: &str,
    budget: &Budget,
    work: &Work<'_>,
) -> Result<VerifiedView, String> {
    ensure_ledger(dir)?;
    let (batch, masks) = crate::analysis_context::visibility_read_at(dir, expected, |conn| {
        Ok((
            info(conn, &expected.analysis_id, batch_id)?,
            masks(conn, batch_id)?,
        ))
    })?;
    let mut batches = Vec::new();
    if batch.active {
        let dictionaries = masks
            .iter()
            .try_fold(batch.payload.dictionary_bytes, |n, m| {
                n.checked_add(m.dictionary_bytes)
                    .ok_or("Dicionários excessivos.")
            })?;
        check_view_budget(1 + masks.len(), dictionaries, budget)?;
        let original = Payload::open(db_path(dir, batch_id)?, &batch.payload, work)?;
        let masks = masks
            .iter()
            .map(|meta| Payload::open(db_path(dir, &meta.id)?, meta, work))
            .collect::<Result<_, _>>()?;
        batches.push(ActiveBatch { original, masks });
    }
    Ok(VerifiedView {
        identity: expected.clone(),
        scope: Scope::ExcludedInBatch(batch_id.into()),
        batches,
        buffer_bytes: budget.stream_buffer_bytes,
    })
}

/// Archive pages contain stable references and provenance, never fabricated log
/// contents. The command hydrates matching original records lazily or reports
/// that this source generation is unavailable.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveRow {
    pub key: Key,
    pub event_ref: String,
    pub restored_from_batch: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePage {
    pub analysis: Identity,
    pub batch: BatchInfo,
    pub rows: Vec<ArchiveRow>,
    pub active_members: Option<u64>,
    pub sources: BTreeMap<String, SourceDescriptor>,
    pub next_cursor: Option<Key>,
}
pub fn archive_page(
    dir: &Path,
    expected: &Identity,
    batch_id: &str,
    after: Option<&Key>,
    limit: usize,
    work: &Work<'_>,
) -> Result<ArchivePage, String> {
    ensure_ledger(dir)?;
    let (batch, mask_meta) = crate::analysis_context::visibility_read_at(dir, expected, |conn| {
        Ok((
            info(conn, &expected.analysis_id, batch_id)?,
            masks(conn, batch_id)?,
        ))
    })?;
    let active_members = if !batch.active {
        Some(0)
    } else if mask_meta.is_empty() {
        Some(batch.members)
    } else {
        None
    };
    let mask_payloads = if batch.active {
        mask_meta
            .iter()
            .map(|meta| Payload::open(db_path(dir, &meta.id)?, meta, work))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let payload = Payload::open(db_path(dir, batch_id)?, &batch.payload, work)?;
    let mut cursor = payload.cursor(64 << 10);
    if let Some(after) = after {
        let sid = payload
            .sources
            .binary_search_by(|source| source.0.cmp(&after.source_key))
            .map_err(|_| "Cursor não pertence à origem deste lote.")?;
        cursor.last_sid = sid as i64;
        cursor.last_locator = after.locator.encode()?;
        // This is a seeked page, not a full integrity-count traversal.
        cursor.check_total = false;
    }
    let mut out = Vec::new();
    let limit = limit.clamp(1, 500);
    while out.len() <= limit {
        match cursor.next(work) {
            Ok(Some(member)) => out.push(member),
            Ok(None) => break,
            Err(error) => return Err(error),
        }
    }
    let next_cursor = if out.len() > limit {
        out.pop();
        out.last().map(|member| member.key.clone())
    } else {
        None
    };
    let source_keys: std::collections::BTreeSet<_> = out
        .iter()
        .map(|member| member.key.source_key.as_str())
        .collect();
    let sources = payload
        .sources
        .iter()
        .filter(|(key, _)| source_keys.contains(key.as_str()))
        .cloned()
        .collect();
    let mut rows = Vec::with_capacity(out.len());
    for member in out {
        work.check()?;
        let mut restored = !batch.active;
        for mask in &mask_payloads {
            work.check()?;
            if mask.contains(&member)? {
                restored = true;
                break;
            }
        }
        rows.push(ArchiveRow {
            key: member.key,
            event_ref: member.event_ref,
            restored_from_batch: restored,
        });
    }
    Ok(ArchivePage {
        analysis: expected.clone(),
        batch,
        rows,
        active_members,
        sources,
        next_cursor,
    })
}

const PORTABLE_METADATA_BYTES: usize = 8 << 20;
const PORTABLE_PAYLOADS: usize = 4096;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PortableBatch {
    pub batch: BatchInfo,
    pub masks: Vec<PayloadMeta>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PortableLedger {
    pub schema_version: u32,
    pub identity: Identity,
    pub batches: Vec<PortableBatch>,
}
pub(crate) struct PortableFile {
    meta: PayloadMeta,
    lease: MembershipLease,
}
impl PortableFile {
    pub(crate) fn meta(&self) -> &PayloadMeta {
        &self.meta
    }
    pub(crate) fn open_reader(&self) -> Result<File, String> {
        self.lease.validate()?;
        let file = File::open(&self.lease.path).map_err(|e| e.to_string())?;
        self.lease.validate()?;
        Ok(file)
    }
}
pub(crate) struct CapturedPortable {
    pub manifest: PortableLedger,
    pub files: Vec<PortableFile>,
}
pub(crate) struct PreparedPortable {
    snapshot: Snapshot,
    ledger: PortableLedger,
    files: Vec<PortableFile>,
    database: PathBuf,
}
impl PreparedPortable {
    pub(crate) fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    /// Preparation-time availability diagnostics may follow reference staging.
    /// Ownership and revision fields remain fixed before the save token exists.
    pub(crate) fn add_import_diagnostic(
        &mut self,
        diagnostic: crate::analysis_context::Diagnostic,
    ) -> Result<(), String> {
        if self.snapshot.migration_diagnostics.contains(&diagnostic) {
            return Ok(());
        }
        self.snapshot.migration_diagnostics.push(diagnostic);
        if let Err(error) = crate::analysis_context::validate_snapshot_size(&self.snapshot) {
            self.snapshot.migration_diagnostics.pop();
            return Err(error);
        }
        Ok(())
    }
}
fn portable_file(dir: &Path, meta: &PayloadMeta, budget: &Budget) -> Result<PortableFile, String> {
    validate_portable_meta(meta, budget)?;
    let path = db_path(dir, &meta.id)?;
    if !std::fs::symlink_metadata(&path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_file()
    {
        return Err("Payload portátil não é um arquivo regular.".into());
    }
    let file = File::open(&path).map_err(|e| e.to_string())?;
    FileExt::lock_shared(&file).map_err(|e| e.to_string())?;
    let stamp = stamp(&file)?;
    if stamp.0 != meta.bytes {
        return Err("Tamanho do payload portátil não confere.".into());
    }
    Ok(PortableFile {
        meta: meta.clone(),
        lease: MembershipLease {
            file: std::sync::Arc::new(file),
            path,
            stamp,
        },
    })
}
fn validate_portable_meta(meta: &PayloadMeta, budget: &Budget) -> Result<(), String> {
    uuid::Uuid::parse_str(&meta.id).map_err(|_| "Identificador de payload portátil inválido.")?;
    if meta.format_version != FORMAT
        || meta.sha256.len() != 64
        || !meta.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        || meta.bytes == 0
        || meta.bytes > budget.max_payload_bytes
        || meta.members == 0
        || meta.members > budget.max_members
        || meta.dictionary_bytes > budget.dictionary_bytes
    {
        return Err("Payload portátil excede os limites ou usa formato inválido.".into());
    }
    Ok(())
}
fn portable_charge(total: &mut usize, bytes: usize) -> Result<(), String> {
    *total = total
        .checked_add(bytes)
        .ok_or("Metadados portáteis excessivos.")?;
    if *total > PORTABLE_METADATA_BYTES {
        return Err("Histórico portátil excede 8 MiB de metadados.".into());
    }
    Ok(())
}
fn portable_body<'a>(
    row: &'a rusqlite::Row<'_>,
    column: usize,
    max: usize,
) -> Result<&'a str, String> {
    let value = row
        .get_ref(column)
        .map_err(|e| e.to_string())?
        .as_str()
        .map_err(|e| e.to_string())?;
    if value.len() > max {
        return Err("Metadados portáteis persistidos excedem o limite.".into());
    }
    Ok(value)
}
/// Captures metadata and immutable file leases within the caller's read snapshot.
/// Payload hashing/streaming is deliberately left outside that transaction.
pub(crate) fn capture_portable(
    conn: &Connection,
    dir: &Path,
    snapshot: &Snapshot,
    budget: &Budget,
) -> Result<CapturedPortable, String> {
    let current: Snapshot = conn
        .query_row(
            "SELECT body FROM case_analysis WHERE case_id=?1",
            [&snapshot.case_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| e.to_string())
        .and_then(|body| serde_json::from_str(&body).map_err(|e| e.to_string()))?;
    if current.identity() != snapshot.identity() {
        return Err("A configuração mudou durante a exportação; tente novamente.".into());
    }
    let mut captured = CapturedPortable {
        manifest: PortableLedger {
            schema_version: FORMAT,
            identity: snapshot.identity(),
            batches: Vec::new(),
        },
        files: Vec::new(),
    };
    let has_tables:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='exclusion_batches')",[],|row|row.get(0)).map_err(|e|e.to_string())?;
    if !has_tables {
        if snapshot.visibility_revision != 0 {
            return Err("Histórico de exclusões obrigatório está ausente.".into());
        }
        return Ok(captured);
    }
    let mut metadata_bytes = encoded(&captured.manifest, PORTABLE_METADATA_BYTES)?.len();
    let mut seen = std::collections::BTreeSet::new();
    let mut stmt = conn
        .prepare("SELECT id,body,active FROM exclusion_batches WHERE analysis_id=?1 ORDER BY id")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query([&snapshot.analysis_id])
        .map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let body = portable_body(row, 1, MAX_RECEIPT_BYTES * 3)?;
        portable_charge(&mut metadata_bytes, body.len() + 32)?;
        let batch: BatchInfo = serde_json::from_str(body).map_err(|e| e.to_string())?;
        let id = portable_body(row, 0, 64)?;
        let active: bool = row.get(2).map_err(|e| e.to_string())?;
        if batch.id != id
            || batch.payload.id != id
            || batch.analysis_id != snapshot.analysis_id
            || batch.active != active
            || batch.members != batch.payload.members
        {
            return Err("Histórico portátil tem identidade inconsistente.".into());
        }
        let mut group = PortableBatch {
            batch,
            masks: Vec::new(),
        };
        if captured.files.len() >= PORTABLE_PAYLOADS || !seen.insert(group.batch.payload.id.clone())
        {
            return Err("Histórico portátil excede o limite ou repete payloads.".into());
        }
        captured
            .files
            .push(portable_file(dir, &group.batch.payload, budget)?);
        let mut masks = conn
            .prepare("SELECT id,payload FROM exclusion_masks WHERE batch_id=?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        let mut mask_rows = masks.query([id]).map_err(|e| e.to_string())?;
        while let Some(row) = mask_rows.next().map_err(|e| e.to_string())? {
            let body = portable_body(row, 1, 4096)?;
            portable_charge(&mut metadata_bytes, body.len() + 1)?;
            let meta: PayloadMeta = serde_json::from_str(body).map_err(|e| e.to_string())?;
            if meta.id != portable_body(row, 0, 64)?
                || captured.files.len() >= PORTABLE_PAYLOADS
                || !seen.insert(meta.id.clone())
            {
                return Err("Histórico portátil excede o limite ou repete payloads.".into());
            }
            captured.files.push(portable_file(dir, &meta, budget)?);
            group.masks.push(meta);
        }
        captured.manifest.batches.push(group);
    }
    if snapshot.visibility_revision == 0 && !captured.manifest.batches.is_empty()
        || snapshot.visibility_revision != 0 && captured.manifest.batches.is_empty()
    {
        return Err("Revisão de visibilidade não corresponde ao histórico portátil.".into());
    }
    Ok(captured)
}
fn metadata_size(value: &impl Serialize) -> Result<usize, String> {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > PORTABLE_METADATA_BYTES.saturating_sub(self.0) {
                return Err(std::io::Error::other("Metadados portáteis excedem 8 MiB."));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).map_err(|e| e.to_string())?;
    Ok(count.0)
}
fn rebuild_portable_payload(
    dir: &Path,
    payload: &Payload,
    admission: Admission,
    purpose: Purpose,
    budget: &Budget,
    work: &Work<'_>,
) -> Result<Staged, String> {
    let sources: Vec<_> = payload
        .sources
        .iter()
        .map(|(_, source)| source.clone())
        .collect();
    let mut cursor = payload.cursor(
        budget
            .stream_buffer_bytes
            .saturating_sub(MAX_MEMBER_BYTES)
            .max(1),
    );
    let mut ended = false;
    let members = std::iter::from_fn(|| {
        if ended {
            return None;
        }
        match cursor.next(work) {
            Ok(Some(member)) => Some(
                payload
                    .sources
                    .binary_search_by(|(key, _)| key.cmp(&member.key.source_key))
                    .map_err(|_| "Fonte portátil ausente.".into())
                    .map(|source_index| InputMember {
                        source_index,
                        locator: member.key.locator,
                        event_ref: member.event_ref,
                    }),
            ),
            Ok(None) => {
                ended = true;
                None
            }
            Err(error) => {
                ended = true;
                Some(Err(error))
            }
        }
    });
    stage_impl(
        dir,
        admission,
        purpose,
        &sources,
        |emit| {
            for member in members {
                emit(member?)?;
            }
            Ok(())
        },
        budget,
        work,
        false,
    )
}
fn finish_portable_payload(
    dir: &Path,
    staged: Staged,
    budget: &Budget,
) -> Result<PortableFile, String> {
    let target = db_path(dir, &staged.payload.id)?;
    if target.exists() {
        return Err("Identidade local do payload já existe.".into());
    }
    std::fs::rename(staged.directory.path().join("payload.sqlite3"), &target)
        .map_err(|e| e.to_string())?;
    sync_dir(&root(dir))?;
    portable_file(dir, &staged.payload, budget)
}
/// Rebuilds untrusted membership into new application-owned immutable payloads.
/// All large I/O is complete before a PreparedPortable reaches the save token.
pub(crate) fn prepare_portable(
    dir: &Path,
    foreign: &Snapshot,
    ledger: &PortableLedger,
    local_case_id: &str,
    payloads: &BTreeMap<String, PathBuf>,
    budget: &Budget,
    work: &Work<'_>,
) -> Result<PreparedPortable, String> {
    work.check()?;
    metadata_size(ledger)?;
    if ledger.schema_version != FORMAT
        || ledger.identity != foreign.identity()
        || ledger.batches.len() > PORTABLE_PAYLOADS
        || (foreign.visibility_revision == 0) != ledger.batches.is_empty()
    {
        return Err("Identidade ou histórico portátil incompatível.".into());
    }
    let mut snapshot = crate::analysis_context::prepare_portable_snapshot(local_case_id, foreign)?;
    snapshot.config_revision = 0;
    snapshot.visibility_revision = u64::from(!ledger.batches.is_empty());
    let mut expected = std::collections::BTreeSet::new();
    let mut total_bytes = 0u64;
    let mut active = 0usize;
    let mut dictionaries = 0u64;
    for group in &ledger.batches {
        let batch = &group.batch;
        if batch.id != batch.payload.id
            || batch.analysis_id != foreign.analysis_id
            || batch.members != batch.payload.members
            || batch.label.len() > 4096
            || batch.reason.len() > 8192
            || batch.active == batch.restored_at_ms.is_some()
        {
            return Err("Lote portátil inválido.".into());
        }
        encoded(&batch.scope, MAX_RECEIPT_BYTES)?;
        encoded(&batch.source_receipt, MAX_RECEIPT_BYTES)?;
        for meta in std::iter::once(&batch.payload).chain(&group.masks) {
            validate_portable_meta(meta, budget)?;
            if !expected.insert(meta.id.clone()) || expected.len() > PORTABLE_PAYLOADS {
                return Err("Payload portátil repetido ou excessivo.".into());
            }
            total_bytes = total_bytes
                .checked_add(meta.bytes)
                .ok_or("Arquivo portátil excessivo.")?;
            if total_bytes > 32u64 << 30 {
                return Err("Payloads portáteis excedem 32 GiB.".into());
            }
            if batch.active {
                active += 1;
                dictionaries = dictionaries
                    .checked_add(meta.dictionary_bytes)
                    .ok_or("Dicionários excessivos.")?;
            }
        }
    }
    check_view_budget(active, dictionaries, budget)?;
    if expected.len() != payloads.len() || expected.iter().any(|id| !payloads.contains_key(id)) {
        return Err("Conjunto de payloads portáteis não corresponde ao manifesto.".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let database = dir
        .canonicalize()
        .map_err(|e| e.to_string())?
        .join("investigations.sqlite3");
    let mut prepared = PreparedPortable {
        snapshot,
        ledger: PortableLedger {
            schema_version: FORMAT,
            identity: ledger.identity.clone(),
            batches: Vec::new(),
        },
        files: Vec::new(),
        database,
    };
    prepared.ledger.identity = prepared.snapshot.identity();
    for group in &ledger.batches {
        work.check()?;
        let admission = Admission {
            analysis: prepared.snapshot.identity(),
            source_receipt: serde_json::json!({"portableOrigin":ledger.identity,"originalBatchId":group.batch.id}),
        };
        encoded(&admission.source_receipt, MAX_RECEIPT_BYTES)?;
        let original = Payload::open(
            payloads[&group.batch.payload.id].clone(),
            &group.batch.payload,
            work,
        )?;
        let staged = rebuild_portable_payload(
            dir,
            &original,
            admission.clone(),
            Purpose::Exclude,
            budget,
            work,
        )?;
        let fresh_id = staged.payload.id.clone();
        let fresh_file = finish_portable_payload(dir, staged, budget)?;
        let mut batch = group.batch.clone();
        batch.id = fresh_id.clone();
        batch.analysis_id = prepared.snapshot.analysis_id.clone();
        batch.payload = fresh_file.meta.clone();
        prepared.files.push(fresh_file);
        let mut masks = Vec::new();
        for meta in &group.masks {
            work.check()?;
            let mask = Payload::open(payloads[&meta.id].clone(), meta, work)?;
            verify_subset(&original, &mask, work, budget.stream_buffer_bytes)?;
            let staged = rebuild_portable_payload(
                dir,
                &mask,
                admission.clone(),
                Purpose::RestoreSelection {
                    batch_id: fresh_id.clone(),
                },
                budget,
                work,
            )?;
            let file = finish_portable_payload(dir, staged, budget)?;
            masks.push(file.meta.clone());
            prepared.files.push(file);
        }
        prepared.ledger.batches.push(PortableBatch { batch, masks });
    }
    work.check()?;
    // Foreign JSON dictionaries may normalize to a larger canonical encoding.
    // Enforce the actual rebuilt footprint, not only the archive's declared one.
    let mut rebuilt_handles = 0usize;
    let mut rebuilt_dictionaries = 0u64;
    for group in prepared
        .ledger
        .batches
        .iter()
        .filter(|group| group.batch.active)
    {
        for meta in std::iter::once(&group.batch.payload).chain(&group.masks) {
            rebuilt_handles = rebuilt_handles
                .checked_add(1)
                .ok_or("Payloads excessivos.")?;
            rebuilt_dictionaries = rebuilt_dictionaries
                .checked_add(meta.dictionary_bytes)
                .ok_or("Dicionários excessivos.")?;
        }
    }
    check_view_budget(rebuilt_handles, rebuilt_dictionaries, budget)?;
    // Validate conflicts across active overlapping batches before publication.
    // Restored history remains preserved, and cannot affect the visible union.
    let mut active_batches = Vec::new();
    for group in prepared
        .ledger
        .batches
        .iter()
        .filter(|group| group.batch.active)
    {
        let original = Payload::open(db_path(dir, &group.batch.id)?, &group.batch.payload, work)?;
        let masks = group
            .masks
            .iter()
            .map(|meta| Payload::open(db_path(dir, &meta.id)?, meta, work))
            .collect::<Result<Vec<_>, _>>()?;
        active_batches.push(ActiveBatch { original, masks });
    }
    VerifiedView {
        identity: prepared.snapshot.identity(),
        scope: Scope::ActiveUnion,
        batches: active_batches,
        buffer_bytes: budget.stream_buffer_bytes,
    }
    .visit(work, |_| Ok(true))?;
    for file in &prepared.files {
        file.lease.validate()?;
    }
    Ok(prepared)
}
pub(crate) fn insert_prepared(
    tx: &rusqlite::Transaction<'_>,
    prepared: &PreparedPortable,
) -> Result<Snapshot, String> {
    let database: String = tx
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if Path::new(&database)
        .canonicalize()
        .map_err(|e| e.to_string())?
        != prepared.database
    {
        return Err("Importação preparada pertence a outro armazenamento.".into());
    }
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM case_analysis WHERE case_id=?1)",
            [&prepared.snapshot.case_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists {
        return crate::analysis_context::insert_portable_snapshot(tx, &prepared.snapshot);
    }
    for file in &prepared.files {
        file.lease.validate()?;
    }
    ledger_schema(tx)?;
    let snapshot = crate::analysis_context::insert_portable_snapshot(tx, &prepared.snapshot)?;
    for group in &prepared.ledger.batches {
        tx.execute(
            "INSERT INTO exclusion_batches VALUES(?1,?2,?3,?4)",
            params![
                group.batch.id,
                snapshot.analysis_id,
                encoded(&group.batch, MAX_RECEIPT_BYTES * 3)?,
                group.batch.active
            ],
        )
        .map_err(|e| e.to_string())?;
        for mask in &group.masks {
            tx.execute(
                "INSERT INTO exclusion_masks VALUES(?1,?2,?3)",
                params![mask.id, group.batch.id, encoded(mask, 4096)?],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{cell::Cell, fs::OpenOptions};
    fn work() -> Work<'static> {
        Work {
            cancelled: &|| false,
            progress: &|_, _, _| {},
        }
    }
    fn source() -> SourceDescriptor {
        SourceDescriptor {
            version: "version-one".into(),
            record_space: "metadata-one".into(),
            label: "fixture.log".into(),
            event_ref_prefix: Some("version-one".into()),
        }
    }
    fn input(offset: u64) -> Result<InputMember, String> {
        Ok(InputMember {
            source_index: 0,
            locator: Locator::ByteOffset(offset),
            event_ref: format!("version-one:{offset}"),
        })
    }
    struct Fixture {
        dir: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            crate::case_store::save_at(dir.path(), json!({"cases":[{"id":"a"},{"id":"b"}]}))
                .unwrap();
            Self { dir }
        }
        fn path(&self) -> &Path {
            self.dir.path()
        }
        fn admission(&self) -> Admission {
            admission(self.path(), "a")
        }
        fn stage(&self, purpose: Purpose, offsets: &[u64]) -> Staged {
            stage(
                self.path(),
                self.admission(),
                purpose,
                &[source()],
                offsets.iter().copied().map(input),
                &Budget::default(),
                &work(),
            )
            .unwrap()
        }
        fn exclude(&self, offsets: &[u64]) -> Receipt {
            publish(
                self.path(),
                self.stage(Purpose::Exclude, offsets),
                "Lote",
                "reason",
                json!({"kind":"selection"}),
                &Budget::default(),
                &work(),
            )
            .unwrap()
        }
        fn members(&self) -> Vec<Member> {
            let view = visibility(
                self.path(),
                &self.admission().analysis,
                &Budget::default(),
                &work(),
            )
            .unwrap();
            let mut rows = Vec::new();
            view.visit(&work(), |member| {
                rows.push(member.clone());
                Ok(true)
            })
            .unwrap();
            rows
        }
        fn offsets(&self) -> Vec<u64> {
            self.members()
                .into_iter()
                .map(|m| match m.key.locator {
                    Locator::ByteOffset(value) => value,
                    _ => panic!("unexpected locator"),
                })
                .collect()
        }
    }
    fn admission(dir: &Path, id: &str) -> Admission {
        let loaded = crate::case_store::load_at(dir).unwrap();
        let case = loaded["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .unwrap();
        let snapshot: Snapshot = serde_json::from_value(case["analysisContext"].clone()).unwrap();
        Admission {
            analysis: snapshot.identity(),
            source_receipt: json!({"generation":7,"sources":["version-one"]}),
        }
    }
    #[test]
    fn staged_payload_is_not_visible_until_atomic_publication() {
        let f = Fixture::new();
        let staged = f.stage(Purpose::Exclude, &[5, 1, 1, 3]);
        assert_eq!(staged.members(), 3);
        assert!(f.offsets().is_empty());
        let receipt = publish(
            f.path(),
            staged,
            "test",
            "reason",
            json!({}),
            &Budget::default(),
            &work(),
        )
        .unwrap();
        assert_eq!(receipt.analysis_context.visibility_revision, 1);
        assert_eq!(receipt.analysis_context.config_revision, 0);
        assert_eq!(receipt.selected_members, 3);
        assert_eq!(receipt.newly_visible, None);
        assert_eq!(f.offsets(), vec![1, 3, 5]);
        let conn = Connection::open(db_path(f.path(), &receipt.batch_id).unwrap()).unwrap();
        assert_eq!(
            conn.query_row::<usize, _, _>(
                "SELECT count(*) FROM members WHERE event_ref IS NULL",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            3
        );
    }
    #[test]
    fn partial_restore_preserves_overlapping_batches_and_whole_restore_is_small() {
        let f = Fixture::new();
        let a = f.exclude(&[1, 2, 3]);
        let b = f.exclude(&[3, 4]);
        let mask = f.stage(
            Purpose::RestoreSelection {
                batch_id: a.batch_id.clone(),
            },
            &[2, 3],
        );
        publish(
            f.path(),
            mask,
            "",
            "",
            json!({}),
            &Budget::default(),
            &work(),
        )
        .unwrap();
        assert_eq!(f.offsets(), vec![1, 3, 4]);
        restore_batch(f.path(), &f.admission(), &b.batch_id, &work()).unwrap();
        assert_eq!(f.offsets(), vec![1]);
        restore_batch(f.path(), &f.admission(), &a.batch_id, &work()).unwrap();
        assert!(f.offsets().is_empty());
        let batches = list(f.path(), &f.admission().analysis, None, 100).unwrap();
        assert_eq!(batches.len(), 2);
        assert!(batches
            .iter()
            .all(|b| !b.active && b.restored_at_ms.is_some()));
    }
    #[test]
    fn repeated_partial_masks_do_not_count_as_multiple_memberships() {
        let f = Fixture::new();
        let a = f.exclude(&[1, 2, 3]);
        for selected in [&[1, 2][..], &[2][..]] {
            let mask = f.stage(
                Purpose::RestoreSelection {
                    batch_id: a.batch_id.clone(),
                },
                selected,
            );
            publish(
                f.path(),
                mask,
                "",
                "",
                json!({}),
                &Budget::default(),
                &work(),
            )
            .unwrap();
        }
        assert_eq!(f.offsets(), vec![3]);
    }
    #[test]
    fn masks_must_be_a_verified_subset_of_the_original_batch() {
        let f = Fixture::new();
        let a = f.exclude(&[1, 2]);
        let before = f.admission();
        let result = stage(
            f.path(),
            before.clone(),
            Purpose::RestoreSelection {
                batch_id: a.batch_id.clone(),
            },
            &[source()],
            [input(99)],
            &Budget::default(),
            &work(),
        );
        assert!(result.err().unwrap().contains("não pertencem"));
        let wrong = Ok(InputMember {
            source_index: 0,
            locator: Locator::ByteOffset(1),
            event_ref: "wrong-reference".into(),
        });
        assert!(stage(
            f.path(),
            before.clone(),
            Purpose::RestoreSelection {
                batch_id: a.batch_id
            },
            &[source()],
            [wrong],
            &Budget::default(),
            &work()
        )
        .is_err());
        assert_eq!(f.admission(), before);
        assert_eq!(f.offsets(), vec![1, 2]);
    }
    #[test]
    fn stale_visibility_publish_leaves_an_owned_orphan_without_half_publication() {
        let f = Fixture::new();
        let stale = f.stage(Purpose::Exclude, &[9]);
        let orphan = stale.payload.id.clone();
        f.exclude(&[1]);
        let before = f.admission();
        assert!(publish(
            f.path(),
            stale,
            "",
            "",
            json!({}),
            &Budget::default(),
            &work()
        )
        .is_err());
        assert_eq!(f.admission(), before);
        assert_eq!(f.offsets(), vec![1]);
        let path = db_path(f.path(), &orphan).unwrap();
        assert!(path.exists());
        let conn = Connection::open(path).unwrap();
        let captured: String = conn
            .query_row("SELECT value FROM meta WHERE key='admission'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Admission>(&captured)
                .unwrap()
                .analysis
                .case_id,
            "a"
        );
    }
    #[test]
    fn cancellation_during_staging_discards_pending_payload_without_visibility_change() {
        let f = Fixture::new();
        let cancelled = Cell::new(false);
        let before = f.admission();
        let records = (0..20).map(|offset| {
            if offset == 3 {
                cancelled.set(true);
            }
            input(offset)
        });
        let work = Work {
            cancelled: &|| cancelled.get(),
            progress: &|_, _, _| {},
        };
        assert!(stage(
            f.path(),
            before.clone(),
            Purpose::Exclude,
            &[source()],
            records,
            &Budget::default(),
            &work
        )
        .is_err());
        assert_eq!(f.admission(), before);
        assert!(f.offsets().is_empty());
        assert_eq!(std::fs::read_dir(root(f.path())).unwrap().count(), 0);
    }
    #[test]
    fn cancellation_before_commit_rolls_back_but_late_cancel_keeps_success() {
        let f = Fixture::new();
        let staged = f.stage(Purpose::Exclude, &[1]);
        let calls = Cell::new(0);
        let work_cancel = Work {
            cancelled: &|| {
                let value = calls.get() + 1;
                calls.set(value);
                value >= 3
            },
            progress: &|_, _, _| {},
        };
        assert!(publish(
            f.path(),
            staged,
            "",
            "",
            json!({}),
            &Budget::default(),
            &work_cancel
        )
        .is_err());
        assert_eq!(f.admission().analysis.visibility_revision, 0);
        assert!(f.offsets().is_empty());
        let staged = f.stage(Purpose::Exclude, &[2]);
        let cancelled = Cell::new(false);
        let late = Work {
            cancelled: &|| cancelled.get(),
            progress: &|phase, _, _| {
                if phase == "published" {
                    cancelled.set(true);
                }
            },
        };
        let receipt = publish(
            f.path(),
            staged,
            "",
            "",
            json!({}),
            &Budget::default(),
            &late,
        )
        .unwrap();
        assert!(cancelled.get());
        assert_eq!(receipt.analysis_context.visibility_revision, 1);
        assert_eq!(f.offsets(), vec![2]);
    }
    #[test]
    fn corrupt_required_payload_fails_closed_and_metadata_only_restore_can_recover() {
        use std::io::{Seek, SeekFrom, Write};
        let f = Fixture::new();
        let batch = f.exclude(&[1, 2]);
        let path = db_path(f.path(), &batch.batch_id).unwrap();
        let mut file = OpenOptions::new().write(true).open(path).unwrap();
        file.seek(SeekFrom::Start(200)).unwrap();
        file.write_all(b"corrupted").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(visibility(
            f.path(),
            &f.admission().analysis,
            &Budget::default(),
            &work()
        )
        .is_err());
        restore_batch(f.path(), &f.admission(), &batch.batch_id, &work()).unwrap();
        assert!(f.offsets().is_empty());
    }
    #[test]
    fn payload_or_member_budget_failure_is_explicit_and_atomic() {
        let f = Fixture::new();
        let mut budget = Budget {
            max_members: 2,
            ..Budget::default()
        };
        assert!(stage(
            f.path(),
            f.admission(),
            Purpose::Exclude,
            &[source()],
            (0..3).map(input),
            &budget,
            &work()
        )
        .is_err());
        budget.max_members = 100;
        budget.max_payload_bytes = 4096;
        assert!(stage(
            f.path(),
            f.admission(),
            Purpose::Exclude,
            &[source()],
            [input(1)],
            &budget,
            &work()
        )
        .is_err());
        assert!(f.offsets().is_empty());
        assert_eq!(f.admission().analysis.visibility_revision, 0);
    }
    #[test]
    fn active_payload_limit_is_enforced_before_visibility_can_exceed_it() {
        let f = Fixture::new();
        f.exclude(&[1]);
        let budget = Budget {
            max_open_payloads: 1,
            ..Budget::default()
        };
        let staged = f.stage(Purpose::Exclude, &[2]);
        assert!(publish(f.path(), staged, "", "", json!({}), &budget, &work()).is_err());
        assert_eq!(f.offsets(), vec![1]);
        assert_eq!(f.admission().analysis.visibility_revision, 1);
    }
    #[test]
    fn archive_keyset_pages_preserve_utf8_and_u64_offsets_without_source_remapping() {
        let f = Fixture::new();
        let batch = f.exclude(&[0, 5, u64::MAX]);
        let identity = f.admission().analysis;
        let first = archive_page(f.path(), &identity, &batch.batch_id, None, 2, &work()).unwrap();
        let second = archive_page(
            f.path(),
            &identity,
            &batch.batch_id,
            Some(&first.rows[1].key),
            2,
            &work(),
        )
        .unwrap();
        assert_eq!(first.rows.len(), 2);
        assert!(first.next_cursor.is_some());
        assert_eq!(
            first.sources.values().next().unwrap().version,
            "version-one"
        );
        assert_eq!(second.rows.len(), 1);
        assert!(second.next_cursor.is_none());
        assert_eq!(second.rows[0].key.locator, Locator::ByteOffset(u64::MAX));
        assert!(archive_page(
            f.path(),
            &identity,
            &batch.batch_id,
            Some(&second.rows[0].key),
            2,
            &work()
        )
        .unwrap()
        .rows
        .is_empty());
        let mut changed = source();
        changed.version = "rotated-version".into();
        let lookup = Member {
            key: Key {
                source_key: changed.key().unwrap(),
                locator: Locator::ByteOffset(5),
            },
            event_ref: "version-one:5".into(),
        };
        let view = visibility(f.path(), &identity, &Budget::default(), &work()).unwrap();
        assert!(!view.contains(&lookup).unwrap());
        assert_eq!(
            view.sources().values().next().unwrap().version,
            "version-one"
        );
        let selected = Ok(InputMember {
            source_index: 0,
            locator: Locator::StableRecord("行:ç".into()),
            event_ref: "保存:α".into(),
        });
        let staged = stage(
            f.path(),
            f.admission(),
            Purpose::Exclude,
            &[source()],
            [selected],
            &Budget::default(),
            &work(),
        )
        .unwrap();
        publish(
            f.path(),
            staged,
            "",
            "",
            json!({}),
            &Budget::default(),
            &work(),
        )
        .unwrap();
        assert!(f
            .members()
            .iter()
            .any(|member| member.event_ref == "保存:α"));
    }
    #[test]
    fn conflicting_reference_at_the_same_locator_is_rejected() {
        let f = Fixture::new();
        let wrong = Ok(InputMember {
            source_index: 0,
            locator: Locator::ByteOffset(1),
            event_ref: "different".into(),
        });
        assert!(stage(
            f.path(),
            f.admission(),
            Purpose::Exclude,
            &[source()],
            [input(1), wrong],
            &Budget::default(),
            &work()
        )
        .is_err());
        assert!(f.offsets().is_empty());
    }
    #[test]
    fn independent_cases_cannot_restore_or_read_each_others_batches() {
        let f = Fixture::new();
        let batch = f.exclude(&[1]);
        let other = admission(f.path(), "b");
        assert!(list(f.path(), &other.analysis, None, 100)
            .unwrap()
            .is_empty());
        assert!(restore_batch(f.path(), &other, &batch.batch_id, &work()).is_err());
        assert!(archive_page(
            f.path(),
            &other.analysis,
            &batch.batch_id,
            None,
            10,
            &work()
        )
        .is_err());
        assert_eq!(f.offsets(), vec![1]);
    }
    #[test]
    fn ordered_stream_can_stop_early_with_bounded_lookahead() {
        let f = Fixture::new();
        let offsets: Vec<_> = (0..500).rev().collect();
        f.exclude(&offsets);
        let view = visibility(
            f.path(),
            &f.admission().analysis,
            &Budget::default(),
            &work(),
        )
        .unwrap();
        let mut values = Vec::new();
        view.visit(&work(), |member| {
            values.push(member.clone());
            Ok(values.len() < 3)
        })
        .unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values[0].key.locator, Locator::ByteOffset(0));
        let mut cursor = view.batches[0].original.cursor(5000);
        cursor.next(&work()).unwrap();
        assert!(cursor.read <= BATCH_ROWS as u64);
        assert!(cursor.buffer.len() < BATCH_ROWS);
    }
    #[test]
    fn abrupt_process_exit_before_commit_never_publishes_half_a_batch() {
        const ENV: &str = "LOGINSIGHT_EXCLUSION_CRASH_FIXTURE";
        if let Some(dir) = std::env::var_os(ENV) {
            let dir = PathBuf::from(dir);
            let staged = stage(
                &dir,
                admission(&dir, "a"),
                Purpose::Exclude,
                &[source()],
                [input(8)],
                &Budget::default(),
                &work(),
            )
            .unwrap();
            let calls = Cell::new(0);
            let crash = Work {
                cancelled: &|| {
                    let next = calls.get() + 1;
                    calls.set(next);
                    if next == 3 {
                        std::process::exit(77);
                    }
                    false
                },
                progress: &|_, _, _| {},
            };
            let _ = publish(
                &dir,
                staged,
                "crash",
                "",
                json!({}),
                &Budget::default(),
                &crash,
            );
            panic!("crash point was not reached");
        }
        let f = Fixture::new();
        let result=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","exclusion_store::tests::abrupt_process_exit_before_commit_never_publishes_half_a_batch","--nocapture"]).env(ENV,f.path()).output().unwrap();
        assert_eq!(
            result.status.code(),
            Some(77),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(f.admission().analysis.visibility_revision, 0);
        assert!(f.offsets().is_empty());
        assert!(std::fs::read_dir(root(f.path()))
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "sqlite3")));
        f.exclude(&[9]);
        assert_eq!(f.offsets(), vec![9]);
    }
    #[test]
    fn aggregate_dictionary_and_buffer_budgets_reject_publication_atomically() {
        let f = Fixture::new();
        f.exclude(&[1]);
        let staged = f.stage(Purpose::Exclude, &[2]);
        let budget = Budget {
            dictionary_bytes: staged.payload.dictionary_bytes,
            ..Budget::default()
        };
        assert!(publish(f.path(), staged, "", "", json!({}), &budget, &work()).is_err());
        assert_eq!(f.offsets(), vec![1]);
        let budget = Budget {
            stream_buffer_bytes: MAX_MEMBER_BYTES,
            ..Budget::default()
        };
        assert!(visibility(f.path(), &f.admission().analysis, &budget, &work()).is_err());
    }
    #[test]
    fn batch_list_is_newest_first_and_member_seek_needs_no_temporary_sort() {
        let f = Fixture::new();
        let a = f.exclude(&[1]);
        let b = f.exclude(&[2]);
        let expected = f.admission().analysis;
        let first = list(f.path(), &expected, None, 1).unwrap();
        assert_eq!(first[0].id, b.batch_id);
        let next = list(f.path(), &expected, Some(&b.batch_id), 1).unwrap();
        assert_eq!(next[0].id, a.batch_id);
        let view = visibility(f.path(), &expected, &Budget::default(), &work()).unwrap();
        let mut stmt=view.batches[0].original.conn.prepare("EXPLAIN QUERY PLAN SELECT source_id,locator,event_ref FROM members WHERE (source_id,locator)>(?1,?2) ORDER BY source_id,locator LIMIT ?3").unwrap();
        let plans = stmt
            .query_map(params![-1, Vec::<u8>::new(), 64], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plans.iter().any(|plan| plan.contains("PRIMARY KEY")),
            "{plans:?}"
        );
        assert!(
            !plans.iter().any(|plan| plan.contains("TEMP B-TREE")),
            "{plans:?}"
        );
    }
    #[test]
    fn archive_status_and_selected_batch_scope_do_not_confuse_global_overlap() {
        let f = Fixture::new();
        let a = f.exclude(&[1, 2]);
        f.exclude(&[2]);
        let selected = f.stage(
            Purpose::RestoreSelection {
                batch_id: a.batch_id.clone(),
            },
            &[2],
        );
        publish(
            f.path(),
            selected,
            "",
            "",
            json!({}),
            &Budget::default(),
            &work(),
        )
        .unwrap();
        let page = archive_page(
            f.path(),
            &f.admission().analysis,
            &a.batch_id,
            None,
            500,
            &work(),
        )
        .unwrap();
        assert_eq!(page.active_members, None);
        assert!(!page.rows[0].restored_from_batch);
        assert!(page.rows[1].restored_from_batch);
        assert_eq!(f.offsets(), vec![1, 2]);
        let scoped = batch_visibility(
            f.path(),
            &f.admission().analysis,
            &a.batch_id,
            &Budget::default(),
            &work(),
        )
        .unwrap();
        assert_eq!(scoped.scope(), &Scope::ExcludedInBatch(a.batch_id.clone()));
        let mut rows = Vec::new();
        scoped
            .visit(&work(), |row| {
                rows.push(row.key.locator.clone());
                Ok(true)
            })
            .unwrap();
        assert_eq!(rows, vec![Locator::ByteOffset(1)]);
        drop(scoped);
        restore_batch(f.path(), &f.admission(), &a.batch_id, &work()).unwrap();
        let page = archive_page(
            f.path(),
            &f.admission().analysis,
            &a.batch_id,
            None,
            500,
            &work(),
        )
        .unwrap();
        assert_eq!(page.active_members, Some(0));
        assert!(page.rows.iter().all(|row| row.restored_from_batch));
    }
    fn captured(f: &Fixture) -> (Snapshot, CapturedPortable, BTreeMap<String, PathBuf>) {
        let conn = crate::case_store::context_connection(f.path()).unwrap();
        conn.execute_batch("BEGIN DEFERRED").unwrap();
        let mut case = json!({"id":"a"});
        crate::analysis_context::attach(&conn, &mut case).unwrap();
        let snapshot: Snapshot = serde_json::from_value(case["analysisContext"].clone()).unwrap();
        let capture = capture_portable(&conn, f.path(), &snapshot, &Budget::default()).unwrap();
        conn.execute_batch("COMMIT").unwrap();
        let paths = capture
            .files
            .iter()
            .map(|file| {
                (
                    file.meta.id.clone(),
                    db_path(f.path(), &file.meta.id).unwrap(),
                )
            })
            .collect();
        (snapshot, capture, paths)
    }
    #[test]
    fn portable_pristine_case_needs_no_ledger_tables_and_keeps_fresh_identity() {
        let f = Fixture::new();
        let (snapshot, capture, paths) = captured(&f);
        assert!(capture.files.is_empty());
        let target = tempfile::tempdir().unwrap();
        let prepared = prepare_portable(
            target.path(),
            &snapshot,
            &capture.manifest,
            "new",
            &paths,
            &Budget::default(),
            &work(),
        )
        .unwrap();
        assert_ne!(prepared.snapshot().analysis_id, snapshot.analysis_id);
        assert_eq!(prepared.snapshot().visibility_revision, 0);
        fn shared<T: Send + Sync>() {}
        shared::<PreparedPortable>();
    }
    #[test]
    fn portable_roundtrip_keeps_overlap_partial_masks_and_inactive_history_atomic() {
        let f = Fixture::new();
        let first = f.exclude(&[1, 2, 3]);
        f.exclude(&[3, 4]);
        let old = f.exclude(&[8]);
        publish(
            f.path(),
            f.stage(
                Purpose::RestoreSelection {
                    batch_id: first.batch_id,
                },
                &[2, 3],
            ),
            "",
            "",
            json!({}),
            &Budget::default(),
            &work(),
        )
        .unwrap();
        restore_batch(f.path(), &f.admission(), &old.batch_id, &work()).unwrap();
        let (mut snapshot, mut capture, paths) = captured(&f);
        snapshot.config_revision = u64::MAX;
        snapshot.visibility_revision = u64::MAX;
        capture.manifest.identity = snapshot.identity();
        let target = tempfile::tempdir().unwrap();
        let prepared = prepare_portable(
            target.path(),
            &snapshot,
            &capture.manifest,
            "new",
            &paths,
            &Budget::default(),
            &work(),
        )
        .unwrap();
        assert_eq!(prepared.snapshot().config_revision, 0);
        assert_eq!(prepared.snapshot().visibility_revision, 1);
        assert!(prepared
            .files
            .iter()
            .all(|file| !paths.contains_key(&file.meta.id)));
        let mut conn = crate::case_store::context_connection(target.path()).unwrap();
        {
            let tx = conn.transaction().unwrap();
            tx.execute("INSERT INTO cases VALUES('new','{}',0)", [])
                .unwrap();
            insert_prepared(&tx, &prepared).unwrap();
            tx.rollback().unwrap();
        }
        assert_eq!(
            conn.query_row::<usize, _, _>(
                "SELECT count(*) FROM case_analysis WHERE case_id='new'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            0
        );
        let tx = conn.transaction().unwrap();
        tx.execute("INSERT INTO cases VALUES('new','{}',0)", [])
            .unwrap();
        let current = insert_prepared(&tx, &prepared).unwrap();
        tx.commit().unwrap();
        let view = visibility(
            target.path(),
            &current.identity(),
            &Budget::default(),
            &work(),
        )
        .unwrap();
        let mut refs = Vec::new();
        view.visit(&work(), |member| {
            refs.push(member.event_ref.clone());
            Ok(true)
        })
        .unwrap();
        assert_eq!(refs, ["version-one:1", "version-one:3", "version-one:4"]);
        let history = list(target.path(), &current.identity(), None, 100).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history.iter().filter(|batch| !batch.active).count(), 1);
        drop(view);
        let restored = restore_batch(
            target.path(),
            &Admission {
                analysis: current.identity(),
                source_receipt: json!({}),
            },
            &history.iter().find(|batch| batch.active).unwrap().id,
            &work(),
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        let retry = insert_prepared(&tx, &prepared).unwrap();
        tx.commit().unwrap();
        assert_eq!(retry.identity(), restored.analysis_context.identity());
    }
    #[test]
    fn portable_rejects_extra_sql_objects_even_with_matching_checksum() {
        let f = Fixture::new();
        f.exclude(&[1]);
        let (snapshot, mut capture, paths) = captured(&f);
        let meta = &mut capture.manifest.batches[0].batch.payload;
        let conn = Connection::open(&paths[&meta.id]).unwrap();
        conn.execute_batch("CREATE VIEW unexpected AS SELECT * FROM members")
            .unwrap();
        drop(conn);
        let (bytes, sha) = hash_file(&paths[&meta.id], &work()).unwrap();
        meta.bytes = bytes;
        meta.sha256 = sha;
        let target = tempfile::tempdir().unwrap();
        let error = prepare_portable(
            target.path(),
            &snapshot,
            &capture.manifest,
            "new",
            &paths,
            &Budget::default(),
            &work(),
        )
        .err()
        .unwrap();
        assert!(error.contains("Esquema"), "{error}");
    }
    #[test]
    fn portable_rejects_cross_case_metadata_missing_payload_and_cancellation() {
        let f = Fixture::new();
        f.exclude(&[1]);
        let (snapshot, mut capture, paths) = captured(&f);
        let target = tempfile::tempdir().unwrap();
        let cancelled = Work {
            cancelled: &|| true,
            progress: &|_, _, _| {},
        };
        assert!(prepare_portable(
            target.path(),
            &snapshot,
            &capture.manifest,
            "new",
            &paths,
            &Budget::default(),
            &cancelled
        )
        .is_err());
        assert!(prepare_portable(
            target.path(),
            &snapshot,
            &capture.manifest,
            "new",
            &BTreeMap::new(),
            &Budget::default(),
            &work()
        )
        .is_err());
        capture.manifest.batches[0].batch.analysis_id = uuid::Uuid::new_v4().to_string();
        assert!(prepare_portable(
            target.path(),
            &snapshot,
            &capture.manifest,
            "new",
            &paths,
            &Budget::default(),
            &work()
        )
        .is_err());
    }
    #[test]
    fn callback_staging_preserves_exact_members_and_latches_swallowed_errors() {
        let f = Fixture::new();
        let staged = stage_with(
            f.path(),
            f.admission(),
            Purpose::Exclude,
            &[source()],
            |emit| {
                for offset in [3, 1, 3, 2] {
                    emit(input(offset)?)?;
                }
                Ok(())
            },
            &Budget::default(),
            &work(),
        )
        .unwrap();
        assert_eq!(staged.members(), 3);
        assert!(f.offsets().is_empty());
        publish(
            f.path(),
            staged,
            "",
            "",
            json!({}),
            &Budget::default(),
            &work(),
        )
        .unwrap();
        assert_eq!(f.offsets(), [1, 2, 3]);
        let before = f.admission();
        let error = stage_with(
            f.path(),
            before.clone(),
            Purpose::Exclude,
            &[source()],
            |emit| {
                emit(input(9)?)?;
                let _ = emit(InputMember {
                    source_index: 99,
                    locator: Locator::ByteOffset(10),
                    event_ref: "invalid".into(),
                });
                let _ = emit(input(11)?);
                Ok(())
            },
            &Budget::default(),
            &work(),
        )
        .err()
        .unwrap();
        assert!(error.contains("fonte desconhecida"));
        assert_eq!(f.admission(), before);
        assert_eq!(f.offsets(), [1, 2, 3]);
        assert!(stage_with(
            f.path(),
            f.admission(),
            Purpose::Exclude,
            &[source()],
            |emit| {
                emit(input(20)?)?;
                Err("producer failed".into())
            },
            &Budget::default(),
            &work()
        )
        .is_err());
    }
    #[test]
    fn portable_rechecks_dictionary_budget_after_normalizing_foreign_json() {
        let f = Fixture::new();
        let mut descriptor = source();
        descriptor.event_ref_prefix = None;
        for offset in [1, 2] {
            let staged = stage(
                f.path(),
                f.admission(),
                Purpose::Exclude,
                &[descriptor.clone()],
                [input(offset)],
                &Budget::default(),
                &work(),
            )
            .unwrap();
            publish(
                f.path(),
                staged,
                "",
                "",
                json!({}),
                &Budget::default(),
                &work(),
            )
            .unwrap();
        }
        let (snapshot, mut capture, paths) = captured(&f);
        let mut declared = 0;
        for group in &mut capture.manifest.batches {
            let meta = &mut group.batch.payload;
            let path = &paths[&meta.id];
            let conn = Connection::open(path).unwrap();
            let body: String = conn
                .query_row("SELECT body FROM sources WHERE id=0", [], |row| row.get(0))
                .unwrap();
            let mut source: Value = serde_json::from_str(&body).unwrap();
            source.as_object_mut().unwrap().remove("eventRefPrefix");
            let body = source.to_string();
            conn.execute("UPDATE sources SET body=?1 WHERE id=0", [&body])
                .unwrap();
            drop(conn);
            meta.dictionary_bytes = body.len() as u64 + 64;
            declared += meta.dictionary_bytes;
            let (bytes, sha) = hash_file(path, &work()).unwrap();
            meta.bytes = bytes;
            meta.sha256 = sha;
        }
        let budget = Budget {
            dictionary_bytes: declared,
            ..Default::default()
        };
        let target = tempfile::tempdir().unwrap();
        let error = prepare_portable(
            target.path(),
            &snapshot,
            &capture.manifest,
            "new",
            &paths,
            &budget,
            &work(),
        )
        .err()
        .unwrap();
        assert!(error.contains("Dicionários"), "{error}");
    }
    #[test]
    fn pending_orphan_process_fixture() {
        const ENV: &str = "LOGINSIGHT_TEST_PENDING_OWNER_EXIT";
        let Some(path) = std::env::var_os(ENV) else {
            return;
        };
        let path = PathBuf::from(path);
        let pending = PendingDirectory::create_at(&path, 0).unwrap();
        std::fs::write(
            pending.path().join("payload.sqlite3"),
            b"unfinished payload",
        )
        .unwrap();
        std::fs::write(
            path.join("child-pending-path"),
            pending.path().to_str().unwrap(),
        )
        .unwrap();
        std::process::exit(77);
    }
    fn crash_pending(dir: &Path) -> PathBuf {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "exclusion_store::tests::pending_orphan_process_fixture",
                "--nocapture",
            ])
            .env("LOGINSIGHT_TEST_PENDING_OWNER_EXIT", dir)
            .output()
            .unwrap();
        assert_eq!(
            status.status.code(),
            Some(77),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        let path = PathBuf::from(std::fs::read_to_string(dir.join("child-pending-path")).unwrap());
        assert!(path.starts_with(root(dir)));
        path
    }
    #[test]
    fn pending_cleanup_recovers_crash_but_keeps_aged_live_preview_and_published_files() {
        let f = Fixture::new();
        let active = PendingDirectory::create_at(f.path(), 0).unwrap();
        std::fs::write(active.path().join("payload.sqlite3"), b"live preview").unwrap();
        let orphan = crash_pending(f.path());
        let published = root(f.path()).join(format!("{}.sqlite3", uuid::Uuid::new_v4()));
        std::fs::write(&published, b"published").unwrap();
        let legacy = root(f.path()).join("pending-legacy");
        std::fs::create_dir(&legacy).unwrap();
        std::fs::write(legacy.join("payload.sqlite3"), b"unknown").unwrap();
        let summary =
            cleanup_pending_at(f.path(), Duration::from_millis(1), 4096, &work(), 1000).unwrap();
        assert_eq!(summary.removed, 1);
        assert!(!orphan.exists());
        assert!(active.path().join("payload.sqlite3").exists());
        assert_eq!(std::fs::read(&published).unwrap(), b"published");
        assert!(legacy.join("payload.sqlite3").exists());
    }
    #[test]
    fn pending_cleanup_skips_wrong_marker_replacement_and_is_cancellable_bounded() {
        let f = Fixture::new();
        let orphan = crash_pending(f.path());
        let stopped = cleanup_pending_at(f.path(), Duration::ZERO, 0, &work(), 1000).unwrap();
        assert!(stopped.scan_limited);
        assert_eq!(stopped.inspected, 0);
        assert!(orphan.exists());
        let cancelled = Work {
            cancelled: &|| true,
            progress: &|_, _, _| {},
        };
        assert!(cleanup_pending_at(f.path(), Duration::ZERO, 4096, &cancelled, 1000).is_err());
        assert!(orphan.exists());
        let owner = root(f.path()).join(format!(
            "{}.owner",
            orphan.file_name().unwrap().to_str().unwrap()
        ));
        let bytes = std::fs::read(&owner).unwrap();
        let mut marker: Value = serde_json::from_slice(&bytes).unwrap();
        marker["kind"] = json!("unknown");
        std::fs::write(&owner, marker.to_string()).unwrap();
        std::fs::write(orphan.join(PENDING_MARKER), marker.to_string()).unwrap();
        assert_eq!(
            cleanup_pending_at(f.path(), Duration::ZERO, 4096, &work(), 1000)
                .unwrap()
                .removed,
            0
        );
        std::fs::write(&owner, &bytes).unwrap();
        std::fs::write(orphan.join(PENDING_MARKER), &bytes).unwrap();
        let original = root(f.path()).join("saved-original");
        std::fs::rename(&orphan, &original).unwrap();
        std::fs::create_dir(&orphan).unwrap();
        std::fs::write(orphan.join(PENDING_MARKER), &bytes).unwrap();
        std::fs::write(orphan.join("payload.sqlite3"), b"replacement").unwrap();
        assert_eq!(
            cleanup_pending_at(f.path(), Duration::ZERO, 4096, &work(), 1000)
                .unwrap()
                .removed,
            0
        );
        assert_eq!(
            std::fs::read(orphan.join("payload.sqlite3")).unwrap(),
            b"replacement"
        );
        assert!(original.join("payload.sqlite3").exists());
    }
    #[cfg(unix)]
    #[test]
    fn pending_cleanup_never_follows_directory_marker_or_payload_symlinks() {
        use std::os::unix::fs::symlink;
        for mode in ["directory", "marker", "payload"] {
            let f = Fixture::new();
            let orphan = crash_pending(f.path());
            let protected = f.path().join("protected");
            std::fs::create_dir(&protected).unwrap();
            std::fs::write(protected.join("keep"), b"original").unwrap();
            match mode {
                "directory" => {
                    std::fs::rename(&orphan, root(f.path()).join("saved-original")).unwrap();
                    symlink(&protected, &orphan).unwrap();
                }
                "marker" => {
                    let owner = root(f.path()).join(format!(
                        "{}.owner",
                        orphan.file_name().unwrap().to_str().unwrap()
                    ));
                    std::fs::rename(&owner, protected.join("marker")).unwrap();
                    symlink(protected.join("marker"), owner).unwrap();
                }
                _ => {
                    std::fs::remove_file(orphan.join("payload.sqlite3")).unwrap();
                    symlink(protected.join("keep"), orphan.join("payload.sqlite3")).unwrap();
                }
            }
            assert_eq!(
                cleanup_pending_at(f.path(), Duration::ZERO, 4096, &work(), 1000)
                    .unwrap()
                    .removed,
                0,
                "{mode}"
            );
            assert_eq!(std::fs::read(protected.join("keep")).unwrap(), b"original");
            assert!(orphan.exists());
        }
    }
    #[test]
    fn retained_pending_sweep_advances_small_passes_and_resumes_after_cancellation() {
        let f = Fixture::new();
        std::fs::create_dir_all(root(f.path())).unwrap();
        let mut published = Vec::new();
        for _ in 0..64 {
            let file = root(f.path()).join(format!("{}.sqlite3", uuid::Uuid::new_v4()));
            std::fs::write(&file, b"published").unwrap();
            published.push(file);
        }
        let orphan = crash_pending(f.path());
        let mut sweep = PendingSweep::new();
        assert!(
            sweep
                .pass_at(f.path(), Duration::ZERO, 0, &work(), 1000)
                .unwrap()
                .scan_limited
        );
        let cancelled = Work {
            cancelled: &|| true,
            progress: &|_, _, _| {},
        };
        assert!(sweep
            .pass_at(f.path(), Duration::ZERO, 2, &cancelled, 1000)
            .is_err());
        let (mut removed, mut inspected, mut complete) = (0, 0, false);
        for _ in 0..100 {
            let pass = sweep
                .pass_at(f.path(), Duration::ZERO, 2, &work(), 1000)
                .unwrap();
            assert!(pass.inspected <= 2);
            removed += pass.removed;
            inspected += pass.inspected;
            if !pass.scan_limited {
                complete = true;
                break;
            }
        }
        assert!(
            complete,
            "a stable prefix must not starve the pending stage"
        );
        assert!(inspected >= 64);
        assert_eq!(removed, 1);
        assert!(!orphan.exists());
        assert!(published
            .iter()
            .all(|file| std::fs::read(file).unwrap() == b"published"));
        fn movable<T: Send>() {}
        movable::<PendingSweep>();
    }
    #[test]
    fn pending_sweep_rebinds_when_the_caller_changes_storage_roots() {
        let first = Fixture::new();
        let old = crash_pending(first.path());
        let second = Fixture::new();
        let new = crash_pending(second.path());
        let mut sweep = PendingSweep::new();
        sweep
            .pass_at(first.path(), Duration::ZERO, 0, &work(), 1000)
            .unwrap();
        assert_eq!(
            sweep
                .pass_at(second.path(), Duration::ZERO, 4096, &work(), 1000)
                .unwrap()
                .removed,
            1
        );
        assert!(old.exists());
        assert!(!new.exists());
    }
    #[cfg(unix)]
    #[test]
    fn pending_sweep_rejects_stale_directory_handle_after_root_replacement() {
        let f = Fixture::new();
        let old = crash_pending(f.path());
        let mut sweep = PendingSweep::new();
        sweep
            .pass_at(f.path(), Duration::ZERO, 0, &work(), 1000)
            .unwrap();
        let old_name = old.file_name().unwrap().to_os_string();
        let saved = f.path().join("saved-previous-root");
        std::fs::rename(root(f.path()), &saved).unwrap();
        let new = crash_pending(f.path());
        assert_eq!(
            sweep
                .pass_at(f.path(), Duration::ZERO, 4096, &work(), 1000)
                .unwrap()
                .removed,
            1
        );
        assert!(!new.exists());
        assert!(saved.join(old_name).join("payload.sqlite3").exists());
    }
    #[test]
    fn portable_import_diagnostics_are_bounded_atomic_and_do_not_change_ownership() {
        let f = Fixture::new();
        let (foreign, capture, paths) = captured(&f);
        let target = tempfile::tempdir().unwrap();
        let mut prepared = prepare_portable(
            target.path(),
            &foreign,
            &capture.manifest,
            "new",
            &paths,
            &Budget::default(),
            &work(),
        )
        .unwrap();
        let original = prepared.snapshot().clone();
        let diagnostic = crate::analysis_context::Diagnostic {
            definition_index: None,
            code: "reference_runtime_budget".into(),
            message: "Arquivo preservado; consulta excede o orçamento disponível.".into(),
        };
        prepared.add_import_diagnostic(diagnostic.clone()).unwrap();
        prepared.add_import_diagnostic(diagnostic.clone()).unwrap();
        assert_eq!(prepared.snapshot().identity(), original.identity());
        assert_eq!(prepared.snapshot().config, original.config);
        assert_eq!(prepared.snapshot().migration_diagnostics, vec![diagnostic]);
        let accepted = prepared.snapshot().clone();
        assert!(prepared
            .add_import_diagnostic(crate::analysis_context::Diagnostic {
                definition_index: None,
                code: "oversized".into(),
                message: "x".repeat(4 << 20)
            })
            .is_err());
        assert_eq!(prepared.snapshot(), &accepted);
    }
}

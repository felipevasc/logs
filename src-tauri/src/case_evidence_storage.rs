//! Immutable length-framed JSON envelopes. Preparing this module's files does
//! not publish a Case or install the older-writer barrier.
use super::*;
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, LazyLock,
    },
};

const MAGIC: &[u8; 8] = b"LIEV0001";
const END_MAGIC: &[u8; 8] = b"LIEVEND1";
const FOOTER_BYTES: u64 = 20;
const HEADER_BYTES: usize = 64 << 10;
const DIRECTORY_ENTRY: u64 = 16;
const COPY_BYTES: usize = 256 << 10;
const INVALID: &str = "CASE_EVIDENCE_CORRUPT: A evidência nativa está ausente, alterada ou possui formato inválido. Os registros não foram substituídos.";
const LIMIT: &str = "CASE_EVIDENCE_LIMIT: A captura excede os limites de preservação; reduza a seleção. Nenhum registro foi publicado.";

const MAX_LIVE_READERS: usize = 32;
const CACHED_READERS: usize = 16;
static LIVE_READERS: AtomicUsize = AtomicUsize::new(0);
struct ReaderCredit;
impl ReaderCredit {
    fn acquire() -> Result<Self, String> {
        crate::operations::check()?;
        LIVE_READERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_LIVE_READERS).then_some(n + 1)
            })
            .map_err(|_| {
                "CASE_EVIDENCE_BUSY: O limite de leitores de evidência está em uso.".to_string()
            })?;
        Ok(Self)
    }
}
impl Drop for ReaderCredit {
    fn drop(&mut self) {
        LIVE_READERS.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Reader {
    file: File,
    _credit: ReaderCredit,
}
impl std::ops::Deref for Reader {
    type Target = File;
    fn deref(&self) -> &File {
        &self.file
    }
}
impl std::ops::DerefMut for Reader {
    fn deref_mut(&mut self) -> &mut File {
        &mut self.file
    }
}
static PAYLOAD_READERS: LazyLock<
    parking_lot::Mutex<VecDeque<(PathBuf, BatchRef, Arc<VerifiedBatch>)>>,
> = LazyLock::new(|| parking_lot::Mutex::new(VecDeque::new()));
/// Restore calls this under root quiescence. In-flight readers remain charged
/// and independently held; no lock is released by cache eviction alone.
pub(crate) fn clear_cached_readers(root: &Path) -> Result<(), String> {
    // Container opens store canonical profile paths (verbatim on Windows).
    // Resolve before locking, and keep sibling/nested profile caches separate.
    let directory = root.canonicalize().map_err(|e| e.to_string())?.join("evidence-v1");
    let retired = {
        let mut readers = PAYLOAD_READERS.lock();
        let mut retired = Vec::new();
        let mut keep = VecDeque::new();
        while let Some(entry) = readers.pop_front() {
            if entry.0.parent() == Some(directory.as_path()) {
                retired.push(entry);
            } else {
                keep.push_back(entry);
            }
        }
        *readers = keep;
        retired
    };
    drop(retired);
    Ok(())
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Header {
    schema_version: u32,
    owner: EvidenceOwner,
    batch_id: String,
    #[serde(deserialize_with = "native_deserialize_value")]
    origin_receipt: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Extent {
    offset: u64,
    bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    bytes: u64,
    modified: std::time::SystemTime,
    file_id: Option<(u64, u64)>,
}
impl Stamp {
    fn of(file: &File) -> Result<Self, String> {
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() {
            return Err(INVALID.into());
        }
        Ok(Self {
            bytes: meta.len(),
            modified: meta.modified().map_err(|e| e.to_string())?,
            file_id: crate::sources::file_identity(file),
        })
    }
}

fn uuid(value: &str) -> Result<(), String> {
    if uuid::Uuid::parse_str(value).is_err() {
        return Err(INVALID.into());
    }
    Ok(())
}
fn owner(value: &EvidenceOwner) -> Result<(), String> {
    uuid(&value.store_id)?;
    uuid(&value.analysis_id)?;
    if value.case_id.is_empty() || value.case_id.len() > 4096 {
        return Err(INVALID.into());
    }
    Ok(())
}
fn regular(path: &Path) -> Result<Reader, String> {
    let credit = ReaderCredit::acquire()?;
    let before = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(INVALID.into());
    }
    let file = crate::case_archive_format::open_regular(path)?;
    let after = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !after.is_file()
        || after.file_type().is_symlink()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(INVALID.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if crate::sources::file_identity(&file) != Some((before.dev(), before.ino()))
            || before.dev() != after.dev()
            || before.ino() != after.ino()
        {
            return Err(INVALID.into());
        }
    }
    Ok(Reader {
        file,
        _credit: credit,
    })
}
fn directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(INVALID.into());
    }
    Ok(())
}
fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

struct Limited<'a, W> {
    inner: &'a mut W,
    bytes: usize,
    max: usize,
}
impl<W: Write> Write for Limited<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.max.saturating_sub(self.bytes) {
            return Err(std::io::Error::other(LIMIT));
        }
        self.inner.write_all(bytes)?;
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
fn encoded(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    serde_json::to_writer(
        &mut Limited {
            inner: &mut bytes,
            bytes: 0,
            max: limit,
        },
        value,
    )
    .map_err(|e| e.to_string())?;
    Ok(bytes)
}
fn digest(file: &mut File, max: u64) -> Result<(String, u64), String> {
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; COPY_BYTES];
    let mut bytes = 0u64;
    loop {
        crate::operations::check()?;
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes = bytes
            .checked_add(n as u64)
            .filter(|n| *n <= max)
            .ok_or(LIMIT)?;
        hash.update(&buffer[..n]);
    }
    Ok((format!("{:x}", hash.finalize()), bytes))
}

/// The caller must finish its source/config/catalog/visibility checks before
/// returning success from the producer; only then does stage_records seal it.
pub(crate) struct RecordSink {
    file: File,
    owner: EvidenceOwner,
    batch_id: String,
    extents: Vec<Extent>,
    bytes: u64,
}
impl RecordSink {
    pub(crate) fn push_event(&mut self, event: &crate::model::Event) -> Result<(), String> {
        crate::operations::check()?;
        fn shape(value: &Value, depth: usize, left: &mut usize) -> bool {
            if depth > 32 || *left == 0 {
                return false;
            }
            *left -= 1;
            match value {
                Value::Array(values) => values.iter().all(|v| shape(v, depth + 1, left)),
                Value::Object(values) => values.values().all(|v| shape(v, depth + 1, left)),
                _ => true,
            }
        }
        let mut left = 16_384;
        if !event.fields.values().all(|v| shape(v, 0, &mut left))
            || !event.derived_originals.values().all(|v| match v {
                crate::model::DerivedOriginal::Missing => true,
                crate::model::DerivedOriginal::Present(v) => shape(v, 0, &mut left),
            })
            || event.derived_diagnostics.len() > 16_384
            || crate::query::event_payload_bytes(event) > EVENT_OWNED_BYTES
        {
            return Err(LIMIT.into());
        }
        let bytes = encoded(event, ENVELOPE_BYTES)?;
        self.push_envelope(std::str::from_utf8(&bytes).map_err(|_| INVALID)?)
    }
    pub(crate) fn push_envelope(&mut self, envelope: &str) -> Result<(), String> {
        crate::operations::check()?;
        if envelope.len() > ENVELOPE_BYTES || self.extents.len() >= CAPTURE_RECORDS {
            return Err(LIMIT.into());
        }
        // Checked borrowed JSON validates syntax without constructing or normalizing
        // its number/object tree. Formatting and unknown members remain bytes.
        let raw = super::raw_json::RawJson::checked(envelope)?;
        if !raw.get().starts_with('{') {
            return Err("CASE_EVIDENCE_RECORD: O envelope deve ser um objeto JSON.".into());
        }
        let after = self
            .bytes
            .checked_add(8)
            .and_then(|n| n.checked_add(envelope.len() as u64))
            .ok_or(LIMIT)?;
        let eventual = after
            .checked_add((self.extents.len() as u64 + 1) * DIRECTORY_ENTRY + FOOTER_BYTES)
            .ok_or(LIMIT)?;
        if eventual > CAPTURE_BYTES {
            return Err(LIMIT.into());
        }
        self.file
            .write_all(&(envelope.len() as u64).to_le_bytes())
            .and_then(|_| self.file.write_all(envelope.as_bytes()))
            .map_err(|e| e.to_string())?;
        self.extents.push(Extent {
            offset: self.bytes + 8,
            bytes: envelope.len() as u64,
        });
        self.bytes = after;
        Ok(())
    }
    pub(super) fn fits_envelope(&self, bytes: usize) -> bool {
        self.extents.len() < CAPTURE_RECORDS
            && bytes <= ENVELOPE_BYTES
            && self
                .bytes
                .checked_add(8)
                .and_then(|n| n.checked_add(bytes as u64))
                .and_then(|n| {
                    n.checked_add((self.extents.len() as u64 + 1) * DIRECTORY_ENTRY + FOOTER_BYTES)
                })
                .is_some_and(|bytes| bytes <= CAPTURE_BYTES)
    }
    pub(crate) fn records(&self) -> usize {
        self.extents.len()
    }
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// A sealed preparation retains its staging owner lease. It contains no SQLite
/// connection and is Send+Sync. Publication remains an explicit later operation.
pub(crate) struct PreparedContainer {
    _owner_lease: Arc<File>,
    directory: tempfile::TempDir,
    root: PathBuf,
    pub(crate) batch: BatchRef,
    pub(crate) reference: EvidenceRef,
    manifest: ContainerManifest,
    manifest_bytes: u64,
}
impl PreparedContainer {
    pub(crate) fn manifest(&self) -> &ContainerManifest {
        &self.manifest
    }
    pub(crate) fn manifest_bytes(&self) -> u64 {
        self.manifest_bytes
    }
    pub(crate) fn batch_path(&self) -> PathBuf {
        self.directory.path().join("records.batch")
    }
    pub(crate) fn manifest_path(&self) -> PathBuf {
        self.directory.path().join("members.json")
    }
    pub(crate) fn open_batch(&self) -> Result<VerifiedBatch, String> {
        VerifiedBatch::open(&self.batch_path(), &self.batch)
    }

    /// Finish disk publication before entering the investigations transaction.
    /// UUID destinations never overwrite an existing payload. Failed SQL may
    /// leave owned unreferenced immutable files, never partially visible rows.
    pub(crate) fn publish_files(&self, root: &Path) -> Result<(), String> {
        self.publish(root, true)
    }
    pub(super) fn publish_batch(&self, root: &Path) -> Result<(), String> {
        self.publish(root, false)
    }
    fn publish(&self, root: &Path, include_manifest: bool) -> Result<(), String> {
        if root.canonicalize().map_err(|e| e.to_string())? != self.root {
            return Err(INVALID.into());
        }
        let directory = root.join("evidence-v1");
        directory_safe(&directory)?;
        for (source, destination, expected, expected_bytes) in [
            (
                self.batch_path(),
                directory.join(format!("{}.batch", self.batch.batch_id)),
                self.batch.sha256.as_str(),
                self.batch.bytes,
            ),
            (
                self.manifest_path(),
                directory.join(format!("{}.manifest", self.reference.manifest_id)),
                self.reference.manifest_sha256.as_str(),
                self.manifest_bytes,
            ),
        ]
        .into_iter()
        .take(if include_manifest { 2 } else { 1 })
        {
            crate::operations::check()?;
            if destination.exists() {
                let mut existing = regular(&destination)?;
                if digest(&mut existing, CAPTURE_BYTES)?.0 != expected {
                    return Err(INVALID.into());
                }
                continue;
            }
            let mut file = regular(&source)?;
            let before = Stamp::of(&file)?;
            if before.bytes != expected_bytes || expected_bytes > CAPTURE_BYTES {
                return Err(INVALID.into());
            }
            let mut temporary =
                tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
            let mut buffer = vec![0; COPY_BYTES];
            let mut hash = Sha256::new();
            let mut copied = 0u64;
            loop {
                crate::operations::check()?;
                let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                copied = copied
                    .checked_add(n as u64)
                    .filter(|n| *n <= expected_bytes)
                    .ok_or(INVALID)?;
                temporary
                    .write_all(&buffer[..n])
                    .map_err(|e| e.to_string())?;
                hash.update(&buffer[..n]);
            }
            if copied != expected_bytes
                || Stamp::of(&file)? != before
                || format!("{:x}", hash.finalize()) != expected
            {
                return Err(INVALID.into());
            }
            temporary.as_file().sync_all().map_err(|e| e.to_string())?;
            temporary
                .persist_noclobber(&destination)
                .map_err(|e| e.error.to_string())?;
        }
        sync_directory(&directory)
    }
}
fn directory_safe(path: &Path) -> Result<(), String> {
    directory(path)
}

pub(crate) fn stage_records(
    root: &Path,
    owner_value: &EvidenceOwner,
    origin_receipt: &Value,
    produce: impl FnOnce(&mut RecordSink) -> Result<(), String>,
) -> Result<PreparedContainer, String> {
    crate::operations::check()?;
    owner(owner_value)?;
    let base = root.join("evidence-v1");
    directory(&base)?;
    let staging = tempfile::Builder::new()
        .prefix("pending-")
        .tempdir_in(&base)
        .map_err(|e| e.to_string())?;
    let mut lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(staging.path().join("owner.lock"))
        .map_err(|e| e.to_string())?;
    lease.try_lock_exclusive().map_err(|e| e.to_string())?;
    lease
        .write_all(b"loginsight-native-evidence-pending-v1\n")
        .and_then(|_| lease.sync_all())
        .map_err(|e| e.to_string())?;
    let batch_id = uuid::Uuid::new_v4().to_string();
    encoded(origin_receipt, HEADER_BYTES / 2)?;
    let header = Header {
        schema_version: SCHEMA_VERSION,
        owner: owner_value.clone(),
        batch_id: batch_id.clone(),
        origin_receipt: origin_receipt.clone(),
    };
    let header = encoded(&header, HEADER_BYTES)?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(staging.path().join("records.batch"))
        .map_err(|e| e.to_string())?;
    file.write_all(MAGIC)
        .and_then(|_| file.write_all(&(header.len() as u32).to_le_bytes()))
        .and_then(|_| file.write_all(&header))
        .map_err(|e| e.to_string())?;
    let mut sink = RecordSink {
        file,
        owner: owner_value.clone(),
        batch_id: batch_id.clone(),
        extents: Vec::new(),
        bytes: 12 + header.len() as u64,
    };
    produce(&mut sink)?;
    crate::operations::check()?;
    let directory_offset = sink.bytes;
    for extent in &sink.extents {
        sink.file
            .write_all(&extent.offset.to_le_bytes())
            .and_then(|_| sink.file.write_all(&extent.bytes.to_le_bytes()))
            .map_err(|e| e.to_string())?;
    }
    sink.file
        .write_all(&(sink.extents.len() as u32).to_le_bytes())
        .and_then(|_| sink.file.write_all(&directory_offset.to_le_bytes()))
        .and_then(|_| sink.file.write_all(END_MAGIC))
        .and_then(|_| sink.file.sync_all())
        .map_err(|e| e.to_string())?;
    let (sha256, bytes) = digest(&mut sink.file, CAPTURE_BYTES)?;
    let batch = BatchRef {
        schema_version: SCHEMA_VERSION,
        owner: sink.owner,
        batch_id: sink.batch_id,
        sha256,
        records: sink.extents.len() as u32,
        bytes,
    };
    let manifest = ContainerManifest {
        schema_version: SCHEMA_VERSION,
        owner: owner_value.clone(),
        container_id: uuid::Uuid::new_v4().to_string(),
        manifest_id: uuid::Uuid::new_v4().to_string(),
        members: (0..batch.records)
            .map(|ordinal| Member {
                occurrence_id: uuid::Uuid::new_v4().to_string(),
                origin_position: ordinal,
                record: RecordLocator {
                    batch_id: batch_id.clone(),
                    ordinal,
                },
            })
            .collect(),
    };
    let manifest_bytes = encoded(&manifest, MANIFEST_BYTES as usize)?;
    let reference = EvidenceRef {
        kind: CommittedKind::NativeEvidence,
        schema_version: SCHEMA_VERSION,
        owner: owner_value.clone(),
        container_id: manifest.container_id.clone(),
        manifest_id: manifest.manifest_id.clone(),
        manifest_sha256: format!("{:x}", Sha256::digest(&manifest_bytes)),
        member_count: batch.records,
    };
    let mut manifest_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging.path().join("members.json"))
        .map_err(|e| e.to_string())?;
    manifest_file
        .write_all(&manifest_bytes)
        .and_then(|_| manifest_file.sync_all())
        .map_err(|e| e.to_string())?;
    sync_directory(staging.path())?;
    drop(sink.file);
    drop(manifest_file);
    let prepared = PreparedContainer {
        directory: staging,
        _owner_lease: Arc::new(lease),
        root: root.canonicalize().map_err(|e| e.to_string())?,
        batch,
        reference,
        manifest,
        manifest_bytes: manifest_bytes.len() as u64,
    };
    // Validate the actual sealed bytes before returning a usable preparation.
    prepared.open_batch()?;
    Ok(prepared)
}

/// Verified payload with at most one record buffer per call. The immutable
/// file lease survives the preparation/view that first opened it.
pub(crate) struct VerifiedBatch {
    file: parking_lot::Mutex<Reader>,
    path: PathBuf,
    stamp: Stamp,
    reference: BatchRef,
    extents: Vec<Extent>,
    origin: Value,
    _origin_credit: crate::case_work_budget::Lease,
}
impl VerifiedBatch {
    pub(crate) fn open(path: &Path, reference: &BatchRef) -> Result<Self, String> {
        crate::operations::check()?;
        owner(&reference.owner)?;
        uuid(&reference.batch_id)?;
        if reference.schema_version != SCHEMA_VERSION
            || reference.records as usize > CAPTURE_RECORDS
            || reference.bytes > CAPTURE_BYTES
            || reference.bytes < 12 + FOOTER_BYTES
        {
            return Err(INVALID.into());
        }
        let mut file = regular(path)?;
        FileExt::try_lock_shared(&file.file).map_err(|_| {
            "CASE_EVIDENCE_BUSY: A evidência está em uso por outra operação; tente novamente."
                .to_string()
        })?;
        let stamp = Stamp::of(&file)?;
        if stamp.bytes != reference.bytes {
            return Err(INVALID.into());
        }
        let (hash, _) = digest(&mut file, CAPTURE_BYTES)?;
        if hash != reference.sha256 {
            return Err(INVALID.into());
        }
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut fixed = [0; 12];
        file.read_exact(&mut fixed).map_err(|e| e.to_string())?;
        if &fixed[..8] != MAGIC {
            return Err(INVALID.into());
        }
        let header_len = u32::from_le_bytes(fixed[8..].try_into().unwrap()) as usize;
        if header_len > HEADER_BYTES || 12 + header_len as u64 > reference.bytes - FOOTER_BYTES {
            return Err(INVALID.into());
        }
        let mut origin_credit = crate::case_cache::reserve_work(
            crate::case_work_budget::global(), header_len.checked_mul(2)
                .and_then(|bytes| bytes.checked_add(1024)).ok_or(LIMIT)?,
        )?;
        let mut header = vec![0; header_len];
        file.read_exact(&mut header).map_err(|e| e.to_string())?;
        let raw = RawJson::checked(std::str::from_utf8(&header).map_err(|_| INVALID)?)?;
        let plan = preflight_value(raw)?;
        origin_credit.merge(crate::case_cache::reserve_work(
            crate::case_work_budget::global(), plan.materialization_credit,
        )?)?;
        let header: Header = serde_json::from_value(materialize_value(raw, plan)?)
            .map_err(|e| e.to_string())?;
        if header.schema_version != SCHEMA_VERSION
            || header.owner != reference.owner
            || header.batch_id != reference.batch_id
        {
            return Err(INVALID.into());
        }
        file.seek(SeekFrom::End(-(FOOTER_BYTES as i64)))
            .map_err(|e| e.to_string())?;
        let mut footer = [0; FOOTER_BYTES as usize];
        file.read_exact(&mut footer).map_err(|e| e.to_string())?;
        let records = u32::from_le_bytes(footer[..4].try_into().unwrap());
        let offset = u64::from_le_bytes(footer[4..12].try_into().unwrap());
        if &footer[12..] != END_MAGIC
            || records != reference.records
            || offset.checked_add(u64::from(records) * DIRECTORY_ENTRY + FOOTER_BYTES)
                != Some(reference.bytes)
        {
            return Err(INVALID.into());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut extents = Vec::with_capacity(records as usize);
        let mut end = 12 + header_len as u64;
        for _ in 0..records {
            crate::operations::check()?;
            let mut entry = [0; 16];
            file.read_exact(&mut entry).map_err(|e| e.to_string())?;
            let extent = Extent {
                offset: u64::from_le_bytes(entry[..8].try_into().unwrap()),
                bytes: u64::from_le_bytes(entry[8..].try_into().unwrap()),
            };
            if extent.offset != end.checked_add(8).ok_or(INVALID)?
                || extent.bytes > ENVELOPE_BYTES as u64
            {
                return Err(INVALID.into());
            }
            end = extent
                .offset
                .checked_add(extent.bytes)
                .filter(|end| *end <= offset)
                .ok_or(INVALID)?;
            extents.push(extent);
        }
        if end != offset || Stamp::of(&file)? != stamp {
            return Err(INVALID.into());
        }
        Ok(Self {
            file: parking_lot::Mutex::new(file),
            path: path.to_path_buf(),
            stamp,
            reference: reference.clone(),
            extents,
            origin: header.origin_receipt,
            _origin_credit: origin_credit,
        })
    }
    pub(crate) fn reference(&self) -> &BatchRef {
        &self.reference
    }
    pub(crate) fn origin_receipt(&self) -> &Value {
        &self.origin
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        crate::operations::check()?;
        if Stamp::of(&*regular(&self.path)?)? != self.stamp
            || Stamp::of(&self.file.lock())? != self.stamp
        {
            return Err(INVALID.into());
        }
        Ok(())
    }
    pub(crate) fn envelope(&self, ordinal: u32) -> Result<String, String> {
        crate::operations::check()?;
        let extent = self.extents.get(ordinal as usize).ok_or(INVALID)?;
        let mut file = self.file.lock();
        if Stamp::of(&file)? != self.stamp {
            return Err(INVALID.into());
        }
        file.seek(SeekFrom::Start(extent.offset - 8))
            .map_err(|e| e.to_string())?;
        let mut size = [0; 8];
        file.read_exact(&mut size).map_err(|e| e.to_string())?;
        if u64::from_le_bytes(size) != extent.bytes {
            return Err(INVALID.into());
        }
        let mut bytes = vec![0; extent.bytes as usize];
        file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        if Stamp::of(&file)? != self.stamp {
            return Err(INVALID.into());
        }
        drop(file);
        let text = String::from_utf8(bytes).map_err(|_| INVALID)?;
        let raw = super::raw_json::RawJson::checked(&text)?;
        if !raw.get().starts_with('{') {
            return Err(INVALID.into());
        }
        Ok(text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Visit {
    Continue,
    Stop,
}

/// Receipt verification is not live-owner admission. The caller must establish
/// the current store/Case/analysis before opening this immutable view. The
/// current implementation never garbage-collects published payloads; readers
/// validate transient manifest handles; payload readers share one bounded process cache.
pub(crate) struct VerifiedContainer {
    root: PathBuf,
    reference: EvidenceRef,
    manifest: ContainerManifest,
    manifest_path: PathBuf,
    manifest_stamp: Stamp,
    batches: BTreeMap<String, BatchRef>,
    known_stamps: parking_lot::Mutex<BTreeMap<String, Stamp>>,
}
impl VerifiedContainer {
    pub(crate) fn open(
        root: &Path,
        reference: &EvidenceRef,
        batches: impl IntoIterator<Item = BatchRef>,
    ) -> Result<Self, String> {
        Self::open_inner(root, reference, batches, None)
    }
    pub(super) fn open_sized(
        root: &Path,
        reference: &EvidenceRef,
        batches: impl IntoIterator<Item = BatchRef>,
        expected_bytes: u64,
    ) -> Result<Self, String> {
        Self::open_inner(root, reference, batches, Some(expected_bytes))
    }
    fn open_inner(
        root: &Path,
        reference: &EvidenceRef,
        batches: impl IntoIterator<Item = BatchRef>,
        expected_bytes: Option<u64>,
    ) -> Result<Self, String> {
        crate::operations::check()?;
        owner(&reference.owner)?;
        uuid(&reference.container_id)?;
        uuid(&reference.manifest_id)?;
        if reference.schema_version != SCHEMA_VERSION
            || reference.member_count as usize > MANIFEST_MEMBERS
        {
            return Err(INVALID.into());
        }
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let path = root
            .join("evidence-v1")
            .join(format!("{}.manifest", reference.manifest_id));
        let mut file = regular(&path)?;
        FileExt::try_lock_shared(&file.file).map_err(|_| {
            "CASE_EVIDENCE_BUSY: A evidência está em uso por outra operação; tente novamente."
                .to_string()
        })?;
        let stamp = Stamp::of(&file)?;
        if expected_bytes.is_some_and(|bytes| bytes != stamp.bytes) {
            return Err(INVALID.into());
        }
        if stamp.bytes > MANIFEST_BYTES {
            return Err(LIMIT.into());
        }
        if digest(&mut file, MANIFEST_BYTES)?.0 != reference.manifest_sha256 {
            return Err(INVALID.into());
        }
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut bytes = vec![0; stamp.bytes as usize];
        file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        let mut trailing = [0; 1];
        if file.read(&mut trailing).map_err(|e| e.to_string())? != 0 {
            return Err(INVALID.into());
        }
        if Stamp::of(&file)? != stamp {
            return Err(INVALID.into());
        }
        let manifest: ContainerManifest =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if manifest.schema_version != SCHEMA_VERSION
            || manifest.owner != reference.owner
            || manifest.container_id != reference.container_id
            || manifest.manifest_id != reference.manifest_id
            || manifest.members.len() != reference.member_count as usize
        {
            return Err(INVALID.into());
        }
        let mut metadata = BTreeMap::new();
        for batch in batches {
            if metadata.len() >= 1024 {
                return Err(LIMIT.into());
            }
            uuid(&batch.batch_id)?;
            if batch.owner != reference.owner
                || batch.schema_version != SCHEMA_VERSION
                || batch.records as usize > CAPTURE_RECORDS
                || batch.bytes > CAPTURE_BYTES
                || metadata.insert(batch.batch_id.clone(), batch).is_some()
            {
                return Err(INVALID.into());
            }
        }
        let mut occurrences = HashSet::new();
        let mut positions = HashSet::new();
        for member in &manifest.members {
            crate::operations::check()?;
            uuid(&member.occurrence_id)?;
            if !occurrences.insert(member.occurrence_id.as_str())
                || member.origin_position as usize >= MANIFEST_MEMBERS
                || !positions.insert(member.origin_position)
                || !metadata
                    .get(&member.record.batch_id)
                    .is_some_and(|batch| member.record.ordinal < batch.records)
            {
                return Err(INVALID.into());
            }
        }
        drop(file);
        Ok(Self {
            root,
            reference: reference.clone(),
            manifest,
            manifest_path: path,
            manifest_stamp: stamp,
            batches: metadata,
            known_stamps: parking_lot::Mutex::new(BTreeMap::new()),
        })
    }
    pub(crate) fn reference(&self) -> &EvidenceRef {
        &self.reference
    }
    pub(crate) fn manifest(&self) -> &ContainerManifest {
        &self.manifest
    }
    pub(crate) fn batch_references(&self) -> impl Iterator<Item = &BatchRef> {
        self.batches.values()
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        crate::operations::check()?;
        if Stamp::of(&*regular(&self.manifest_path)?)? != self.manifest_stamp {
            return Err(INVALID.into());
        }
        let known: Vec<_> = self
            .known_stamps
            .lock()
            .iter()
            .map(|(id, stamp)| (id.clone(), stamp.clone()))
            .collect();
        for (id, stamp) in known {
            let path = self.root.join("evidence-v1").join(format!("{id}.batch"));
            if Stamp::of(&*regular(&path)?)? != stamp {
                return Err(INVALID.into());
            }
        }
        Ok(())
    }
    fn batch(&self, id: &str) -> Result<Arc<VerifiedBatch>, String> {
        let meta = self.batches.get(id).ok_or(INVALID)?;
        let path = self.root.join("evidence-v1").join(format!("{id}.batch"));
        let cached = {
            let mut cache = PAYLOAD_READERS.lock();
            cache
                .iter()
                .position(|(p, m, _)| p == &path && m == meta)
                .map(|position| {
                    let item = cache.remove(position).unwrap();
                    let reader = Arc::clone(&item.2);
                    cache.push_back(item);
                    reader
                })
        };
        if let Some(reader) = cached {
            if reader.validate().is_ok() {
                self.known_stamps
                    .lock()
                    .insert(id.into(), reader.stamp.clone());
                return Ok(reader);
            }
            let retired = {
                let mut cache = PAYLOAD_READERS.lock();
                cache
                    .iter()
                    .position(|(p, m, _)| p == &path && m == meta)
                    .map(|position| cache.remove(position).unwrap())
            };
            drop(retired);
            drop(reader);
        }
        let reader = Arc::new(VerifiedBatch::open(&path, meta)?);
        let (ready, evicted) = {
            let mut cache = PAYLOAD_READERS.lock();
            if let Some((_, _, ready)) = cache.iter().find(|(p, m, _)| p == &path && m == meta) {
                (Arc::clone(ready), None)
            } else {
                cache.push_back((path, meta.clone(), Arc::clone(&reader)));
                (
                    Arc::clone(&reader),
                    if cache.len() > CACHED_READERS {
                        cache.pop_front()
                    } else {
                        None
                    },
                )
            }
        };
        drop(evicted);
        drop(reader);
        self.known_stamps
            .lock()
            .insert(id.into(), ready.stamp.clone());
        Ok(ready)
    }
    pub(crate) fn visit_envelopes(
        &self,
        range: std::ops::Range<usize>,
        mut visit: impl FnMut(MemberHandle, &str) -> Result<Visit, String>,
    ) -> Result<(), String> {
        self.visit_envelopes_with_origin(range, |member, _, text| visit(member, text))
    }
    pub(crate) fn visit_envelopes_with_origin(
        &self,
        range: std::ops::Range<usize>,
        mut visit: impl FnMut(MemberHandle, u32, &str) -> Result<Visit, String>,
    ) -> Result<(), String> {
        self.validate()?;
        if range.start > range.end || range.end > self.manifest.members.len() {
            return Err(INVALID.into());
        }
        for member in &self.manifest.members[range] {
            crate::operations::check()?;
            let batch = self.batch(&member.record.batch_id)?;
            let text = batch.envelope(member.record.ordinal)?;
            let handle = MemberHandle {
                container_id: self.reference.container_id.clone(),
                manifest_id: self.reference.manifest_id.clone(),
                occurrence_id: member.occurrence_id.clone(),
            };
            if visit(handle, member.origin_position, &text)? == Visit::Stop {
                break;
            }
        }
        self.validate()
    }
    /// `reserve` runs after bounded preflight and BEFORE typed allocation. Its
    /// RAII credit is handed to the consumer with the Event; decode errors drop
    /// it. No complete Event vector or unbounded producer queue is retained.
    pub(crate) fn visit_events<C>(
        &self,
        range: std::ops::Range<usize>,
        mut reserve: impl FnMut(&MemberHandle, RecordPlan) -> Result<C, String>,
        mut visit: impl FnMut(MemberHandle, crate::model::Event, RecordPlan, C) -> Result<Visit, String>,
    ) -> Result<(), String> {
        self.visit_envelopes(range, |member, envelope| {
            let plan = preflight_envelope(envelope)?;
            let credit = reserve(&member, plan)?;
            let event = materialize_event(envelope, plan)?;
            visit(member, event, plan, credit)
        })
    }
    pub(crate) fn envelope_for_member(&self, handle: &MemberHandle) -> Result<String, String> {
        self.validate()?;
        if handle.container_id != self.reference.container_id
            || handle.manifest_id != self.reference.manifest_id
        {
            return Err(INVALID.into());
        }
        let member = self
            .manifest
            .members
            .iter()
            .find(|member| member.occurrence_id == handle.occurrence_id)
            .ok_or(INVALID)?;
        let text = self
            .batch(&member.record.batch_id)?
            .envelope(member.record.ordinal)?;
        self.validate()?;
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner_value() -> EvidenceOwner {
        EvidenceOwner {
            store_id: uuid::Uuid::new_v4().to_string(),
            case_id: "c".into(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
        }
    }
    #[test]
    fn reader_admission_is_shared_and_precedes_the_next_file_open() {
        const ENV: &str = "LOGINSIGHT_TEST_NATIVE_READER_LIMIT";
        if std::env::var_os(ENV).is_none() {
            let output=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","case_evidence::storage::tests::reader_admission_is_shared_and_precedes_the_next_file_open","--nocapture"])
                .env(ENV,"1").output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("tiny");
        fs::write(&path, b"tiny").unwrap();
        let mut readers = Vec::new();
        for _ in 0..MAX_LIVE_READERS {
            readers.push(regular(&path).unwrap());
        }
        assert_eq!(LIVE_READERS.load(Ordering::Acquire), MAX_LIVE_READERS);
        // Admission fails before even attempting to open this missing file.
        assert!(regular(&root.path().join("missing"))
            .err()
            .unwrap()
            .contains("CASE_EVIDENCE_BUSY"));
        drop(readers);
        assert_eq!(LIVE_READERS.load(Ordering::Acquire), 0);
        let staged = stage_records(root.path(), &owner_value(), &Value::Null, |sink| {
            sink.push_envelope("{\"id\":0}")
        })
        .unwrap();
        staged.publish_files(root.path()).unwrap();
        let mut containers = Vec::new();
        for _ in 0..80 {
            containers.push(
                VerifiedContainer::open(root.path(), &staged.reference, [staged.batch.clone()])
                    .unwrap(),
            );
        }
        assert_eq!(
            LIVE_READERS.load(Ordering::Acquire),
            0,
            "verified manifests retain metadata, not one descriptor each"
        );
        for container in &containers {
            container
                .visit_envelopes(0..1, |_, raw| {
                    assert_eq!(raw, "{\"id\":0}");
                    Ok(Visit::Continue)
                })
                .unwrap();
        }
        assert_eq!(
            LIVE_READERS.load(Ordering::Acquire),
            1,
            "one shared payload reader across containers"
        );
        let in_flight = containers[0].batch(&staged.batch.batch_id).unwrap();
        let other_root = root.path().join("another-profile");
        fs::create_dir(&other_root).unwrap();
        let other = stage_records(&other_root, &owner_value(), &Value::Null, |sink| {
            sink.push_envelope("{\"id\":1}")
        }).unwrap();
        other.publish_files(&other_root).unwrap();
        let other_container = VerifiedContainer::open(&other_root, &other.reference, [other.batch.clone()]).unwrap();
        other_container.visit_envelopes(0..1, |_, _| Ok(Visit::Continue)).unwrap();
        assert_eq!(LIVE_READERS.load(Ordering::Acquire), 2);
        assert!(clear_cached_readers(&root.path().join("missing")).is_err());
        assert_eq!(LIVE_READERS.load(Ordering::Acquire), 2);
        let lexical = root.path().join("path-component");
        fs::create_dir(&lexical).unwrap();
        // A real lexical alias reproduces the mismatch on every platform.
        clear_cached_readers(&lexical.join("..")).unwrap();
        assert_eq!(LIVE_READERS.load(Ordering::Acquire), 2, "in-flight and other-profile readers survive");
        drop(in_flight);
        assert_eq!(LIVE_READERS.load(Ordering::Acquire), 1);
        clear_cached_readers(&root.path().canonicalize().unwrap()).unwrap();
        assert_eq!(LIVE_READERS.load(Ordering::Acquire), 1, "nested profile is independent");
        clear_cached_readers(&other_root).unwrap();
        assert_eq!(LIVE_READERS.load(Ordering::Acquire), 0);
    }
    #[test]
    fn envelopes_keep_whitespace_numbers_unknown_members_and_duplicate_occurrences() {
        let root = tempfile::tempdir().unwrap();
        let raw = "{\n  \"id\": 0, \"unknown\": [18446744073709551615, 1.0, -0.0], \"x\": true\n}";
        let prepared = stage_records(
            root.path(),
            &owner_value(),
            &serde_json::json!({"source":"fixture"}),
            |sink| {
                sink.push_envelope(raw)?;
                sink.push_envelope(raw)
            },
        )
        .unwrap();
        let batch = prepared.open_batch().unwrap();
        assert_eq!(batch.envelope(0).unwrap(), raw);
        assert_eq!(batch.envelope(1).unwrap(), raw);
        assert_eq!(prepared.manifest().members.len(), 2);
        assert_eq!(
            prepared
                .manifest()
                .members
                .iter()
                .map(|m| m.origin_position)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_ne!(
            prepared.manifest().members[0].occurrence_id,
            prepared.manifest().members[1].occurrence_id
        );
        assert_eq!(batch.origin_receipt()["source"], "fixture");
        prepared.publish_files(root.path()).unwrap();
        prepared.publish_files(root.path()).unwrap();
        assert!(batch.envelope(2).is_err());
    }
    #[test]
    fn origin_receipt_reopens_literal_objects_and_exact_fraction() {
        let root = tempfile::tempdir().unwrap();
        let origin = serde_json::json!({
            "single": {"$serde_json::private::RawValue":"1"},
            "nested": [{"$serde_json::private::RawValue":"null","extra":2}],
            "fraction": f64::from_bits(0x3e5b597464455d8a),
        });
        let staged = stage_records(root.path(), &owner_value(), &origin, |sink| sink.push_envelope("{}")).unwrap();
        staged.publish_files(root.path()).unwrap();
        let path = root.path().join("evidence-v1").join(format!("{}.batch", staged.batch.batch_id));
        let opened = VerifiedBatch::open(&path, &staged.batch).unwrap();
        assert_eq!(serde_json::to_string(opened.origin_receipt()).unwrap(), serde_json::to_string(&origin).unwrap());
        assert_eq!(opened.origin_receipt()["fraction"].as_f64().unwrap().to_bits(), 0x3e5b597464455d8a);
        assert!(opened._origin_credit.bytes() > 0);
    }
    #[test]
    fn failed_producer_never_seals_or_publishes_a_payload() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            stage_records(root.path(), &owner_value(), &Value::Null, |sink| {
                sink.push_envelope("{}")?;
                Err("source changed".into())
            })
            .is_err()
        );
        assert_eq!(
            fs::read_dir(root.path().join("evidence-v1"))
                .unwrap()
                .count(),
            0
        );
    }
    #[test]
    fn malformed_nonobject_and_oversized_records_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        for raw in ["[1]", "{broken", "{} trailing"] {
            assert!(
                stage_records(root.path(), &owner_value(), &Value::Null, |sink| sink
                    .push_envelope(raw))
                .is_err()
            );
        }
    }
    #[test]
    fn corruption_and_owner_mismatch_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let prepared = stage_records(root.path(), &owner_value(), &Value::Null, |sink| {
            sink.push_envelope("{\"id\":0}")
        })
        .unwrap();
        let mut wrong = prepared.batch.clone();
        wrong.owner = owner_value();
        assert!(VerifiedBatch::open(&prepared.batch_path(), &wrong).is_err());
        let batch = prepared.open_batch().unwrap();
        OpenOptions::new()
            .write(true)
            .open(prepared.batch_path())
            .unwrap()
            .set_len(8)
            .unwrap();
        assert!(batch.envelope(0).is_err());
        assert!(prepared.open_batch().is_err());
    }
    #[test]
    fn changed_staging_size_fails_before_any_destination_publication() {
        let root = tempfile::tempdir().unwrap();
        let prepared = stage_records(root.path(), &owner_value(), &Value::Null, |sink| {
            sink.push_envelope("{}")
        })
        .unwrap();
        OpenOptions::new()
            .append(true)
            .open(prepared.batch_path())
            .unwrap()
            .write_all(&[0; 1024])
            .unwrap();
        assert!(prepared.publish_files(root.path()).is_err());
        assert!(!root
            .path()
            .join("evidence-v1")
            .join(format!("{}.batch", prepared.batch.batch_id))
            .exists());
        assert!(!root
            .path()
            .join("evidence-v1")
            .join(format!("{}.manifest", prepared.reference.manifest_id))
            .exists());
    }
    #[test]
    fn exclusive_payload_owner_returns_busy_without_waiting() {
        let root = tempfile::tempdir().unwrap();
        let prepared = stage_records(root.path(), &owner_value(), &Value::Null, |sink| {
            sink.push_envelope("{}")
        })
        .unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(prepared.batch_path())
            .unwrap();
        file.try_lock_exclusive().unwrap();
        assert!(prepared
            .open_batch()
            .err()
            .unwrap()
            .contains("CASE_EVIDENCE_BUSY"));
    }
}

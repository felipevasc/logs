//! Crash-safe, append-only metadata checkpoints. A manifest is the sole commit
//! point: bytes beyond its verified prefix are never treated as indexed rows.
use crate::model::LineMeta;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAGIC: &[u8; 8] = b"LIDJ0001";
const RECORD_BYTES: usize = 27;
const HEADER_BYTES: usize = 72;
const BLOCK_ROWS: usize = 8192;
const MAX_MANIFEST_BYTES: u64 = 4_000_000;
pub(crate) const VERSION: u32 = 1;


/// Durability cadence, independent from parser wave sizes and spool buffering.
/// Limits are checked only at a parser-provided safe boundary. A wave or one
/// indivisible record can overshoot them; the elapsed limit is not a deadline.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CheckpointPolicy {
    pub rows: usize,
    pub source_bytes: usize,
    pub elapsed: Duration,
}
impl Default for CheckpointPolicy {
    fn default() -> Self {
        Self { rows: 65_536, source_bytes: 16 << 20, elapsed: Duration::from_secs(2) }
    }
}
impl CheckpointPolicy {
    pub(crate) fn named(name: &str) -> Result<Self, String> {
        match name {
            "batched" => Ok(Self::default()),
            "wave" => Ok(Self::every_wave()),
            _ => Err("LOGINSIGHT_METADATA_CHECKPOINT_POLICY deve ser batched ou wave.".into()),
        }
    }

    pub(crate) fn configured() -> Self {
        match std::env::var_os("LOGINSIGHT_METADATA_CHECKPOINT_POLICY") {
            None => Self::default(),
            Some(value) => match value.to_str().and_then(|name| Self::named(name).ok()) {
                Some(policy) => policy,
                None => {
                    eprintln!("[índice] LOGINSIGHT_METADATA_CHECKPOINT_POLICY inválido; usando checkpoint a cada wave");
                    Self::every_wave()
                }
            },
        }
    }

    fn due(self, rows: usize, source_bytes: usize, elapsed: Duration, finalizing: bool) -> bool {
        finalizing || (rows > 0 || source_bytes > 0)
            && (rows >= self.rows || source_bytes >= self.source_bytes || elapsed >= self.elapsed)
    }

    /// Compatibility rollback: keep the previous per-wave durability cadence.
    pub(crate) fn every_wave() -> Self {
        Self { rows: 1, source_bytes: 1, elapsed: Duration::ZERO }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Progress {
    pub phase_id: &'static str,
    pub phase: String,
    pub completed: usize,
    pub total: usize,
    pub unit: &'static str,
    /// Only sealed rows count as reused/durable. The mutable multiline tail is
    /// persisted separately but cannot yet be counted as a finalized record.
    pub resumed_rows: usize,
    pub checkpoint_rows: usize,
    /// Raw metadata parser calls; column discovery is a separate bounded phase.
    pub parsed_rows: usize,
}
impl Progress {
    pub(crate) fn new(
        phase_id: &'static str,
        phase: &'static str,
        completed: usize,
        total: usize,
        unit: &'static str,
    ) -> Self {
        Self {
            phase_id,
            phase: phase.into(),
            completed,
            total,
            unit,
            resumed_rows: 0,
            checkpoint_rows: 0,
            parsed_rows: 0,
        }
    }
}
pub(crate) type Reporter<'a> = Option<&'a dyn Fn(&Progress)>;
pub(crate) fn report(reporter: Reporter<'_>, progress: Progress) {
    if let Some(reporter) = reporter {
        reporter(&progress);
    }
}

#[derive(Default)]
pub(crate) struct Resume {
    pub lines: crate::metadata_store::LineBuilder,
    pub cursor: usize,
    pub scan_complete: bool,
    pub columns: Option<Vec<String>>,
    pub resumed_rows: usize,
}

#[derive(Serialize, Deserialize, Clone)]
struct Tail {
    offset: u64,
    len: u32,
    ts: i64,
    level: u8,
    code_off: u32,
    code_len: u16,
}
impl From<LineMeta> for Tail {
    fn from(m: LineMeta) -> Self {
        Self {
            offset: m.offset,
            len: m.len,
            ts: m.ts,
            level: m.level,
            code_off: m.code_off,
            code_len: m.code_len,
        }
    }
}
impl From<Tail> for LineMeta {
    fn from(m: Tail) -> Self {
        Self {
            offset: m.offset,
            len: m.len,
            ts: m.ts,
            level: m.level,
            code_off: m.code_off,
            code_len: m.code_len,
        }
    }
}
#[derive(Serialize, Deserialize)]
struct State {
    version: u32,
    key: String,
    source_bytes: u64,
    payload_bytes: u64,
    payload_sha256: String,
    sealed_rows: usize,
    cursor: u64,
    tail: Option<Tail>,
    scan_complete: bool,
    columns: Option<Vec<String>>,
}
#[derive(Serialize, Deserialize)]
struct Envelope {
    state: State,
    sha256: String,
}

pub(crate) enum OpenError {
    Cancelled(String),
    Busy,
    Unavailable(String),
}
impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled(message) | Self::Unavailable(message) => f.write_str(message),
            Self::Busy => f.write_str("Outro processo está preparando estes metadados; tente novamente após ele concluir."),
        }
    }
}

pub(crate) struct Journal {
    state_path: PathBuf,
    key: String,
    source_bytes: usize,
    file: BufWriter<File>,
    hash: Sha256,
    sealed_rows: usize,
    resumed_rows: usize,
    multiline: bool,
    committed_cursor: usize,
    committed_at: Instant,
    // Never unlink lock files: another process may already hold a handle.
    _lock: File,
}

pub(crate) fn encode_record(m: &LineMeta, out: &mut Vec<u8>) {
    out.extend_from_slice(&m.offset.to_le_bytes());
    out.extend_from_slice(&m.len.to_le_bytes());
    out.extend_from_slice(&m.ts.to_le_bytes());
    out.push(m.level);
    out.extend_from_slice(&m.code_off.to_le_bytes());
    out.extend_from_slice(&m.code_len.to_le_bytes());
}
fn decode_record(b: &[u8]) -> Option<LineMeta> {
    Some(LineMeta {
        offset: u64::from_le_bytes(b.get(0..8)?.try_into().ok()?),
        len: u32::from_le_bytes(b.get(8..12)?.try_into().ok()?),
        ts: i64::from_le_bytes(b.get(12..20)?.try_into().ok()?),
        level: *b.get(20)?,
        code_off: u32::from_le_bytes(b.get(21..25)?.try_into().ok()?),
        code_len: u16::from_le_bytes(b.get(25..27)?.try_into().ok()?),
    })
}
fn valid_record(m: &LineMeta, cursor: u64, previous_end: Option<u64>) -> bool {
    m.len > 0
        && m.offset
            .checked_add(u64::from(m.len))
            .is_some_and(|end| end <= cursor)
        && previous_end.is_none_or(|end| m.offset >= end)
        && (m.code_len == 0 || u64::from(m.code_off) + u64::from(m.code_len) <= u64::from(m.len))
}
fn state_digest(state: &State) -> Result<String, String> {
    let data = serde_json::to_vec(state).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(data)))
}

/// Open only a fully committed, column-complete journal without decoding it
/// into a per-row heap allocation. The shared lease outlives every cloned view.
pub(crate) fn open_complete(
    dir: &Path, key: &str, source_bytes: usize, initial_cursor: usize,
    multiline: bool, reporter: Reporter<'_>,
) -> Result<Option<(crate::metadata_store::LineStore, Vec<String>)>, String> {
    crate::operations::check()?;
    let lock_path = dir.join(format!("{key}.lock"));
    let Ok(lock) = OpenOptions::new().read(true).write(true).open(lock_path) else { return Ok(None); };
    // A writer may be advancing a partial checkpoint. Leave its bounded,
    // cancellable exclusive-lock wait to Journal::open rather than racing it.
    if FileExt::try_lock_shared(&lock).is_err() { return Ok(None); }
    let path = dir.join(format!("{key}.lines"));
    let state_path = dir.join(format!("{key}.state"));
    let receipt_generation = crate::cache_validation::Generation::capture(&[path.clone(), state_path.clone()]);
    let Some((state, file)) = read_state(&path, &state_path, key, source_bytes, initial_cursor, multiline) else { return Ok(None); };
    if !state.scan_complete || state.columns.is_none() { return Ok(None); }
    let payload_bytes = usize::try_from(state.payload_bytes).map_err(|_| "Metadados excedem o espaço de endereçamento.")?;
    let generation = file.metadata().map_err(|e| e.to_string())?;
    // SAFETY: cooperating writers and pruning require the same stable lock
    // exclusively. This shared lease is retained by the immutable LineStore.
    // The map covers only the manifest's committed prefix, never transient tail.
    let Ok(mapped) = (unsafe { memmap2::MmapOptions::new().len(payload_bytes).map(&file) }) else { return Ok(None); };
    if &mapped[..8] != MAGIC || &mapped[8..HEADER_BYTES] != key.as_bytes() { return Ok(None); }
    let context = format!("metadata-map-v1|{}", state_digest(&state)?);
    let reused = receipt_generation.as_ref().is_some_and(|g| g.covers(&path, &file) && g.verified(&path, &context));
    if !reused {
        let mut hash = Sha256::new(); hash.update(&mapped[..HEADER_BYTES]);
        let mut previous_end = None; let mut completed = 0; let mut last_report = Instant::now();
        report(reporter, Progress::new("metadata-map-validate", "Verificando metadados mapeados", 0, state.sealed_rows, "registros"));
        for chunk in mapped[HEADER_BYTES..].chunks(BLOCK_ROWS * RECORD_BYTES) {
            crate::operations::check()?; hash.update(chunk);
            for bytes in chunk.chunks_exact(RECORD_BYTES) {
                let Some(meta) = decode_record(bytes) else { return Ok(None); };
                if !valid_record(&meta, state.cursor, previous_end) { return Ok(None); }
                previous_end = meta.offset.checked_add(u64::from(meta.len));
                completed += 1;
            }
            if last_report.elapsed() >= Duration::from_millis(150) {
                report(reporter, Progress::new("metadata-map-validate", "Verificando metadados mapeados", completed, state.sealed_rows, "registros"));
                last_report = Instant::now();
            }
        }
        if completed != state.sealed_rows || format!("{:x}", hash.finalize()) != state.payload_sha256 { return Ok(None); }
    }
    crate::operations::check()?;
    // Verification touches every page. Drop that view, then remap the SAME
    // opened file under the SAME retained shared lease, so later queries fault
    // only the pages they need into this process. The OS page cache stays
    // reclaimable; this is not a physical-RAM limit or cache-dropping operation.
    drop(mapped);
    let current = file.metadata().map_err(|e| e.to_string())?;
    if current.len() != generation.len() || current.modified().ok() != generation.modified().ok() {
        return Err("Metadados mudaram durante a validação.".into());
    }
    if let Some(generation) = &receipt_generation {
        if !generation.is_current() || !generation.covers(&path, &file) {
            return Err("Metadados mudaram durante a validação.".into());
        }
        if !reused { generation.remember(&path, &context); }
    }
    // SAFETY: the file descriptor and shared writer/pruner lease were retained
    // without interruption; no references into the old mapping survive.
    let mapped = unsafe { memmap2::MmapOptions::new().len(payload_bytes).map(&file) }
        .map_err(|e| format!("Não foi possível remapear metadados verificados: {e}"))?;
    crate::operations::check()?;
    let lines = crate::metadata_store::LineStore::from_validated_journal(mapped, std::sync::Arc::new(lock), HEADER_BYTES, state.sealed_rows)?;
    let completed = state.sealed_rows;
    let mut done = if reused {
        Progress::new("metadata-map-reuse", "Reutilizando metadados salvos", completed, completed, "registros")
    } else {
        Progress::new("metadata-map-validate", "Metadados mapeados verificados", completed, completed, "registros")
    };
    done.resumed_rows = completed; done.checkpoint_rows = completed; report(reporter, done);
    Ok(Some((lines, state.columns.unwrap())))
}

fn read_state(path: &Path, state_path: &Path, key: &str, source_bytes: usize, initial_cursor: usize, multiline: bool) -> Option<(State, File)> {
    let metadata = std::fs::metadata(state_path).ok()?;
    if metadata.len() > MAX_MANIFEST_BYTES { return None; }
    let envelope: Envelope = serde_json::from_slice(&std::fs::read(state_path).ok()?).ok()?;
    if state_digest(&envelope.state).ok()? != envelope.sha256 { return None; }
    let state = envelope.state;
    let expected = (state.sealed_rows as u64).checked_mul(RECORD_BYTES as u64)?.checked_add(HEADER_BYTES as u64)?;
    if state.version != VERSION || state.key != key || state.source_bytes != source_bytes as u64
        || state.payload_bytes != expected || state.sealed_rows > source_bytes || state.sealed_rows > 500_000_000
        || state.cursor < initial_cursor as u64 || state.cursor > source_bytes as u64
        || state.columns.is_some() && !state.scan_complete || state.tail.is_some() && (!multiline || state.scan_complete)
        || state.scan_complete && state.cursor != source_bytes as u64 { return None; }
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() < expected { return None; }
    Some((state, file))
}

impl Journal {
    pub(crate) fn open(
        dir: &Path,
        key: &str,
        source_bytes: usize,
        initial_cursor: usize,
        multiline: bool,
        reporter: Reporter<'_>,
    ) -> Result<(Self, Resume), OpenError> {
        crate::operations::check().map_err(OpenError::Cancelled)?;
        std::fs::create_dir_all(dir).map_err(|e| OpenError::Unavailable(e.to_string()))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(format!("{key}.lock")))
            .map_err(|e| OpenError::Unavailable(e.to_string()))?;
        let waiting = Instant::now();
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    crate::operations::check().map_err(OpenError::Cancelled)?;
                    report(
                        reporter,
                        Progress::new(
                            "metadata-lock",
                            "Aguardando metadados de outro processo",
                            0,
                            0,
                            "",
                        ),
                    );
                    if waiting.elapsed() >= Duration::from_secs(5) {
                        return Err(OpenError::Busy);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(OpenError::Unavailable(e.to_string())),
            }
        }
        let path = dir.join(format!("{key}.lines"));
        let state_path = dir.join(format!("{key}.state"));
        report(
            reporter,
            Progress::new(
                "metadata-validate",
                "Verificando checkpoint de metadados",
                0,
                0,
                "",
            ),
        );
        let restored = restore(
            &path,
            &state_path,
            key,
            source_bytes,
            initial_cursor,
            multiline,
            reporter,
        )
        .map_err(OpenError::Cancelled)?;
        let (resume, hash, payload_bytes, sealed_rows) = match restored {
            Some(restored) => restored,
            None => {
                let header = [MAGIC.as_slice(), key.as_bytes()].concat();
                let mut file =
                    File::create(&path).map_err(|e| OpenError::Unavailable(e.to_string()))?;
                file.write_all(&header)
                    .map_err(|e| OpenError::Unavailable(e.to_string()))?;
                // An obsolete/corrupt marker must not survive a reset payload.
                match std::fs::remove_file(&state_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(OpenError::Unavailable(e.to_string())),
                }
                let mut hash = Sha256::new();
                hash.update(&header);
                (
                    Resume {
                        cursor: initial_cursor,
                        ..Resume::default()
                    },
                    hash,
                    HEADER_BYTES as u64,
                    0,
                )
            }
        };
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| OpenError::Unavailable(e.to_string()))?;
        // Only discard an uncommitted tail. Calling set_len with an unchanged
        // length still updates mtime on some filesystems, making a clean warm
        // reopen look like a rewritten checkpoint.
        if file.metadata().map_err(|e| OpenError::Unavailable(e.to_string()))?.len() != payload_bytes {
            file.set_len(payload_bytes)
                .map_err(|e| OpenError::Unavailable(e.to_string()))?;
        }
        file.seek(SeekFrom::Start(payload_bytes))
            .map_err(|e| OpenError::Unavailable(e.to_string()))?;
        let resumed_rows = resume.resumed_rows;
        Ok((
            Self {
                state_path,
                key: key.into(),
                source_bytes,
                file: BufWriter::with_capacity(1 << 20, file),
                hash,
                sealed_rows,
                resumed_rows,
                multiline,
                committed_cursor: resume.cursor,
                committed_at: Instant::now(),
                _lock: lock,
            },
            resume,
        ))
    }

    #[cfg(test)]
    fn checkpoint(&mut self, rows: &[LineMeta], cursor: usize, complete: bool, columns: Option<&[String]>, reporter: Reporter<'_>, validate: &dyn Fn() -> Result<(), String>) -> Result<(), String> {
        let mut spool = crate::metadata_store::LineBuilder::default();
        spool.extend(rows.iter().copied())?;
        self.checkpoint_rows(&mut spool, cursor, complete, columns, reporter, validate)
    }

    pub(crate) fn checkpoint_if_due(
        &mut self,
        policy: CheckpointPolicy,
        lines: &mut crate::metadata_store::LineBuilder,
        cursor: usize,
        scan_complete: bool,
        columns: Option<&[String]>,
        reporter: Reporter<'_>,
        validate: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        crate::operations::check()?;
        let sealed = if self.multiline && !scan_complete {
            lines.len().saturating_sub(1)
        } else { lines.len() };
        if sealed < self.sealed_rows || cursor < self.committed_cursor || cursor > self.source_bytes
            || columns.is_some() && !scan_complete {
            return Err("Estado de checkpoint de metadados inconsistente.".into());
        }
        // A deferred checkpoint never flushes the spool, appends a journal,
        // hashes records or syncs. The parser's existing bounded spool and all
        // cancellation/source-generation checks keep their original cadence.
        if policy.due(sealed - self.sealed_rows, cursor - self.committed_cursor,
                      self.committed_at.elapsed(), scan_complete || columns.is_some()) {
            self.checkpoint_rows(lines, cursor, scan_complete, columns, reporter, validate)?;
        }
        Ok(())
    }

    pub(crate) fn checkpoint_rows(
        &mut self,
        lines: &mut crate::metadata_store::LineBuilder,
        cursor: usize,
        scan_complete: bool,
        columns: Option<&[String]>,
        reporter: Reporter<'_>,
        validate: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        crate::operations::check()?;
        validate()?;
        lines.flush()?;
        let sealed = if self.multiline && !scan_complete {
            lines.len().saturating_sub(1)
        } else {
            lines.len()
        };
        if sealed < self.sealed_rows
            || cursor < self.committed_cursor
            || cursor > self.source_bytes
            || columns.is_some() && !scan_complete
        {
            return Err("Estado de checkpoint de metadados inconsistente.".into());
        }
        let mut progress = Progress::new(
            "metadata-checkpoint-write",
            "Gravando checkpoint de metadados",
            0,
            sealed - self.sealed_rows,
            "registros",
        );
        progress.resumed_rows = self.resumed_rows;
        progress.checkpoint_rows = self.sealed_rows;
        report(reporter, progress.clone());
        let mut buffer = Vec::with_capacity(BLOCK_ROWS * RECORD_BYTES);
        let old_sealed = self.sealed_rows;
        for start in (old_sealed..sealed).step_by(BLOCK_ROWS) {
            crate::operations::check()?;
            let chunk = lines.read_chunk(start..(start + BLOCK_ROWS).min(sealed))?;
            buffer.clear();
            for m in &chunk {
                encode_record(m, &mut buffer);
            }
            self.file.write_all(&buffer).map_err(|e| e.to_string())?;
            self.hash.update(&buffer);
            progress.completed += chunk.len();
        }
        report(reporter, progress);
        let mut sync = Progress::new(
            "metadata-checkpoint-sync",
            "Sincronizando checkpoint de metadados",
            0,
            0,
            "",
        );
        sync.resumed_rows = self.resumed_rows;
        sync.checkpoint_rows = self.sealed_rows;
        report(reporter, sync);
        crate::operations::check()?;
        self.file.flush().map_err(|e| e.to_string())?;
        self.file.get_ref().sync_all().map_err(|e| e.to_string())?;
        crate::operations::check()?;
        validate()?;
        let mut publishing = Progress::new(
            "metadata-checkpoint-publish",
            "Publicando checkpoint de metadados",
            0,
            0,
            "",
        );
        publishing.resumed_rows = self.resumed_rows;
        publishing.checkpoint_rows = self.sealed_rows;
        report(reporter, publishing);
        crate::operations::check()?;
        let state = State {
            version: VERSION,
            key: self.key.clone(),
            source_bytes: self.source_bytes as u64,
            payload_bytes: HEADER_BYTES as u64 + sealed as u64 * RECORD_BYTES as u64,
            payload_sha256: format!("{:x}", self.hash.clone().finalize()),
            sealed_rows: sealed,
            cursor: cursor as u64,
            tail: if sealed < lines.len() { Some(lines.at(sealed)?.into()) } else { None },
            scan_complete,
            columns: columns.map(<[String]>::to_vec),
        };
        publish(&self.state_path, state, &|| {
            let mut ready = Progress::new(
                "metadata-checkpoint-manifest-ready", "Descritor de metadados sincronizado", 0, 0, "",
            );
            ready.resumed_rows = self.resumed_rows;
            ready.checkpoint_rows = self.sealed_rows;
            report(reporter, ready);
        })?;
        self.sealed_rows = sealed;
        self.committed_cursor = cursor;
        self.committed_at = Instant::now();
        let mut committed = Progress::new(
            "metadata-checkpoint-committed",
            "Checkpoint de metadados preservado",
            cursor,
            self.source_bytes,
            "bytes",
        );
        committed.resumed_rows = self.resumed_rows;
        committed.checkpoint_rows = sealed;
        report(reporter, committed);
        Ok(())
    }
}

// Hash and decode in the same bounded pass. Cancellation is an error, not a
// cache miss that would restart parsing under an already-cancelled operation.
fn restore(
    path: &Path,
    state_path: &Path,
    key: &str,
    source_bytes: usize,
    initial_cursor: usize,
    multiline: bool,
    reporter: Reporter<'_>,
) -> Result<Option<(Resume, Sha256, u64, usize)>, String> {
    let Some((state, file)) = read_state(path, state_path, key, source_bytes, initial_cursor, multiline) else { return Ok(None); };
    let mut file = BufReader::with_capacity(1 << 20, file);
    let mut header = [0u8; HEADER_BYTES];
    if file.read_exact(&mut header).is_err()
        || &header[..8] != MAGIC
        || &header[8..] != key.as_bytes()
    {
        return Ok(None);
    }
    let mut hash = Sha256::new();
    hash.update(header);
    let mut lines = crate::metadata_store::LineBuilder::default();
    let mut buffer = vec![0; BLOCK_ROWS * RECORD_BYTES];
    let mut last_report = Instant::now();
    let mut previous_end = None;
    while lines.len() < state.sealed_rows {
        crate::operations::check()?;
        let count = (state.sealed_rows - lines.len()).min(BLOCK_ROWS);
        let bytes = &mut buffer[..count * RECORD_BYTES];
        if file.read_exact(bytes).is_err() {
            return Ok(None);
        }
        hash.update(&*bytes);
        for raw in bytes.chunks_exact(RECORD_BYTES) {
            let Some(m) = decode_record(raw) else {
                return Ok(None);
            };
            if !valid_record(&m, state.cursor, previous_end) {
                return Ok(None);
            }
            previous_end = m.offset.checked_add(u64::from(m.len));
            lines.push(m)?;
        }
        if last_report.elapsed() >= Duration::from_millis(150) {
            report(
                reporter,
                Progress::new(
                    "metadata-restore",
                    "Restaurando metadados preservados",
                    lines.len(),
                    state.sealed_rows,
                    "registros",
                ),
            );
            last_report = Instant::now();
        }
    }
    if format!("{:x}", hash.clone().finalize()) != state.payload_sha256 {
        return Ok(None);
    }
    if let Some(tail) = state.tail {
        let m = tail.into();
        if !valid_record(&m, state.cursor, previous_end) {
            return Ok(None);
        }
        lines.push(m)?;
    }
    lines.flush()?;
    crate::operations::check()?;
    let mut progress = Progress::new(
        "metadata-restore",
        "Metadados preservados restaurados",
        state.sealed_rows,
        state.sealed_rows,
        "registros",
    );
    progress.resumed_rows = state.sealed_rows;
    progress.checkpoint_rows = state.sealed_rows;
    report(reporter, progress);
    Ok(Some((
        Resume {
            lines,
            cursor: state.cursor as usize,
            scan_complete: state.scan_complete,
            columns: state.columns,
            resumed_rows: state.sealed_rows,
        },
        hash,
        state.payload_bytes,
        state.sealed_rows,
    )))
}

fn publish(target: &Path, state: State, manifest_ready: &dyn Fn()) -> Result<(), String> {
    let temporary = target.with_extension(format!("{}.pending", uuid::Uuid::new_v4()));
    let result = (|| {
        let envelope = Envelope {
            sha256: state_digest(&state)?,
            state,
        };
        let bytes = serde_json::to_vec(&envelope).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("Descritor de metadados excede o limite do checkpoint (4 MB).".into());
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        manifest_ready();
        crate::operations::check()?;
        std::fs::rename(&temporary, target).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(parent) = target.parent() {
            File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// Pruning follows the writer lock and never unlinks the lock inode itself.
pub(crate) fn prune(dir: &Path, cutoff: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((key, _)) = name.split_once('.') else {
            continue;
        };
        if key.len() != 64 || !key.bytes().all(|c| c.is_ascii_hexdigit()) || name.ends_with(".lock")
        {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|m| m.accessed().or_else(|_| m.modified()).ok())
            .is_some_and(|time| time < cutoff);
        if !stale {
            continue;
        }
        let Ok(lock) = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(format!("{key}.lock")))
        else {
            continue;
        };
        if lock.try_lock_exclusive().is_err() {
            continue;
        }
        let state = dir.join(format!("{key}.state"));
        let active = std::fs::metadata(&state)
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|time| time >= cutoff);
        if !active {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_uses_rows_source_bytes_or_elapsed_at_a_safe_boundary() {
        let policy = CheckpointPolicy::default();
        let almost = policy.elapsed - Duration::from_nanos(1);
        assert!(!policy.due(policy.rows - 1, policy.source_bytes - 1, almost, false));
        assert!(policy.due(policy.rows, 1, Duration::ZERO, false));
        assert!(policy.due(0, policy.source_bytes, Duration::ZERO, false), "mutable tail is protected by scanned bytes");
        assert!(policy.due(0, 1, policy.elapsed, false), "tail-only progress is protected by time");
        assert!(!policy.due(0, 0, policy.elapsed * 2, false), "no periodic rewrite without progress");
        assert!(policy.due(0, 0, Duration::ZERO, true), "EOF/columns are unconditional");
        assert!(CheckpointPolicy::named("wave").unwrap().due(0, 1, Duration::ZERO, false));
        assert!(!CheckpointPolicy::named("batched").unwrap().due(1, 1, Duration::ZERO, false));
        assert!(CheckpointPolicy::named("invalid").is_err());
    }

    #[test]
    fn small_spool_batches_do_not_publish_and_eof_columns_still_force_commit() {
        let dir = tempfile::tempdir().unwrap();
        let key = "d".repeat(64);
        let (mut journal, _) = Journal::open(dir.path(), &key, 100, 0, false, None).unwrap_or_else(|e| panic!("{e}"));
        let policy = CheckpointPolicy { rows: 4, source_bytes: 1000, elapsed: Duration::from_secs(3600) };
        let mut lines = crate::metadata_store::LineBuilder::default();
        let commits = std::cell::Cell::new(0);
        let observer = |p: &Progress| { if p.phase_id == "metadata-checkpoint-committed" { commits.set(commits.get() + 1); } };
        for i in 0..4 {
            lines.push(LineMeta { offset: i * 20, len: 19, ..Default::default() }).unwrap();
            journal.checkpoint_if_due(policy, &mut lines, (i as usize + 1) * 20, false, None, Some(&observer), &|| Ok(())).unwrap();
            if i < 3 {
                assert!(!dir.path().join(format!("{key}.state")).exists());
                assert_eq!(journal.sealed_rows, 0);
                assert_eq!(journal.committed_cursor, 0);
            }
        }
        assert_eq!(commits.get(), 1);
        assert_eq!(journal.sealed_rows, 4);
        assert_eq!(journal.committed_cursor, 80);
        lines.push(LineMeta { offset: 80, len: 19, ..Default::default() }).unwrap();
        journal.checkpoint_if_due(policy, &mut lines, 100, true, None, Some(&observer), &|| Ok(())).unwrap();
        journal.checkpoint_if_due(policy, &mut lines, 100, true, Some(&[]), Some(&observer), &|| Ok(())).unwrap();
        assert_eq!(commits.get(), 3, "EOF and columns remain separate commit points");
        drop(journal);
        let (_, restored) = Journal::open(dir.path(), &key, 100, 0, false, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(restored.resumed_rows, 5);
        assert_eq!(restored.cursor, 100);
        assert!(restored.scan_complete && restored.columns.is_some());
        for i in 0..5 { assert_eq!(restored.lines.at(i).unwrap().offset, i as u64 * 20); }
    }

    #[test]
    fn resumed_schedule_counts_bytes_after_the_committed_tail_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let key = "e".repeat(64);
        let policy = CheckpointPolicy { rows: 4, source_bytes: 100, elapsed: Duration::from_secs(3600) };
        let (mut journal, _) = Journal::open(dir.path(), &key, 300, 0, true, None).unwrap_or_else(|e| panic!("{e}"));
        let mut lines = crate::metadata_store::LineBuilder::default();
        lines.push(LineMeta { len: 99, ..Default::default() }).unwrap();
        journal.checkpoint_if_due(policy, &mut lines, 100, false, None, None, &|| Ok(())).unwrap();
        assert_eq!(journal.sealed_rows, 0);
        drop(journal);
        let (mut journal, mut resume) = Journal::open(dir.path(), &key, 300, 0, true, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(resume.cursor, 100);
        assert_eq!(resume.lines.len(), 1);
        resume.lines.last_mut().unwrap().len = 149;
        journal.checkpoint_if_due(policy, &mut resume.lines, 150, false, None, None, &|| Ok(())).unwrap();
        assert_eq!(journal.committed_cursor, 100, "elapsed/byte budget restarts from durable cursor");
        drop(journal);
        let (_, restored) = Journal::open(dir.path(), &key, 300, 0, true, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(restored.cursor, 100);
        assert_eq!(restored.lines.at(0).unwrap().len, 99, "uncommitted tail changes cannot leak into the manifest");
    }

    #[test]
    fn packed_codec_keeps_all_fields_and_rejects_invalid_bounds() {
        let m = LineMeta {
            offset: 71,
            len: 103,
            ts: -987_654_321,
            level: 6,
            code_off: 17,
            code_len: 31,
        };
        let mut bytes = Vec::new();
        encode_record(&m, &mut bytes);
        assert_eq!(bytes.len(), 27);
        let restored = decode_record(&bytes).unwrap();
        let mut again = Vec::new();
        encode_record(&restored, &mut again);
        assert_eq!(again, bytes);
        assert!(valid_record(&m, 174, Some(70)));
        assert!(!valid_record(&m, 173, None));
        assert!(!valid_record(&m, 174, Some(72)));
        assert!(!valid_record(&LineMeta { code_off: 100, ..m }, 174, None));
        assert!(!valid_record(
            &LineMeta {
                offset: u64::MAX,
                ..m
            },
            u64::MAX,
            None
        ));
    }

    #[test]
    fn marker_covers_tail_cursor_and_parser_generation_key() {
        let dir = tempfile::tempdir().unwrap();
        let key = "a".repeat(64);
        let (mut journal, seed) =
            Journal::open(dir.path(), &key, 300, 0, true, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(seed.lines.is_empty());
        let m = LineMeta {
            offset: 0,
            len: 11,
            ts: 123,
            ..Default::default()
        };
        journal
            .checkpoint(&[m], 12, false, None, None, &|| Ok(()))
            .unwrap();
        drop(journal);
        let (_, restored) =
            Journal::open(dir.path(), &key, 300, 0, true, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(restored.resumed_rows, 0);
        assert_eq!(restored.lines.len(), 1);
        assert_eq!(restored.lines.at(0).unwrap().ts, 123);
        let path = dir.path().join(format!("{key}.state"));
        let mut envelope: Envelope =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        envelope.state.tail.as_mut().unwrap().len = 12;
        std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        let (_, invalid) =
            Journal::open(dir.path(), &key, 300, 0, true, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(invalid.lines.is_empty());
    }

    #[test]
    fn valid_checksum_does_not_make_out_of_bounds_or_wrong_version_usable() {
        let dir = tempfile::tempdir().unwrap();
        let key = "b".repeat(64);
        for (version, cursor, tail_end) in
            [(VERSION + 1, 20, 10), (VERSION, 301, 10), (VERSION, 20, 21)]
        {
            let (mut journal, _) = Journal::open(dir.path(), &key, 300, 0, true, None)
                .unwrap_or_else(|e| panic!("{e}"));
            journal
                .checkpoint(
                    &[LineMeta {
                        len: 10,
                        ..Default::default()
                    }],
                    20,
                    false,
                    None,
                    None,
                    &|| Ok(()),
                )
                .unwrap();
            drop(journal);
            let path = dir.path().join(format!("{key}.state"));
            let mut envelope: Envelope =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            envelope.state.version = version;
            envelope.state.cursor = cursor;
            envelope.state.tail.as_mut().unwrap().len = tail_end;
            envelope.sha256 = state_digest(&envelope.state).unwrap();
            std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
            let (_, restored) = Journal::open(dir.path(), &key, 300, 0, true, None)
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(restored.lines.is_empty());
        }
    }

    #[test]
    fn complete_mapping_is_remapped_verified_and_protected_from_pruning() {
        let dir = tempfile::tempdir().unwrap(); let key = "c".repeat(64);
        let (mut journal, _) = Journal::open(dir.path(), &key, 300, 0, false, None).unwrap_or_else(|e| panic!("{e}"));
        journal.checkpoint(&[LineMeta { offset: 0, len: 11, ts: 123, ..Default::default() }, LineMeta { offset: 12, len: 10, ts: -7, ..Default::default() }], 300, true, Some(&["field".into()]), None, &|| Ok(())).unwrap();
        drop(journal);
        crate::cache_validation::settle_filesystem_clock();
        let (first, columns) = open_complete(dir.path(), &key, 300, 0, false, None).unwrap().unwrap();
        let phases = std::cell::RefCell::new(Vec::new());
        let (second, _) = open_complete(dir.path(), &key, 300, 0, false, Some(&|progress| phases.borrow_mut().push(progress.phase_id))).unwrap().unwrap();
        assert_eq!(*phases.borrow(), ["metadata-map-reuse"], "completed metadata must reopen without a per-row validation pass");
        assert_eq!(first.resident_rows(), 0); assert_eq!(first.at(1).ts, -7); assert_eq!(columns, ["field"]);
        let mut mapped_rows = Vec::new();
        for row in first.iter() { encode_record(&row, &mut mapped_rows); }
        assert_eq!(mapped_rows, std::fs::read(dir.path().join(format!("{key}.lines"))).unwrap()[HEADER_BYTES..], "remapping must preserve every metadata field");
        let writer = OpenOptions::new().read(true).write(true).open(dir.path().join(format!("{key}.lock"))).unwrap();
        assert!(FileExt::try_lock_exclusive(&writer).is_err());
        prune(dir.path(), std::time::SystemTime::now() + Duration::from_secs(60));
        assert!(dir.path().join(format!("{key}.lines")).exists());
        drop(first); drop(second);
        FileExt::try_lock_exclusive(&writer).unwrap(); FileExt::unlock(&writer).unwrap();
        let path = dir.path().join(format!("{key}.lines"));
        let mut bytes = std::fs::read(&path).unwrap(); bytes[HEADER_BYTES + 12] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert!(open_complete(dir.path(), &key, 300, 0, false, None).unwrap().is_none(), "full payload checksum must reject changed timestamps");
    }
    #[test]
    fn shared_complete_read_retains_tail_and_exclusive_resume_truncates_it() {
        let dir = tempfile::tempdir().unwrap(); let key = "e".repeat(64);
        let (mut journal, _) = Journal::open(dir.path(), &key, 300, 0, false, None).unwrap_or_else(|e| panic!("{e}"));
        journal.checkpoint(&[LineMeta { offset: 0, len: 11, ts: 123, ..Default::default() }, LineMeta { offset: 12, len: 10, ts: -7, ..Default::default() }], 300, true, Some(&["field".into()]), None, &|| Ok(())).unwrap();
        drop(journal);
        let path = dir.path().join(format!("{key}.lines"));
        let committed = std::fs::read(&path).unwrap();
        OpenOptions::new().append(true).open(&path).unwrap().write_all(b"unfinished").unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let (mapped, _) = open_complete(dir.path(), &key, 300, 0, false, None).unwrap().unwrap();
        assert_eq!(mapped.len(), 2); assert_eq!(mapped.at(1).ts, -7);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), committed.len() as u64 + 10);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified);
        let writer = OpenOptions::new().read(true).write(true).open(dir.path().join(format!("{key}.lock"))).unwrap();
        assert!(FileExt::try_lock_exclusive(&writer).is_err());
        drop(mapped); drop(writer);
        let (_writer, resumed) = Journal::open(dir.path(), &key, 300, 0, false, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(resumed.resumed_rows, 2);
        assert_eq!(std::fs::read(&path).unwrap(), committed);
    }
    #[test]
    fn incomplete_journal_never_publishes_a_mapped_index() {
        let dir = tempfile::tempdir().unwrap(); let key = "d".repeat(64);
        let (mut journal, _) = Journal::open(dir.path(), &key, 300, 0, false, None).unwrap_or_else(|e| panic!("{e}"));
        journal.checkpoint(&[LineMeta { len: 11, ..Default::default() }], 12, false, None, None, &|| Ok(())).unwrap();
        drop(journal);
        assert!(open_complete(dir.path(), &key, 300, 0, false, None).unwrap().is_none());
    }

}

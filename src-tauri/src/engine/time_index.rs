//! Optional exact timestamp summaries beside immutable query checkpoints.
//! Seven disjoint sorted timestamp arrays support rank queries without loading
//! all timestamps into RAM. Missing or invalid sidecars never imply no matches.
use crate::model::{class_label, label_class};
use crate::query::{build_stats, sort_levels, stats_layout, PreparedFilter, Stats};
use duckdb::arrow::array::{Array, Int64Array};
use duckdb::Connection;
use fs2::FileExt;
use memmap2::Mmap;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"LITIME01";
const LEVELS: usize = 7;
const HEADER: usize = 8 + 32 + 8 + LEVELS * 16;
const CHECKSUM: usize = 32;
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    manifest: [u8; 32],
    rows: u64,
}

pub(crate) fn path(store: &Path) -> PathBuf {
    store.with_extension("time-v1")
}
fn lock_path(store: &Path) -> PathBuf {
    store.with_extension("time.lock")
}

/// The caller first validates the ordinary checkpoint. This digest ties the
/// auxiliary data to its exact publication without changing the old manifest.
pub(crate) fn identity(store: &Path, expected_rows: usize) -> Result<Identity> {
    let manifest = store.with_extension("complete.json");
    let mut file = File::open(manifest).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Manifesto do checkpoint excedeu o limite.".into());
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if value["rows"].as_u64() != Some(expected_rows as u64)
        || value["key"].as_str() != store.file_stem().and_then(|name| name.to_str())
    {
        return Err("Resumo temporal não corresponde ao checkpoint.".into());
    }
    Ok(Identity {
        manifest: Sha256::digest(&bytes).into(),
        rows: expected_rows as u64,
    })
}

struct Data {
    file: File,
    mmap: Mmap,
    modified: Option<std::time::SystemTime>,
    totals: [u64; LEVELS],
    offsets: [usize; LEVELS + 1],
}

pub(crate) struct TimeIndex {
    data: Data,
    _lease: File,
    // Pin the exact main checkpoint publication as well as the sidecar. A
    // Registry cache can then hand this Arc to sessions without filesystem I/O.
    _checkpoint_lease: File,
}

pub(crate) type ReadSet = Arc<[Arc<TimeIndex>]>;

/// Registry-owned handles, verified only by background work. Collecting a
/// capability performs no filesystem I/O and never returns a partial part set.
#[derive(Default)]
pub(crate) struct VerifiedCache {
    entries: std::collections::HashMap<String, Arc<TimeIndex>>,
}
impl VerifiedCache {
    pub(crate) fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }
    pub(crate) fn insert(&mut self, key: String, index: Arc<TimeIndex>) {
        self.entries.insert(key, index);
    }
    pub(crate) fn complete<'a>(&self, keys: impl IntoIterator<Item = &'a str>) -> Option<ReadSet> {
        let handles: Option<Vec<_>> = keys
            .into_iter()
            .map(|key| self.entries.get(key).cloned())
            .collect();
        let handles = handles?;
        (!handles.is_empty()).then(|| handles.into())
    }
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&String) -> bool) {
        self.entries.retain(|key, _| keep(key));
    }
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(
        bytes[at..at + 8]
            .try_into()
            .expect("validated fixed-size field"),
    )
}

fn load(
    store: &Path,
    identity: &Identity,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Data>> {
    check(cancelled)?;
    let file = match File::open(path(store)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let maximum = identity
        .rows
        .checked_mul(8)
        .and_then(|n| n.checked_add((HEADER + CHECKSUM) as u64))
        .ok_or("Tamanho inválido do resumo temporal.")?;
    if metadata.len() < (HEADER + CHECKSUM) as u64 || metadata.len() > maximum {
        return Err("Resumo temporal incompleto ou com tamanho inválido.".into());
    }
    // The stable shared/exclusive sidecar lease prevents our writers from
    // replacing or truncating the mapped file for the lifetime of this reader.
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| e.to_string())?;
    if &mmap[..8] != MAGIC
        || mmap[8..40] != identity.manifest
        || read_u64(&mmap, 40) != identity.rows
    {
        return Err("Identidade ou versão do resumo temporal inválida.".into());
    }
    let end = mmap.len() - CHECKSUM;
    let mut hash = Sha256::new();
    for bytes in mmap[..end].chunks(64 << 10) {
        check(cancelled)?;
        hash.update(bytes);
    }
    let digest: [u8; CHECKSUM] = hash.finalize().into();
    if mmap[end..] != digest {
        return Err("Checksum do resumo temporal inválido.".into());
    }
    let mut totals = [0u64; LEVELS];
    let mut offsets = [HEADER; LEVELS + 1];
    let mut total = 0u64;
    for level in 0..LEVELS {
        totals[level] = read_u64(&mmap, 48 + level * 8);
        total = total
            .checked_add(totals[level])
            .ok_or("Contagem temporal inválida.")?;
        let timed = read_u64(&mmap, 48 + LEVELS * 8 + level * 8);
        if timed > totals[level] {
            return Err("Contagem temporal inválida.".into());
        }
        let bytes = usize::try_from(timed)
            .ok()
            .and_then(|n| n.checked_mul(8))
            .ok_or("Tamanho temporal inválido.")?;
        offsets[level + 1] = offsets[level]
            .checked_add(bytes)
            .ok_or("Tamanho temporal inválido.")?;
        if offsets[level + 1] > end {
            return Err("Partição temporal incompleta.".into());
        }
        let mut previous = None;
        for (row, bytes) in mmap[offsets[level]..offsets[level + 1]]
            .chunks_exact(8)
            .enumerate()
        {
            if row % 8192 == 0 {
                check(cancelled)?;
            }
            let value = i64::from_le_bytes(bytes.try_into().expect("eight bytes"));
            if value == 0 || previous.is_some_and(|old| old > value) {
                return Err("Partição temporal fora de ordem.".into());
            }
            previous = Some(value);
        }
    }
    if total != identity.rows || offsets[LEVELS] != end {
        return Err("Contagem temporal não corresponde ao checkpoint.".into());
    }
    check(cancelled)?;
    // Verification visits every byte once. Remap afterwards so retaining many
    // segment readers does not also retain every verified page in this process's
    // resident mapping; query ranks fault in only the pages they actually use.
    // The OS file cache remains reclaimable and is not a separate RAM guarantee.
    drop(mmap);
    let current = file.metadata().map_err(|e| e.to_string())?;
    if current.len() != metadata.len() || current.modified().ok() != metadata.modified().ok() {
        return Err("Resumo temporal mudou durante a validação.".into());
    }
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| e.to_string())?;
    Ok(Some(Data {
        file,
        mmap,
        modified: metadata.modified().ok(),
        totals,
        offsets,
    }))
}

fn lock_busy(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}
fn shared_lease(path: &Path) -> Result<Option<File>> {
    let lease = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    match FileExt::try_lock_shared(&lease) {
        Ok(()) => Ok(Some(lease)),
        Err(e) if lock_busy(&e) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn open(store: &Path, identity: &Identity) -> Result<Option<Arc<TimeIndex>>> {
    open_cancellable(store, identity, &|| false)
}

/// Bulk verification belongs on the single background worker, never inside a
/// Registry lock or the first foreground count/statistics request.
pub(crate) fn open_cancellable(
    store: &Path,
    expected: &Identity,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Arc<TimeIndex>>> {
    check(cancelled)?;
    // Global order: base publication lease, then auxiliary publication lease.
    let Some(checkpoint) = shared_lease(&store.with_extension("build.lock"))? else {
        return Ok(None);
    };
    if identity(store, expected.rows as usize)? != *expected {
        return Err("Checkpoint temporal mudou antes da leitura.".into());
    }
    let Some(lease) = shared_lease(&lock_path(store))? else {
        return Ok(None);
    };
    Ok(load(store, expected, cancelled)?.map(|data| {
        Arc::new(TimeIndex {
            data,
            _lease: lease,
            _checkpoint_lease: checkpoint,
        })
    }))
}

/// The caller already owns the base build.lock exclusively. Stable lock files
/// are retained; live readers always hold both leases and prevent deletion.
pub(crate) fn remove_under_build_lock(store: &Path) -> Result<()> {
    if !path(store).try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    let lease = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path(store))
        .map_err(|e| e.to_string())?;
    FileExt::try_lock_exclusive(&lease).map_err(|e| format!("Resumo temporal em uso: {e}"))?;
    match std::fs::remove_file(path(store)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Build only this auxiliary file from a validated main.ev read-only store.
/// The caller supplies an already budgeted DuckDB connection. Sorting may use
/// its bounded spill space; Rust retains only an Arrow batch and a 64 KiB buffer.
pub(crate) fn ensure(
    conn: &Connection,
    store: &Path,
    identity: &Identity,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<bool> {
    check(cancelled)?;
    if let Ok(Some(_)) = open_cancellable(store, identity, cancelled) {
        check(cancelled)?;
        return Ok(false);
    }
    let _checkpoint =
        shared_lease(&store.with_extension("build.lock"))?.ok_or("Checkpoint temporal ocupado.")?;
    if self::identity(store, identity.rows as usize)? != *identity {
        return Err("Checkpoint mudou antes da preparação temporal.".into());
    }
    let lease = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path(store))
        .map_err(|e| e.to_string())?;
    lease
        .try_lock_exclusive()
        .map_err(|e| format!("Resumo temporal ocupado: {e}"))?;
    // Do not recursively acquire a shared lease while holding the writer lease.
    if let Ok(Some(_)) = load(store, identity, cancelled) {
        return Ok(false);
    }
    check(cancelled)?;
    let parent = store.parent().ok_or("Checkpoint sem diretório.")?;
    let key = store
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Checkpoint sem chave.")?;
    let temporary = tempfile::Builder::new()
        .prefix(&format!("{key}.time."))
        .suffix(".pending")
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    let mut file = std::io::BufWriter::with_capacity(64 << 10, temporary.as_file());
    file.write_all(&[0; HEADER]).map_err(|e| e.to_string())?;
    let mut totals = [0u64; LEVELS];
    let mut timed = [0u64; LEVELS];
    let mut observed = 0u64;
    let interrupt = conn.interrupt_handle();
    std::thread::scope(|threads| -> Result<()> {
        let (stop, finished) = std::sync::mpsc::channel::<()>();
        threads.spawn(move || loop {
            match finished.recv_timeout(std::time::Duration::from_millis(25)) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if cancelled() => {
                    interrupt.interrupt();
                    break;
                }
                Err(_) => {}
            }
        });
        let result = (|| {
            let mut statement = conn
                .prepare("SELECT lvl::BIGINT, ts FROM main.ev ORDER BY lvl, ts NULLS FIRST")
                .map_err(|e| e.to_string())?;
            drop(statement.stream_arrow([]).map_err(|e| e.to_string())?);
            while let Some(batch) = statement.step().map_err(|e| e.to_string())? {
                check(cancelled)?;
                let levels = batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or("Classe temporal inválida.")?;
                let times = batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or("Timestamp em formato inválido.")?;
                for row in 0..batch.len() {
                    if row % 8192 == 0 {
                        check(cancelled)?;
                    }
                    observed += 1;
                    if observed > identity.rows {
                        return Err("Fonte temporal excedeu o checkpoint validado.".into());
                    }
                    if levels.is_null(row) {
                        return Err("Classe temporal ausente.".into());
                    }
                    let level = usize::try_from(levels.value(row)).map_err(|e| e.to_string())?;
                    if level >= LEVELS {
                        return Err("Classe temporal fora do intervalo.".into());
                    }
                    totals[level] += 1;
                    if !times.is_null(row) && times.value(row) != 0 {
                        timed[level] += 1;
                        file.write_all(&times.value(row).to_le_bytes())
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
            check(cancelled)
        })();
        drop(stop);
        result
    })?;
    if totals.iter().sum::<u64>() != identity.rows {
        return Err("Fonte temporal mudou durante a preparação.".into());
    }
    file.flush().map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    file.write_all(MAGIC).map_err(|e| e.to_string())?;
    file.write_all(&identity.manifest)
        .map_err(|e| e.to_string())?;
    file.write_all(&identity.rows.to_le_bytes())
        .map_err(|e| e.to_string())?;
    for n in totals.iter().chain(timed.iter()) {
        file.write_all(&n.to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    let mut input = temporary.reopen().map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 << 10];
    loop {
        check(cancelled)?;
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    let mut output = temporary.as_file();
    output.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    output
        .write_all(&digest.finalize())
        .map_err(|e| e.to_string())?;
    output.sync_all().map_err(|e| e.to_string())?;
    // Recheck the publication, not just the source row count, before commit.
    if self::identity(store, identity.rows as usize)? != *identity {
        return Err("Checkpoint mudou durante a preparação temporal.".into());
    }
    check(cancelled)?;
    drop(input);
    temporary.persist(path(store)).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(true)
}

fn check(cancelled: &(dyn Fn() -> bool + Sync)) -> Result<()> {
    if cancelled() {
        Err("Operação cancelada.".into())
    } else {
        crate::operations::check()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Predicate {
    levels: u8,
    lower: i64,
    upper: i64,
    timed_only: bool,
}

/// Only proven conjunctions are answered here. Arbitrary expressions, aliases,
/// custom level labels and derived fields retain the canonical SQL route.
pub(crate) fn predicate(filters: &[PreparedFilter]) -> Option<Predicate> {
    const EXACT: f64 = 9_007_199_254_740_991.0;
    let mut out = Predicate {
        levels: (1 << LEVELS) - 1,
        lower: i64::MIN,
        upper: i64::MAX,
        timed_only: false,
    };
    for filter in filters {
        let f = &filter.f;
        match (f.column.as_str(), f.op.as_str()) {
            ("level", "equals_exact") => out.levels &= 1 << label_class(&f.value)?,
            ("timestamp", "gt" | "gte" | "lt" | "lte" | "between") => {
                out.timed_only = true;
                let (first, second) = filter.numeric_bounds();
                let Some(first) = first else {
                    out.levels = 0;
                    continue;
                };
                if !first.is_finite() || first.abs() > EXACT {
                    return None;
                }
                match f.op.as_str() {
                    "gt" => out.lower = out.lower.max(first.floor() as i64 + 1),
                    "gte" => out.lower = out.lower.max(first.ceil() as i64),
                    "lt" => out.upper = out.upper.min(first.ceil() as i64 - 1),
                    "lte" => out.upper = out.upper.min(first.floor() as i64),
                    "between" => {
                        let Some(second) = second else {
                            out.levels = 0;
                            continue;
                        };
                        if !second.is_finite() || second.abs() > EXACT {
                            return None;
                        }
                        out.lower = out.lower.max(first.ceil() as i64);
                        out.upper = out.upper.min(second.floor() as i64);
                    }
                    _ => unreachable!(),
                }
            }
            _ => return None,
        }
    }
    if out.lower > out.upper {
        out.levels = 0;
    }
    Some(out)
}

impl Data {
    fn unchanged(&self) -> bool {
        self.file
            .metadata()
            .is_ok_and(|m| m.len() == self.mmap.len() as u64 && m.modified().ok() == self.modified)
    }
    fn len(&self, level: usize) -> usize {
        (self.offsets[level + 1] - self.offsets[level]) / 8
    }
    fn value(&self, level: usize, at: usize) -> i64 {
        let start = self.offsets[level] + at * 8;
        i64::from_le_bytes(
            self.mmap[start..start + 8]
                .try_into()
                .expect("validated timestamp"),
        )
    }
    fn partition(&self, level: usize, mut matches: impl FnMut(i64) -> bool) -> usize {
        let (mut low, mut high) = (0, self.len(level));
        while low < high {
            let middle = low + (high - low) / 2;
            if matches(self.value(level, middle)) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low
    }
    fn range(&self, level: usize, predicate: &Predicate) -> std::ops::Range<usize> {
        self.partition(level, |t| t < predicate.lower)
            ..self.partition(level, |t| t <= predicate.upper)
    }
}

pub(crate) fn count(indexes: &[Arc<TimeIndex>], predicate: &Predicate) -> Result<Option<usize>> {
    if indexes.is_empty() {
        return Ok(None);
    }
    let mut total = 0u64;
    for index in indexes {
        crate::operations::check()?;
        if !index.data.unchanged() {
            return Ok(None);
        }
        for level in 0..LEVELS {
            if predicate.levels & (1 << level) == 0 {
                continue;
            }
            let n = if predicate.timed_only {
                index.data.range(level, predicate).len() as u64
            } else {
                index.data.totals[level]
            };
            total = total
                .checked_add(n)
                .ok_or("Contagem temporal excedeu o limite.")?;
        }
    }
    usize::try_from(total).map(Some).map_err(|e| e.to_string())
}

pub(crate) fn stats(indexes: &[Arc<TimeIndex>], predicate: &Predicate) -> Result<Option<Stats>> {
    if indexes.is_empty() {
        return Ok(None);
    }
    let mut levels = std::collections::HashMap::<&'static str, i64>::new();
    let (mut min, mut max) = (i64::MAX, i64::MIN);
    for index in indexes {
        crate::operations::check()?;
        if !index.data.unchanged() {
            return Ok(None);
        }
        for level in 0..LEVELS {
            if predicate.levels & (1 << level) == 0 {
                continue;
            }
            let range = index.data.range(level, predicate);
            let count = if predicate.timed_only {
                range.len() as u64
            } else {
                index.data.totals[level]
            };
            if count > 0 {
                let n = i64::try_from(count).map_err(|e| e.to_string())?;
                let entry = levels.entry(class_label(level as u8)).or_default();
                *entry = entry
                    .checked_add(n)
                    .ok_or("Contagem temporal excedeu o limite.")?;
            }
            if !range.is_empty() {
                min = min.min(index.data.value(level, range.start));
                max = max.max(index.data.value(level, range.end - 1));
            }
        }
    }
    let (mut buckets, mut width) = (Vec::new(), 0);
    if min <= max {
        let number;
        (width, number) = stats_layout(min, max);
        let mut counts = vec![0i64; number];
        for index in indexes {
            crate::operations::check()?;
            if !index.data.unchanged() {
                return Ok(None);
            }
            for level in 0..LEVELS {
                if predicate.levels & (1 << level) == 0 {
                    continue;
                }
                let range = index.data.range(level, predicate);
                let mut start = range.start;
                for (bucket, count) in counts.iter_mut().enumerate() {
                    let end = if bucket + 1 == number {
                        range.end
                    } else {
                        index
                            .data
                            .partition(level, |t| t.saturating_sub(min) / width <= bucket as i64)
                            .clamp(start, range.end)
                    };
                    *count = count
                        .checked_add((end - start) as i64)
                        .ok_or("Contagem temporal excedeu o limite.")?;
                    start = end;
                }
            }
        }
        buckets = counts
            .into_iter()
            .enumerate()
            .map(|(b, n)| (min.saturating_add((b as i64).saturating_mul(width)), n))
            .collect();
    }
    let mut levels: Vec<_> = levels
        .into_iter()
        .map(|(name, n)| (name.to_string(), n))
        .collect();
    sort_levels(&mut levels);
    Ok(Some(build_stats(buckets, width, levels)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Event;
    use crate::query::{prepare, Filter};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture(rows: &[(u8, Option<i64>)]) -> (tempfile::TempDir, Connection, PathBuf, Identity) {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("fixture.duckdb");
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("SET threads=1; SET memory_limit='32MB'; SET max_temp_directory_size='1MB'; CREATE TABLE ev(lvl UTINYINT,ts BIGINT)").unwrap();
        for &(level, time) in rows {
            connection
                .execute("INSERT INTO ev VALUES (?,?)", duckdb::params![level, time])
                .unwrap();
        }
        std::fs::write(
            store.with_extension("complete.json"),
            json!({"key":"fixture","rows":rows.len(),"version":5}).to_string(),
        )
        .unwrap();
        let identity = identity(&store, rows.len()).unwrap();
        (directory, connection, store, identity)
    }

    fn filters(value: serde_json::Value) -> Vec<PreparedFilter> {
        prepare(&serde_json::from_value::<Vec<Filter>>(value).unwrap())
    }

    fn reference(rows: &[(u8, Option<i64>)], filters: &[PreparedFilter]) -> (usize, Stats) {
        let events: Vec<_> = rows
            .iter()
            .filter_map(|&(level, time)| {
                let mut event = Event::empty();
                event.timestamp = time.filter(|&t| t != 0);
                event.level = if level == 6 {
                    "custom level".into()
                } else {
                    class_label(level).into()
                };
                if !filters
                    .iter()
                    .all(|filter| crate::query::matches(&event, filter))
                {
                    return None;
                }
                // Indexed recovery intentionally collapses OTHER into Informação.
                event.level = class_label(level).into();
                Some(event)
            })
            .collect();
        (events.len(), crate::query::stats(&events, &[]))
    }

    #[test]
    fn temporal_sidecar_exact_stats_match_recovery_for_levels_bounds_and_ties() {
        let rows = vec![
            (0, Some(100)),
            (1, Some(-100)),
            (2, None),
            (3, Some(0)),
            (4, Some(40)),
            (5, Some(100)),
            (6, Some(-100)),
            (0, None),
            (2, Some(i64::MAX)),
            (1, Some(i64::MIN)),
            (6, Some(40)),
            (2, Some(100)),
            (3, Some(-1)),
            (4, Some(1)),
        ];
        let (directory, connection, store, identity) = fixture(&rows);
        assert!(ensure(&connection, &store, &identity, &|| false).unwrap());
        let index = open(&store, &identity).unwrap().unwrap();
        let cases = [
            json!([]),
            json!([{"column":"level","op":"equals_exact","value":"Informação"}]),
            json!([{"column":"level","op":"equals_exact","value":"Erro"}]),
            json!([{"column":"timestamp","op":"gt","value":"0"}]),
            json!([{"column":"timestamp","op":"gte","value":"-100"}]),
            json!([{"column":"timestamp","op":"lt","value":"0"}]),
            json!([{"column":"timestamp","op":"lte","value":"100"}]),
            json!([{"column":"timestamp","op":"between","value":"-100","value2":"100"}]),
            json!([{"column":"timestamp","op":"between","value":"100","value2":"-100"}]),
            json!([{"column":"timestamp","op":"gte","value":"0.5"},{"column":"timestamp","op":"lt","value":"100.5"}]),
            json!([{"column":"timestamp","op":"gte","value":"-99.5"},{"column":"level","op":"equals_exact","value":"Aviso"}]),
            json!([{"column":"level","op":"equals_exact","value":"Erro"},{"column":"level","op":"equals_exact","value":"Aviso"}]),
            json!([{"column":"timestamp","op":"gt","value":"invalid"}]),
        ];
        for case in cases {
            let prepared = filters(case.clone());
            let request = predicate(&prepared).unwrap();
            let expected = reference(&rows, &prepared);
            assert_eq!(
                count(&[Arc::clone(&index)], &request).unwrap(),
                Some(expected.0),
                "{case}"
            );
            assert_eq!(
                serde_json::to_value(stats(&[Arc::clone(&index)], &request).unwrap().unwrap())
                    .unwrap(),
                serde_json::to_value(expected.1).unwrap(),
                "{case}"
            );
        }
        let before = std::fs::metadata(path(&store)).unwrap().modified().unwrap();
        assert!(!ensure(&connection, &store, &identity, &|| false).unwrap());
        assert_eq!(
            std::fs::metadata(path(&store)).unwrap().modified().unwrap(),
            before
        );
        assert!(
            std::fs::metadata(path(&store)).unwrap().len()
                <= (HEADER + CHECKSUM + rows.len() * 8) as u64
        );
        drop(index);
        drop(directory);
    }

    #[test]
    fn temporal_sidecar_merges_parts_without_approximating_histogram_boundaries() {
        let first = [(0, Some(5)), (2, Some(500)), (6, None)];
        let second = [(1, Some(-500)), (0, Some(5)), (3, Some(0)), (6, Some(101))];
        let (_one, conn_a, store_a, identity_a) = fixture(&first);
        let (_two, conn_b, store_b, identity_b) = fixture(&second);
        ensure(&conn_a, &store_a, &identity_a, &|| false).unwrap();
        ensure(&conn_b, &store_b, &identity_b, &|| false).unwrap();
        let indexes = [
            open(&store_a, &identity_a).unwrap().unwrap(),
            open(&store_b, &identity_b).unwrap().unwrap(),
        ];
        let rows: Vec<_> = first.into_iter().chain(second).collect();
        let request = predicate(&[]).unwrap();
        let expected = reference(&rows, &[]);
        assert_eq!(count(&indexes, &request).unwrap(), Some(expected.0));
        assert_eq!(
            serde_json::to_value(stats(&indexes, &request).unwrap().unwrap()).unwrap(),
            serde_json::to_value(expected.1).unwrap()
        );
    }

    #[test]
    fn temporal_sidecar_empty_and_untimed_records_keep_exact_level_totals() {
        for rows in [Vec::new(), vec![(0, None), (1, Some(0)), (6, None)]] {
            let (_directory, connection, store, identity) = fixture(&rows);
            ensure(&connection, &store, &identity, &|| false).unwrap();
            let index = open(&store, &identity).unwrap().unwrap();
            for prepared in [
                filters(json!([])),
                filters(json!([{"column":"timestamp","op":"gte","value":"-100"}])),
            ] {
                let expected = reference(&rows, &prepared);
                let request = predicate(&prepared).unwrap();
                assert_eq!(
                    count(&[Arc::clone(&index)], &request).unwrap(),
                    Some(expected.0)
                );
                assert_eq!(
                    serde_json::to_value(stats(&[Arc::clone(&index)], &request).unwrap().unwrap())
                        .unwrap(),
                    serde_json::to_value(expected.1).unwrap()
                );
            }
        }
    }

    #[test]
    fn temporal_sidecar_declines_unproven_filters() {
        for case in [
            json!([{"column":"level","op":"equals_exact","value":"custom level"}]),
            json!([{"column":"level","op":"contains","value":"Erro"}]),
            json!([{"column":"source","op":"equals_exact","value":"api"}]),
            json!([{"column":"_all","op":"query","value":"timestamp:>100"}]),
            json!([{"column":"timestamp","op":"gte","value":"1e100"}]),
        ] {
            assert!(predicate(&filters(case.clone())).is_none(), "{case}");
        }
    }

    #[test]
    fn temporal_sidecar_detects_corruption_missing_and_changed_publications() {
        let (_directory, connection, store, identity) = fixture(&[(0, Some(5)), (2, Some(-1))]);
        assert!(open(&store, &identity).unwrap().is_none());
        ensure(&connection, &store, &identity, &|| false).unwrap();
        let original = std::fs::read(path(&store)).unwrap();
        for position in [0, 8, 40, 48, HEADER, original.len() - 1] {
            let mut changed = original.clone();
            changed[position] ^= 1;
            std::fs::write(path(&store), changed).unwrap();
            assert!(open(&store, &identity).is_err(), "corruption at {position}");
        }
        std::fs::write(path(&store), &original[..original.len() - 1]).unwrap();
        assert!(open(&store, &identity).is_err());
        assert!(ensure(&connection, &store, &identity, &|| false).unwrap());
        std::fs::write(
            store.with_extension("complete.json"),
            json!({"key":"fixture","rows":2,"version":6}).to_string(),
        )
        .unwrap();
        let replacement = self::identity(&store, 2).unwrap();
        assert!(open(&store, &replacement).is_err());
        assert!(ensure(&connection, &store, &replacement, &|| false).unwrap());
    }

    #[test]
    fn temporal_sidecar_cancellation_and_wrong_row_count_do_not_publish_partial_data() {
        let (_directory, connection, store, identity) = fixture(&[(0, Some(5)), (2, Some(-1))]);
        let calls = AtomicUsize::new(0);
        assert!(ensure(&connection, &store, &identity, &|| calls
            .fetch_add(1, Ordering::SeqCst)
            >= 3)
        .is_err());
        assert!(!path(&store).exists());
        connection
            .execute("INSERT INTO ev VALUES (0,42)", [])
            .unwrap();
        assert!(ensure(&connection, &store, &identity, &|| false).is_err());
        assert!(!path(&store).exists());
        connection
            .execute("DELETE FROM ev WHERE ts=42", [])
            .unwrap();
        assert!(ensure(&connection, &store, &identity, &|| false).unwrap());
    }

    #[test]
    fn temporal_sidecar_rechecks_manifest_before_publication() {
        let (_directory, connection, store, identity) = fixture(&[(0, Some(5))]);
        std::fs::write(
            store.with_extension("complete.json"),
            json!({"key":"fixture","rows":1,"version":6}).to_string(),
        )
        .unwrap();
        assert!(ensure(&connection, &store, &identity, &|| false).is_err());
        assert!(!path(&store).exists());
    }

    #[test]
    fn temporal_sidecar_reader_lease_and_file_length_prevent_unsafe_reuse() {
        let (_directory, connection, store, identity) = fixture(&[(0, Some(5))]);
        ensure(&connection, &store, &identity, &|| false).unwrap();
        let index = open(&store, &identity).unwrap().unwrap();
        let writer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path(&store))
            .unwrap();
        assert!(writer.try_lock_exclusive().is_err());
        // Unix permits non-cooperating external truncation of an active mmap;
        // Windows itself refuses truncation of an actively mapped file.
        #[cfg(unix)]
        {
            OpenOptions::new()
                .write(true)
                .open(path(&store))
                .unwrap()
                .set_len(0)
                .unwrap();
            let request = predicate(&[]).unwrap();
            assert_eq!(count(&[Arc::clone(&index)], &request).unwrap(), None);
            assert!(stats(&[index], &request).unwrap().is_none());
        }
    }
    #[test]
    fn verified_handles_pin_base_and_sidecar_and_cleanup_releases_both() {
        let (_directory, connection, store, identity) = fixture(&[(0, Some(5)), (2, Some(-1))]);
        ensure(&connection, &store, &identity, &|| false).unwrap();
        let index = open(&store, &identity).unwrap().unwrap();
        let base = OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.with_extension("build.lock"))
            .unwrap();
        assert!(FileExt::try_lock_exclusive(&base).is_err());
        assert!(remove_under_build_lock(&store).is_err());
        assert!(path(&store).exists());
        drop(index);
        FileExt::try_lock_exclusive(&base).unwrap();
        remove_under_build_lock(&store).unwrap();
        assert!(!path(&store).exists());
        assert!(lock_path(&store).exists());
        assert!(store.with_extension("build.lock").exists());
    }

    #[test]
    fn cancelling_verification_releases_its_publication_leases() {
        let (_directory, connection, store, identity) = fixture(&[(0, Some(5)), (2, Some(-1))]);
        ensure(&connection, &store, &identity, &|| false).unwrap();
        let calls = AtomicUsize::new(0);
        assert!(
            open_cancellable(&store, &identity, &|| calls.fetch_add(1, Ordering::SeqCst)
                >= 2)
            .is_err()
        );
        for path in [store.with_extension("build.lock"), lock_path(&store)] {
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .unwrap();
            FileExt::try_lock_exclusive(&lock).unwrap();
        }
    }

    #[test]
    fn lock_contention_is_optional_on_every_supported_platform() {
        assert!(lock_busy(&fs2::lock_contended_error()));
        let (_directory, _connection, store, identity) = fixture(&[(0, Some(5))]);
        let base = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(store.with_extension("build.lock"))
            .unwrap();
        FileExt::try_lock_exclusive(&base).unwrap();
        assert!(open(&store, &identity).unwrap().is_none());
    }
    #[test]
    fn verified_capability_requires_every_part_and_eviction_releases_old_owners() {
        let (_one, a, path_a, identity_a) = fixture(&[(0, Some(5))]);
        let (_two, b, path_b, identity_b) = fixture(&[(2, Some(7))]);
        ensure(&a, &path_a, &identity_a, &|| false).unwrap();
        let mut cache = VerifiedCache::default();
        cache.insert("a".into(), open(&path_a, &identity_a).unwrap().unwrap());
        assert!(cache.complete(["a", "b"]).is_none());
        ensure(&b, &path_b, &identity_b, &|| false).unwrap();
        let primary_before = std::fs::read(path_b.with_extension("complete.json")).unwrap();
        let mut bytes = std::fs::read(path(&path_b)).unwrap();
        bytes[48] ^= 1;
        std::fs::write(path(&path_b), bytes).unwrap();
        assert!(open(&path_b, &identity_b).is_err());
        assert_eq!(
            std::fs::read(path_b.with_extension("complete.json")).unwrap(),
            primary_before
        );
        assert!(cache.complete(["a", "b"]).is_none());
        ensure(&b, &path_b, &identity_b, &|| false).unwrap();
        cache.insert("b".into(), open(&path_b, &identity_b).unwrap().unwrap());
        let readers = cache.complete(["a", "b"]).unwrap();
        assert_eq!(count(&readers, &predicate(&[]).unwrap()).unwrap(), Some(2));
        cache.retain(|key| key == "a");
        assert!(cache.complete(["a", "b"]).is_none());
        let base = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path_b.with_extension("build.lock"))
            .unwrap();
        assert!(
            FileExt::try_lock_exclusive(&base).is_err(),
            "active reader set owns its leases after cache eviction"
        );
        drop(readers);
        FileExt::try_lock_exclusive(&base).unwrap();
    }
}

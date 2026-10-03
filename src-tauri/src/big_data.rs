//! Embedded, immutable search acceleration. Every result is a candidate: the
//! application's existing predicates remain authoritative after this index.
#[path = "big_data_io.rs"]
mod io_diagnostics;

use crate::model::{CodesConfig, Event, STANDARD_COLUMNS};
use crate::sources::{CompiledDerived, CustomParse, FileIndex};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::ops::Bound;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tantivy::collector::{Collector, Count, SegmentCollector};
use tantivy::columnar::{Column, StrColumn};
use tantivy::query::{
    AllQuery, BooleanQuery, EmptyQuery, InvertedIndexRangeQuery, Occur, Query, RangeQuery,
    TermQuery,
};
use tantivy::schema::{
    BytesOptions, Field, IndexRecordOption, Schema, TantivyDocument, TextFieldIndexing,
    TextOptions, Value as TantivyValue, FAST, INDEXED, STORED, STRING,
};
use tantivy::tokenizer::NgramTokenizer;
use tantivy::{DocAddress, DocId, Index, Score, Searcher, SegmentOrdinal, SegmentReader, Term};

const VERSION: u32 = 3;
const EXACT_HASH_BYTES: usize = 8;
const WRITER_BUDGET: usize = 128 * 1024 * 1024;
const PREPARE_BATCH_DOCS: usize = 256;
const PREPARE_BATCH_BYTES: usize = 2 * 1024 * 1024;
const POSTINGS_TIMESTAMP_SPAN_MS: i64 = 4096;
// Large values still have complete n-grams and a stored event. Whole-value
// equality falls back to all rows if the input exceeds this extraction bound.
const MAX_EXACT_TERM: usize = 16 * 1024;

/// Reclaim expired generations only at startup, before this process opens them.
/// Every recursive target is canonicalized and confined to the cache directory.
pub fn prune(dir: &Path) {
    let Ok(root) = dir.canonicalize() else { return };
    let now = std::time::SystemTime::now();
    let expired = now - std::time::Duration::from_secs(30 * 24 * 3600);
    let orphaned = now - std::time::Duration::from_secs(24 * 3600);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let entries: Vec<_> = entries.flatten().collect();
    let mut live = HashSet::new();
    for entry in &entries {
        if entry.path().extension().is_some_and(|e| e == "json") {
            let old = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .is_some_and(|t| t < expired);
            if old {
                let _ = std::fs::remove_file(entry.path());
                continue;
            }
            if let Ok(bytes) = std::fs::read(entry.path()) {
                if let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) {
                    if is_filename(&manifest.generation) {
                        live.insert(manifest.generation);
                    }
                }
            }
        }
    }
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("index-") && !name.ends_with(".pending") {
            continue;
        }
        if live.contains(&name) {
            continue;
        }
        let old = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|t| t < orphaned);
        if old {
            if let Ok(path) = entry.path().canonicalize() {
                if path != root && path.starts_with(&root) {
                    let _ = std::fs::remove_dir_all(path);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ExactMode {
    Sensitive,
    AsciiInsensitive,
    TrimLower,
}

#[derive(Clone, Debug)]
pub enum Predicate {
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Equals {
        column: Option<String>,
        value: String,
        mode: ExactMode,
    },
    Contains(String),
    Timestamp {
        lower: Option<(i64, bool)>,
        upper: Option<(i64, bool)>,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BigDataInfo {
    pub event_count: usize,
    pub index_bytes: u64,
    pub reused: bool,
    pub build_ms: u64,
    pub engine: String,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct CachedFile {
    name: String,
    bytes: u64,
    modified: String,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    fingerprint: String,
    generation: String,
    event_count: usize,
    index_bytes: u64,
    build_ms: u64,
    files: Vec<CachedFile>,
}

// Only removes this builder's newly created generation until its manifest is
// published. Previously published generations can still be mapped by readers.
struct UnpublishedGeneration {
    path: PathBuf,
    published: bool,
}
impl Drop for UnpublishedGeneration {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Clone, Copy)]
struct Fields {
    row: Field,
    timestamp: Field,
    grams: Field,
    exact: Field,
    event: Field,
    source: Field,
    code: Field,
    level: Field,
}

pub struct BigDataIndex {
    searcher: Searcher,
    addresses: Vec<DocAddress>,
    fast_strings: Vec<[Option<StrColumn>; 3]>,
    fields: Fields,
    info: BigDataInfo,
    resource_storage_path: PathBuf,
}

fn schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let row = b.add_u64_field("row", FAST);
    let timestamp = b.add_i64_field("timestamp", FAST | INDEXED);
    let options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("grams")
            .set_index_option(IndexRecordOption::Basic),
    );
    let grams = b.add_text_field("grams", options);
    // One shared binary dictionary avoids copying long values and repeating
    // identical normalization variants in three separate term dictionaries.
    // Fieldnorms/frequencies are unnecessary: every hit is verified below.
    let exact = b.add_bytes_field("exact_hash", BytesOptions::default().set_indexed());
    let event = b.add_bytes_field("event", STORED);
    let source = b.add_text_field("source", STRING | FAST);
    let code = b.add_text_field("code", STRING | FAST);
    let level = b.add_text_field("level", STRING | FAST);
    (
        b.build(),
        Fields {
            row,
            timestamp,
            grams,
            exact,
            event,
            source,
            code,
            level,
        },
    )
}

fn configure(index: &mut Index) -> Result<(), String> {
    index.tokenizers().register(
        "grams",
        NgramTokenizer::new(3, 3, false).map_err(|e| e.to_string())?,
    );
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(4));
    index
        .set_multithread_executor(threads)
        .map_err(|e| e.to_string())
}

fn digest_value(hash: &mut Sha256, value: &impl Serialize) {
    struct Sink<'a>(&'a mut Sha256);
    impl Write for Sink<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Sink(&mut *hash), value).expect("search fingerprint serializable");
    hash.update([0]);
}

fn digest_catalog(hash: &mut Sha256, catalog: &CodesConfig) {
    let mut sources: Vec<_> = catalog.sources.iter().collect();
    sources.sort_unstable_by(|a, b| a.0.cmp(b.0));
    for (source, codes) in sources {
        digest_value(hash, source);
        let mut codes: Vec<_> = codes.iter().collect();
        codes.sort_unstable_by(|a, b| a.0.cmp(b.0));
        for (code, info) in codes {
            digest_value(hash, &(code, info));
        }
    }
}

fn derived_uses_security_rules(derived: &[CompiledDerived]) -> bool {
    derived.iter().flat_map(|d| &d.rules).any(|r| {
        r.filter
            .as_ref()
            .is_some_and(|f| matches!(f.op.as_str(), "query" | "detection" | "threat_rule"))
    })
}

// Conditional derived fields can depend on these rules. Hash their contents
// once at preparation, so invalidating a reader cannot reopen a stale generation.
fn digest_security_dependencies(hash: &mut Sha256, root: &Path) {
    for text in [
        include_str!("../resources/detection-rules.json"),
        include_str!("../resources/detection-expansion.json"),
        include_str!("../resources/detection-advanced.json"),
        include_str!("../resources/detection-content.json"),
        include_str!("../resources/threat-rules.json"),
    ] {
        digest_value(hash, &text);
    }
    let mut paths: Vec<_> = [
        "detections.json",
        "detection-rules.json",
        "threat-rules.json",
    ]
    .into_iter()
    .map(|name| root.join(name))
    .collect();
    paths.extend(crate::sigma::rule_files(&root.join("sigma")));
    for path in paths {
        digest_value(
            hash,
            &path.strip_prefix(root).unwrap_or(&path).to_string_lossy(),
        );
        match std::fs::File::open(&path) {
            Ok(mut file) => {
                digest_value(hash, &file.metadata().map(|m| m.len()).unwrap_or(0));
                let mut buffer = [0u8; 32 * 1024];
                loop {
                    match file.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => hash.update(&buffer[..n]),
                        Err(error) => {
                            digest_value(hash, &format!("read:{:?}", error.kind()));
                            break;
                        }
                    }
                }
                hash.update([0]);
            }
            Err(error) => digest_value(hash, &format!("open:{:?}", error.kind())),
        }
    }
}

pub fn fingerprint(
    idx: &FileIndex,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> String {
    let mut hash = Sha256::new();
    digest_value(
        &mut hash,
        &(
            "big-data-files",
            VERSION,
            crate::index_cache::INDEX_DIR,
            chrono::Local::now().offset().to_string(),
        ),
    );
    for p in &idx.parts {
        digest_value(
            &mut hash,
            &(
                &p.identity,
                &p.path,
                &p.file_name,
                &p.format,
                &p.header,
                p.base,
                p.mmap.len(),
            ),
        );
        match &p.custom {
            Some(CustomParse::Regex(re)) => digest_value(&mut hash, &("regex", re.as_str())),
            Some(CustomParse::Delimited { sep, fields }) => {
                digest_value(&mut hash, &("delimited", sep, fields))
            }
            None => digest_value(&mut hash, &"default"),
        }
        if let Some(ts) = &p.ts_config {
            digest_value(
                &mut hash,
                &(
                    ts.timezone_offset_minutes,
                    ts.clock_adjustment_ms,
                    &ts.sources,
                    &ts.format,
                    &ts.complement,
                ),
            );
            for (re, template) in &ts.rules {
                digest_value(
                    &mut hash,
                    &(re.as_ref().map(regex::Regex::as_str), template),
                );
            }
        } else {
            digest_value(&mut hash, &"no-ts-config");
        }
    }
    for m in &idx.lines {
        hash.update(m.offset.to_le_bytes());
        hash.update(m.len.to_le_bytes());
        hash.update(m.ts.to_le_bytes());
        hash.update([m.level]);
        hash.update(m.code_off.to_le_bytes());
        hash.update(m.code_len.to_le_bytes());
    }
    digest_catalog(&mut hash, codes);
    digest_catalog(&mut hash, system);
    for d in derived {
        digest_value(&mut hash, &(&d.name, &d.source));
        for r in &d.rules {
            digest_value(&mut hash, &(r.re.as_str(), &r.template, &r.filter));
        }
    }
    if derived_uses_security_rules(derived) {
        digest_security_dependencies(&mut hash, &crate::config_dir());
    }
    format!("{:x}", hash.finalize())
}

pub fn fingerprint_events(events: &[Event]) -> String {
    let mut hash = Sha256::new();
    digest_value(&mut hash, &("big-data-events", VERSION, events.len()));
    for event in events {
        digest_value(&mut hash, event);
    }
    format!("{:x}", hash.finalize())
}

// Scoped and global values have separate domains; byte lengths make arbitrary
// field names and delimiters unambiguous before hashing. A hash collision can
// only add a candidate because the original matcher remains authoritative.
fn exact_term(column: Option<&str>, value: &str) -> Option<[u8; EXACT_HASH_BYTES]> {
    if value.len().saturating_add(column.map_or(0, str::len)) > MAX_EXACT_TERM {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(b"loginsight-exact-v2\0");
    match column {
        Some(c) => {
            hash.update([1]);
            hash.update((c.len() as u64).to_le_bytes());
            hash.update(c.as_bytes());
        }
        None => hash.update([0]),
    }
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value.as_bytes());
    // This is a candidate key, never evidence identity or final equality.
    // Truncation reduces high-cardinality dictionary size. Any collision adds
    // candidates only; every result still passes the original predicate.
    Some(
        hash.finalize()[..EXACT_HASH_BYTES]
            .try_into()
            .expect("fixed hash width"),
    )
}

fn insert_exact_terms(
    terms: &mut HashSet<[u8; EXACT_HASH_BYTES]>,
    column: Option<&str>,
    value: &str,
) {
    if let Some(term) = exact_term(column, value) {
        terms.insert(term);
    }
    let ascii = value.to_ascii_lowercase();
    if ascii != value {
        if let Some(term) = exact_term(column, &ascii) {
            terms.insert(term);
        }
    }
    let folded = value.trim().to_lowercase();
    if folded != value && folded != ascii {
        if let Some(term) = exact_term(column, &folded) {
            terms.insert(term);
        }
    }
}

fn nested_values(value: &Value, values: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            values.insert(value.to_string());
            for v in map.values() {
                nested_values(v, values);
            }
        }
        Value::Array(items) => {
            values.insert(value.to_string());
            for v in items {
                nested_values(v, values);
            }
        }
        Value::String(s) => {
            values.insert(s.clone());
        }
        other => {
            values.insert(other.to_string());
        }
    }
}

fn document(event: &Event, row: usize, f: Fields) -> Result<TantivyDocument, String> {
    let mut doc = TantivyDocument::default();
    doc.add_u64(f.row, row as u64);
    if let Some(ts) = event.timestamp {
        doc.add_i64(f.timestamp, ts);
    }
    doc.add_text(f.source, &event.source);
    doc.add_text(f.code, &event.code);
    doc.add_text(f.level, &event.level);
    doc.add_bytes(
        f.event,
        &serde_json::to_vec(event).map_err(|e| e.to_string())?,
    );
    let mut columns: HashSet<&str> = STANDARD_COLUMNS.iter().copied().collect();
    columns.extend(["id", "event_ref", "raw", "parse_status"]);
    columns.extend(event.fields.keys().map(String::as_str));
    columns.extend(crate::entities::ROLES.iter().map(|r| r.column));
    let mut terms = HashSet::new();
    let mut values = HashSet::new();
    // All raw and parsed values are included. Decoded JSON strings and virtual
    // entities are essential: they need not appear literally in the raw bytes.
    for column in columns {
        let Some(value) = event.col_ref(column) else {
            continue;
        };
        values.insert(value.to_string());
        insert_exact_terms(&mut terms, Some(column), &value);
    }
    for v in event.fields.values() {
        nested_values(v, &mut values);
    }
    // Query-language aliases use a shared evaluation context. Keep those
    // values as well as Event::col_ref's values in the global superset.
    let context = crate::querylang::Ctx::new(event);
    for role in crate::entities::ROLES {
        if let Some(value) = context.role(role.role) {
            values.insert(value.to_string());
        }
    }
    // Legacy `_all` contains/regex can cross this newline boundary. Equality
    // matches individual values, never this synthetic joined representation.
    let joined = format!("{}\n{}", event.message, event.raw);
    if !values.contains(&joined) {
        doc.add_text(f.grams, joined.to_lowercase());
    }
    for value in values {
        doc.add_text(f.grams, value.to_lowercase());
        insert_exact_terms(&mut terms, None, &value);
    }
    for term in terms {
        doc.add_bytes(f.exact, &term);
    }
    Ok(doc)
}

// Bound additional preparation buffers by both document count and payload.
// One indivisible document can cross the byte target; no event is truncated.
// Tantivy's own writer/queue and the event currently being parsed are separate.
fn prepared_document_bytes(doc: &TantivyDocument) -> usize {
    std::mem::size_of::<TantivyDocument>()
        .saturating_add(doc.node_data.capacity())
        // The private field-address Vec grows geometrically. This conservative
        // allowance covers its capacity without depending on its packed layout.
        .saturating_add(doc.len().max(4).saturating_mul(32))
}

fn prepare_batch(
    next_row: &AtomicUsize,
    count: usize,
    generation: Option<u64>,
    byte_limit: usize,
    prepare: &(impl Fn(usize) -> Result<TantivyDocument, String> + Sync),
) -> Result<Vec<(usize, TantivyDocument)>, String> {
    let work = || {
        let mut batch = Vec::new();
        let mut bytes = 0usize;
        while batch.len() < PREPARE_BATCH_DOCS && bytes < byte_limit {
            crate::operations::check()?;
            let row = next_row.fetch_add(1, Ordering::Relaxed);
            if row >= count {
                break;
            }
            let doc = prepare(row)?;
            bytes = bytes.saturating_add(prepared_document_bytes(&doc));
            batch.push((row, doc));
        }
        Ok(batch)
    };
    // Parsing/enrichment can consult cooperative cancellation. Rayon workers
    // do not inherit the caller's thread-local generation automatically.
    match generation {
        Some(generation) => crate::operations::run(generation, work)?,
        None => work(),
    }
}

fn is_filename(value: &str) -> bool {
    let mut parts = Path::new(value).components();
    matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none()
}

fn file_metadata(path: &Path) -> Result<(u64, String), String> {
    let m = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !m.is_file() {
        return Err("Cache contém entrada inválida.".into());
    }
    let modified = m
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string();
    Ok((m.len(), modified))
}

fn cache_files(path: &Path) -> Result<Vec<CachedFile>, String> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".lock") {
            continue;
        }
        let (bytes, modified) = file_metadata(&entry.path())?;
        files.push(CachedFile {
            name,
            bytes,
            modified,
        });
    }
    files.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

impl BigDataIndex {
    pub fn open_or_build(
        idx: &FileIndex,
        codes: &CodesConfig,
        system: &CodesConfig,
        derived: &[CompiledDerived],
        dir: &Path,
        progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<Self, String> {
        let key = fingerprint(idx, codes, system, derived);
        Self::open_or_build_with(idx.lines.len(), &key, dir, progress, |i| {
            crate::sources::event_at_uncached(idx, i, codes, system, derived)
        })
    }

    pub fn open_or_build_events(
        events: &[Event],
        dir: &Path,
        progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<Self, String> {
        let key = fingerprint_events(events);
        Self::open_or_build_with(events.len(), &key, dir, progress, |i| events[i].clone())
    }

    fn open_or_build_with(
        count: usize,
        fingerprint: &str,
        dir: &Path,
        progress: Option<&dyn Fn(usize, usize)>,
        event: impl Fn(usize) -> Event + Sync,
    ) -> Result<Self, String> {
        crate::operations::check()?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let manifest_path = dir.join(format!("{fingerprint}.json"));
        if let Ok(manifest) = std::fs::read(&manifest_path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).map_err(|e| e.to_string()))
        {
            if manifest.version == VERSION
                && manifest.fingerprint == fingerprint
                && manifest.event_count == count
                && is_filename(&manifest.generation)
                && manifest
                    .generation
                    .starts_with(&format!("index-{fingerprint}-"))
            {
                if let Ok(index) = Self::open_generation(dir, &manifest, true) {
                    // Preserve recently reused generations in the startup policy.
                    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&manifest_path) {
                        let _ = file.set_modified(std::time::SystemTime::now());
                    }
                    if let Some(cb) = progress {
                        cb(count, count);
                    }
                    return Ok(index);
                }
            }
        }
        let start = Instant::now();
        let temporary = tempfile::Builder::new()
            .prefix("big-data-")
            .suffix(".pending")
            .tempdir_in(dir)
            .map_err(|e| e.to_string())?;
        let (schema, fields) = schema();
        let (mut index, write_failure) = io_diagnostics::create(temporary.path(), schema)?;
        configure(&mut index)?;
        let available = std::thread::available_parallelism().map_or(1, usize::from);
        let threads = available.min(4);
        let mut writer = index
            .writer_with_num_threads::<TantivyDocument>(threads, WRITER_BUDGET)
            .map_err(|e| write_failure.message(e))?;
        let preparers = available.saturating_sub(threads).clamp(1, 4);
        // A private pool avoids occupying every global Rayon thread while the
        // Tantivy workers are also active. Small inputs keep the serial path.
        let preparation_pool = (preparers > 1 && count >= 1024)
            .then(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(preparers)
                    .thread_name(|i| format!("big-data-prepare-{i}"))
                    .build()
                    .ok()
            })
            .flatten();
        if let Some(cb) = progress {
            cb(0, count);
        }
        if let Some(pool) = preparation_pool {
            let generation = crate::operations::current_generation();
            let next_row = AtomicUsize::new(0);
            let mut completed = 0usize;
            while completed < count {
                crate::operations::check()?;
                let batches: Vec<_> = pool.install(|| {
                    (0..preparers)
                        .into_par_iter()
                        .map(|_| {
                            prepare_batch(
                                &next_row,
                                count,
                                generation,
                                PREPARE_BATCH_BYTES,
                                &|row| document(&event(row), row, fields),
                            )
                        })
                        .collect()
                });
                let mut prepared = Vec::new();
                for batch in batches {
                    prepared.extend(batch?);
                }
                // Retain input order for store compression and writer locality;
                // persistent row IDs remain independent of segment addresses.
                prepared.sort_unstable_by_key(|(row, _)| *row);
                for (_, doc) in prepared {
                    if completed % 256 == 0 {
                        crate::operations::check()?;
                    }
                    writer.add_document(doc).map_err(|e| write_failure.message(e))?;
                    completed += 1;
                    if completed % 2048 == 0 {
                        if let Some(cb) = progress {
                            cb(completed, count);
                        }
                    }
                }
            }
        } else {
            for i in 0..count {
                if i % 256 == 0 {
                    crate::operations::check()?;
                }
                writer
                    .add_document(document(&event(i), i, fields)?)
                    .map_err(|e| write_failure.message(e))?;
                if (i + 1) % 2048 == 0 {
                    if let Some(cb) = progress {
                        cb(i + 1, count);
                    }
                }
            }
        }
        crate::operations::check()?;
        writer.commit().map_err(|e| write_failure.message(e))?;
        writer.wait_merging_threads().map_err(|e| write_failure.message(e))?;
        drop(index);
        crate::operations::check()?;
        let generation = format!("index-{fingerprint}-{}", uuid::Uuid::new_v4());
        let generation_path = dir.join(&generation);
        std::fs::rename(temporary.path(), &generation_path).map_err(|e| e.to_string())?;
        let mut generation_guard = UnpublishedGeneration {
            path: generation_path.clone(),
            published: false,
        };
        let files = cache_files(&generation_path)?;
        let manifest = Manifest {
            version: VERSION,
            fingerprint: fingerprint.to_string(),
            generation,
            event_count: count,
            index_bytes: files.iter().map(|f| f.bytes).sum(),
            build_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            files,
        };
        let result = Self::open_generation(dir, &manifest, false)?;
        crate::operations::check()?;
        let mut pending = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        serde_json::to_writer(pending.as_file_mut(), &manifest).map_err(|e| e.to_string())?;
        pending.as_file().sync_all().map_err(|e| e.to_string())?;
        pending
            .persist(&manifest_path)
            .map_err(|e| e.error.to_string())?;
        generation_guard.published = true;
        if let Some(cb) = progress {
            cb(count, count);
        }
        Ok(result)
    }

    fn open_generation(dir: &Path, manifest: &Manifest, reused: bool) -> Result<Self, String> {
        if manifest.version != VERSION
            || !is_filename(&manifest.generation)
            || !manifest.generation.starts_with(&format!("index-{}-", manifest.fingerprint))
        {
            return Err("Manifesto de cache inválido.".into());
        }
        let path = dir.join(&manifest.generation);
        // Check the complete inventory, not just whichever entries a damaged
        // manifest happens to retain. Omissions/duplicates must not disable
        // size/mtime validation for a subset of the immutable generation.
        if cache_files(&path)? != manifest.files {
            return Err("Cache alterado; reconstrução necessária.".into());
        }
        let mut index = Index::open_in_dir(&path).map_err(|e| e.to_string())?;
        let (expected_schema, fields) = schema();
        if index.schema() != expected_schema {
            return Err("Schema de cache incompatível.".into());
        }
        configure(&mut index)?;
        let reader = index.reader().map_err(|e| e.to_string())?;
        let searcher = reader.searcher();
        if searcher.num_docs() != manifest.event_count as u64 {
            return Err("Contagem de cache inválida.".into());
        }
        let invalid = DocAddress::new(u32::MAX, u32::MAX);
        let mut addresses = vec![invalid; manifest.event_count];
        let mut fast_strings = Vec::with_capacity(searcher.segment_readers().len());
        for (ord, segment) in searcher.segment_readers().iter().enumerate() {
            let fast = segment.fast_fields();
            fast_strings.push([
                fast.str("source").map_err(|e| e.to_string())?,
                fast.str("code").map_err(|e| e.to_string())?,
                fast.str("level").map_err(|e| e.to_string())?,
            ]);
            let rows = segment
                .fast_fields()
                .u64("row")
                .map_err(|e| e.to_string())?;
            for doc in 0..segment.max_doc() {
                if doc % 8192 == 0 {
                    crate::operations::check()?;
                }
                let row = rows
                    .first(doc)
                    .and_then(|r| usize::try_from(r).ok())
                    .filter(|&r| r < addresses.len())
                    .ok_or("Posição de cache inválida.")?;
                if addresses[row] != invalid {
                    return Err("Posição de cache duplicada.".into());
                }
                addresses[row] = DocAddress::new(ord as u32, doc);
            }
        }
        if addresses.contains(&invalid) {
            return Err("Posição de cache ausente.".into());
        }
        Ok(Self {
            searcher,
            addresses,
            fast_strings,
            fields,
            info: BigDataInfo {
                event_count: manifest.event_count,
                index_bytes: manifest.index_bytes,
                reused,
                build_ms: manifest.build_ms,
                engine: "Tantivy 0.26 · índices invertidos + colunas".into(),
            },
            resource_storage_path: path,
        })
    }

    pub fn info(&self) -> BigDataInfo {
        self.info.clone()
    }

    /// Known owned metadata only; mmap pages, Tantivy dictionaries and its
    /// internal block cache are accounted for by process metrics, not this sum.
    pub(crate) fn resource_heap_bytes(&self) -> u64 {
        (std::mem::size_of::<Self>() as u64)
            .saturating_add(
                (self.addresses.capacity() as u64)
                    .saturating_mul(std::mem::size_of::<DocAddress>() as u64),
            )
            .saturating_add(
                (self.fast_strings.capacity() as u64)
                    .saturating_mul(std::mem::size_of::<[Option<StrColumn>; 3]>() as u64),
            )
            .saturating_add(self.info.engine.capacity() as u64)
    }

    /// Generation identity lets the inventory count shared disk files once
    /// even when distinct readers own separate address vectors.
    pub(crate) fn resource_storage_path(&self) -> &Path {
        &self.resource_storage_path
    }

    pub fn event(&self, row: usize) -> Option<Event> {
        let address = *self.addresses.get(row)?;
        let doc = self.searcher.doc::<TantivyDocument>(address).ok()?;
        serde_json::from_slice(doc.get_first(self.fields.event)?.as_bytes()?).ok()
    }

    pub fn projection_key(
        &self,
        row: usize,
        columns: [bool; 3],
    ) -> Option<(u32, [Option<u64>; 3])> {
        let address = *self.addresses.get(row)?;
        let values = self.fast_strings.get(address.segment_ord as usize)?;
        let mut ordinals = [None; 3];
        for slot in 0..3 {
            if columns[slot] {
                ordinals[slot] = values[slot]
                    .as_ref()
                    .and_then(|value| value.term_ords(address.doc_id).next());
            }
        }
        Some((address.segment_ord, ordinals))
    }

    pub fn col_str(&self, row: usize, column: &str) -> Option<String> {
        let slot = match column {
            "source" => 0,
            "code" => 1,
            "level" => 2,
            _ => return None,
        };
        let address = *self.addresses.get(row)?;
        let values = self.fast_strings.get(address.segment_ord as usize)?[slot].as_ref()?;
        let ord = values.term_ords(address.doc_id).next()?;
        let mut out = String::new();
        values.ord_to_str(ord, &mut out).ok()?.then_some(out)
    }

    /// Count compact dictionary ordinals, decoding each visited value once per
    /// segment. Keys retain their original text; only whitespace-only values
    /// become None, matching query::push_group. Output follows first occurrence
    /// in `rows`, including when equal keys appear in different segments.
    /// None requests the original path on an unsupported column, invalid row,
    /// dictionary error or cancellation; partial counts are never published.
    pub fn count_terms(&self, rows: &[usize], column: &str) -> Option<Vec<(Option<String>, u64)>> {
        let slot = match column {
            "source" => 0,
            "code" => 1,
            "level" => 2,
            _ => return None,
        };
        let generation = crate::operations::current_generation();
        // Sparse maps scale with the visited groups, not the entire dictionary
        // of a large segment when only a small filtered set is requested.
        let mut ordinal_counts: Vec<HashMap<Option<u64>, (u64, usize)>> =
            (0..self.fast_strings.len())
                .map(|_| HashMap::new())
                .collect();
        for (position, &row) in rows.iter().enumerate() {
            if position % 8192 == 0 && crate::operations::cancelled_for(generation) {
                return None;
            }
            let address = *self.addresses.get(row)?;
            let segment = address.segment_ord as usize;
            let values = self.fast_strings.get(segment)?[slot].as_ref();
            let ord = values.and_then(|values| values.term_ords(address.doc_id).next());
            let count = ordinal_counts
                .get_mut(segment)?
                .entry(ord)
                .or_insert((0, position));
            count.0 += 1;
        }
        let mut merged: HashMap<Option<String>, (u64, usize)> = HashMap::new();
        let mut decoded = 0usize;
        for (segment, ordinals) in ordinal_counts.into_iter().enumerate() {
            for (ord, (count, first)) in ordinals {
                if decoded % 1024 == 0 && crate::operations::cancelled_for(generation) {
                    return None;
                }
                decoded += 1;
                let key = match ord {
                    Some(ord) => {
                        let values = self.fast_strings.get(segment)?[slot].as_ref()?;
                        let mut value = String::new();
                        if !values.ord_to_str(ord, &mut value).ok()? {
                            return None;
                        }
                        (!value.trim().is_empty()).then_some(value)
                    }
                    None => None,
                };
                let group = merged.entry(key).or_insert((0, first));
                group.0 += count;
                group.1 = group.1.min(first);
            }
        }
        if crate::operations::cancelled_for(generation) {
            return None;
        }
        let mut groups: Vec<_> = merged.into_iter().collect();
        groups.sort_unstable_by_key(|(_, (_, first))| *first);
        Some(
            groups
                .into_iter()
                .map(|(key, (count, _))| (key, count))
                .collect(),
        )
    }

    fn query(&self, predicate: &Predicate) -> Box<dyn Query> {
        match predicate {
            Predicate::And(children) if children.is_empty() => Box::new(AllQuery),
            Predicate::Or(children) if children.is_empty() => Box::new(EmptyQuery),
            Predicate::And(children) => Box::new(BooleanQuery::new(
                children
                    .iter()
                    .map(|p| (Occur::Must, self.query(p)))
                    .collect(),
            )),
            Predicate::Or(children) => Box::new(BooleanQuery::new(
                children
                    .iter()
                    .map(|p| (Occur::Should, self.query(p)))
                    .collect(),
            )),
            Predicate::Equals {
                column,
                value,
                mode,
            } => {
                let normalized = match mode {
                    ExactMode::Sensitive => value.clone(),
                    ExactMode::AsciiInsensitive => value.to_ascii_lowercase(),
                    ExactMode::TrimLower => value.trim().to_lowercase(),
                };
                if let Some(term) = exact_term(column.as_deref(), &normalized) {
                    Box::new(TermQuery::new(
                        Term::from_field_bytes(self.fields.exact, &term),
                        IndexRecordOption::Basic,
                    ))
                } else {
                    Box::new(AllQuery)
                }
            }
            Predicate::Contains(text) => {
                let text = text.to_lowercase();
                let mut tokenizer = NgramTokenizer::new(3, 3, false).expect("valid ngram size");
                use tantivy::tokenizer::{TokenStream, Tokenizer};
                let mut stream = tokenizer.token_stream(&text);
                let mut grams = HashSet::new();
                stream.process(&mut |token| {
                    grams.insert(token.text.clone());
                });
                if grams.is_empty() {
                    return Box::new(AllQuery);
                }
                Box::new(BooleanQuery::new(
                    grams
                        .into_iter()
                        .map(|g| {
                            (
                                Occur::Must,
                                Box::new(TermQuery::new(
                                    Term::from_field_text(self.fields.grams, &g),
                                    IndexRecordOption::Basic,
                                )) as Box<dyn Query>,
                            )
                        })
                        .collect(),
                ))
            }
            Predicate::Timestamp { lower, upper } => {
                if lower.is_none() && upper.is_none() {
                    return Box::new(AllQuery);
                }
                if let (Some((lo, li)), Some((hi, ui))) = (lower, upper) {
                    if lo > hi || (lo == hi && (!li || !ui)) {
                        return Box::new(EmptyQuery);
                    }
                }
                let bound = |v: &Option<(i64, bool)>| match v {
                    Some((n, true)) => {
                        Bound::Included(Term::from_field_i64(self.fields.timestamp, *n))
                    }
                    Some((n, false)) => {
                        Bound::Excluded(Term::from_field_i64(self.fields.timestamp, *n))
                    }
                    None => Bound::Unbounded,
                };
                // Tantivy's general RangeQuery always prefers FAST when the
                // field has it. Tiny time windows otherwise scan overlapping
                // segments, even when only a handful of timestamps can match.
                // Millisecond i64 values bound dictionary work here to at most
                // 4,097 terms per segment; never truncate postings or results.
                let narrow = match (lower, upper) {
                    (Some((lo, _)), Some((hi, _))) => hi
                        .checked_sub(*lo)
                        .is_some_and(|span| span <= POSTINGS_TIMESTAMP_SPAN_MS),
                    _ => false,
                };
                if narrow {
                    Box::new(InvertedIndexRangeQuery::new(bound(lower), bound(upper)))
                } else {
                    Box::new(RangeQuery::new(bound(lower), bound(upper)))
                }
            }
        }
    }

    pub fn candidates(&self, predicate: &Predicate) -> Result<Vec<usize>, String> {
        crate::operations::check()?;
        let mut rows = self
            .searcher
            .search(
                &*self.query(predicate),
                &RowsCollector {
                    generation: crate::operations::current_generation(),
                },
            )
            .map_err(|e| e.to_string())?;
        crate::operations::check()?;
        rows.sort_unstable();
        rows.dedup();
        Ok(rows)
    }

    // Inclusion-exclusion gives a proven lower bound for an intersection:
    // N - sum(N - |term_i|). Common grams can therefore choose the streaming
    // path from dictionary statistics, without enumerating broad postings.
    fn candidate_lower_bound(&self, predicate: &Predicate, total: usize) -> Result<usize, String> {
        match predicate {
            Predicate::And(children) => {
                let mut excluded = 0usize;
                for child in children {
                    excluded =
                        excluded.saturating_add(total - self.candidate_lower_bound(child, total)?);
                    if excluded >= total {
                        return Ok(0);
                    }
                }
                Ok(total - excluded)
            }
            Predicate::Or(children) => {
                let mut bound = 0;
                for child in children {
                    bound = bound.max(self.candidate_lower_bound(child, total)?);
                }
                Ok(bound)
            }
            Predicate::Equals {
                column,
                value,
                mode,
            } => {
                let normalized = match mode {
                    ExactMode::Sensitive => value.clone(),
                    ExactMode::AsciiInsensitive => value.to_ascii_lowercase(),
                    ExactMode::TrimLower => value.trim().to_lowercase(),
                };
                let Some(term) = exact_term(column.as_deref(), &normalized) else {
                    return Ok(total);
                };
                self.searcher
                    .doc_freq(&Term::from_field_bytes(self.fields.exact, &term))
                    .map(|count| (count as usize).min(total))
                    .map_err(|e| e.to_string())
            }
            Predicate::Contains(text) => {
                use tantivy::tokenizer::{TokenStream, Tokenizer};
                let text = text.to_lowercase();
                let mut tokenizer = NgramTokenizer::new(3, 3, false).expect("valid ngram size");
                let mut stream = tokenizer.token_stream(&text);
                let mut grams = HashSet::new();
                stream.process(&mut |token| {
                    grams.insert(token.text.clone());
                });
                let mut excluded = 0usize;
                for gram in grams {
                    let count = self
                        .searcher
                        .doc_freq(&Term::from_field_text(self.fields.grams, &gram))
                        .map_err(|e| e.to_string())?;
                    excluded = excluded.saturating_add(total - (count as usize).min(total));
                    if excluded >= total {
                        return Ok(0);
                    }
                }
                Ok(total - excluded)
            }
            Predicate::Timestamp {
                lower: None,
                upper: None,
            } => Ok(total),
            Predicate::Timestamp { .. } => Ok(0),
        }
    }

    pub fn candidates_if_selective(
        &self,
        predicate: &Predicate,
        total: usize,
    ) -> Result<Option<Vec<usize>>, String> {
        crate::operations::check()?;
        let threshold = (total / 4).max(4096);
        if total > threshold && self.candidate_lower_bound(predicate, total)? > threshold {
            crate::operations::check()?;
            return Ok(None);
        }
        let query = self.query(predicate);
        let count = self
            .searcher
            .search(&*query, &Count)
            .map_err(|e| e.to_string())?;
        crate::operations::check()?;
        if count > threshold {
            return Ok(None);
        }
        let mut rows = self
            .searcher
            .search(
                &*query,
                &RowsCollector {
                    generation: crate::operations::current_generation(),
                },
            )
            .map_err(|e| e.to_string())?;
        crate::operations::check()?;
        rows.sort_unstable();
        rows.dedup();
        Ok(Some(rows))
    }
}

struct RowsCollector {
    generation: Option<u64>,
}
struct SegmentRows {
    rows: Column<u64>,
    matched: Vec<usize>,
    generation: Option<u64>,
    visited: usize,
    aborted: bool,
}
impl Collector for RowsCollector {
    type Fruit = Vec<usize>;
    type Child = SegmentRows;
    fn for_segment(
        &self,
        _: SegmentOrdinal,
        segment: &SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        Ok(SegmentRows {
            rows: segment.fast_fields().u64("row")?,
            matched: Vec::new(),
            generation: self.generation,
            visited: 0,
            aborted: false,
        })
    }
    fn requires_scoring(&self) -> bool {
        false
    }
    fn merge_fruits(&self, children: Vec<Vec<usize>>) -> tantivy::Result<Vec<usize>> {
        Ok(children.into_iter().flatten().collect())
    }
}
impl SegmentCollector for SegmentRows {
    type Fruit = Vec<usize>;
    fn collect(&mut self, doc: DocId, _: Score) {
        self.visited += 1;
        if self.visited % 1024 == 0 {
            self.aborted |= crate::operations::cancelled_for(self.generation);
        }
        if self.aborted {
            return;
        }
        if let Some(row) = self.rows.first(doc) {
            self.matched.push(row as usize);
        }
    }
    fn harvest(self) -> Vec<usize> {
        self.matched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parallel_preparation_bounds_payload_preserves_rows_and_inherits_cancellation() {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        let (_, fields) = schema();
        let next_row = AtomicUsize::new(0);
        let generation = crate::operations::generation();
        let count = 33;
        let byte_limit = 8 * 1024;
        let mut all = Vec::new();
        while all.len() < count {
            let batches: Vec<_> = pool.install(|| {
                (0..2)
                    .into_par_iter()
                    .map(|_| {
                        prepare_batch(&next_row, count, Some(generation), byte_limit, &|row| {
                            assert_eq!(crate::operations::current_generation(), Some(generation));
                            let mut doc = TantivyDocument::default();
                            doc.add_u64(fields.row, row as u64);
                            // An oversized record is kept whole, in its own
                            // batch when first, or as the last indivisible item.
                            doc.add_bytes(
                                fields.event,
                                &vec![row as u8; if row == 7 { 32_000 } else { 1700 }],
                            );
                            Ok(doc)
                        })
                    })
                    .collect()
            });
            for batch in batches {
                let batch = batch.unwrap();
                assert!(batch.len() <= PREPARE_BATCH_DOCS);
                let mut bytes = 0usize;
                for (row, doc) in batch {
                    assert!(bytes < byte_limit);
                    bytes += prepared_document_bytes(&doc);
                    assert_eq!(
                        doc.get_first(fields.row).unwrap().as_u64(),
                        Some(row as u64)
                    );
                    assert_eq!(
                        doc.get_first(fields.event)
                            .unwrap()
                            .as_bytes()
                            .unwrap()
                            .len(),
                        if row == 7 { 32_000 } else { 1700 }
                    );
                    all.push(row);
                }
            }
        }
        all.sort_unstable();
        assert_eq!(all, (0..count).collect::<Vec<_>>());

        let stale_generation = generation.wrapping_sub(1);
        let untouched = AtomicUsize::new(0);
        assert!(pool
            .install(|| prepare_batch(
                &untouched,
                1,
                Some(stale_generation),
                byte_limit,
                &|_| panic!("cancelled preparation must not read the event"),
            ))
            .is_err());
        assert_eq!(untouched.load(Ordering::Relaxed), 0);
    }

    fn examples() -> Vec<Event> {
        let mut a = Event::empty();
        a.id = 42;
        a.source = " Serviço ".into();
        a.code = "E001".into();
        a.timestamp = Some(100);
        a.message = "Falha no C:\\Windows\\system32 e AÇÃO".into();
        a.raw = r#"{"message":"Falha no C:\\Windows\\system32 e A\u00c7\u00c3O"}"#.into();
        a.fields.insert("nested".into(), json!({"User": "Alice", "values": ["literal-json", 19], "child": {"marker":"nested-object"}}));
        a.fields.insert("UserName".into(), json!("alice"));
        a.fields.insert("a:b".into(), json!("collision:one"));
        let mut b = Event::empty();
        b.id = 900;
        b.source = "serviço".into();
        b.code = "e001".into();
        b.timestamp = Some(200);
        b.message = "uma falha comum".into();
        b.fields.insert("a".into(), json!("b:collision:one"));
        let mut c = Event::empty();
        c.id = 700;
        c.message = "boundary".into();
        c.raw = "after".into();
        vec![a, b, c]
    }

    #[test]
    fn parsed_unicode_nested_values_and_positional_identity_survive() {
        let cache = tempfile::tempdir().unwrap();
        let events = examples();
        let engine = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
        assert_eq!(
            engine
                .candidates(&Predicate::Contains("windows\\system32".into()))
                .unwrap(),
            vec![0]
        );
        let candidates = engine
            .candidates(&Predicate::Contains("ação".into()))
            .unwrap();
        assert!(candidates.contains(&0));
        // Other fields (e.g. the default level "Informação") can add safe
        // candidates. The message filter must still retain exactly this event.
        let filter = crate::query::Filter {
            column: "message".into(),
            op: "contains".into(),
            value: "ação".into(),
            value2: None,
        };
        assert_eq!(
            candidates
                .into_iter()
                .filter(|&i| crate::query::matches_filter(&events[i], &filter))
                .collect::<Vec<_>>(),
            vec![0]
        );
        assert_eq!(
            engine
                .candidates(&Predicate::Contains("literal-json".into()))
                .unwrap(),
            vec![0]
        );
        assert_eq!(
            engine
                .candidates(&Predicate::Contains("boundary\nafter".into()))
                .unwrap(),
            vec![2]
        );
        assert_eq!(
            engine
                .candidates(&Predicate::Equals {
                    column: None,
                    value: "Alice".into(),
                    mode: ExactMode::Sensitive
                })
                .unwrap(),
            vec![0]
        );
        assert_eq!(
            engine
                .candidates(&Predicate::Equals {
                    column: None,
                    value: r#"{"marker":"nested-object"}"#.into(),
                    mode: ExactMode::Sensitive
                })
                .unwrap(),
            vec![0]
        );
        assert_eq!(
            engine
                .candidates(&Predicate::Equals {
                    column: Some("a:b".into()),
                    value: "collision:one".into(),
                    mode: ExactMode::Sensitive
                })
                .unwrap(),
            vec![0]
        );
        assert_eq!(
            engine
                .candidates(&Predicate::Equals {
                    column: Some("source".into()),
                    value: "SERVIÇO".into(),
                    mode: ExactMode::TrimLower
                })
                .unwrap(),
            vec![0, 1]
        );
        assert_eq!(
            engine
                .candidates(&Predicate::Timestamp {
                    lower: Some((100, false)),
                    upper: Some((200, true))
                })
                .unwrap(),
            vec![1]
        );
        assert_eq!(engine.col_str(0, "source").as_deref(), Some(" Serviço "));
        assert_eq!(engine.col_str(0, "code").as_deref(), Some("E001"));
        assert_eq!(engine.col_str(0, "message"), None);
        let saved = engine.event(0).unwrap();
        assert_eq!(saved.id, 42);
        assert_eq!(saved.fields, events[0].fields);
        assert_eq!(saved.raw, events[0].raw);
    }

    #[test]
    fn timestamp_range_strategies_preserve_boundaries_missing_values_and_extremes() {
        let cache = tempfile::tempdir().unwrap();
        let timestamps = [
            Some(i64::MIN),
            Some(i64::MIN + 1),
            Some(-4096),
            Some(-1),
            Some(0),
            Some(1),
            Some(4095),
            Some(4096),
            Some(4097),
            Some(i64::MAX - 1),
            Some(i64::MAX),
            None,
        ];
        let events: Vec<_> = timestamps
            .iter()
            .enumerate()
            .map(|(row, &timestamp)| {
                let mut event = Event::empty();
                event.id = row + 100;
                event.timestamp = timestamp;
                event
            })
            .collect();
        let engine = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
        for (lower, upper) in [
            (Some((-1, true)), Some((1, true))),
            (Some((-1, false)), Some((1, false))),
            (Some((0, true)), Some((0, true))),
            (Some((0, false)), Some((0, true))),
            (Some((2, true)), Some((1, true))),
            (Some((0, true)), Some((4096, true))),
            (Some((0, true)), Some((4097, true))),
            (Some((i64::MIN, true)), Some((i64::MIN + 1, false))),
            (Some((i64::MAX - 1, false)), Some((i64::MAX, true))),
            (Some((i64::MIN, true)), Some((i64::MAX, true))),
            (None, Some((1, false))),
            (Some((-1, false)), None),
            (None, None),
        ] {
            let predicate = Predicate::Timestamp { lower, upper };
            let expected: Vec<_> = timestamps
                .iter()
                .enumerate()
                .filter_map(|(row, &ts)| {
                    if lower.is_none() && upper.is_none() {
                        return Some(row);
                    }
                    let ts = ts?;
                    let above =
                        lower.is_none_or(|(n, inclusive)| if inclusive { ts >= n } else { ts > n });
                    let below =
                        upper.is_none_or(|(n, inclusive)| if inclusive { ts <= n } else { ts < n });
                    (above && below).then_some(row)
                })
                .collect();
            assert_eq!(
                engine.candidates(&predicate).unwrap(),
                expected,
                "{predicate:?}"
            );
            assert_eq!(
                engine
                    .candidates_if_selective(&predicate, events.len())
                    .unwrap(),
                Some(expected)
            );
        }
        assert!(engine
            .query(&Predicate::Timestamp {
                lower: Some((0, true)),
                upper: Some((4096, true)),
            })
            .is::<InvertedIndexRangeQuery>());
        assert!(engine
            .query(&Predicate::Timestamp {
                lower: Some((0, true)),
                upper: Some((4097, true)),
            })
            .is::<RangeQuery>());
        assert!(engine
            .query(&Predicate::Timestamp {
                lower: Some((i64::MIN, true)),
                upper: Some((i64::MAX, true)),
            })
            .is::<RangeQuery>());
        let stale_generation = crate::operations::generation().wrapping_sub(1);
        assert!(
            crate::operations::run(stale_generation, || engine.candidates(
                &Predicate::Timestamp {
                    lower: Some((0, true)),
                    upper: Some((100, true)),
                }
            ))
            .is_err()
        );
    }

    #[test]
    #[ignore = "manual same-index FAST/postings diagnostic; optional BIG_DATA_RANGE_BENCH_MANIFEST"]
    fn benchmark_timestamp_range_strategies() {
        let (searcher, timestamp_field, count) =
            if let Some(path) = std::env::var_os("BIG_DATA_RANGE_BENCH_MANIFEST") {
                let path = PathBuf::from(path);
                let manifest: Manifest =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                let engine =
                    BigDataIndex::open_generation(path.parent().unwrap(), &manifest, true).unwrap();
                (
                    engine.searcher.clone(),
                    engine.fields.timestamp,
                    manifest.event_count,
                )
            } else {
                // Diagnostic fixture only: identical timestamp/row fields without
                // parsed logs, so this does not measure application ingestion.
                let count = 1_000_000usize;
                let (schema, fields) = schema();
                let mut index = Index::create_in_ram(schema);
                configure(&mut index).unwrap();
                let mut writer = index
                    .writer_with_num_threads::<TantivyDocument>(4, WRITER_BUDGET)
                    .unwrap();
                for row in 0..count {
                    let mut doc = TantivyDocument::default();
                    doc.add_u64(fields.row, row as u64);
                    doc.add_i64(fields.timestamp, 1_700_000_000_000 + row as i64);
                    writer.add_document(doc).unwrap();
                }
                writer.commit().unwrap();
                writer.wait_merging_threads().unwrap();
                (index.reader().unwrap().searcher(), fields.timestamp, count)
            };
        let lo = 1_700_000_000_000i64 + count as i64 / 2;
        let lower = Bound::Included(Term::from_field_i64(timestamp_field, lo));
        let upper = Bound::Included(Term::from_field_i64(timestamp_field, lo + 100));
        let fast = RangeQuery::new(lower.clone(), upper.clone());
        let postings = InvertedIndexRangeQuery::new(lower, upper);
        let collect = |query: &dyn Query| {
            let count = searcher.search(query, &Count).unwrap();
            let mut rows = searcher
                .search(query, &RowsCollector { generation: None })
                .unwrap();
            rows.sort_unstable();
            assert_eq!(count, rows.len());
            rows
        };
        let expected = collect(&fast);
        assert_eq!(expected.len(), 101);
        assert_eq!(collect(&postings), expected);
        let mut samples = [Vec::new(), Vec::new()];
        for trial in 0..7 {
            // Alternate execution order on the same reader and warmed pages.
            for strategy in [trial % 2, 1 - trial % 2] {
                let query: &dyn Query = if strategy == 0 { &fast } else { &postings };
                let started = Instant::now();
                let rows = collect(query);
                samples[strategy].push(started.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(rows, expected);
            }
        }
        for (name, values) in ["fast", "postings"].into_iter().zip(samples) {
            let mut sorted = values.clone();
            sorted.sort_by(f64::total_cmp);
            println!(
                "TIMESTAMP_RANGE_BENCH {}",
                json!({
                    "strategy": name, "events": count, "matches": expected.len(),
                    "segments": searcher.segment_readers().len(), "samplesMs": values,
                    "medianMs": sorted[3], "p95Ms": sorted[6],
                    "fixture": if std::env::var_os("BIG_DATA_RANGE_BENCH_MANIFEST").is_some() { "existing_application_index" } else { "timestamp_only_diagnostic" },
                })
            );
        }
    }

    #[test]
    fn approximate_grams_require_full_match_verification() {
        let cache = tempfile::tempdir().unwrap();
        let mut event = Event::empty();
        event.message = "abc ... bcd".into();
        let engine = BigDataIndex::open_or_build_events(&[event], cache.path(), None).unwrap();
        // All trigrams exist, but the original substring does not.
        assert_eq!(
            engine
                .candidates(&Predicate::Contains("abcd".into()))
                .unwrap(),
            vec![0]
        );
        assert_eq!(
            engine
                .candidates(&Predicate::Contains("zz".into()))
                .unwrap(),
            vec![0]
        );
        assert!(engine
            .candidates(&Predicate::Contains("absent-needle".into()))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn shared_hash_terms_deduplicate_variants_and_separate_domains() {
        let mut terms = HashSet::new();
        insert_exact_terms(&mut terms, None, "lowercase unchanged");
        assert_eq!(terms.len(), 1);
        insert_exact_terms(&mut terms, None, "lowercase unchanged");
        assert_eq!(terms.len(), 1);
        terms.clear();
        insert_exact_terms(&mut terms, None, "ALICE");
        assert_eq!(terms.len(), 2); // ASCII and trim/lower share one hash.
        assert_ne!(exact_term(Some("a"), "bc"), exact_term(Some("ab"), "c"));
        assert_ne!(exact_term(Some("a:b"), "c"), exact_term(Some("a"), "b:c"));
        assert_ne!(exact_term(None, "a:bc"), exact_term(Some("a"), "bc"));
        assert_ne!(exact_term(None, "bc"), exact_term(Some(""), "bc"));
        assert!(exact_term(None, &"x".repeat(MAX_EXACT_TERM + 1)).is_none());
    }

    #[test]
    fn shared_normalizations_are_candidates_and_original_equality_stays_authoritative() {
        let cache = tempfile::tempdir().unwrap();
        let events: Vec<_> = ["API-A", "api-a", " API-A "]
            .into_iter()
            .map(|source| {
                let mut event = Event::empty();
                event.source = source.into();
                event
            })
            .collect();
        let engine = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
        for (mode, op, value, expected) in [
            (ExactMode::Sensitive, "equals_exact", "api-a", vec![1]),
            (ExactMode::AsciiInsensitive, "equals", "API-A", vec![0, 1]),
            (ExactMode::TrimLower, "in", "API-A", vec![0, 1, 2]),
        ] {
            let candidates = engine
                .candidates(&Predicate::Equals {
                    column: Some("source".into()),
                    value: value.into(),
                    mode,
                })
                .unwrap();
            assert_eq!(candidates, vec![0, 1, 2]);
            let filter = crate::query::Filter {
                column: "source".into(),
                op: op.into(),
                value: value.into(),
                value2: None,
            };
            let verified: Vec<_> = candidates
                .into_iter()
                .filter(|&i| crate::query::matches_filter(&events[i], &filter))
                .collect();
            assert_eq!(verified, expected);
        }
    }

    #[test]
    fn synthetic_all_join_only_has_grams_and_long_equalities_keep_safe_fallback() {
        let cache = tempfile::tempdir().unwrap();
        let mut event = Event::empty();
        event.message = "boundary".into();
        event.raw = "after".into();
        let engine = BigDataIndex::open_or_build_events(&[event], cache.path(), None).unwrap();
        assert_eq!(
            engine
                .candidates(&Predicate::Contains("boundary\nafter".into()))
                .unwrap(),
            vec![0]
        );
        assert!(engine
            .candidates(&Predicate::Equals {
                column: None,
                value: "boundary\nafter".into(),
                mode: ExactMode::Sensitive,
            })
            .unwrap()
            .is_empty());
        assert_eq!(
            engine
                .candidates(&Predicate::Equals {
                    column: None,
                    value: "x".repeat(MAX_EXACT_TERM + 1),
                    mode: ExactMode::Sensitive,
                })
                .unwrap(),
            vec![0]
        );
    }

    #[test]
    fn ordinal_counts_preserve_first_seen_order_empty_keys_and_filtered_rows() {
        let cache = tempfile::tempdir().unwrap();
        let events: Vec<_> = ["", "  ", " beta ", "beta", "alpha", "beta"]
            .into_iter()
            .map(|source| {
                let mut event = Event::empty();
                event.source = source.into();
                event
            })
            .collect();
        let engine = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
        let rows = [4, 3, 0, 2, 1, 5, 3];
        assert_eq!(
            engine.count_terms(&rows, "source"),
            Some(vec![
                (Some("alpha".into()), 1),
                (Some("beta".into()), 3),
                (None, 2),
                (Some(" beta ".into()), 1),
            ])
        );
        assert_eq!(engine.count_terms(&rows, "code"), Some(vec![(None, 7)]));
        assert_eq!(
            engine.count_terms(&rows, "level"),
            Some(vec![(Some("Informação".into()), 7)])
        );
        assert_eq!(engine.count_terms(&[], "source"), Some(vec![]));
        assert!(engine.count_terms(&rows, "message").is_none());
        assert!(engine.count_terms(&[events.len()], "source").is_none());
    }

    #[test]
    fn conditional_field_dependencies_track_rule_content_and_sigma_membership() {
        let dir = tempfile::tempdir().unwrap();
        let signature = || {
            let mut hash = Sha256::new();
            digest_security_dependencies(&mut hash, dir.path());
            format!("{:x}", hash.finalize())
        };
        let absent = signature();
        std::fs::write(
            dir.path().join("detections.json"),
            r#"{"disabled":["rule-a"]}"#,
        )
        .unwrap();
        let enabled = signature();
        assert_ne!(absent, enabled);
        // Same byte length still changes the generation signature.
        std::fs::write(
            dir.path().join("detections.json"),
            r#"{"disabled":["rule-b"]}"#,
        )
        .unwrap();
        let changed = signature();
        assert_ne!(enabled, changed);
        let sigma = dir.path().join("sigma");
        std::fs::create_dir(&sigma).unwrap();
        let rule = sigma.join("rule.yml");
        std::fs::write(&rule, "title: one\ndetection: {}\n").unwrap();
        assert_ne!(changed, signature());
        std::fs::remove_file(rule).unwrap();
        assert_eq!(changed, signature());
        std::fs::write(dir.path().join("threat-rules.json"), "{}").unwrap();
        assert_ne!(changed, signature());
    }

    #[test]
    fn immutable_cache_reuses_and_recovers_from_corruption() {
        let cache = tempfile::tempdir().unwrap();
        let events = examples();
        let first = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
        assert!(!first.info().reused);
        assert!(first.info().index_bytes > 0);
        drop(first);
        let second = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
        assert!(second.info().reused);
        drop(second);
        let key = fingerprint_events(&events);
        let manifest: Manifest = serde_json::from_slice(
            &std::fs::read(cache.path().join(format!("{key}.json"))).unwrap(),
        )
        .unwrap();
        std::fs::write(
            cache.path().join(manifest.generation).join("meta.json"),
            "corrupt",
        )
        .unwrap();
        let rebuilt = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
        assert!(!rebuilt.info().reused);
        assert_eq!(
            rebuilt
                .candidates(&Predicate::Contains("windows\\system32".into()))
                .unwrap(),
            vec![0]
        );
    }

    #[test]
    fn incomplete_or_duplicated_manifest_inventory_is_never_reused() {
        let cache = tempfile::tempdir().unwrap();
        let events = examples();
        let key = fingerprint_events(&events);
        let manifest_path = cache.path().join(format!("{key}.json"));
        drop(BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap());
        for duplicate in [false, true] {
            let mut manifest: Manifest = serde_json::from_slice(
                &std::fs::read(&manifest_path).unwrap(),
            ).unwrap();
            if duplicate {
                let first = &manifest.files[0];
                manifest.files.push(CachedFile {
                    name: first.name.clone(), bytes: first.bytes, modified: first.modified.clone(),
                });
            } else {
                manifest.files.clear();
            }
            std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            let rebuilt = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
            assert!(!rebuilt.info().reused);
            assert_eq!(rebuilt.event(0).unwrap().event_ref, events[0].event_ref);
            assert_eq!(rebuilt.event(0).unwrap().raw, events[0].raw);
            drop(rebuilt);
            let reused = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
            assert!(reused.info().reused);
        }
    }

    #[test]
    fn cancel_during_build_never_publishes_manifest() {
        let cache = tempfile::tempdir().unwrap();
        let events = examples();
        let generation = crate::operations::generation();
        let result = crate::operations::run(generation, || {
            BigDataIndex::open_or_build_events(
                &events,
                cache.path(),
                Some(&|_, _| crate::operations::cancel()),
            )
        });
        assert!(result.is_err());
        assert!(!cache
            .path()
            .join(format!("{}.json", fingerprint_events(&events)))
            .exists());
    }

    #[test]
    fn cancel_after_prepared_documents_never_publishes_manifest() {
        let cache = tempfile::tempdir().unwrap();
        let events: Vec<_> = (0..4096)
            .map(|row| {
                let mut event = Event::empty();
                event.id = row + 10_000;
                event.source = format!("service-{}", row % 3);
                event
            })
            .collect();
        let generation = crate::operations::generation();
        let result = crate::operations::run(generation, || {
            BigDataIndex::open_or_build_events(
                &events,
                cache.path(),
                Some(&|completed, _| {
                    if completed >= 2048 {
                        crate::operations::cancel();
                    }
                }),
            )
        });
        assert!(result.is_err());
        assert!(!cache
            .path()
            .join(format!("{}.json", fingerprint_events(&events)))
            .exists());
        assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
    }

    #[test]
    fn broad_candidates_fall_back_before_allocating_all_row_ids() {
        let cache = tempfile::tempdir().unwrap();
        let mut event = Event::empty();
        event.message = "common-token".into();
        let events = vec![event; 4100];
        let engine = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
        assert_eq!(
            engine
                .candidate_lower_bound(&Predicate::Contains("common-token".into()), events.len())
                .unwrap(),
            events.len()
        );
        let combined = Predicate::And(vec![
            Predicate::Contains("common-token".into()),
            Predicate::Contains("absent-needle".into()),
        ]);
        assert_eq!(
            engine
                .candidate_lower_bound(&combined, events.len())
                .unwrap(),
            0
        );
        assert_eq!(
            engine
                .candidates_if_selective(&combined, events.len())
                .unwrap(),
            Some(Vec::new())
        );
        assert!(engine
            .candidates_if_selective(&Predicate::Contains("common-token".into()), events.len())
            .unwrap()
            .is_none());
        assert_eq!(
            engine
                .candidates_if_selective(&Predicate::Contains("absent-needle".into()), events.len())
                .unwrap(),
            Some(Vec::new())
        );
    }

    #[test]
    fn startup_cleanup_keeps_referenced_cache_and_removes_expired_orphans() {
        let cache = tempfile::tempdir().unwrap();
        let events = examples();
        let engine = BigDataIndex::open_or_build_events(&events, cache.path(), None).unwrap();
        drop(engine);
        let orphan = cache.path().join("index-unused-orphan");
        std::fs::create_dir(&orphan).unwrap();
        std::fs::write(orphan.join("marker"), "temporary generation").unwrap();
        // Directory times can be set with the same standard API as files.
        let expired = std::time::SystemTime::now() - std::time::Duration::from_secs(31 * 24 * 3600);
        #[cfg(windows)]
        let old = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .access_mode(0x0100)
                .custom_flags(0x02000000)
                .open(&orphan)
                .unwrap()
        };
        #[cfg(not(windows))]
        let old = std::fs::File::open(&orphan).unwrap();
        old.set_modified(expired).unwrap();
        drop(old);
        prune(cache.path());
        assert!(!orphan.exists());
        assert!(
            BigDataIndex::open_or_build_events(&events, cache.path(), None)
                .unwrap()
                .info()
                .reused
        );
    }
}
